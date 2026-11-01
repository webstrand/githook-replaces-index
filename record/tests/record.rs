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
    // Nothing is left beside the notes ref.
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

/// Runs `record --abandoned`, which must succeed, and returns its standard error.
fn record_abandoned(repo: &Path) -> String {
    let output = isolated(repo, RECORD, &["--abandoned"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "{stderr}");
    stderr
}

#[test]
fn records_nothing_when_nothing_was_abandoned() {
    let (dir, _) = repo(["a"]);
    let stderr = record_abandoned(dir.path());
    assert_eq!(stderr, "");
    assert_eq!(git(dir.path(), &["rev-parse", "-q", "--verify", REF]), None);
}

#[test]
fn records_the_abandoned_records() {
    let (dir, ids) = repo(["a", "b"]);
    let (repo, [a, b]) = (dir.path(), ids.each_ref());
    let records = repo.join(".git/replaces-abandoned");
    std::fs::create_dir(&records).unwrap();
    std::fs::write(
        records.join("000000000001.000000000-0"),
        format!("{a} {b}\n"),
    )
    .unwrap();
    record_abandoned(repo);
    assert_eq!(note(repo, b), Some(replaces(&[a])));
    assert_eq!(std::fs::read_dir(&records).unwrap().count(), 0);
}

/// Holds the lock of the notes ref for 2.5 seconds, from now.
fn lock_for_a_while(repo: &Path) -> std::thread::JoinHandle<()> {
    let dir = repo.join(".git/refs/notes");
    std::fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("replaces.lock");
    std::fs::write(&lock, "").unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2500));
        std::fs::remove_file(lock).unwrap();
    })
}

#[test]
fn says_what_it_waits_for() {
    let (dir, ids) = repo(["a", "b"]);
    let (repo, [a, b]) = (dir.path(), ids.each_ref());
    let release = lock_for_a_while(repo);
    let mut child = isolated(repo, RECORD, &[])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{a} {b}\n").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    release.join().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("replaces-index: refs/notes/replaces is locked, retrying"),
        "{stderr}"
    );
    assert_eq!(note(repo, b), Some(replaces(&[a])));
}

/// Makes the notes ref a commit whose tree holds only a note of `lines` at `path`, as git stores a
/// note when its tree has a fanout.
fn note_at(repo: &Path, path: &str, lines: &str) {
    notes_at(repo, &[(path, lines)]);
}

