//! Runs the hook in a scratch repository, as git runs it after each amend and rebase.

use std::path::Path;
use std::process::Command;

const HOOK: &str = env!("CARGO_BIN_EXE_githook-replaces-index-post-rewrite");

/// A git command in `repo`, isolated from the configuration and the session around the test.
fn git(repo: &Path, args: &[&str]) -> Command {
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

fn ok_command(mut command: Command) -> String {
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

fn ok(repo: &Path, args: &[&str]) -> String {
    ok_command(git(repo, args))
}

fn repo() -> tempfile::TempDir {
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

fn commit(repo: &Path, name: &str) -> String {
    std::fs::write(repo.join(name), name).unwrap();
    ok(repo, &["add", name]);
    ok(repo, &["commit", "-q", "-m", name]);
    ok(repo, &["rev-parse", "HEAD"])
}

/// The lines of the note of `commit`, sorted.
fn note(repo: &Path, commit: &str) -> Vec<String> {
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
fn rebase(repo: &Path, onto: &str, script: &str) {
    let mut command = git(repo, &["rebase", "-q", "-i", "--force-rebase", onto]);
    command.env("GIT_SEQUENCE_EDITOR", format!("sed -i -e '{script}'"));
    ok_command(command);
}

#[test]
fn an_amend_stands_in_place_of_the_amended_commit() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    ok(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended"],
    );
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended twice"],
    );
    let a3 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert!(note(repo.path(), &a).is_empty());
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    let mut expected = [format!("replaces: {a}"), format!("replaces: {a2}")];
    expected.sort();
    assert_eq!(note(repo.path(), &a3), expected);
}

#[test]
fn a_fixup_replaces_every_commit_folded_together() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    rebase(repo.path(), "HEAD~2", "2s/^pick/fixup/");
    let head = ok(repo.path(), &["rev-parse", "HEAD"]);
    let mut expected = [format!("replaces: {a}"), format!("replaces: {b}")];
    expected.sort();
    assert_eq!(note(repo.path(), &head), expected);
    assert!(note(repo.path(), &a).is_empty());
    assert!(note(repo.path(), &b).is_empty());
}

#[test]
fn a_rebase_follows_an_amend_during_it() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    let base = ok(repo.path(), &["rev-parse", "HEAD~2"]);
    let script = "1s/$/\\nexec git commit -q --amend -m amended/";
    rebase(repo.path(), &base, script);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    let b2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    // The commit that the rebase made of `a` before the amend is named nowhere.
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert_eq!(note(repo.path(), &b2), [format!("replaces: {b}")]);
    let notes = ok(repo.path(), &["notes", "--ref=replaces", "list"]);
    assert_eq!(notes.lines().count(), 2);
}
