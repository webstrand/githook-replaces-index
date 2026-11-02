//! Reads git's output in a scratch repository. The library runs git in the directory the process
//! runs in, so the whole test is one function, which runs in that repository alone.

use std::process::Command;

use replaces_index_runner as runner;

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

/// A tree of `files`, each a path and its contents, and the blobs of their contents.
fn tree(files: &[(&str, &str)]) -> String {
    git(&["read-tree", "--empty"]);
    for (path, contents) in files {
        std::fs::write("blob", contents).unwrap();
        let blob = git(&["hash-object", "-w", "blob"]);
        git(&[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{blob},{path}"),
        ]);
    }
    git(&["write-tree"])
}

#[test]
fn reads_what_git_prints() {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: this file holds a single test, so no other thread reads the environment.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GIT_INDEX_FILE", dir.path().join("index"));
        std::env::remove_var("GIT_DIR");
    }
    std::env::set_current_dir(dir.path()).unwrap();
    git(&["init", "-q"]);

    let before = tree(&[("kept", "k"), ("changed", "1"), ("gone", "g")]);
    let after = tree(&[("kept", "k"), ("changed", "2"), ("dir/added", "a")]);
    let blob = |path: &str, tree: &str| git(&["rev-parse", &format!("{tree}:{path}")]);
    let mut changed = runner::diff_tree(&before, &after).unwrap();
    changed.sort_by(|x, y| x.path.cmp(&y.path));
    let changed: Vec<_> = changed
        .into_iter()
        .map(|file| (file.path, file.old, file.new))
        .collect();
    assert_eq!(
        changed,
        [
            (
                "changed".to_owned(),
                Some(blob("changed", &before)),
                Some(blob("changed", &after))
            ),
            (
                "dir/added".to_owned(),
                None,
                Some(blob("dir/added", &after))
            ),
            ("gone".to_owned(), Some(blob("gone", &before)), None),
        ]
    );

    // An object the repository lacks, and contents that hold newlines and end without one.
    let missing = "0".repeat(40);
    std::fs::write("blob", "two\nlines").unwrap();
    let lines = git(&["hash-object", "-w", "blob"]);
    assert_eq!(
        runner::cat_file(&[lines.as_str(), &missing, &blob("kept", &before)]).unwrap(),
        [Some(b"two\nlines".to_vec()), None, Some(b"k".to_vec())]
    );

    let fields: Vec<_> = runner::nul_fields(b"a\0\xff\0b\0").collect();
    assert_eq!(fields, [Some("a"), None, Some("b")]);
}