/// Makes the notes ref a commit whose tree holds only `notes`, each of its lines at its path.
fn notes_at(repo: &Path, notes: &[(&str, &str)]) {
    let index = repo.join(".git/note-index");
    let with_index = |args: &[&str], input: Option<&str>| -> String {
        let mut command = isolated(repo, "git", args);
        command
            .env("GIT_INDEX_FILE", &index)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.unwrap_or("").as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    with_index(&["read-tree", "--empty"], None);
    for (path, lines) in notes {
        let blob = with_index(&["hash-object", "-w", "--stdin"], Some(lines));
        let info = format!("100644,{blob},{path}");
        with_index(&["update-index", "--add", "--cacheinfo", &info], None);
    }
    let tree = with_index(&["write-tree"], None);
    let commit = with_index(&["commit-tree", &tree, "-m", "notes"], None);
    git(repo, &["update-ref", REF, &commit]).unwrap();
}

/// Records `b` to `c` over a note of `b` stored at `split` of its id.
fn inherits_through_fanout(split: fn(&str) -> String) {
    let (dir, ids) = repo(["a", "b", "c"]);
    let (repo, [a, b, c]) = (dir.path(), ids.each_ref());
    note_at(repo, &split(b), &format!("replaces: {a}\n"));
    record(repo, &format!("{b} {c}\n"));
    assert_eq!(note(repo, c), Some(replaces(&[a, b])));
}

#[test]
fn reads_a_note_under_one_level_of_fanout() {
    inherits_through_fanout(|id| format!("{}/{}", &id[..2], &id[2..]));
}

#[test]
fn reads_a_note_under_two_levels_of_fanout() {
    inherits_through_fanout(|id| format!("{}/{}/{}", &id[..2], &id[2..4], &id[4..]));
}

/// Runs `record` with `input`, whatever its outcome.
fn run_record(repo: &Path, input: &str) -> std::process::Output {
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
    child.wait_with_output().unwrap()
}

#[test]
fn refuses_what_is_not_a_full_id() {
    let (dir, ids) = repo(["a", "b"]);
    let (repo, [a, b]) = (dir.path(), ids.each_ref());
    let inputs = [
        "a b\n".to_owned(),
        format!("{} {}\n", &a[..7], &b[..7]),
        format!("{} {}\n", a.to_uppercase(), b.to_uppercase()),
        // An id of the length of SHA-256, in a SHA-1 repository.
        format!("{a}{} {b}\n", "0".repeat(24)),
    ];
    for input in inputs {
        let output = run_record(repo, &input);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{stderr}");
        assert!(
            stderr.contains("is not the full id of a commit"),
            "{stderr}"
        );
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
    assert_eq!(git(repo, &["rev-parse", "-q", "--verify", REF]), None);
}

#[test]
fn rewrites_a_note_at_the_path_it_was_found_at() {
    // `git notes`, as when notes.rewriteRef copies a note, may give a tree a fanout that its number
    // of notes would not give it in fast-import, which then writes the note at another path.
    let (dir, ids) = repo(["a", "b", "c", "x"]);
    let (repo, [a, b, c, x]) = (dir.path(), ids.each_ref());
    let nested = format!("{}/{}", &c[..2], &c[2..]);
    notes_at(
        repo,
        &[
            (b, &format!("replaces: {a}\n")),
            (&nested, &format!("replaces: {x}\n")),
        ],
    );
    record(repo, &format!("{b} {c}\n"));
    let files = git(repo, &["ls-tree", "-r", "--name-only", REF]).unwrap();
    let of_c = files
        .lines()
        .filter(|path| path.replace('/', "") == *c)
        .count();
    assert_eq!(of_c, 1, "{files}");
    assert_eq!(note(repo, c), Some(replaces(&[a, b, x])));
}

#[test]
fn says_when_a_lock_may_have_been_left_behind() {
    let (dir, ids) = repo(["a", "b"]);
    let (repo, [a, b]) = (dir.path(), ids.each_ref());
    let release = lock_for_a_while(repo);
    let lock = repo.join(".git/refs/notes/replaces.lock");
    let taken = std::time::SystemTime::now() - std::time::Duration::from_secs(15);
    std::fs::File::options()
        .write(true)
        .open(&lock)
        .unwrap()
        .set_modified(taken)
        .unwrap();
    let output = run_record(repo, &format!("{a} {b}\n"));
    release.join().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("replaces.lock was taken")
            && stderr.contains("removing it lets the write through"),
        "{stderr}"
    );
}

#[test]
fn refuses_a_new_commit_the_clone_lacks_without_retrying() {
    let (dir, ids) = repo(["a"]);
    let (repo, [a]) = (dir.path(), ids.each_ref());
    let missing = "f".repeat(40);
    let output = run_record(repo, &format!("{a} {missing}\n"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("{missing} is not a commit of this clone")),
        "{stderr}"
    );
    let crashes = std::fs::read_dir(repo.join(".git"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("fast_import_crash")
        })
        .count();
    assert_eq!(crashes, 0);
}

#[test]
fn records_in_a_sha256_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "--object-format=sha256"]).unwrap();
    git(repo, &["commit", "-q", "--allow-empty", "-m", "a"]).unwrap();
    let a = git(repo, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
    git(
        repo,
        &["commit", "-q", "--amend", "--allow-empty", "-m", "b"],
    )
    .unwrap();
    let b = git(repo, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
    record(repo, &format!("{a} {b}\n"));
    assert_eq!(note(repo, &b), Some(replaces(&[&a])));
    // An id of the length of SHA-1 is no full id in a SHA-256 repository.
    let output = run_record(repo, &format!("{} {b}\n", &a[..40]));
    assert!(!output.status.success());
}
