//! Each run of the hook records the abandoned records before its own rewrite, and removes them once
//! recorded.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

use common::{
    abandoned, amend_of, amended, commit, failing, fake_git, git, hook, note, ok, repo, stderr_of,
};

/// Writes an abandoned record named `name` that holds `text`, as the hook writes them.
fn keep(repo: &Path, name: &str, text: &str) -> PathBuf {
    let dir = repo.join(".git/replaces-abandoned");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

/// The names in the directory of abandoned records, those of temporary files included.
fn names(repo: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(repo.join(".git/replaces-abandoned")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// Amends HEAD through git, which runs the hook, and returns the amended commit, the new one, and
/// the standard error.
fn amend(repo: &Path) -> (String, String, String) {
    let old = ok(repo, &["rev-parse", "HEAD"]);
    let stderr = stderr_of(git(repo, &["commit", "-q", "--amend", "-m", "amended"]));
    (old, ok(repo, &["rev-parse", "HEAD"]), stderr)
}

#[test]
fn the_next_amend_records_an_abandoned_record() {
    let repo = repo();
    let x = commit(repo.path(), "x");
    let x2 = amend_of(repo.path(), &x);
    keep(
        repo.path(),
        "000000000001.000000000-0",
        &format!("{x} {x2}\n"),
    );
    let (a, a2, stderr) = amend(repo.path());
    assert_eq!(
        note(repo.path(), &x2),
        [format!("replaces: {x}")],
        "{stderr}"
    );
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(names(repo.path()).is_empty());
}

#[test]
fn records_are_recorded_oldest_first() {
    let repo = repo();
    let x = commit(repo.path(), "x");
    let x2 = amend_of(repo.path(), &x);
    let x3 = amend_of(repo.path(), &x2);
    // The newer record comes first in the directory's own order only by chance; its name sorts
    // after the older one.
    keep(
        repo.path(),
        "000000000002.000000000-0",
        &format!("{x2} {x3}\n"),
    );
    keep(
        repo.path(),
        "000000000001.000000000-0",
        &format!("{x} {x2}\n"),
    );
    amend(repo.path());
    // The later replacement inherits what the earlier one recorded in the same run.
    let mut expected = [format!("replaces: {x}"), format!("replaces: {x2}")];
    expected.sort();
    assert_eq!(note(repo.path(), &x3), expected);
}

#[test]
fn a_record_already_in_the_notes_is_removed() {
    let (repo, a, a2) = amended();
    ok(
        repo.path(),
        &[
            "notes",
            "--ref=replaces",
            "add",
            "-m",
            &format!("replaces: {a}"),
            &a2,
        ],
    );
    let before = ok(repo.path(), &["rev-parse", "refs/notes/replaces"]);
    keep(
        repo.path(),
        "000000000001.000000000-0",
        &format!("{a} {a2}\n"),
    );
    let output = hook(repo.path(), "amend", "", None).output().unwrap();
    assert!(output.status.success());
    // Nothing was written, and the record is gone all the same.
    assert_eq!(
        ok(repo.path(), &["rev-parse", "refs/notes/replaces"]),
        before
    );
    assert!(names(repo.path()).is_empty());
}

#[test]
fn lines_that_cannot_be_recorded_are_dropped_with_a_warning() {
    let (repo, a, a2) = amended();
    let gone = "1".repeat(40);
    keep(
        repo.path(),
        "000000000001.000000000-0",
        &format!("{a} {gone}\nnot a replacement\n{a} {a2}\n"),
    );
    keep(
        repo.path(),
        "000000000002.000000000-0",
        "nothing to record\n",
    );
    let output = hook(repo.path(), "amend", "", None).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains(&gone), "{stderr}");
    assert!(stderr.contains("not a replacement"), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(names(repo.path()).is_empty());
}

#[test]
fn a_record_half_written_is_left_alone() {
    let (repo, a, a2) = amended();
    keep(
        repo.path(),
        ".000000000001.000000000-0.tmp",
        &format!("{a} {a2}\n"),
    );
    let output = hook(repo.path(), "amend", "", None).output().unwrap();
    assert!(output.status.success());
    assert!(note(repo.path(), &a2).is_empty());
    assert_eq!(names(repo.path()), [".000000000001.000000000-0.tmp"]);
}

#[test]
fn records_wait_while_the_rewrite_cannot_be_recorded() {
    let repo = repo();
    let x = commit(repo.path(), "x");
    let x2 = amend_of(repo.path(), &x);
    let earlier = keep(
        repo.path(),
        "000000000001.000000000-0",
        &format!("{x} {x2}\n"),
    );
    let a = commit(repo.path(), "a");
    let a2 = amend_of(repo.path(), &a);
    let bin = fake_git(repo.path(), "fast-import");
    let (stderr, _) = failing(hook(
        repo.path(),
        "amend",
        format!("{a} {a2}\n"),
        Some(&bin),
    ));
    // The earlier record stays as it was, and the new one holds the rewrite alone.
    assert!(earlier.exists(), "{stderr}");
    assert_eq!(
        abandoned(repo.path()),
        [format!("{x} {x2}"), format!("{a} {a2}")]
    );
    assert!(stderr.contains("--abandoned"), "{stderr}");
}

#[test]
fn two_runs_may_record_the_same_record() {
    let repo = repo();
    let x = commit(repo.path(), "x");
    let x2 = amend_of(repo.path(), &x);
    keep(
        repo.path(),
        "000000000001.000000000-0",
        &format!("{x} {x2}\n"),
    );
    let a = commit(repo.path(), "a");
    let a2 = amend_of(repo.path(), &a);
    let b = commit(repo.path(), "b");
    let b2 = amend_of(repo.path(), &b);
    let runs: Vec<_> = [(a.clone(), a2.clone()), (b.clone(), b2.clone())]
        .into_iter()
        .map(|(old, new)| {
            let mut command = hook(repo.path(), "amend", format!("{old} {new}\n"), None);
            thread::spawn(move || command.output().unwrap())
        })
        .collect();
    for run in runs {
        let output = run.join().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
    }
    assert_eq!(note(repo.path(), &x2), [format!("replaces: {x}")]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert_eq!(note(repo.path(), &b2), [format!("replaces: {b}")]);
    assert!(names(repo.path()).is_empty());
}

#[test]
fn the_main_checkout_records_what_a_worktree_abandoned() {
    let (repo, a, a2) = amended();
    let worktree = repo.path().join("worktree");
    ok(
        repo.path(),
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree.to_str().unwrap(),
        ],
    );
    let bin = fake_git(repo.path(), "fast-import");
    failing(hook(&worktree, "amend", format!("{a} {a2}\n"), Some(&bin)));
    assert_eq!(abandoned(repo.path()), [format!("{a} {a2}")]);
    amend(repo.path());
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(names(repo.path()).is_empty());
}

#[test]
fn a_record_that_cannot_be_read_is_left_where_it_is() {
    let (repo, a, a2) = amended();
    let dir = repo.path().join(".git/replaces-abandoned");
    fs::create_dir_all(dir.join("000000000001.000000000-0")).unwrap();
    keep(
        repo.path(),
        "000000000002.000000000-0",
        &format!("{a} {a2}\n"),
    );
    let output = hook(repo.path(), "amend", "", None).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("could not be read"), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert_eq!(names(repo.path()), ["000000000001.000000000-0"]);
}
