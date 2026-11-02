//! Runs `git whereis` in a scratch repository.

use std::path::Path;
use std::process::Command;

const WHEREIS: &str = env!("CARGO_BIN_EXE_git-whereis");

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

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["commit", "-q", "--allow-empty", "-m", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn whereis(repo: &Path, name: &str) -> String {
    let (ok, stdout, stderr) = run(repo, WHEREIS, &[name]);
    assert!(ok, "{stderr}");
    stdout
}

#[test]
fn lists_a_rewritten_commit_beside_the_commits_made_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "theirs"]);
    commit(repo, "base");
    git(repo, &["branch", "mine"]);
    let a = commit(repo, "a");
    git(repo, &["checkout", "-q", "mine"]);
    let a2 = commit(repo, "a'");
    git(repo, &["checkout", "-q", "--detach"]);
    let a3 = commit(repo, "a''");
    let ref_arg = "--ref=replaces";
    // `a` was rebased onto `mine` as `a'`, which was amended into `a''`, on no branch; `a` stays on
    // `theirs`.
    git(
        repo,
        &[
            "notes",
            ref_arg,
            "add",
            "-m",
            &format!("replaces: {a}"),
            &a2,
        ],
    );
    let note = format!("replaces: {a}\nreplaces: {a2}");
    git(repo, &["notes", ref_arg, "add", "-m", &note, &a3]);
    let mut into = [a2.clone(), a3.clone()];
    into.sort();
    let held = |c: &String| if *c == a2 { "mine" } else { "(no ref)" };

    assert_eq!(
        whereis(repo, &a),
        format!(
            "self {a} theirs\ninto {} {}\ninto {} {}\n",
            into[0],
            held(&into[0]),
            into[1],
            held(&into[1])
        )
    );
    assert_eq!(
        whereis(repo, &a2),
        format!("from {a} theirs\nself {a2} mine\ninto {a3} (no ref)\n")
    );
}

#[test]
fn finds_a_commit_the_clone_lacks_by_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    let b = commit(repo, "b");
    // A remote whose HEAD names its branch: the HEAD, short-named `origin`, is not listed.
    git(repo, &["update-ref", "refs/remotes/origin/main", &b]);
    git(
        repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    let gone = "f".repeat(40);
    let ref_arg = "--ref=replaces";
    git(
        repo,
        &[
            "notes",
            ref_arg,
            "add",
            "-m",
            &format!("replaces: {gone}"),
            &b,
        ],
    );

    assert_eq!(
        whereis(repo, "ffffffff"),
        format!("self {gone} (not in this clone)\ninto {b} main origin/main\n")
    );
    let (ok, _, stderr) = run(repo, WHEREIS, &["eeeeeeee"]);
    assert!(!ok);
    assert_eq!(
        stderr,
        "whereis: eeeeeeee is not a commit of this clone or of refs/notes/replaces\n"
    );
}

#[test]
fn builds_the_index_from_a_subdirectory_as_from_the_top() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "base");
    let a = commit(repo, "a");
    let a2 = commit(repo, "a'");
    let note = format!("replaces: {a}");
    git(repo, &["notes", "--ref=replaces", "add", "-m", &note, &a2]);
    let sub = repo.join("sub");
    std::fs::create_dir(&sub).unwrap();

    let expected = format!("from {a} main\nself {a2} main\n");
    assert_eq!(whereis(&sub, &a2), expected);
    assert_eq!(whereis(repo, &a2), expected);
}

#[test]
fn says_how_to_rebuild_a_damaged_index() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "main"]);
    let a = commit(repo, "a");
    whereis(repo, &a);
    let index = repo.join(".git/replaces/index.redb");
    for damage in [&b""[..], b"not a database"] {
        std::fs::write(&index, damage).unwrap();
        let (ok, _, stderr) = run(repo, WHEREIS, &[&a]);
        assert!(!ok);
        assert!(stderr.contains("is damaged"), "{stderr}");
        assert!(stderr.contains("may be deleted"), "{stderr}");
        assert!(index.exists());
    }
    std::fs::remove_file(&index).unwrap();
    assert_eq!(whereis(repo, &a), format!("self {a} main\n"));
}
