//! A rewrite that the hook cannot record is kept as an abandoned record in the common git
//! directory, and printed as a command that records it by hand.

mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    abandoned, amend_of, amended, commit, failing, fake_git, fake_git_holding, fake_git_racing,
    git, hook, interrupt_when, note, notes_lock, ok, printed, rebase, rebase_keeping, release,
    repo, stderr_of, with_git_env,
};

/// Asserts that the hook failed, kept `lines` as the only abandoned record, and printed them.
fn assert_abandoned(repo: &Path, stderr: &str, lines: &[String]) {
    assert_eq!(abandoned(repo), lines, "{stderr}");
    assert!(stderr.contains(&printed(lines)), "{stderr}");
    assert!(stderr.contains("was kept in"), "{stderr}");
}

/// Asserts that the hook failed, could keep no record, and printed `lines`.
fn assert_printed_only(repo: &Path, stderr: &str, lines: &[String]) {
    assert!(abandoned(repo).is_empty(), "{stderr}");
    assert!(stderr.contains(&printed(lines)), "{stderr}");
    assert!(stderr.contains("could not be kept"), "{stderr}");
}

#[test]
fn a_held_lock_abandons_an_amend() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    fs::write(notes_lock(repo.path()), "").unwrap();
    // The hook retries for as long as the lock is held, until the operator interrupts it.
    let stderr = interrupt_when(
        git(
            repo.path(),
            &["commit", "-q", "--amend", "-m", "a, amended"],
        ),
        "signal to abandon",
    );
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert!(stderr.contains("is locked, retrying"), "{stderr}");
    assert_abandoned(repo.path(), &stderr, &[format!("{a} {a2}")]);
}

#[test]
fn a_held_lock_abandons_the_end_of_a_rebase() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    // A new base, so that the rebase makes new commits of `a` and `b` within the same second.
    ok(repo.path(), &["switch", "-q", "-c", "upstream", "HEAD~2"]);
    commit(repo.path(), "upstream");
    ok(repo.path(), &["switch", "-q", "-"]);
    fs::write(notes_lock(repo.path()), "").unwrap();
    let mut command = git(repo.path(), &["rebase", "-q", "-i", "upstream"]);
    let script = "1s/$/\\nexec git commit -q --amend -m amended/";
    command.env("GIT_SEQUENCE_EDITOR", format!("sed -i -e '{script}'"));
    let stderr = interrupt_when(command, "signal to abandon");
    let a2 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    let b2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    // The record holds the amended commit, not the one the rebase made before the amend.
    assert_abandoned(
        repo.path(),
        &stderr,
        &[format!("{a} {a2}"), format!("{b} {b2}")],
    );
}

#[test]
fn a_lock_that_goes_away_is_waited_for() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let lock = notes_lock(repo.path());
    fs::write(&lock, "").unwrap();
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_secs(1));
        fs::remove_file(lock).unwrap();
    });
    stderr_of(git(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended"],
    ));
    release.join().unwrap();
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(abandoned(repo.path()).is_empty());
}

#[test]
fn a_lock_held_by_writers_that_move_the_ref_is_waited_for() {
    let repo = repo();
    let base = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(
        repo.path(),
        &["notes", "--ref=replaces", "add", "-m", "writer", &base],
    );
    let a = commit(repo.path(), "a");
    let lock = notes_lock(repo.path());
    fs::write(&lock, "").unwrap();
    // Another writer takes the lock in turns with this one, and moves the ref each time. It moves
    // it only from the commit it read, as `record` does; `git notes add` would not, and could
    // drop the hook's note.
    let path = repo.path().to_owned();
    let writer = thread::spawn(move || {
        let end = Instant::now() + Duration::from_millis(1500);
        let mut n = 0;
        while Instant::now() < end {
            fs::remove_file(&lock).unwrap();
            n += 1;
            let old = ok(&path, &["rev-parse", "refs/notes/replaces"]);
            let message = format!("writer {n}");
            let tree = format!("{old}^{{tree}}");
            let new = ok(&path, &["commit-tree", &tree, "-p", &old, "-m", &message]);
            let _ = git(&path, &["update-ref", "refs/notes/replaces", &new, &old]).output();
            while fs::File::create_new(&lock).is_err() {
                thread::sleep(Duration::from_millis(5));
            }
            thread::sleep(Duration::from_millis(30));
        }
        fs::remove_file(&lock).unwrap();
    });
    let stderr = stderr_of(git(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended"],
    ));
    writer.join().unwrap();
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(
        note(repo.path(), &a2),
        [format!("replaces: {a}")],
        "{stderr}"
    );
    assert!(abandoned(repo.path()).is_empty());
}

