//! Pulls a remote's replaces notes: fetches its refs/notes/replaces into
//! refs/notes/remotes/<remote>/replaces, and merges that into the local ref with the
//! `cat_sort_uniq` strategy, which takes the union of each note's lines. Lines are only ever added
//! to the notes, so the union is the whole merge. The remote is not changed; it pulls from this
//! clone in turn.

mod error;

use replaces_index_record::REF;
use runner::git;

pub use crate::error::Error;

/// What a pull found at the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pulled {
    /// The remote's notes were merged into the local ones.
    Merged,
    /// The remote has no replaces notes.
    Absent,
}

/// Pulls the replaces notes of `remote`.
pub fn pull(remote: &str) -> Result<Pulled, Error> {
    let theirs = format!("refs/notes/remotes/{remote}/replaces");

    // ls-remote exits with 2 when the remote has no matching ref.
    let mut list = git(["ls-remote", "--exit-code", remote, REF]);
    let listed = runner::run(&mut list).map_err(|source| Error::List {
        remote: remote.to_owned(),
        source,
    })?;
    if listed.status.code() == Some(2) {
        return Ok(Pulled::Absent);
    }
    runner::check(&list, &listed).map_err(|source| Error::List {
        remote: remote.to_owned(),
        source,
    })?;

    runner::status(&mut git([
        "fetch",
        "-q",
        remote,
        &format!("+{REF}:{theirs}"),
    ]))
    .map_err(|source| Error::Fetch {
        remote: remote.to_owned(),
        source,
    })?;
    let ours = runner::run(&mut git(["rev-parse", "-q", "--verify", REF])).map_err(|source| {
        Error::Merge {
            remote: remote.to_owned(),
            source,
        }
    })?;
    let mut merge = if ours.status.success() {
        git([
            "notes",
            &format!("--ref={REF}"),
            "merge",
            "-q",
            "-s",
            "cat_sort_uniq",
            &theirs,
        ])
    } else {
        git(["update-ref", REF, &theirs])
    };
    runner::status(&mut merge).map_err(|source| Error::Merge {
        remote: remote.to_owned(),
        source,
    })?;
    Ok(Pulled::Merged)
}
