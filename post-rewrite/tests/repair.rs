//! Lines are only ever added to the notes, so a step of the reflog of the notes ref that drops one
//! was made by a writer that overwrote another, and the hook puts the lines back.

mod common;

use std::path::Path;

use common::{amend_of, amended, commit, hook, note, ok, repo};

/// Runs the hook on the amend `old` to `new`, which must succeed, and returns its standard error.
fn amend(repo: &Path, old: &str, new: &str) -> String {
    let output = hook(repo, "amend", format!("{old} {new}\n"), None)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{stderr}");
    stderr
}

/// Overwrites the last write to the notes ref as `git notes add` does in a git that does not check
/// the ref: it adds a note on `commit` to the notes as they were before that write, and commits
/// that on top of the write.
fn overwrite(repo: &Path, commit: &str) {
    let last = ok(repo, &["rev-parse", "refs/notes/replaces"]);
    let before = ok(repo, &["rev-parse", "refs/notes/replaces~"]);
    ok(repo, &["update-ref", "refs/notes/loaded", &before]);
    ok(
        repo,
        &["notes", "--ref=loaded", "add", "-f", "-m", "other", commit],
    );
    let tree = ok(repo, &["rev-parse", "refs/notes/loaded^{tree}"]);
    let message = "Notes added by 'git notes add'";
    let new = ok(repo, &["commit-tree", &tree, "-p", &last, "-m", message]);
    ok(
        repo,
        &[
            "update-ref",
            "-m",
            "notes: Notes added by 'git notes add'",
            "refs/notes/replaces",
            &new,
            &last,
        ],
    );
    ok(repo, &["update-ref", "-d", "refs/notes/loaded"]);
}

#[test]
fn lines_that_another_writer_dropped_are_put_back() {
    let (repo, a, a2) = amended();
    let x = commit(repo.path(), "x");
    let x2 = amend_of(repo.path(), &x);
    amend(repo.path(), &x, &x2);
    amend(repo.path(), &a, &a2);
    overwrite(repo.path(), &x);
    assert!(note(repo.path(), &a2).is_empty());

    let b = commit(repo.path(), "b");
    let b2 = amend_of(repo.path(), &b);
    let stderr = amend(repo.path(), &b, &b2);
    assert!(stderr.contains("put back"), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    // What the other writer added, and what the hook recorded since, stay.
    assert_eq!(note(repo.path(), &x), ["other"]);
    assert_eq!(note(repo.path(), &b2), [format!("replaces: {b}")]);

    // The step is put right once.
    let c = commit(repo.path(), "c");
    let c2 = amend_of(repo.path(), &c);
    let stderr = amend(repo.path(), &c, &c2);
    assert!(!stderr.contains("put back"), "{stderr}");
}

#[test]
fn a_note_removed_by_hand_is_put_back() {
    let (repo, a, a2) = amended();
    amend(repo.path(), &a, &a2);
    ok(repo.path(), &["notes", "--ref=replaces", "remove", &a2]);
    let b = commit(repo.path(), "b");
    let b2 = amend_of(repo.path(), &b);
    let stderr = amend(repo.path(), &b, &b2);
    assert!(stderr.contains("put back"), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}

#[test]
fn writes_that_only_add_lines_are_left_alone() {
    let (repo, a, a2) = amended();
    amend(repo.path(), &a, &a2);
    let a3 = amend_of(repo.path(), &a2);
    let stderr = amend(repo.path(), &a2, &a3);
    assert_eq!(stderr, "");
    let writes = ok(repo.path(), &["rev-list", "--count", "refs/notes/replaces"]);
    assert_eq!(writes, "2");
}

#[test]
fn a_clone_without_a_reflog_of_its_notes_is_not_checked() {
    let repo = repo();
    ok(repo.path(), &["config", "core.logAllRefUpdates", "false"]);
    let x = commit(repo.path(), "x");
    let x2 = amend_of(repo.path(), &x);
    amend(repo.path(), &x, &x2);
    let a = commit(repo.path(), "a");
    let a2 = amend_of(repo.path(), &a);
    amend(repo.path(), &a, &a2);
    overwrite(repo.path(), &x);
    let b = commit(repo.path(), "b");
    let b2 = amend_of(repo.path(), &b);
    let stderr = amend(repo.path(), &b, &b2);
    assert_eq!(stderr, "");
    assert!(note(repo.path(), &a2).is_empty());
}

#[test]
fn a_note_that_git_notes_add_overwrote_is_put_back() {
    let (repo, a, a2) = amended();
    let x = commit(repo.path(), "x");
    let x2 = amend_of(repo.path(), &x);
    amend(repo.path(), &x, &x2);
    // While `git notes add` waits for its editor, the hook records an amend. A git that moves the
    // ref without naming the commit it read then drops that note; one that names it refuses.
    let editor = repo.path().join("editor");
    std::fs::write(
        &editor,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{a} {a2}' | '{}' filter-repo\necho other > \"$1\"\n",
            common::HOOK
        ),
    )
    .unwrap();
    std::fs::set_permissions(&editor, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut notes_add = common::git(repo.path(), &["notes", "--ref=replaces", "add", "-f", &x]);
    notes_add.env("GIT_EDITOR", &editor);
    let overwrote = notes_add.output().unwrap().status.success();

    let b = commit(repo.path(), "b");
    let b2 = amend_of(repo.path(), &b);
    let stderr = amend(repo.path(), &b, &b2);
    // Only a git that overwrote the note leaves something to put back.
    assert_eq!(stderr.contains("put back"), overwrote, "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert_eq!(note(repo.path(), &x2), [format!("replaces: {x}")]);
}
