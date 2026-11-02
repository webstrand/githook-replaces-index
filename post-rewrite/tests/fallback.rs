//! When the hook cannot keep track of the deferred replacements of a rebase, it records what it has
//! in a cruder form, which names commits that it would otherwise leave out, rather than abandon the
//! rewrite.

mod common;

use std::fs;
use std::path::Path;

use common::{
    abandoned, amend_of, commit, fake_git, git, hook, interrupt_waiting, note, notes_lock, ok,
    rebase_keeping, repo, stderr_of,
};

/// A scratch repository with commits `a` and `b` on a branch, and a branch `upstream` with a
/// commit that they lack, so that a rebase onto it makes new commits of both.
fn diverged() -> (tempfile::TempDir, String, String) {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    ok(repo.path(), &["switch", "-q", "-c", "upstream", "HEAD~2"]);
    commit(repo.path(), "upstream");
    ok(repo.path(), &["switch", "-q", "-"]);
    (repo, a, b)
}

/// Rebases onto `upstream` with a todo list that `script` edits, and returns the standard error.
fn rebase_onto_upstream(repo: &Path, script: &str) -> String {
    let mut command = git(repo, &["rebase", "-q", "-i", "upstream"]);
    command.env("GIT_SEQUENCE_EDITOR", format!("sed -i -e '{script}'"));
    stderr_of(command)
}

/// The replacements that the hook deferred during the rebase in progress.
fn deferred(repo: &Path) -> Vec<(String, String)> {
    let text = fs::read_to_string(repo.join(".git/rebase-merge/replaces-index-deferred")).unwrap();
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (old, new) = line.split_once(' ').unwrap();
            (old.to_owned(), new.to_owned())
        })
        .collect()
}

/// Asserts that the hook said why it recorded the rewrite in a cruder form, and kept no record.
fn assert_fell_back(repo: &Path, stderr: &str) {
    assert!(stderr.contains("replaces-index:"), "{stderr}");
    assert!(abandoned(repo).is_empty(), "{stderr}");
}

/// An exec line that makes the file of the deferred replacements unreadable, as a directory.
const BREAK_DEFERRED: &str =
    "exec f=$(git rev-parse --git-path rebase-merge/replaces-index-deferred); rm -f $f; mkdir $f";

#[test]
fn a_replacement_that_cannot_be_deferred_is_recorded_at_once() {
    let (repo, a, _) = diverged();
    rebase_keeping(repo.path(), "upstream", "1s/^pick/edit/");
    let a1 = ok(repo.path(), &["rev-parse", "HEAD"]);
    fs::create_dir(
        repo.path()
            .join(".git/rebase-merge/replaces-index-deferred"),
    )
    .unwrap();
    let stderr = stderr_of(git(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended"],
    ));
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_fell_back(repo.path(), &stderr);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a1}")]);
    let stderr = stderr_of(git(repo.path(), &["rebase", "--continue"]));
    assert_fell_back(repo.path(), &stderr);
    // git reports the commit that the amend made as the one the rebase made of `a`.
    let mut expected = [format!("replaces: {a}"), format!("replaces: {a1}")];
    expected.sort();
    assert_eq!(note(repo.path(), &a2), expected, "{stderr}");
}

#[test]
fn a_single_commit_rewrite_during_a_rebase_whose_state_cannot_be_found_is_recorded_at_once() {
    let (repo, _, _) = diverged();
    rebase_keeping(repo.path(), "upstream", "1s/^pick/edit/");
    let a1 = ok(repo.path(), &["rev-parse", "HEAD"]);
    let a2 = amend_of(repo.path(), &a1);
    let bin = fake_git(repo.path(), "--git-path");
    let output = hook(repo.path(), "amend", format!("{a1} {a2}\n"), Some(&bin))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_fell_back(repo.path(), &stderr);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a1}")]);
}

#[test]
fn a_rebase_whose_deferred_replacements_cannot_be_read_records_what_git_reported() {
    let (repo, a, b) = diverged();
    let script = format!("1s|$|\\nexec git commit -q --amend -m amended\\n{BREAK_DEFERRED}|");
    let stderr = rebase_onto_upstream(repo.path(), &script);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    let b1 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_fell_back(repo.path(), &stderr);
    assert_eq!(note(repo.path(), &b1), [format!("replaces: {b}")]);
    // The rebase reported the commit it made of `a` before the amend, which is all there is.
    let notes = ok(repo.path(), &["notes", "--ref=replaces", "list"]);
    assert_eq!(notes.lines().count(), 2, "{notes}");
    assert!(note(repo.path(), &a2).is_empty());
    assert!(notes.lines().all(|line| !line.ends_with(&a)));
}

#[test]
fn a_rebase_whose_deferred_replacements_cannot_be_read_abandons_what_git_reported() {
    let (repo, a, b) = diverged();
    fs::write(notes_lock(repo.path()), "").unwrap();
    let script = format!("1s|$|\\nexec git commit -q --amend -m amended\\n{BREAK_DEFERRED}|");
    let mut command = git(repo.path(), &["rebase", "-q", "-i", "upstream"]);
    command.env("GIT_SEQUENCE_EDITOR", format!("sed -i -e '{script}'"));
    let stderr = interrupt_waiting(command);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    let b1 = ok(repo.path(), &["rev-parse", "HEAD"]);
    let lines = abandoned(repo.path());
    assert_eq!(lines.len(), 2, "{stderr}");
    let (old, new) = lines[0].split_once(' ').unwrap();
    assert_eq!(old, a);
    assert_ne!(new, a2);
    assert_eq!(lines[1], format!("{b} {b1}"));
}

/// Stops a rebase onto `upstream` after an amend of its commit of `a`, and returns that commit
/// and the commit that the amend made.
fn stopped_after_an_amend(repo: &Path) -> (String, String) {
    rebase_keeping(
        repo,
        "upstream",
        "1s/$/\\nexec git commit -q --amend -m amended\\nbreak/",
    );
    let deferred = deferred(repo);
    assert_eq!(deferred.len(), 1);
    deferred.into_iter().next().unwrap()
}

#[test]
fn a_rebase_whose_branch_cannot_be_read_records_every_deferred_replacement() {
    let (repo, a, _) = diverged();
    let (a1, a2) = stopped_after_an_amend(repo.path());
    let bin = fake_git(repo.path(), "merge-base");
    let output = hook(repo.path(), "rebase", format!("{a} {a1}\n"), Some(&bin))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_fell_back(repo.path(), &stderr);
    assert_eq!(note(repo.path(), &a1), [format!("replaces: {a}")]);
    let mut expected = [format!("replaces: {a}"), format!("replaces: {a1}")];
    expected.sort();
    assert_eq!(note(repo.path(), &a2), expected);
}

#[test]
fn a_rebase_whose_state_cannot_be_found_records_what_git_reported() {
    let (repo, a, _) = diverged();
    let (a1, a2) = stopped_after_an_amend(repo.path());
    let bin = fake_git(repo.path(), "--git-path");
    let output = hook(repo.path(), "rebase", format!("{a} {a1}\n"), Some(&bin))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_fell_back(repo.path(), &stderr);
    assert_eq!(note(repo.path(), &a1), [format!("replaces: {a}")]);
    assert!(note(repo.path(), &a2).is_empty());
}

#[test]
fn filter_repo_does_not_look_for_a_rebase() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let a2 = amend_of(repo.path(), &a);
    let bin = fake_git(repo.path(), "--git-path");
    let output = hook(
        repo.path(),
        "filter-repo",
        format!("{a} {a2}\n"),
        Some(&bin),
    )
    .output()
    .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains("replaces-index:"), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}