#[test]
fn a_notes_ref_that_is_no_commit_abandons_the_rewrite() {
    let (repo, a, a2) = amended();
    let blob = ok(repo.path(), &["hash-object", "-w", "--stdin"]);
    ok(repo.path(), &["update-ref", "refs/notes/replaces", &blob]);
    let (stderr, _) = failing(hook(repo.path(), "amend", format!("{a} {a2}\n"), None));
    assert_abandoned(repo.path(), &stderr, &[format!("{a} {a2}")]);
}

/// Runs the hook on an amend with a git that fails on `fail`, after an amend recorded without
/// failure when `recorded_before`, and asserts that the amend is abandoned.
fn abandons_when_git_fails(fail: &str, recorded_before: bool) {
    let (repo, a, a2) = amended();
    if recorded_before {
        let output = hook(repo.path(), "amend", format!("{a} {a2}\n"), None)
            .output()
            .unwrap();
        assert!(output.status.success());
    }
    let (old, new) = if recorded_before {
        (a2.clone(), amend_of(repo.path(), &a2))
    } else {
        (a, a2)
    };
    let bin = fake_git(repo.path(), fail);
    let input = format!("{old} {new}\n");
    let (stderr, _) = failing(hook(repo.path(), "amend", input, Some(&bin)));
    assert!(
        stderr.contains(&format!("fake git fails on {fail}")),
        "{stderr}"
    );
    assert_abandoned(repo.path(), &stderr, &[format!("{old} {new}")]);
}

#[test]
fn a_failed_fast_import_abandons_the_rewrite() {
    abandons_when_git_fails("fast-import", false);
}

#[test]
fn a_failed_lookup_of_the_committer_abandons_the_rewrite() {
    abandons_when_git_fails("var", false);
}

#[test]
fn a_failed_listing_of_the_notes_abandons_the_rewrite() {
    abandons_when_git_fails("notes", true);
}

#[test]
fn a_failed_read_of_the_notes_abandons_the_rewrite() {
    abandons_when_git_fails("cat-file", true);
}

#[test]
fn an_amend_recorded_at_once_during_a_rebase_is_abandoned() {
    let repo = repo();
    let x = commit(repo.path(), "x");
    commit(repo.path(), "a");
    // The rebase stops before its first pick, on `x`, which was there before it began.
    rebase_keeping(repo.path(), "HEAD~", "1s/^/break\\n/");
    let x2 = amend_of(repo.path(), &x);
    let bin = fake_git(repo.path(), "fast-import");
    let input = format!("{x} {x2}\n");
    let (stderr, _) = failing(hook(repo.path(), "amend", input, Some(&bin)));
    assert_abandoned(repo.path(), &stderr, &[format!("{x} {x2}")]);
}

#[test]
fn a_git_that_cannot_run_prints_the_rewrite() {
    let (repo, a, a2) = amended();
    let bin = repo.path().join("no-bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("git"), "not a program").unwrap();
    let mut command = hook(repo.path(), "amend", format!("{a} {a2}\n"), None);
    command.env("PATH", &bin);
    let (stderr, _) = failing(command);
    assert_printed_only(repo.path(), &stderr, &[format!("{a} {a2}")]);
}

#[test]
fn a_record_that_cannot_be_written_is_printed() {
    let (repo, a, a2) = amended();
    fs::write(repo.path().join(".git/replaces-abandoned"), "in the way").unwrap();
    let bin = fake_git(repo.path(), "fast-import");
    let (stderr, _) = failing(hook(
        repo.path(),
        "amend",
        format!("{a} {a2}\n"),
        Some(&bin),
    ));
    fs::remove_file(repo.path().join(".git/replaces-abandoned")).unwrap();
    assert_printed_only(repo.path(), &stderr, &[format!("{a} {a2}")]);
}

#[test]
fn a_full_disk_prints_the_rewrite() {
    if !Command::new("unshare")
        .args(["-Urm", "true"])
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("skipped: unshare cannot make a user and mount namespace here");
        return;
    }
    let (repo, a, a2) = amended();
    let mount = tempfile::tempdir().unwrap();
    // The repository is copied onto a small tmpfs, which is then filled.
    let script = r#"set -e
mount -t tmpfs -o size=4m tmpfs "$1"
cp -a "$2" "$1/repo"
cd "$1/repo"
head -c 8m /dev/zero > "$1/fill" 2>/dev/null || true
exec "$3" amend
"#;
    let mut command = Command::new("unshare");
    command
        .args(["-Urm", "sh", "-c", script, "sh"])
        .arg(mount.path())
        .arg(repo.path())
        .arg(common::HOOK);
    with_git_env(&mut command, repo.path());
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(format!("{a} {a2}\n").as_bytes()).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&printed(&[format!("{a} {a2}")])),
        "{stderr}"
    );
    assert!(stderr.contains("could not be kept"), "{stderr}");
}

