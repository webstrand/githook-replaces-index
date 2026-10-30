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
    rebase_with(repo, &["--force-rebase", onto], script);
}

/// Rebases as `rebase` does, but keeps each commit whose parent is unchanged, as a rebase does by
/// default.
fn rebase_keeping(repo: &Path, onto: &str, script: &str) {
    rebase_with(repo, &[onto], script);
}

fn rebase_with(repo: &Path, args: &[&str], script: &str) {
    let mut command = git(repo, &["rebase", "-q", "-i"]);
    command.args(args);
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

#[test]
fn a_rebase_does_not_follow_an_amend_that_was_undone() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    // A new base, so that the rebase makes another commit of `a` within the same second.
    ok(repo.path(), &["switch", "-q", "-c", "upstream", "HEAD~"]);
    commit(repo.path(), "upstream");
    ok(repo.path(), &["switch", "-q", "-"]);
    rebase(repo.path(), "upstream", "1s/^pick/edit/");
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["commit", "-q", "--amend", "-m", "undone"]);
    let undone = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["reset", "-q", "--hard", &a2]);
    ok(repo.path(), &["rebase", "--continue"]);
    assert_eq!(ok(repo.path(), &["rev-parse", "HEAD"]), a2);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(note(repo.path(), &undone).is_empty());
}

#[test]
fn a_rebase_records_an_amend_of_a_commit_it_kept() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    let script = "1s/$/\\nexec git commit -q --amend -m amended/";
    rebase_keeping(repo.path(), "HEAD~2", script);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    let b2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert_eq!(note(repo.path(), &b2), [format!("replaces: {b}")]);
}

#[test]
fn a_rebase_that_rewrites_nothing_else_records_an_amend() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    // Every pick keeps its commit, so git reports no rewrite when the rebase ends.
    let script = "$s/$/\\nexec git commit -q --amend -m amended/";
    rebase_keeping(repo.path(), "HEAD~", script);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}

#[test]
fn a_rebase_records_an_amend_of_the_commit_it_rebases_onto() {
    let repo = repo();
    let base = ok(repo.path(), &["rev-parse", "HEAD"]);
    let a = commit(repo.path(), "a");
    rebase_keeping(repo.path(), "HEAD~", "1s/^/break\\n/");
    ok(
        repo.path(),
        &[
            "commit",
            "-q",
            "--amend",
            "--allow-empty",
            "-m",
            "base, amended",
        ],
    );
    let base2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["rebase", "--continue"]);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(ok(repo.path(), &["rev-parse", "HEAD~"]), base2);
    assert_eq!(note(repo.path(), &base2), [format!("replaces: {base}")]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}
