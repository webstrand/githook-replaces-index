//! Runs the hook in a scratch repository, as git runs it after each amend and rebase.

mod common;

use common::{commit, note, ok, rebase, rebase_keeping, refuse_unchecked_writes, repo};

#[test]
fn an_amend_stands_in_place_of_the_amended_commit() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    ok(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended"],
    );
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended twice"],
    );
    let a3 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert!(note(repo.path(), &a).is_empty());
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    let mut expected = [format!("replaces: {a}"), format!("replaces: {a2}")];
    expected.sort();
    assert_eq!(note(repo.path(), &a3), expected);
}

#[test]
fn a_fixup_replaces_every_commit_folded_together() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    rebase(repo.path(), "HEAD~2", "2s/^pick/fixup/");
    let head = ok(repo.path(), &["rev-parse", "HEAD"]);
    let mut expected = [format!("replaces: {a}"), format!("replaces: {b}")];
    expected.sort();
    assert_eq!(note(repo.path(), &head), expected);
    assert!(note(repo.path(), &a).is_empty());
    assert!(note(repo.path(), &b).is_empty());
}

#[test]
fn a_rebase_follows_an_amend_during_it() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    let base = ok(repo.path(), &["rev-parse", "HEAD~2"]);
    let script = "1s/$/\\nexec git commit -q --amend -m amended/";
    rebase(repo.path(), &base, script);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    let b2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    // The commit that the rebase made of `a` before the amend is named nowhere.
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert_eq!(note(repo.path(), &b2), [format!("replaces: {b}")]);
    let notes = ok(repo.path(), &["notes", "--ref=replaces", "list"]);
    assert_eq!(notes.lines().count(), 2);
}

#[test]
fn a_rebase_does_not_follow_an_amend_that_was_undone() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    // A new base, so that the rebase makes another commit of `a` within the same second.
    ok(repo.path(), &["switch", "-q", "-c", "upstream", "HEAD~"]);
    commit(repo.path(), "upstream");
    ok(repo.path(), &["switch", "-q", "-"]);
    rebase(repo.path(), "upstream", "1s/^pick/edit/");
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["commit", "-q", "--amend", "-m", "undone"]);
    let undone = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["reset", "-q", "--hard", &a2]);
    ok(repo.path(), &["rebase", "--continue"]);
    assert_eq!(ok(repo.path(), &["rev-parse", "HEAD"]), a2);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert!(note(repo.path(), &undone).is_empty());
}

#[test]
fn a_rebase_records_an_amend_of_a_commit_it_kept() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    let script = "1s/$/\\nexec git commit -q --amend -m amended/";
    rebase_keeping(repo.path(), "HEAD~2", script);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    let b2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
    assert_eq!(note(repo.path(), &b2), [format!("replaces: {b}")]);
}

#[test]
fn a_rebase_that_rewrites_nothing_else_records_an_amend() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    // Every pick keeps its commit, so git reports no rewrite when the rebase ends.
    let script = "$s/$/\\nexec git commit -q --amend -m amended/";
    rebase_keeping(repo.path(), "HEAD~", script);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}

#[test]
fn a_rebase_records_an_amend_of_the_commit_it_rebases_onto() {
    let repo = repo();
    let base = ok(repo.path(), &["rev-parse", "HEAD"]);
    let a = commit(repo.path(), "a");
    rebase_keeping(repo.path(), "HEAD~", "1s/^/break\\n/");
    ok(
        repo.path(),
        &[
            "commit",
            "-q",
            "--amend",
            "--allow-empty",
            "-m",
            "base, amended",
        ],
    );
    let base2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["rebase", "--continue"]);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(ok(repo.path(), &["rev-parse", "HEAD~"]), base2);
    assert_eq!(note(repo.path(), &base2), [format!("replaces: {base}")]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}

#[test]
fn every_write_moves_the_ref_from_the_commit_it_read() {
    let repo = repo();
    commit(repo.path(), "a");
    ok(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended"],
    );
    refuse_unchecked_writes(repo.path());
    ok(
        repo.path(),
        &["commit", "-q", "--amend", "-m", "a, amended twice"],
    );
    commit(repo.path(), "b");
    rebase(repo.path(), "HEAD~2", "2s/^pick/fixup/");
    let head = ok(repo.path(), &["rev-parse", "HEAD"]);
    // `a`, its two amends, and `b`.
    assert_eq!(note(repo.path(), &head).len(), 4);
}

#[test]
fn a_rebase_follows_two_amends_during_it() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    // A new base, so that the rebase makes another commit of `a` within the same second.
    ok(repo.path(), &["switch", "-q", "-c", "upstream", "HEAD~"]);
    commit(repo.path(), "upstream");
    ok(repo.path(), &["switch", "-q", "-"]);
    let script =
        "1s/$/\\nexec git commit -q --amend -m amended\\nexec git commit -q --amend -m again/";
    rebase(repo.path(), "upstream", script);
    let a3 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(note(repo.path(), &a3), [format!("replaces: {a}")]);
    // Neither the commit the rebase made of `a` nor the first amend of it is named.
    let notes = ok(repo.path(), &["notes", "--ref=replaces", "list"]);
    assert_eq!(notes.lines().count(), 1, "{notes}");
}

#[test]
fn a_rebase_records_two_amends_of_a_commit_it_kept() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    commit(repo.path(), "b");
    let script =
        "1s/$/\\nexec git commit -q --amend -m amended\\nexec git commit -q --amend -m again/";
    rebase_keeping(repo.path(), "HEAD~2", script);
    let a3 = ok(repo.path(), &["rev-parse", "HEAD~"]);
    assert!(note(repo.path(), &a3).contains(&format!("replaces: {a}")));
}

#[test]
fn a_rebase_names_no_commit_amended_away_at_an_edit_of_a_commit_it_kept() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    let b = commit(repo.path(), "b");
    rebase_keeping(repo.path(), "HEAD~2", "1s/^pick/edit/");
    ok(repo.path(), &["commit", "-q", "--amend", "-m", "amended"]);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["commit", "-q", "--amend", "-m", "again"]);
    let a3 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["rebase", "--continue"]);
    let b2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    assert_eq!(note(repo.path(), &a3), [format!("replaces: {a}")]);
    assert_eq!(note(repo.path(), &b2), [format!("replaces: {b}")]);
    assert!(note(repo.path(), &a2).is_empty());
}

#[test]
fn a_rebase_records_one_amend_at_an_edit_of_a_commit_it_kept() {
    let repo = repo();
    let a = commit(repo.path(), "a");
    commit(repo.path(), "b");
    rebase_keeping(repo.path(), "HEAD~2", "1s/^pick/edit/");
    ok(repo.path(), &["commit", "-q", "--amend", "-m", "amended"]);
    let a2 = ok(repo.path(), &["rev-parse", "HEAD"]);
    ok(repo.path(), &["rebase", "--continue"]);
    assert_eq!(note(repo.path(), &a2), [format!("replaces: {a}")]);
}
