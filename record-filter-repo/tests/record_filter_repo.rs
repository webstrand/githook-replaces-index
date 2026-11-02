//! Runs `record-filter-repo` in a scratch repository, on maps written as git filter-repo writes
//! them.

use std::fs;
use std::path::Path;
use std::process::Command;

const RECORD: &str = env!("CARGO_BIN_EXE_replaces-index-record-filter-repo");
const REF: &str = "refs/notes/replaces";

fn id(c: char) -> String {
    c.to_string().repeat(40)
}

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

fn run(repo: &Path, program: &str, args: &[&str]) -> (bool, String, String) {
    let output = isolated(repo, program, args).output().unwrap();
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn note(repo: &Path, commit: &str) -> String {
    run(repo, "git", &["notes", "--ref=replaces", "show", commit]).1
}

/// Makes an empty commit that stands for one that filter-repo made, and returns its id.
fn commit(repo: &Path, message: &str) -> String {
    let (ok, _, stderr) = run(
        repo,
        "git",
        &["commit", "-q", "--allow-empty", "-m", message],
    );
    assert!(ok, "{stderr}");
    run(repo, "git", &["rev-parse", "HEAD"]).1.trim().to_owned()
}

#[test]
fn records_each_run_and_the_replacement_of_the_previous_result() {
    let repo = tempfile::tempdir().unwrap();
    let repo = repo.path();
    assert!(run(repo, "git", &["init", "-q"]).0);
    let dir = repo.join(".git/filter-repo");
    fs::create_dir(&dir).unwrap();
    // The commits that filter-repo replaced are gone; those it made are in the clone.
    let (a, d, z) = (id('a'), id('d'), id('0'));
    let b = commit(repo, "b");

    fs::write(
        dir.join("commit-map"),
        format!("old new\n{a} {b}\n{d} {z}\n"),
    )
    .unwrap();
    let (ok, _, stderr) = run(repo, RECORD, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(note(repo, &b), format!("replaces: {a}\n"));
    assert_eq!(note(repo, &d), "");
    let recorded = run(repo, "git", &["rev-parse", REF]).1;

    // Running it again records nothing new.
    assert!(run(repo, RECORD, &[]).0);
    assert_eq!(run(repo, "git", &["rev-parse", REF]).1, recorded);

    // A second run of filter-repo maps `a` to its newest replacement only.
    let c = commit(repo, "c");
    fs::write(dir.join("commit-map"), format!("old new\n{a} {c}\n")).unwrap();
    let (ok, _, stderr) = run(repo, RECORD, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(note(repo, &b), format!("replaces: {a}\n"));
    let mut expected = [format!("replaces: {a}\n"), format!("replaces: {b}\n")];
    expected.sort();
    assert_eq!(note(repo, &c), expected.concat());
}

#[test]
fn fails_where_filter_repo_has_not_run() {
    let repo = tempfile::tempdir().unwrap();
    let repo = repo.path();
    assert!(run(repo, "git", &["init", "-q"]).0);
    let (ok, _, stderr) = run(repo, RECORD, &[]);
    assert!(!ok);
    assert!(
        stderr.ends_with("git filter-repo has not run here\n"),
        "{stderr}"
    );
}

#[test]
fn keeps_no_copy_of_a_map_it_could_not_record() {
    let repo = tempfile::tempdir().unwrap();
    let repo = repo.path();
    assert!(run(repo, "git", &["init", "-q"]).0);
    let dir = repo.join(".git/filter-repo");
    fs::create_dir(&dir).unwrap();
    let a = id('a');
    let b = commit(repo, "b");
    fs::write(dir.join("commit-map"), format!("old new\n{a} {b}\n")).unwrap();
    // A notes ref that is not a commit cannot be written to.
    let blob = run(repo, "git", &["hash-object", "-w", "--stdin"]).1;
    assert!(run(repo, "git", &["update-ref", REF, blob.trim()]).0);
    let (ok, _, stderr) = run(repo, RECORD, &[]);
    assert!(!ok);
    assert!(stderr.contains("not a commit"), "{stderr}");
    // The copy would mean that the map was recorded, and a later run would compose with it.
    assert!(!dir.join("replaces-index-commit-map").exists());
}

#[test]
fn says_what_it_waits_for() {
    let repo = tempfile::tempdir().unwrap();
    let repo = repo.path();
    assert!(run(repo, "git", &["init", "-q"]).0);
    let dir = repo.join(".git/filter-repo");
    fs::create_dir(&dir).unwrap();
    let a = id('a');
    let b = commit(repo, "b");
    fs::write(dir.join("commit-map"), format!("old new\n{a} {b}\n")).unwrap();
    let notes = repo.join(".git/refs/notes");
    fs::create_dir_all(&notes).unwrap();
    let lock = notes.join("replaces.lock");
    fs::write(&lock, "").unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2500));
        fs::remove_file(lock).unwrap();
    });
    let (ok, _, stderr) = run(repo, RECORD, &[]);
    release.join().unwrap();
    assert!(ok, "{stderr}");
    assert!(
        stderr.contains("replaces-index: refs/notes/replaces is locked, retrying"),
        "{stderr}"
    );
    assert_eq!(note(repo, &b), format!("replaces: {a}\n"));
}
