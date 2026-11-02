//! Compares two chains of commits, `old` and `new`, through the rewrites that relate their
//! commits. This is where one commit replaces another: a commit of `new` replaces a commit of
//! `old` when it was made from it, through any number of rewrites. For each commit of `old` that
//! `new` lacks, the comparison tells whether `new` holds a commit made from it, holds a commit it
//! was made from, or holds nothing related to it.

mod error;

use std::collections::{BTreeSet, HashMap, HashSet};

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
    // What each was made into and from, read, and the index closed, before `held` runs git, so
    // that other processes may open the index meanwhile.
    let index = Index::open().map_err(Error::Index)?;
    let related = commits
        .lines()
        .map(|commit| {
            let into = index.replaced_by(commit).map_err(Error::Index)?;
            let from = index.replaces(commit).map_err(Error::Index)?;
            Ok((commit, into, from))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    drop(index);
    let held = held(
        &new,
        related
            .iter()
            .flat_map(|(_, into, from)| into.iter().chain(from)),
    )?;
    Ok(related
        .into_iter()
        .map(|(commit, into, from)| {
            let held_into: Vec<String> =
                into.iter().filter(|c| held.contains(*c)).cloned().collect();
            let earlier: Vec<String> = from.into_iter().filter(|c| held.contains(c)).collect();
            let status = if !held_into.is_empty() {
                Status::Held(held_into)
            } else if !earlier.is_empty() {
                Status::Later(earlier)
            } else if !into.is_empty() {
                Status::Rewritten(into.into_iter().collect())
            } else {
                Status::Missing
            };
            Entry {
                commit: commit.to_owned(),
                status,
            }
        })
        .collect())
}

/// A commit's abbreviated id and subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    pub short: String,
    pub subject: String,
}

/// The abbreviated id and subject of each of `commits`, which the clone has, by one `git log`.
pub fn describe<'a>(
    commits: impl IntoIterator<Item = &'a str>,
) -> Result<HashMap<String, Described>, Error> {
    let input: String = commits.into_iter().map(|c| format!("{c}\n")).collect();
    // git log reads no commit as HEAD.
    if input.is_empty() {
        return Ok(HashMap::new());
    }
    let mut log = git(["log", "--no-walk=unsorted", "--stdin", "--format=%H %h %s"]);
    let out = runner::stdout_with_input(&mut log, input.as_bytes()).map_err(Error::Describe)?;
    Ok(out
        .lines()
        .filter_map(|line| {
            let (commit, rest) = line.split_once(' ')?;
            let (short, subject) = rest.split_once(' ').unwrap_or((rest, ""));
            let described = Described {
                short: short.to_owned(),
                subject: subject.to_owned(),
            };
            Some((commit.to_owned(), described))
        })
        .collect())
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

/// Those of `commits` that the chain that ends in `tip` holds, by two git commands for all of
/// them rather than one for each. Of those the clone has, `git rev-list` lists every commit they
/// reach without passing through `tip`'s history: a commit that it does not list is one that `tip`
/// reaches. The walk stops where it meets that history, so its cost is that of the commits apart
/// from it, not of the whole history. A commit the clone lacks is held by no chain of it.
fn held<'a>(
    tip: &str,
    commits: impl IntoIterator<Item = &'a String>,
) -> Result<HashSet<String>, Error> {
    let error = |source| Error::Holds {
        tip: tip.to_owned(),
        source,
    };
    let commits: BTreeSet<&str> = commits.into_iter().map(String::as_str).collect();
    if commits.is_empty() {
        return Ok(HashSet::new());
    }
    let present = runner::commits(commits).map_err(error)?;
    if present.is_empty() {
        return Ok(HashSet::new());
    }
    let mut revs: String = present.iter().map(|c| format!("{c}\n")).collect();
    revs.push_str(&format!("^{tip}\n"));
    let apart = runner::stdout_with_input(&mut git(["rev-list", "--stdin"]), revs.as_bytes())
        .map_err(error)?;
    let apart: HashSet<&str> = apart.lines().collect();
    Ok(present
        .into_iter()
        .filter(|c| !apart.contains(c.as_str()))
        .collect())
}
