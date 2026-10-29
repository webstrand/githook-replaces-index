//! Runs git for the replaces-index programs, and reports its failures.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Output, Stdio};

/// A program that could not be run, or that failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not run {command}: {source}")]
    Spawn { command: String, source: io::Error },
    #[error("could not pass input to {command}: {source}")]
    Input { command: String, source: io::Error },
    #[error("{command} failed ({status}){}", stderr_suffix(.stderr))]
    Failed {
        command: String,
        status: ExitStatus,
        stderr: String,
    },
}

fn stderr_suffix(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

/// A git command with the given arguments.
pub fn git<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.args(args);
    command
}

/// Runs a command to its end, with its output captured, whatever its exit status.
pub fn run(command: &mut Command) -> Result<Output, Error> {
    command.output().map_err(|source| Error::Spawn {
        command: describe(command),
        source,
    })
}

/// Runs a command to its end with `input` as its standard input, with its output captured,
/// whatever its exit status.
pub fn run_with_input(command: &mut Command, input: &[u8]) -> Result<Output, Error> {
    let described = describe(command);
    let spawn_error = |source| Error::Spawn {
        command: described.clone(),
        source,
    };
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_error)?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    // Written beside the reads of the output, so that neither side waits on a full pipe.
    let written = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output();
        (writer.join().expect("the writer does not panic"), output)
    });
    let output = written.1.map_err(spawn_error)?;
    // A program that exits without reading all of its input has failed, and says why.
    match written.0 {
        Err(source) if source.kind() != io::ErrorKind::BrokenPipe => Err(Error::Input {
            command: described,
            source,
        }),
        _ => Ok(output),
    }
}

/// Runs a command with `input` as its standard input, and returns its standard output when it
/// succeeds.
pub fn stdout_with_input(command: &mut Command, input: &[u8]) -> Result<String, Error> {
    let output = run_with_input(command, input)?;
    check(command, &output)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs a command, and returns its standard output when it succeeds.
pub fn stdout(command: &mut Command) -> Result<String, Error> {
    let output = run(command)?;
    check(command, &output)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs a command for its effect.
pub fn status(command: &mut Command) -> Result<(), Error> {
    stdout(command).map(drop)
}

/// The failure of a command that ran, if it failed.
pub fn check(command: &Command, output: &Output) -> Result<(), Error> {
    if output.status.success() {
        return Ok(());
    }
    Err(Error::Failed {
        command: describe(command),
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// The output of `git rev-parse` for one of its path options, such as `--git-path <path>`.
pub fn rev_parse_path(args: &[&str]) -> Result<PathBuf, Error> {
    let out = stdout(&mut git(["rev-parse"].iter().chain(args)))?;
    Ok(PathBuf::from(out.trim_end_matches('\n')))
}

/// The contents of the objects `ids`, in their order, read by one `git cat-file --batch`; `None`
/// for an object that the repository lacks.
pub fn cat_file<S: AsRef<str>>(ids: &[S]) -> Result<Vec<Option<Vec<u8>>>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let input: String = ids.iter().map(|id| format!("{}\n", id.as_ref())).collect();
    let mut command = git(["cat-file", "--batch"]);
    let output = run_with_input(&mut command, input.as_bytes())?;
    check(&command, &output)?;
    let malformed = || Error::Failed {
        command: describe(&command),
        status: output.status,
        stderr: "its output could not be read".to_owned(),
    };
    // Each object is `<id> <type> <size>\n<contents>\n`, or `<name> missing\n`.
    let mut rest = output.stdout.as_slice();
    let mut objects = Vec::with_capacity(ids.len());
    for _ in ids {
        let end = rest
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(malformed)?;
        let header = std::str::from_utf8(&rest[..end]).map_err(|_| malformed())?;
        rest = &rest[end + 1..];
        if header.ends_with(" missing") || header.ends_with(" ambiguous") {
            objects.push(None);
            continue;
        }
        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|size| size.parse().ok())
            .ok_or_else(malformed)?;
        if rest.len() < size + 1 {
            return Err(malformed());
        }
        objects.push(Some(rest[..size].to_vec()));
        rest = &rest[size + 1..];
    }
    Ok(objects)
}

/// A command as the shell would show it, for messages.
pub fn describe(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}
