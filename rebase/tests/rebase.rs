//! Runs `rebase` in a scratch clone of a scratch repository whose history was rewritten after
//! the clone was made.

use std::path::Path;
use std::process::Command;

const REBASE: &str = env!("CARGO_BIN_EXE_git-rebase-replaces");

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

fn git(repo: &Path, args: &[&str]) -> String {
    let (ok, stdout, stderr) = run(repo, "git", args);
    assert!(ok, "git {args:?}: {stderr}");
    stdout.trim_end().to_owned()
}

fn commit(repo: &Path, name: &str, message: &str) -> String {
    std::fs::write(repo.join(name), name).unwrap();
    git(repo, &["add", name]);
    git(repo, &["commit", "-q", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

/// Records in the notes of `repo` that `new` was made from `old`.
fn replaced(repo: &Path, old: &str, new: &str) {
    let ref_arg = "--ref=replaces";
    git(
        repo,
        &[
            "notes",
            ref_arg,
            "add",
            "-f",
            "-m",
            &format!("replaces: {old}"),
            new,
        ],
    );
}

/// The subjects of the commits of `repo` from its root to HEAD.
fn subjects(repo: &Path) -> Vec<String> {
    git(repo, &["log", "--reverse", "--format=%s"])
        .lines()
        .map(str::to_owned)
        .collect()
}

/// An upstream with `a` and `b`, a clone with `x` on top of them, and the upstream with `a` and
/// `b` rewritten as `a'` and `b'` afterwards. The clone fetched `b` after it was made, so that
/// the reflog of its remote-tracking branch, which a clone does not start, shows `b`.
fn rewritten() -> (tempfile::TempDir, tempfile::TempDir, [String; 4]) {
    let upstream = tempfile::tempdir().unwrap();
    let clone = tempfile::tempdir().unwrap();
    let up = upstream.path();
    git(up, &["init", "-q", "-b", "main"]);
    commit(up, "base", "base");
    let a = commit(up, "a", "a");
    git(clone.path(), &["clone", "-q", up.to_str().unwrap(), "."]);
    let b = commit(up, "b", "b");
    git(clone.path(), &["pull", "-q", "--ff-only"]);
    commit(clone.path(), "x", "x");

    git(up, &["reset", "-q", "--hard", "HEAD~2"]);
    let a2 = commit(up, "a", "a'");
    let b2 = commit(up, "b", "b'");
    (upstream, clone, [a, b, a2, b2])
}

#[test]
fn leaves_out_the_commits_the_upstream_rewrote() {
    let (upstream, clone, [a, b, a2, b2]) = rewritten();
    replaced(upstream.path(), &a, &a2);
    replaced(upstream.path(), &b, &b2);
    git(clone.path(), &["reflog", "expire", "--expire=now", "--all"]);

    let (ok, stdout, stderr) = run(clone.path(), REBASE, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(
        stdout,
        format!(
            "rebase-replaces: leaving out {} a: @{{upstream}} holds {} made from it\n\
             rebase-replaces: leaving out {} b: @{{upstream}} holds {} made from it\n\
             rebase-replaces: replaying 1 commits onto @{{upstream}}, leaving out 2 it already holds\n",
            &a[..7],
            &a2[..12],
            &b[..7],
            &b2[..12]
        )
    );
    assert_eq!(subjects(clone.path()), ["base", "a'", "b'", "x"]);
}

#[test]
fn leaves_out_the_commits_the_reflog_shows() {
    let (_upstream, clone, _) = rewritten();
    let (ok, stdout, stderr) = run(clone.path(), REBASE, &[]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("leaving out 2 it already holds"),
        "{stdout}"
    );
    assert!(
        stdout.contains("the reflog of @{upstream} shows that it held it"),
        "{stdout}"
    );
    assert_eq!(subjects(clone.path()), ["base", "a'", "b'", "x"]);
}

#[test]
fn replays_a_commit_rewritten_as_commits_the_upstream_lacks() {
    let (upstream, clone, [a, b, a2, _]) = rewritten();
    replaced(upstream.path(), &a, &a2);
    // `b` was rewritten in a clone whose rewrite the upstream does not hold.
    replaced(upstream.path(), &b, &"f".repeat(40));
    git(clone.path(), &["reflog", "expire", "--expire=now", "--all"]);

    // Whether the rebase then succeeds depends on how git replays `b` onto `b'`, which holds
    // its file already; only the choice of what to replay is checked.
    let (_, stdout, stderr) = run(clone.path(), REBASE, &[]);
    assert!(stdout.contains("replaying 2 commits"), "{stdout}");
    let warning = format!(
        "rebase-replaces: {} b was rewritten as {} but @{{upstream}} lacks it; replaying it\n",
        &b[..7],
        "f".repeat(12)
    );
    assert!(stderr.starts_with(&warning), "{stderr}");
}

/// Records in the notes of `repo` that `new` was made from each of `olds`.
fn replaced_from(repo: &Path, olds: &[&str], new: &str) {
    let lines: String = olds
        .iter()
        .map(|old| format!("replaces: {old}\n"))
        .collect();
    git(
        repo,
        &[
            "notes",
            "--ref=replaces",
            "add",
            "-f",
            "-m",
            lines.trim_end(),
            new,
        ],
    );
}

/// `rewritten`, with `b'` changing `b`, and a `fixup! b` commit on top of the clone's `x`, which
/// a rebase with `rebase.autoSquash` folds into `b`, and its id.
fn rewritten_with_fixup() -> (tempfile::TempDir, tempfile::TempDir, [String; 5]) {
    let (upstream, clone, [a, b, a2, _]) = rewritten();
    // `b'` changes `b`, so that git does not skip `b` as a commit the upstream applied already.
    std::fs::write(upstream.path().join("b"), "b, changed").unwrap();
    git(
        upstream.path(),
        &["commit", "-q", "-a", "--amend", "-m", "b'"],
    );
    let b2 = git(upstream.path(), &["rev-parse", "HEAD"]);
    let f = commit(clone.path(), "f", "fixup! b");
    git(clone.path(), &["config", "rebase.autoSquash", "true"]);
    git(clone.path(), &["reflog", "expire", "--expire=now", "--all"]);
    (upstream, clone, [a, b, a2, b2, f])
}

#[test]
fn leaves_out_a_fixup_that_the_upstream_folded_in() {
    let (upstream, clone, [a, b, a2, b2, f]) = rewritten_with_fixup();
    replaced(upstream.path(), &a, &a2);
    replaced_from(upstream.path(), &[&b, &f], &b2);

    let (ok, stdout, stderr) = run(clone.path(), REBASE, &[]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("leaving out 3 it already holds"),
        "{stdout}"
    );
    assert_eq!(subjects(clone.path()), ["base", "a'", "b'", "x"]);
}

#[test]
fn refuses_a_fixup_of_a_commit_left_out_that_the_upstream_lacks() {
    let (upstream, clone, [a, b, a2, b2, f]) = rewritten_with_fixup();
    replaced(upstream.path(), &a, &a2);
    replaced(upstream.path(), &b, &b2);
    let head = git(clone.path(), &["rev-parse", "HEAD"]);

    let (ok, _, stderr) = run(clone.path(), REBASE, &[]);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains(&format!("{} fixup! b would fold into", &f[..7])),
        "{stderr}"
    );
    assert!(stderr.contains("nothing was rebased"), "{stderr}");
    assert_eq!(git(clone.path(), &["rev-parse", "HEAD"]), head);
    assert!(!clone.path().join(".git/rebase-merge").exists());
}

#[test]
fn fetches_the_remote_whose_refspec_makes_the_upstream() {
    let (upstream, clone, [a, b, a2, b2]) = rewritten();
    replaced(upstream.path(), &a, &a2);
    replaced(upstream.path(), &b, &b2);
    // The remote keeps its branches under another name than its own.
    let clone = clone.path();
    git(
        clone,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/mirror/*",
        ],
    );
    git(clone, &["fetch", "-q", "origin"]);
    git(clone, &["branch", "-q", "-u", "mirror/main"]);
    git(clone, &["reflog", "expire", "--expire=now", "--all"]);

    let (ok, stdout, stderr) = run(clone, REBASE, &[]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains("leaving out 2 it already holds"),
        "{stdout}"
    );
    git(
        clone,
        &[
            "rev-parse",
            "--verify",
            "refs/notes/remotes/origin/replaces",
        ],
    );
}

#[test]
fn takes_a_named_upstream_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "base", "base");
    let x = commit(repo, "x", "x");
    git(repo, &["branch", "feature"]);
    // `main` drops `x`, which its reflog shows it held.
    git(repo, &["reset", "-q", "--hard", "HEAD~"]);
    git(repo, &["checkout", "-q", "feature"]);

    let (ok, stdout, stderr) = run(repo, REBASE, &["main"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("leaving out 0"), "{stdout}");
    assert_eq!(git(repo, &["rev-parse", "HEAD"]), x);
}
