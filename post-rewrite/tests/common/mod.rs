//! Scratch repositories in which the hook runs, as git runs it after each amend and rebase.

#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

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
