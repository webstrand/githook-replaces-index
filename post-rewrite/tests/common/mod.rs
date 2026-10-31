//! Scratch repositories in which the hook runs, as git runs it after each amend and rebase.

#![allow(dead_code)]

use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub const HOOK: &str = env!("CARGO_BIN_EXE_githook-replaces-index-post-rewrite");

/// A git command in `repo`, isolated from the configuration and the session around the test.
pub fn git(repo: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_NOTES_REF");
    command
}

pub fn ok_command(mut command: Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("replaces-index:"), "{stderr}");
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}

pub fn ok(repo: &Path, args: &[&str]) -> String {
    ok_command(git(repo, args))
}

pub fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    ok(dir.path(), &["init", "-q"]);
    ok(
        dir.path(),
        &["config", "hook.replaces-index.event", "post-rewrite"],
    );
    ok(dir.path(), &["config", "hook.replaces-index.command", HOOK]);
    ok(dir.path(), &["commit", "-q", "--allow-empty", "-m", "base"]);
    dir
}

pub fn commit(repo: &Path, name: &str) -> String {
    std::fs::write(repo.join(name), name).unwrap();
    ok(repo, &["add", name]);
    ok(repo, &["commit", "-q", "-m", name]);
    ok(repo, &["rev-parse", "HEAD"])
}

/// The lines of the note of `commit`, sorted.
pub fn note(repo: &Path, commit: &str) -> Vec<String> {
    let output = git(repo, &["notes", "--ref=replaces", "show", commit])
        .output()
        .unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Rebases the commits after `onto` with a todo list that `script` edits in place. Each commit
/// is replayed as a new one, even where its parent is unchanged.
pub fn rebase(repo: &Path, onto: &str, script: &str) {
    rebase_with(repo, &["--force-rebase", onto], script);
}

/// Rebases as `rebase` does, but keeps each commit whose parent is unchanged, as a rebase does by
/// default.
pub fn rebase_keeping(repo: &Path, onto: &str, script: &str) {
    rebase_with(repo, &[onto], script);
}

pub fn rebase_with(repo: &Path, args: &[&str], script: &str) {
    let mut command = git(repo, &["rebase", "-q", "-i"]);
    command.args(args);
    command.env("GIT_SEQUENCE_EDITOR", format!("sed -i -e '{script}'"));
    ok_command(command);
}

/// Runs `command`, a git command that runs the hook, and returns the standard error of both. The
/// command must succeed, since git ignores the exit status of post-rewrite.
pub fn stderr_of(mut command: Command) -> String {
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{command:?}: {stderr}");
    stderr
}

/// A scratch repository with a commit `a` and a commit made from it as an amend makes it, without
/// running the hook, and their ids.
pub fn amended() -> (tempfile::TempDir, String, String) {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let a2 = amend_of(repo.path(), "HEAD");
    (repo, a, a2)
}

/// A commit made from `commit` as an amend makes it, without running the hook.
pub fn amend_of(repo: &Path, commit: &str) -> String {
    let parent = format!("{commit}~");
    let tree = format!("{commit}^{{tree}}");
    let message = format!("{commit}, amended");
    ok(repo, &["commit-tree", &tree, "-p", &parent, "-m", &message])
}

/// The hook run directly in `repo`, as git runs it, with `input` on its standard input and `path`,
/// if given, in front of PATH.
pub fn hook(repo: &Path, kind: &str, input: impl AsRef<[u8]>, path: Option<&Path>) -> Command {
    let mut hook = Command::new(HOOK);
    hook.current_dir(repo).arg(kind);
    with_git_env(&mut hook, repo);
    if let Some(path) = path {
        let paths = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(path.to_owned()).chain(std::env::split_paths(&paths));
        hook.env("PATH", std::env::join_paths(paths).unwrap());
    }
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(input.as_ref()).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    hook.stdin(file);
    hook
}

/// Gives `command` the environment of the git commands in `repo`, so that a program it runs sees
/// what it would see under git.
pub fn with_git_env(command: &mut Command, repo: &Path) {
    for (key, value) in git(repo, &[]).get_envs() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
}

/// The output of `command`, which must fail.
pub fn failing(mut command: Command) -> (String, Output) {
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{command:?} succeeded: {stderr}");
    (stderr, output)
}

/// A directory, under `dir`, that holds a `git` that fails when any of its arguments is `fail`, and
/// otherwise runs git.
pub fn fake_git(dir: &Path, fail: &str) -> PathBuf {
    let on_match = "echo \"fake git fails on $arg\" >&2; exit 128";
    fake_git_script(dir, fail, on_match)
}

/// A directory, under `dir`, that holds a `git` that, when any of its arguments is `hold`, writes
/// its process id to `held` in `dir` and waits until `release` exists there, before it runs git.
pub fn fake_git_holding(dir: &Path, hold: &str) -> PathBuf {
    let held = dir.join("held");
    let release = dir.join("release");
    let on_match = format!(
        "echo $$ > '{}.tmp'; mv '{}.tmp' '{}'; while [ ! -e '{}' ]; do sleep 0.02; done; break",
        held.display(),
        held.display(),
        held.display(),
        release.display()
    );
    fake_git_script(dir, hold, &on_match)
}

fn fake_git_script(dir: &Path, arg: &str, on_match: &str) -> PathBuf {
    let body = format!("for arg; do\n  if [ \"$arg\" = '{arg}' ]; then {on_match}; fi\ndone\n");
    write_fake_git(dir, &body)
}

/// The git that a fake one runs.
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("git"))
        .find(|git| git.is_file())
        .unwrap()
}

