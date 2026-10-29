//! Compares two chains of commits, `old` and `new`, through the rewrites that relate their
//! commits. This is where one commit replaces another: a commit of `new` replaces a commit of
//! `old` when it was made from it, through any number of rewrites. For each commit of `old` that
//! `new` lacks, the comparison tells whether `new` holds a commit made from it, holds a commit it
//! was made from, or holds nothing related to it.

mod error;

use replaces_index_store::Index;
use runner::git;

pub use crate::error::Error;

/// What `new` holds of a commit of `old` that it lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// `new` holds these commits, which were made from it.
    Held(Vec<String>),
    /// `new` holds these commits, from which it was made: `old` holds a later version of them.
    Later(Vec<String>),
    /// It was made into these commits, none of which `new` holds.
    Rewritten(Vec<String>),
    /// Nothing related to it by rewrites is in `new`.
    Missing,
}

/// A commit of `old` that `new` lacks, and what `new` holds of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub commit: String,
    pub status: Status,
}

/// Each commit of `old` that `new` lacks, oldest first, with what `new` holds of it.
pub fn compare(old: &str, new: &str) -> Result<Vec<Entry>, Error> {
    let old = resolve(old)?;
    let new = resolve(new)?;
    let commits = runner::stdout(&mut git(["rev-list", "--reverse", &old, "--not", &new]))
        .map_err(Error::List)?;
    let index = Index::open().map_err(Error::Index)?;
    let mut entries = Vec::new();
    for commit in commits.lines() {
        let into = index.replaced_by(commit).map_err(Error::Index)?;
        let held: Vec<String> = into.iter().filter(|c| holds(&new, c)).cloned().collect();
        let status = if !held.is_empty() {
            Status::Held(held)
        } else {
            let from = index.replaces(commit).map_err(Error::Index)?;
            let earlier: Vec<String> = from.into_iter().filter(|c| holds(&new, c)).collect();
            if !earlier.is_empty() {
                Status::Later(earlier)
            } else if !into.is_empty() {
                Status::Rewritten(into.into_iter().collect())
            } else {
                Status::Missing
            }
        };
        entries.push(Entry {
            commit: commit.to_owned(),
            status,
        });
    }
    Ok(entries)
}

/// The full id of the commit that `name` names.
fn resolve(name: &str) -> Result<String, Error> {
    runner::stdout(&mut git([
        "rev-parse",
        "--verify",
        &format!("{name}^{{commit}}"),
    ]))
    .map(|id| id.trim().to_owned())
    .map_err(|source| Error::Resolve {
        name: name.to_owned(),
        source,
    })
}

/// Whether the chain that ends in `tip` holds `commit`. A commit the clone lacks is held by no
/// chain of it.
fn holds(tip: &str, commit: &str) -> bool {
    runner::run(&mut git(["merge-base", "--is-ancestor", commit, tip]))
        .is_ok_and(|output| output.status.success())
}
