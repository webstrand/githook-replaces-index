//! Runs git for the replaces-index programs, and reports its failures.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::Mutex;

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

/// The programs running, so that a process that is stopping can end them.
static CHILDREN: Mutex<Children> = Mutex::new(Children {
    stopping: false,
    ids: Vec::new(),
});

struct Children {
    stopping: bool,
    ids: Vec<u32>,
}

fn children() -> std::sync::MutexGuard<'static, Children> {
    CHILDREN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Ends the programs running, and keeps any other from starting: a thread that would start one
/// waits instead until the process exits.
pub fn stop() {
    let mut children = children();
    children.stopping = true;
    for &id in &children.ids {
        // SAFETY: kill only sends a signal. The id is that of a child of this process; between
        // the wait that reaps it and its removal from the list, it could only name another
        // process once the system has handed out every other id.
        unsafe {
            libc::kill(id as libc::pid_t, libc::SIGTERM);
        }
    }
}

/// Starts `command`, unless the process is stopping.
fn spawn(command: &mut Command) -> io::Result<Child> {
    let mut children = children();
    if children.stopping {
        drop(children);
        loop {
            std::thread::park();
        }
    }
    let child = command.spawn()?;
    children.ids.push(child.id());
    Ok(child)
}

/// Waits for `child` to end, with its output captured.
fn wait(child: Child) -> io::Result<Output> {
    let id = child.id();
    let output = child.wait_with_output();
    children().ids.retain(|&other| other != id);
    output
}

/// Runs a command to its end, with its output captured, whatever its exit status.
pub fn run(command: &mut Command) -> Result<Output, Error> {
    let described = describe(command);
    let to_error = |source| Error::Spawn {
        command: described.clone(),
        source,
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = spawn(command).map_err(to_error)?;
    wait(child).map_err(to_error)
}

/// Runs a command to its end with `input` as its standard input, with its output captured,
/// whatever its exit status.
pub fn run_with_input(command: &mut Command, input: &[u8]) -> Result<Output, Error> {
    let described = describe(command);
    let spawn_error = |source| Error::Spawn {
        command: described.clone(),
        source,
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn(command).map_err(spawn_error)?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    // Written beside the reads of the output, so that neither side waits on a full pipe.
    let written = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = wait(child);
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