/// Writes a `git` that runs `body`, then git, in `fake-bin` under `dir`, and returns that.
fn write_fake_git(dir: &Path, body: &str) -> PathBuf {
    let bin = dir.join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = format!("#!/bin/sh\n{body}exec '{}' \"$@\"\n", real_git().display());
    let git = bin.join("git");
    std::fs::write(&git, script).unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// A directory, under `dir`, that holds a `git` that moves the notes ref itself just before each
/// time it is asked to move it, so that every attempt to move it loses the race. The ref must
/// exist.
pub fn fake_git_racing(dir: &Path) -> PathBuf {
    let real = real_git();
    let real = real.display();
    let body = format!(
        "if [ \"$1\" = update-ref ] && [ \"$4\" = refs/notes/replaces ]; then\n\
         \x20 old=$('{real}' rev-parse refs/notes/replaces)\n\
         \x20 new=$('{real}' commit-tree \"$old^{{tree}}\" -p \"$old\" -m moved)\n\
         \x20 '{real}' update-ref refs/notes/replaces \"$new\" \"$old\"\n\
         fi\n"
    );
    write_fake_git(dir, &body)
}

/// Runs `command` in a process group of its own and, once its standard error contains `waiting`,
/// interrupts the group as Ctrl-C does. Returns the standard error of the command and of every
/// process that shared it, once they have all closed it.
pub fn interrupt_when(mut command: Command, waiting: &str) -> String {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let mut child = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let mut pipe = child.stderr.take().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let reader = {
        let seen = std::sync::Arc::clone(&seen);
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(n @ 1..) = pipe.read(&mut buffer) {
                seen.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..n]));
            }
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !seen.lock().unwrap().contains(waiting) {
        let stderr = seen.lock().unwrap().clone();
        assert!(
            std::time::Instant::now() < deadline,
            "never waiting: {stderr}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    kill("INT", &format!("-{}", child.id()));
    child.wait().unwrap();
    reader.join().unwrap();
    seen.lock().unwrap().clone()
}

/// Waits until the git that `fake_git_holding` made holds, and returns its process id.
pub fn held(dir: &Path) -> u32 {
    let held = dir.join("held");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if let Ok(pid) = std::fs::read_to_string(&held) {
            return pid.trim().parse().unwrap();
        }
        assert!(std::time::Instant::now() < deadline, "git was never held");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Lets the git that `fake_git_holding` made run on.
pub fn release(dir: &Path) {
    std::fs::write(dir.join("release"), "").unwrap();
}

/// Whether process `pid` still exists.
pub fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// Sends `signal` to process `pid`, or to the process group `-pid`.
pub fn kill(signal: &str, pid: &str) {
    let status = Command::new("kill")
        .args([&format!("-{signal}"), "--", pid])
        .status()
        .unwrap();
    assert!(status.success());
}

/// The lock of the notes ref, which keeps every other writer from moving it while it exists.
pub fn notes_lock(repo: &Path) -> PathBuf {
    let dir = repo.join(".git/refs/notes");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("replaces.lock")
}

/// The lines of the abandoned records of `repo`, oldest record first.
pub fn abandoned(repo: &Path) -> Vec<String> {
    let dir = repo.join(".git/replaces-abandoned");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
        .iter()
        .flat_map(|name| {
            let text = std::fs::read_to_string(dir.join(name)).unwrap();
            text.lines().map(str::to_owned).collect::<Vec<_>>()
        })
        .collect()
}

/// The command that the hook prints to record `lines` by hand, up to the program it pipes them to.
pub fn printed(lines: &[String]) -> String {
    let quoted: Vec<String> = lines.iter().map(|line| format!("'{line}'")).collect();
    format!("printf '%s\\n' {} | ", quoted.join(" "))
}
