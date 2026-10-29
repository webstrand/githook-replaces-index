//! Finds the commits related to a commit by rewrites: those it was made from, through any number
//! of rewrites, and those that were made from it, together with the refs of the clone that hold
//! each. Which of them replaces which holds only between two chains, and is left to the reader:
//! a commit that a rewrite was made from may still be on a branch of its own.
//!
//! A commit that the clone lacks may be given by any prefix of its id that names exactly one
//! commit of the index.

mod error;

use replaces_index_store::Index;
use runner::git;

pub use crate::error::Error;

/// How a commit is related to the one asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Relation {
    /// A commit that the one asked about was made from.
    From,
    /// The commit asked about.
    Itself,
    /// A commit that was made from the one asked about.
    Into,
}

impl Relation {
    pub fn label(self) -> &'static str {
        match self {
            Relation::From => "from",
            Relation::Itself => "self",
            Relation::Into => "into",
        }
    }
}

/// Where a commit is, in this clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// The refs that hold the commit, by their short names.
    On(Vec<String>),
    /// The clone has the commit, but no branch, remote-tracking branch or tag holds it.
    Unreferenced,
    /// The clone lacks the commit.
    Missing,
}

/// A commit related to the one asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub commit: String,
    pub relation: Relation,
    pub held: Held,
}

/// The commits related to `name` by rewrites, `name` included: those it was made from first, then
/// itself, then those made from it, each group by id.
pub fn whereis(name: &str) -> Result<Vec<Related>, Error> {
    let index = Index::open().map_err(Error::Index)?;
    let commit = resolve(&index, name)?;
    let from = index.replaces(&commit).map_err(Error::Index)?;
    let into = index.replaced_by(&commit).map_err(Error::Index)?;
    drop(index);

    // The commit asked about, when a cycle of rewrites leads back to it, is listed as itself only.
    let from = from.into_iter().filter(|c| *c != commit);
    let into = into.into_iter().filter(|c| *c != commit);
    let commits = from
        .map(|c| (c, Relation::From))
        .chain([(commit.clone(), Relation::Itself)])
        .chain(into.map(|c| (c, Relation::Into)));
    commits
        .map(|(commit, relation)| {
            Ok(Related {
                held: held(&commit)?,
                commit,
                relation,
            })
        })
        .collect()
}

/// The full id of `name`: a commit of this clone, or a prefix of exactly one commit of the index.
fn resolve(index: &Index, name: &str) -> Result<String, Error> {
    let local = runner::run(&mut git([
        "rev-parse",
        "-q",
        "--verify",
        &format!("{name}^{{commit}}"),
    ]))
    .map_err(Error::Resolve)?;
    if local.status.success() {
        return Ok(String::from_utf8_lossy(&local.stdout).trim().to_owned());
    }
    let mut matches = index
        .with_prefix(&name.to_ascii_lowercase())
        .map_err(Error::Index)?
        .into_iter();
    match (matches.next(), matches.next()) {
        (Some(commit), None) => Ok(commit),
        (Some(_), Some(_)) => Err(Error::Ambiguous(name.to_owned())),
        (None, _) => Err(Error::Unknown(name.to_owned())),
    }
}

/// Where `commit` is in this clone.
fn held(commit: &str) -> Result<Held, Error> {
    let exists = runner::run(&mut git([
        "cat-file",
        "-e",
        &format!("{commit}^{{commit}}"),
    ]))
    .map_err(Error::Refs)?;
    if !exists.status.success() {
        return Ok(Held::Missing);
    }
    let refs = runner::stdout(&mut git([
        "for-each-ref",
        "--format=%(refname) %(refname:short)",
        "--contains",
        commit,
        "refs/heads/",
        "refs/remotes/",
        "refs/tags/",
    ]))
    .map_err(Error::Refs)?;
    // A remote's HEAD, whose short name is the remote's alone, repeats one of its branches.
    let refs: Vec<String> = refs
        .lines()
        .filter_map(|line| line.split_once(' '))
        .filter(|(full, _)| !(full.starts_with("refs/remotes/") && full.ends_with("/HEAD")))
        .map(|(_, short)| short.to_owned())
        .collect();
    Ok(if refs.is_empty() {
        Held::Unreferenced
    } else {
        Held::On(refs)
    })
}
