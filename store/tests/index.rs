//! Builds and queries the index in a scratch repository. The library works on the repository the
//! process runs in, so the whole test is one function, which runs in that repository alone.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use replaces_index_record::{Rewrite, record};
use replaces_index_store::Index;

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

fn rewrite(old: &str, new: &str) {
    record(&Rewrite::parse(&format!("{old} {new}\n"))).unwrap();
}

fn set(ids: &[&String]) -> BTreeSet<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn follows_the_notes_as_they_change() {
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
    git(&["init", "-q"]);
    let [a, b, c, d, x, y] = ["a", "b", "c", "d", "x", "y"].map(commit);

    // Without notes, nothing replaces anything.
    let index = Index::open().unwrap();
    assert!(index.replaced_by(&a).unwrap().is_empty());
    drop(index);
    assert!(Path::new(".git/replaces/index.redb").exists());

    // The index follows each rewrite, in both directions.
    rewrite(&a, &b);
    let index = Index::open().unwrap();
    assert_eq!(index.replaced_by(&a).unwrap(), set(&[&b]));
    assert_eq!(index.replaces(&b).unwrap(), set(&[&a]));
    drop(index);
    rewrite(&b, &c);
    let index = Index::open().unwrap();
    assert_eq!(index.replaced_by(&a).unwrap(), set(&[&b, &c]));
    assert_eq!(index.replaces(&c).unwrap(), set(&[&a, &b]));
    // Readers share the index.
    let other = Index::open().unwrap();
    assert_eq!(other.replaces(&b).unwrap(), set(&[&a]));
    drop((index, other));

    // A note that inherited nothing, as one written before the notes of its parent arrived from
    // another clone, still leads through the notes it names.
    let ref_arg = "--ref=replaces";
    git(&["notes", ref_arg, "add", "-m", &format!("replaces: {c}"), &d]);
    let index = Index::open().unwrap();
    assert_eq!(index.replaces(&d).unwrap(), set(&[&a, &b, &c]));
    assert_eq!(index.replaced_by(&b).unwrap(), set(&[&c, &d]));
    assert_eq!(index.with_prefix(&a[..10]).unwrap(), set(&[&a]));
    drop(index);

    // Notes that were removed, or replaced by notes that do not descend from them, are indexed
    // anew.
    git(&["update-ref", "-d", "refs/notes/replaces"]);
    rewrite(&x, &y);
    let index = Index::open().unwrap();
    assert!(index.replaced_by(&a).unwrap().is_empty());
    assert_eq!(index.replaced_by(&x).unwrap(), set(&[&y]));
    drop(index);
    git(&["update-ref", "-d", "refs/notes/replaces"]);
    let index = Index::open().unwrap();
    assert!(index.replaced_by(&x).unwrap().is_empty());
    drop(index);

    // A process that read the notes ref before another moved it, and brought the index up to
    // date, uses the index as it is, which holds every line of the notes it read.
    let [p, q, r, s] = ["p", "q", "r", "s"].map(commit);
    rewrite(&p, &q);
    let read = git(&["rev-parse", "refs/notes/replaces"]);
    rewrite(&r, &s);
    drop(Index::open().unwrap());
    git(&["update-ref", "refs/notes/replaces", &read]);
    let index = Index::open().unwrap();
    assert_eq!(index.replaced_by(&p).unwrap(), set(&[&q]));
    assert_eq!(index.replaced_by(&r).unwrap(), set(&[&s]));
}
