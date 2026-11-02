//! Lines are only ever added to the notes, so a step of the reflog of the notes ref that drops one
//! was made by a writer that overwrote another, and the hook puts the lines back.

mod common;

use std::path::Path;

use common::{amend_of, amended, commit, git, hook, note, ok, ok_command, repo};

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

#[test]
fn notes_moved_to_another_fanout_are_left_alone() {
    let (repo, a, a2) = amended();
    let repo = repo.path();
    amend(repo, &a, &a2);
    // Moves the note one level of fanout down, as a writer does to every note once their number
    // reaches 256, which git diff-tree shows as a note removed and another added.
    let last = ok(repo, &["rev-parse", "refs/notes/replaces"]);
    let blob = ok(repo, &["rev-parse", &format!("{last}:{a2}")]);
    let index = repo.join(".git/moved-index");
    let with_index = |args: &[&str]| {
        let mut command = git(repo, args);
        command.env("GIT_INDEX_FILE", &index);
        ok_command(command)
    };
    with_index(&["read-tree", "--empty"]);
    let nested = format!("100644,{blob},{}/{}", &a2[..2], &a2[2..]);
    with_index(&["update-index", "--add", "--cacheinfo", &nested]);
    let tree = with_index(&["write-tree"]);
    let moved = ok(repo, &["commit-tree", &tree, "-p", &last, "-m", "fanout"]);
    ok(repo, &["update-ref", "refs/notes/replaces", &moved, &last]);

    let b = commit(repo, "b");
    let b2 = amend_of(repo, &b);
    let stderr = amend(repo, &b, &b2);
    assert!(!stderr.contains("put back"), "{stderr}");
    assert_eq!(note(repo, &a2), [format!("replaces: {a}")]);
}

#[test]
fn a_file_that_is_no_note_may_be_removed() {
    let (repo, a, a2) = amended();
    let repo = repo.path();
    amend(repo, &a, &a2);
    // Adds a file that is no note to the notes tree, then removes it.
    let index = repo.join(".git/other-index");
    let with_index = |args: &[&str]| {
        let mut command = git(repo, args);
        command.env("GIT_INDEX_FILE", &index);
        ok_command(command)
    };
    let readme = ok(repo, &["hash-object", "-w", "--stdin"]);
    for change in [
        vec![
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{readme},README"),
        ],
        vec!["update-index", "--force-remove", "README"],
    ] {
        let last = ok(repo, &["rev-parse", "refs/notes/replaces"]);
        with_index(&["read-tree", &last]);
        with_index(&change);
        let tree = with_index(&["write-tree"]);
        let next = ok(repo, &["commit-tree", &tree, "-p", &last, "-m", "readme"]);
        ok(repo, &["update-ref", "refs/notes/replaces", &next, &last]);
    }

    let b = commit(repo, "b");
    let b2 = amend_of(repo, &b);
    let stderr = amend(repo, &b, &b2);
    assert!(!stderr.contains("put back"), "{stderr}");
}

/// Records a rewrite, whose repair checks the notes as they are, then removes from `repo` every
/// object that no ref reaches, as garbage collection does once the reflogs expire, and then the
/// notes of the commits removed, as `git notes prune` does. A line that came and went between two
/// checks, while the reflog expired, is beyond what the repair can see.
fn collect_and_prune(repo: &Path) {
    let c = commit(repo, "c");
    let c2 = amend_of(repo, &c);
    amend(repo, &c, &c2);
    ok(repo, &["update-ref", "refs/heads/c", &c2]);
    ok(repo, &["reflog", "expire", "--expire=now", "--all"]);
    ok(repo, &["gc", "-q", "--prune=now"]);
    ok(repo, &["notes", "--ref=replaces", "prune"]);
}

/// The commits that have a note.
fn annotated(repo: &Path) -> Vec<String> {
    ok(repo, &["notes", "--ref=replaces", "list"])
        .lines()
        .filter_map(|line| Some(line.split_once(' ')?.1.to_owned()))
        .collect()
}

#[test]
fn the_note_of_a_commit_gone_is_kept_in_the_notes_that_name_it() {
    let (repo, a, a2) = amended();
    let repo = repo.path();
    amend(repo, &a, &a2);
    let a3 = amend_of(repo, &a2);
    amend(repo, &a2, &a3);
    ok(repo, &["update-ref", "refs/heads/kept", &a3]);
    // A line that the note of `a'` holds and that of `a''` lacks, as when the note of `a'` arrived
    // from another clone after `a''` was made.
    let other = format!("replaces: {}", "1".repeat(40));
    ok(
        repo,
        &["notes", "--ref=replaces", "append", "-m", &other, &a2],
    );
    collect_and_prune(repo);
    assert!(!annotated(repo).contains(&a2));

    let b = commit(repo, "b");
    let b2 = amend_of(repo, &b);
    let stderr = amend(repo, &b, &b2);
    assert!(
        stderr.contains(&format!("the note of {a2}, which this clone no longer has")),
        "{stderr}"
    );
    assert!(!stderr.contains("put back"), "{stderr}");
    assert!(note(repo, &a3).contains(&other), "{:?}", note(repo, &a3));
    // The note of `a'` stays pruned.
    assert!(!annotated(repo).contains(&a2));
}

#[test]
fn the_note_of_a_commit_gone_that_no_note_names_is_put_back() {
    let repo = repo();
    let repo = repo.path();
    commit(repo, "base");
    // A commit that no ref reaches, and whose note no other names.
    let x = ok(repo, &["commit-tree", "HEAD^{tree}", "-m", "x"]);
    let line = format!("replaces: {}", "2".repeat(40));
    ok(repo, &["notes", "--ref=replaces", "add", "-m", &line, &x]);
    collect_and_prune(repo);
    assert!(!annotated(repo).contains(&x));

    let b = commit(repo, "b");
    let b2 = amend_of(repo, &b);
    let stderr = amend(repo, &b, &b2);
    assert!(stderr.contains("put back"), "{stderr}");
    assert!(annotated(repo).contains(&x));
}

#[test]
fn a_step_after_the_reflog_expired_is_checked() {
    let (repo, a, a2) = amended();
    let repo = repo.path();
    amend(repo, &a, &a2);
    // The reflog no longer holds the step that made the notes, and what checked them last is
    // unreachable once a writer forces the ref to notes without the note of `a'`.
    ok(repo, &["reflog", "expire", "--expire=now", "--all"]);
    let x = commit(repo, "x");
    let empty = ok(repo, &["mktree"]);
    let other = ok(repo, &["commit-tree", &empty, "-m", "other notes"]);
    let message = format!("replaces: {x}");
    ok(repo, &["update-ref", "refs/notes/replaces", &other]);
    ok(
        repo,
        &["notes", "--ref=replaces", "add", "-m", &message, &x],
    );
    ok(repo, &["gc", "-q", "--prune=now"]);
    assert!(note(repo, &a2).is_empty());

    let b = commit(repo, "b");
    let b2 = amend_of(repo, &b);
    let stderr = amend(repo, &b, &b2);
    assert!(stderr.contains("put back"), "{stderr}");
    assert_eq!(note(repo, &a2), [format!("replaces: {a}")]);
}
