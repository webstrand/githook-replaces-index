//! Runs `record` in a scratch repository.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const RECORD: &str = env!("CARGO_BIN_EXE_replaces-index-record");
const REF: &str = "refs/notes/replaces";

/// A command in `repo`, isolated from the configuration and the session around the test.
fn isolated(repo: &Path, program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
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

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = isolated(repo, "git", args).output().unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap())
}

/// A scratch repository with an empty commit for each of `messages`, and their ids.
fn repo<const N: usize>(messages: [&str; N]) -> (tempfile::TempDir, [String; N]) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]).unwrap();
    let ids = messages.map(|message| {
        git(
            dir.path(),
            &["commit", "-q", "--allow-empty", "-m", message],
        )
        .unwrap();
        git(dir.path(), &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_owned()
    });
    (dir, ids)
}

fn record(repo: &Path, input: &str) {
    let mut child = isolated(repo, RECORD, &[])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn note(repo: &Path, commit: &str) -> Option<String> {
    git(repo, &["notes", "--ref=replaces", "show", commit])
}

/// `replaces:` lines for `commits`, sorted, as a note holds them.
fn replaces(commits: &[&String]) -> String {
    let mut lines: Vec<String> = commits.iter().map(|c| format!("replaces: {c}\n")).collect();
    lines.sort();
    lines.concat()
}

#[test]
fn writes_the_note_of_the_new_commit_only() {
    let (dir, ids) = repo(["a", "b", "c"]);
    let (repo, [a, b, c]) = (dir.path(), ids.each_ref());

    record(repo, &format!("{a} {b}\n"));
    assert_eq!(note(repo, b), Some(replaces(&[a])));
    assert_eq!(note(repo, a), None);

    record(repo, &format!("{b} {c}\n"));
    assert_eq!(note(repo, c), Some(replaces(&[a, b])));
    assert_eq!(note(repo, b), Some(replaces(&[a])));
    assert_eq!(note(repo, a), None);
    assert_eq!(
        git(repo, &["rev-list", "--count", REF]).as_deref(),
        Some("2\n")
    );
}

#[test]
fn leaves_the_ref_alone_when_nothing_changes() {
    let (dir, ids) = repo(["a", "b"]);
    let (repo, [a, b]) = (dir.path(), ids.each_ref());
    record(repo, "");
    assert_eq!(git(repo, &["rev-parse", "-q", "--verify", REF]), None);

    record(repo, &format!("{a} {b}\n"));
    let before = git(repo, &["rev-parse", REF]);
    record(repo, &format!("{a} {b}\n"));
    assert_eq!(git(repo, &["rev-parse", REF]), before);
    // The staging ref is removed.
    assert_eq!(
        git(repo, &["for-each-ref", "refs/replaces-index/"]).as_deref(),
        Some("")
    );
}

#[test]
fn keeps_the_notes_of_a_merge_from_another_clone() {
    let (dir, ids) = repo(["a", "b", "x", "c"]);
    let (repo, [a, b, x, c]) = (dir.path(), ids.each_ref());
    record(repo, &format!("{a} {b}\n"));
    // A line that a merge with another clone's notes added to the note of `b`.
    let ref_arg = "--ref=replaces";
    git(
        repo,
        &[
            "notes",
            ref_arg,
            "append",
            "-m",
            &format!("replaces: {x}"),
            b,
        ],
    )
    .unwrap();

    record(repo, &format!("{b} {c}\n"));
    assert_eq!(note(repo, c), Some(replaces(&[a, b, x])));
}
