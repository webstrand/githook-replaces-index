//! Runs `pull` between scratch repositories.

use std::path::Path;
use std::process::Command;

const PULL: &str = env!("CARGO_BIN_EXE_git-pull-replaces");
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

fn git(repo: &Path, args: &[&str]) -> String {
    let (ok, stdout, stderr) = run(repo, "git", args);
    assert!(ok, "git {args:?}: {stderr}");
    stdout
}

/// Writes a note on `commit` directly, as `record` would have.
fn note_add(repo: &Path, commit: &str, text: &str) {
    let blob = {
        let mut child = isolated(repo, "git", &["hash-object", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    let parent = run(repo, "git", &["rev-parse", "-q", "--verify", REF]).1;
    let mut index = isolated(repo, "git", &["read-tree", "--empty"]);
    let index_file = repo.join(".git/notes-index");
    index.env("GIT_INDEX_FILE", &index_file);
    assert!(index.status().unwrap().success());
    if !parent.is_empty() {
        let mut read = isolated(repo, "git", &["read-tree", parent.trim()]);
        read.env("GIT_INDEX_FILE", &index_file);
        assert!(read.status().unwrap().success());
    }
    let cacheinfo = format!("100644,{},{commit}", blob.trim());
    let mut add = isolated(
        repo,
        "git",
        &["update-index", "--add", "--cacheinfo", &cacheinfo],
    );
    add.env("GIT_INDEX_FILE", &index_file);
    assert!(add.status().unwrap().success());
    let mut write = isolated(repo, "git", &["write-tree"]);
    write.env("GIT_INDEX_FILE", &index_file);
    let tree = String::from_utf8(write.output().unwrap().stdout).unwrap();
    let mut args = vec!["commit-tree", tree.trim(), "-m", "notes"];
    if !parent.is_empty() {
        args.extend(["-p", parent.trim()]);
    }
    let mut commit_tree = isolated(repo, "git", &args);
    commit_tree
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com");
    let new = String::from_utf8(commit_tree.output().unwrap().stdout).unwrap();
    git(repo, &["update-ref", REF, new.trim()]);
}

fn note(repo: &Path, commit: &str) -> String {
    git(repo, &["cat-file", "-p", &format!("{REF}:{commit}")])
}

#[test]
fn merges_the_notes_of_a_remote() {
    let (theirs, ours) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (theirs, ours) = (theirs.path(), ours.path());
    git(theirs, &["init", "-q"]);
    git(ours, &["init", "-q"]);
    git(ours, &["remote", "add", "origin", theirs.to_str().unwrap()]);
    let (a, b, c, x) = (id('a'), id('b'), id('c'), id('e'));
    // `b` was made from `a` in their clone, and from `x` as well in ours, which then made `c` of
    // `b` before it pulled.
    note_add(theirs, &b, &format!("replaces: {a}\n"));
    note_add(ours, &b, &format!("replaces: {x}\n"));
    note_add(ours, &c, &format!("replaces: {b}\nreplaces: {x}\n"));

    let (ok, _, stderr) = run(ours, PULL, &["origin"]);
    assert!(ok, "{stderr}");
    assert_eq!(note(ours, &b), format!("replaces: {a}\nreplaces: {x}\n"));
    // The pull adds no line that neither clone wrote; the index follows `b` to `a`.
    assert_eq!(note(ours, &c), format!("replaces: {b}\nreplaces: {x}\n"));
    // Their notes are kept apart, and the remote is not changed.
    git(
        ours,
        &[
            "rev-parse",
            "--verify",
            "refs/notes/remotes/origin/replaces",
        ],
    );
    assert_eq!(note(theirs, &b), format!("replaces: {a}\n"));
}

#[test]
fn takes_the_notes_of_a_remote_when_it_has_none() {
    let (theirs, ours) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (theirs, ours) = (theirs.path(), ours.path());
    git(theirs, &["init", "-q"]);
    git(ours, &["init", "-q"]);
    git(ours, &["remote", "add", "origin", theirs.to_str().unwrap()]);
    let (a, b) = (id('a'), id('b'));
    note_add(theirs, &b, &format!("replaces: {a}\n"));

    let (ok, _, stderr) = run(ours, PULL, &["origin"]);
    assert!(ok, "{stderr}");
    assert_eq!(note(ours, &b), format!("replaces: {a}\n"));
}

#[test]
fn says_so_when_a_remote_has_no_notes() {
    let (theirs, ours) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (theirs, ours) = (theirs.path(), ours.path());
    git(theirs, &["init", "-q"]);
    git(ours, &["init", "-q"]);
    git(ours, &["remote", "add", "origin", theirs.to_str().unwrap()]);
    let (ok, _, stderr) = run(ours, PULL, &["origin"]);
    assert!(ok, "{stderr}");
    assert_eq!(stderr, format!("pull-replaces: origin has no {REF}\n"));
}

#[test]
fn fails_when_the_remote_cannot_be_reached() {
    let ours = tempfile::tempdir().unwrap();
    let ours = ours.path();
    git(ours, &["init", "-q"]);
    let (ok, _, stderr) = run(ours, PULL, &["nowhere"]);
    assert!(!ok);
    assert!(
        stderr.starts_with("pull-replaces: could not list the refs of nowhere"),
        "{stderr}"
    );
}

/// Makes every update of the notes ref in `repo` fail unless it names the commit it moves the ref
/// from, as a writer that cannot overwrite a concurrent write does. Creating the ref is let be.
fn refuse_unchecked_writes(repo: &Path) {
    let hook = repo.join(".git/hooks/reference-transaction");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    let zero = "0".repeat(40);
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n\
             [ \"$1\" = prepared ] || exit 0\n\
             while read old new ref; do\n\
             \x20 if [ \"$ref\" = {REF} ] && [ \"$old\" = {zero} ] && git rev-parse -q --verify {REF} >/dev/null; then\n\
             \x20   echo \"unchecked write of $ref\" >&2; exit 1\n\
             \x20 fi\n\
             done\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn merges_from_the_commit_it_read() {
    let (theirs, ours) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (theirs, ours) = (theirs.path(), ours.path());
    git(theirs, &["init", "-q"]);
    git(ours, &["init", "-q"]);
    git(ours, &["remote", "add", "origin", theirs.to_str().unwrap()]);
    let (a, b, x) = (id('a'), id('b'), id('e'));
    note_add(theirs, &b, &format!("replaces: {a}\n"));
    note_add(ours, &b, &format!("replaces: {x}\n"));
    refuse_unchecked_writes(ours);

    let (ok, _, stderr) = run(ours, PULL, &["origin"]);
    assert!(ok, "{stderr}");
    assert_eq!(note(ours, &b), format!("replaces: {a}\nreplaces: {x}\n"));
    // The merge keeps their history, so that a later pull of the same notes has nothing to do.
    git(
        ours,
        &[
            "merge-base",
            "--is-ancestor",
            "refs/notes/remotes/origin/replaces",
            REF,
        ],
    );
}

#[test]
fn fast_forwards_from_the_commit_it_read() {
    let (theirs, ours) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (theirs, ours) = (theirs.path(), ours.path());
    git(theirs, &["init", "-q"]);
    git(ours, &["init", "-q"]);
    git(ours, &["remote", "add", "origin", theirs.to_str().unwrap()]);
    let (a, b, c) = (id('a'), id('b'), id('c'));
    note_add(theirs, &b, &format!("replaces: {a}\n"));
    let (ok, _, stderr) = run(ours, PULL, &["origin"]);
    assert!(ok, "{stderr}");
    // Their notes move on, and ours, which hold nothing else, follow.
    note_add(theirs, &c, &format!("replaces: {b}\n"));
    refuse_unchecked_writes(ours);
    let (ok, _, stderr) = run(ours, PULL, &["origin"]);
    assert!(ok, "{stderr}");
    assert_eq!(note(ours, &c), format!("replaces: {b}\n"));
    assert_eq!(
        git(ours, &["rev-parse", REF]),
        git(theirs, &["rev-parse", REF])
    );
}
