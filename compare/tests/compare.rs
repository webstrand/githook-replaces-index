//! Compares chains in a scratch repository. The library works on the repository the process runs
//! in, so the whole test is one function, which runs in that repository alone.

use std::process::Command;

use replaces_index_compare::{Entry, Status, compare};
use replaces_index_record::{Rewrite, record};

fn git(args: &[&str]) -> String {
    let output = Command::new("git").args(args).output().unwrap();
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

fn commit(message: &str) -> String {
    git(&["commit", "-q", "--allow-empty", "-m", message]);
    git(&["rev-parse", "HEAD"])
}

fn made(old: &str, new: &str) {
    record(&Rewrite::parse(&format!("{old} {new}\n"))).unwrap();
}

#[test]
fn tells_what_the_new_chain_holds_of_each_commit() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: this file holds a single test, so no other thread reads the environment.
    unsafe {
        for (key, value) in [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ] {
            std::env::set_var(key, value);
        }
        for key in ["GIT_DIR", "GIT_INDEX_FILE", "GIT_NOTES_REF"] {
            std::env::remove_var(key);
        }
    }
    std::env::set_current_dir(dir.path()).unwrap();
    git(&["init", "-q", "-b", "old"]);
    let base = commit("base");
    git(&["branch", "new"]);
    git(&["branch", "elsewhere"]);
    let [a, b, c, d, f] = ["a", "b", "c", "d", "f"].map(commit);
    git(&["checkout", "-q", "new"]);
    let [a2, b0] = ["a'", "b0"].map(commit);
    git(&["checkout", "-q", "elsewhere"]);
    let c2 = commit("c'");

    // `a` was rebased onto `new` as `a'`, `b` was made from `b0`, which `new` holds, and `c` was
    // rewritten on a branch of its own. `a` stays on `old` all the same.
    made(&a, &a2);
    made(&b0, &b);
    made(&c, &c2);
    // `f` was made from `base`, which both chains hold, and from a commit of another clone, which
    // this one lacks.
    made(&base, &f);
    made(&"e".repeat(40), &f);

    let entry = |commit: &String, status| Entry {
        commit: commit.clone(),
        status,
    };
    assert_eq!(
        compare("old", "new").unwrap(),
        [
            entry(&a, Status::Held(vec![a2.clone()])),
            entry(&b, Status::Later(vec![b0.clone()])),
            entry(&c, Status::Rewritten(vec![c2.clone()])),
            entry(&d, Status::Missing),
            entry(&f, Status::Later(vec![base.clone()])),
        ]
    );
    // The other way round, `new` holds the earlier `b0`, and `a'` is a later version of `a`.
    assert_eq!(
        compare("new", "old").unwrap(),
        [
            entry(&a2, Status::Later(vec![a.clone()])),
            entry(&b0, Status::Held(vec![b.clone()])),
        ]
    );
}