#[test]
fn a_closed_standard_error_still_keeps_the_record() {
    let (repo, a, a2) = amended();
    let bin = fake_git(repo.path(), "fast-import");
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let status = hook(repo.path(), "amend", format!("{a} {a2}\n"), Some(&bin))
        .stderr(writer)
        .status()
        .unwrap();
    assert!(!status.success());
    assert_eq!(abandoned(repo.path()), [format!("{a} {a2}")]);
}

#[test]
fn a_worktree_keeps_its_record_in_the_common_git_directory() {
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
    let (stderr, _) = failing(hook(&worktree, "amend", format!("{a} {a2}\n"), Some(&bin)));
    assert_abandoned(repo.path(), &stderr, &[format!("{a} {a2}")]);
}

#[test]
fn input_that_cannot_be_read_keeps_no_record() {
    let repo = repo();
    let (stderr, _) = failing(hook(repo.path(), "amend", [0xff, b'\n'], None));
    assert!(stderr.contains("could not read"), "{stderr}");
    assert!(abandoned(repo.path()).is_empty());
}

#[test]
fn a_rebase_that_is_recorded_keeps_no_record() {
    let repo = repo();
    commit(repo.path(), "a");
    rebase(repo.path(), "HEAD~", "");
    assert!(abandoned(repo.path()).is_empty());
}

#[test]
fn a_signal_while_the_ref_keeps_moving_abandons_the_rewrite() {
    let (repo, a, a2) = amended();
    let base = ok(repo.path(), &["rev-parse", "HEAD~"]);
    ok(
        repo.path(),
        &["notes", "--ref=replaces", "add", "-m", "other", &base],
    );
    let bin = fake_git_racing(repo.path());
    let stderr = interrupt_when(
        hook(repo.path(), "amend", format!("{a} {a2}\n"), Some(&bin)),
        "signal to abandon",
    );
    assert!(stderr.contains("keeps moving, retrying"), "{stderr}");
    assert!(stderr.contains("waiting for disk"), "{stderr}");
    assert_abandoned(repo.path(), &stderr, &[format!("{a} {a2}")]);
}

#[test]
fn a_lock_held_for_a_while_is_reported_and_waited_for() {
    let (repo, a, a2) = amended();
    let lock = notes_lock(repo.path());
    fs::write(&lock, "").unwrap();
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(2500));
        fs::remove_file(lock).unwrap();
    });
    let output = hook(repo.path(), "amend", format!("{a} {a2}\n"), None)
        .output()
        .unwrap();
    release.join().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("is locked, retrying (attempt "), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(abandoned(repo.path()).is_empty());
}

#[test]
fn a_slow_git_is_reported() {
    let (repo, a, a2) = amended();
    let bin = fake_git_holding(repo.path(), "fast-import");
    let path = repo.path().to_owned();
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(2500));
        release(&path);
    });
    let output = hook(repo.path(), "amend", format!("{a} {a2}\n"), Some(&bin))
        .output()
        .unwrap();
    release.join().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("waiting for git fast-import"), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}

#[test]
fn a_quick_record_says_nothing() {
    let (repo, a, a2) = amended();
    let output = hook(repo.path(), "amend", format!("{a} {a2}\n"), None)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
}

#[test]
fn a_ref_that_git_will_not_move_abandons_the_rewrite() {
    let (repo, a, a2) = amended();
    let bin = fake_git(repo.path(), "update-ref");
    let (stderr, _) = failing(hook(
        repo.path(),
        "amend",
        format!("{a} {a2}\n"),
        Some(&bin),
    ));
    assert!(stderr.contains("fake git fails on update-ref"), "{stderr}");
    assert_abandoned(repo.path(), &stderr, &[format!("{a} {a2}")]);
}
