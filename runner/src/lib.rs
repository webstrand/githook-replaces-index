//! Runs git for the replaces-index programs, and reports its failures.

pub mod refspec;

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
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
    #[error("{command} printed what is not UTF-8, which this program cannot read yet")]
    NotUtf8 { command: String },
    #[error("{command} failed ({status}){}", stderr_suffix(.stderr))]
    Failed {
        command: String,
        status: ExitStatus,
        stderr: String,
    },
}

/// A word that the shell reads back as `word`, whatever its bytes.
pub fn quote(word: &OsStr) -> OsString {
    let mut quoted = vec![b'\''];
    for &byte in word.as_bytes() {
        if byte == b'\'' {
            quoted.extend_from_slice(br"'\''");
        } else {
            quoted.push(byte);
        }
    }
    quoted.push(b'\'');
    OsString::from_vec(quoted)
}

fn stderr_suffix(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

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
    running: Vec::new(),
});

struct Children {
    stopping: bool,
    /// The id of each program running, and the command that runs it.
    running: Vec<(u32, String)>,
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
    for &(id, _) in &children.running {
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
    children.running.push((child.id(), describe(command)));
    Ok(child)
}

/// The commands running, oldest first.
pub fn running() -> Vec<String> {
    children()
        .running
        .iter()
        .map(|(_, command)| command.clone())
        .collect()
}

/// Waits for `child` to end, with its output captured.
fn wait(child: Child) -> io::Result<Output> {
    let id = child.id();
    let output = child.wait_with_output();
    children().running.retain(|&(other, _)| other != id);
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
    let output = stdout_bytes_with_input(command, input)?;
    text(command, output)
}

/// Runs a command, and returns its standard output when it succeeds.
pub fn stdout(command: &mut Command) -> Result<String, Error> {
    let output = stdout_bytes(command)?;
    text(command, output)
}

/// `output` of `command` as text; a failure when it is not UTF-8, rather than text that names
/// something else.
// TODO(EuString): output that is not UTF-8 is refused rather than read.
pub fn text(command: &Command, output: Vec<u8>) -> Result<String, Error> {
    String::from_utf8(output).map_err(|_| Error::NotUtf8 {
        command: describe(command),
    })
}

/// The fields of `output` that a `-z` option of git ends with `\0`, each as text, or `None` for
/// one that is not UTF-8, such as a path of a file in a notes tree that is no note.
pub fn nul_fields(output: &[u8]) -> impl Iterator<Item = Option<&str>> {
    let output = output.strip_suffix(b"\0").unwrap_or(output);
    output
        .split(|&b| b == 0)
        .filter(|field| !field.is_empty())
        .map(|field| std::str::from_utf8(field).ok())
}

/// Runs a command with `input` as its standard input, and returns its standard output as the
/// bytes it wrote when it succeeds.
pub fn stdout_bytes_with_input(command: &mut Command, input: &[u8]) -> Result<Vec<u8>, Error> {
    let output = run_with_input(command, input)?;
    check(command, &output)?;
    Ok(output.stdout)
}

/// Runs a command, and returns its standard output as the bytes it wrote when it succeeds.
pub fn stdout_bytes(command: &mut Command) -> Result<Vec<u8>, Error> {
    let output = run(command)?;
    check(command, &output)?;
    Ok(output.stdout)
}

/// Those of the full ids `ids` that name a commit of the repository, by one `git cat-file`.
pub fn commits<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<HashSet<String>, Error> {
    let input: String = ids.into_iter().map(|id| format!("{id}\n")).collect();
    if input.is_empty() {
        return Ok(HashSet::new());
    }
    let mut check = git(["cat-file", "--batch-check=%(objectname) %(objecttype)"]);
    let out = stdout_with_input(&mut check, input.as_bytes())?;
    // Each line is `<id> <type>`, or `<id> missing`.
    Ok(out
        .lines()
        .filter_map(|line| line.strip_suffix(" commit"))
        .map(str::to_owned)
        .collect())
}

/// Whether commit `ancestor` is `descendant` or one of its ancestors; a failure when git cannot
/// tell, as for a commit the repository lacks.
pub fn is_ancestor(ancestor: &str, descendant: &str) -> Result<bool, Error> {
    let mut command = git(["merge-base", "--is-ancestor", ancestor, descendant]);
    let output = run(&mut command)?;
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    check(&command, &output)?;
    Ok(true)
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
        // TODO(EuString): for a message only; bytes that are not UTF-8 are shown lossily.
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// The output of `git rev-parse` for one of its path options, such as `--git-path <path>`.
pub fn rev_parse_path(args: &[&str]) -> Result<PathBuf, Error> {
    let mut paths = rev_parse_paths(args)?;
    Ok(if paths.is_empty() {
        PathBuf::new()
    } else {
        paths.swap_remove(0)
    })
}

/// The output of `git rev-parse` for its path options, one path per line, as the bytes git prints:
/// a path need not be UTF-8, and one read as text would name another.
pub fn rev_parse_paths(args: &[&str]) -> Result<Vec<PathBuf>, Error> {
    let mut command = git(["rev-parse"].iter().chain(args));
    let output = run(&mut command)?;
    check(&command, &output)?;
    Ok(output
        .stdout
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| PathBuf::from(OsString::from_vec(line.to_vec())))
        .collect())
}

/// A file that `git diff-tree` found changed between two trees: its path, and its blob before and
/// after, none on the side it did not exist on.
pub struct Changed {
    pub path: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// The files that changed between the trees of `from` and `to`, by one
/// `git diff-tree -r -z --no-renames`. A file whose path is not UTF-8 is left out.
pub fn diff_tree(from: &str, to: &str) -> Result<Vec<Changed>, Error> {
    let mut command = git(["diff-tree", "-r", "-z", "--no-renames", from, to]);
    let output = run(&mut command)?;
    check(&command, &output)?;
    let malformed = || Error::Failed {
        command: describe(&command),
        status: output.status,
        stderr: "its output could not be read".to_owned(),
    };
    // `:<mode> <mode> <blob> <blob> <status>\0<path>\0` for each file that changed.
    let mut fields = nul_fields(&output.stdout);
    let mut changed = Vec::new();
    while let Some(header) = fields.next() {
        let path = fields.next().ok_or_else(malformed)?;
        let header = header.ok_or_else(malformed)?;
        let words: Vec<&str> = header.split(' ').collect();
        let [_, _, old, new, status] = words[..] else {
            return Err(malformed());
        };
        let (old, new) = match status {
            "A" => (None, Some(new)),
            "D" => (Some(old), None),
            "M" | "T" => (Some(old), Some(new)),
            _ => return Err(malformed()),
        };
        if let Some(path) = path {
            changed.push(Changed {
                path: path.to_owned(),
                old: old.map(str::to_owned),
                new: new.map(str::to_owned),
            });
        }
    }
    Ok(changed)
}

/// The contents of the objects `ids`, in their order, read by one `git cat-file --batch`; `None`
/// for a name that names no object of the repository, or more than one.
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
    // Each object is `<id> <type> <size>\n<contents>\n`, or `<name> missing\n` or
    // `<name> ambiguous\n`.
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

/// The program and its arguments, joined by spaces and unquoted, for messages.
pub fn describe(command: &Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        // TODO(EuString): for a message only; bytes that are not UTF-8 are shown lossily.
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}
