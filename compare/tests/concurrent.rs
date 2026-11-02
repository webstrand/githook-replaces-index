//! Runs `compare-replaces` in many processes at once, which build and update one index.

use std::path::Path;
use std::process::{Command, Output, Stdio};

const COMPARE: &str = env!("CARGO_BIN_EXE_git-compare-replaces");

fn command(repo: &Path, program: &str, args: &[&str]) -> Command {
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

fn git(repo: &Path, args: &[&str]) -> String {
    let output = command(repo, "git", args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}

/// Runs `compare-replaces old new` in `count` processes started together.
fn all_at_once(repo: &Path, count: usize) -> Vec<Output> {
    let children: Vec<_> = (0..count)
        .map(|_| {
            command(repo, COMPARE, &["old", "new"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .collect()
}

#[test]
fn processes_that_build_and_update_the_index_at_once_agree() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "-q", "-b", "old"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    git(repo, &["branch", "new"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "a"]);
    let a = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["switch", "-q", "new"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "a'"]);
    let a2 = git(repo, &["rev-parse", "HEAD"]);
    let note = format!("replaces: {a}");
    git(repo, &["notes", "--ref=replaces", "add", "-m", &note, &a2]);

    let expected = format!("held {a} as {a2}: a\n");
    for outputs in [all_at_once(repo, 8), {
        // The notes move, so that each process finds the index stale at once.
        git(
            repo,
            &["notes", "--ref=replaces", "append", "-m", "other", &a2],
        );
        all_at_once(repo, 8)
    }] {
        for output in outputs {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                expected,
                "{stderr}"
            );
        }
    }
}
