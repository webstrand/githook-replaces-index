//! A signal that would end the hook before the rewrite is recorded makes it keep the rewrite as
//! an abandoned record first; a signal that stops it lets it carry on once it continues.

mod common;

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{
    abandoned, alive, amend_of, amended, commit, fake_git_holding, held, hook, kill, note, ok,
    printed, rebase_keeping, release, repo,
};

/// Starts the hook on `input` with a git that holds on `hold`, waits until it holds, and returns
/// the hook and the process id of the git it holds.
fn start_held(repo: &Path, kind: &str, input: &str, hold: &str) -> (Child, u32) {
    let bin = fake_git_holding(repo, hold);
    let child = hook(repo, kind, input, Some(&bin))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let git = held(repo);
    (child, git)
}

/// Sends `signal` to the hook alone, as `kill` does, and returns its output once it ends.
fn signalled(child: Child, signal: &str) -> Output {
    kill(signal, &child.id().to_string());
    child.wait_with_output().unwrap()
}

fn assert_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "git {pid} was left running");
        thread::sleep(Duration::from_millis(20));
    }
}

fn abandons_on(signal: &str, number: i32) {
    let (repo, a, a2) = amended();
    let (child, git) = start_held(repo.path(), "amend", &format!("{a} {a2}\n"), "fast-import");
    let output = signalled(child, signal);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.signal(), Some(number), "{stderr}");
    assert!(stderr.contains("waiting for disk (60s timeout"), "{stderr}");
    assert!(stderr.contains("was kept in"), "{stderr}");
    assert!(
        stderr.contains(&printed(&[format!("{a} {a2}")])),
        "{stderr}"
    );
    assert_eq!(abandoned(repo.path()), [format!("{a} {a2}")]);
    assert_gone(git);
    // The git that was ended never wrote the notes.
    release(repo.path());
    assert!(note(repo.path(), &a2).is_empty());
}

#[test]
fn an_interrupt_abandons_the_rewrite() {
    abandons_on("INT", 2);
}

#[test]
fn a_termination_abandons_the_rewrite() {
    abandons_on("TERM", 15);
}

#[test]
fn a_hangup_abandons_the_rewrite() {
    abandons_on("HUP", 1);
}

#[test]
fn a_quit_abandons_the_rewrite() {
    abandons_on("QUIT", 3);
}

#[test]
fn a_signal_while_a_replacement_is_deferred_abandons_the_replacement() {
    let repo = repo();
    commit(repo.path(), "a");
    ok(repo.path(), &["switch", "-q", "-c", "upstream", "HEAD~"]);
    commit(repo.path(), "upstream");
    ok(repo.path(), &["switch", "-q", "-"]);
    // A stop for a break rather than an edit, whose replacement the hook would defer without
    // asking.
    rebase_keeping(repo.path(), "upstream", "1s/$/\\nbreak/");
    let a1 = ok(repo.path(), &["rev-parse", "HEAD"]);
    let a2 = amend_of(repo.path(), &a1);
    // The hook tells whether the rewritten commit was there before the rebase with rev-list.
    let (child, _) = start_held(repo.path(), "amend", &format!("{a1} {a2}\n"), "rev-list");
    let output = signalled(child, "TERM");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.signal(), Some(15), "{stderr}");
    assert_eq!(abandoned(repo.path()), [format!("{a1} {a2}")]);
}

#[test]
fn a_stop_and_a_continue_let_the_hook_record() {
    let (repo, a, a2) = amended();
    let (mut child, _) = start_held(repo.path(), "amend", &format!("{a} {a2}\n"), "fast-import");
    // As a terminal does, the stop goes to the hook and to the git it runs.
    let group = format!("-{}", child.id());
    kill("TSTP", &group);
    thread::sleep(Duration::from_millis(200));
    release(repo.path());
    thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the hook ended while stopped"
    );
    kill("CONT", &group);
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(abandoned(repo.path()).is_empty());
}

#[test]
fn a_signal_while_a_rebase_is_followed_keeps_every_deferred_replacement() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    ok(repo.path(), &["switch", "-q", "-c", "upstream", "HEAD~"]);
    commit(repo.path(), "upstream");
    ok(repo.path(), &["switch", "-q", "-"]);
    rebase_keeping(repo.path(), "upstream", "1s/$/\\nbreak/");
    let a1 = ok(repo.path(), &["rev-parse", "HEAD"]);
    let a2 = amend_of(repo.path(), &a1);
    // The replacement is deferred, as during the rebase.
    let output = hook(repo.path(), "amend", format!("{a1} {a2}\n"), None)
        .output()
        .unwrap();
    assert!(output.status.success());
    // The hook tells whether the commit that the replacement made is on the branch with merge-base.
    let (child, _) = start_held(repo.path(), "rebase", &format!("{a} {a1}\n"), "merge-base");
    let output = signalled(child, "TERM");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.signal(), Some(15), "{stderr}");
    assert_eq!(
        abandoned(repo.path()),
        [format!("{a} {a1}"), format!("{a1} {a2}")]
    );
}
