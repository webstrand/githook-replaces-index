//! Records in refs/notes/replaces the commits that an amend or a rebase made, through `record`:
//! the note of each new commit comes to stand in place of the commit it was made from.
//!
//! During a rebase, git reports each fixup or squash step, and each amend that an `exec` line or
//! a stopped rebase makes, as an amend of a commit the rebase created itself. Such amends are set
//! aside instead of recorded, and when the rebase ends, each replacement it reports is followed
//! through them to the commit that took its place, so that no commit the rebase discarded is
//! named. An amend whose commit is no longer on the branch, because it was undone, for instance by
//! a `git reset` back to the amended commit, is not followed. They are set aside in the rebase's
//! state directory, which git removes when the rebase ends or is aborted. `git am` keeps its
//! state in rebase-apply/ as well, but marks it with `applying` rather than `rebasing`, and runs
//! no post-rewrite when it ends, so an amend while it is stopped is recorded at once.
//!
//! An amend during a rebase of a commit that was there before the rebase began, such as one that a
//! pick kept as it was or the commit the rebase is onto, is recorded at once as well: the rebase
//! does not report such a commit as rewritten, so nothing would lead to the amend, and a rebase
//! that rewrites nothing else runs no post-rewrite when it ends.

mod error;

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};

use replaces_index_record::Rewrite;

pub use crate::error::Error;

/// The command that rewrote the commits, as git passes it to the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    Amend,
    Rebase,
    /// A run of git filter-repo, which `record-filter-repo` passes on.
    FilterRepo,
}

/// Records the rewrite that `input` reports, one `<old> <new>` line per rewritten commit.
pub fn post_rewrite(kind: Kind, input: &str) -> Result<(), Failure> {
    let rewrite = resolve(kind, Rewrite::parse(input));
    replaces_index_record::record(&rewrite).map_err(|error| Failure {
        error: Error::Record(error),
        rewrite,
    })
}

/// A rewrite that could not be recorded, and why.
#[derive(Debug)]
pub struct Failure {
    pub error: Error,
    /// What remains to be recorded: every commit of the rewrite that was not set aside.
    pub rewrite: Rewrite,
}

/// What of `rewrite` is to be recorded now: during a rebase, the amends of commits that the rebase
/// made are set aside, and the replacements it reports when it ends are followed through them.
///
/// When the hook cannot keep track of the amends, it says why and records a cruder form, which
/// names commits that it would otherwise leave out, or which leaves out amends it cannot read:
/// each replacement it records holds all the same, and a rewrite is not lost to a failure that
/// recording it would not meet.
fn resolve(kind: Kind, mut rewrite: Rewrite) -> Rewrite {
    if kind == Kind::FilterRepo {
        return rewrite;
    }
    let state = match rebase_state() {
        Ok(Some(state)) => state,
        Ok(None) => return rewrite,
        Err(err) => {
            warn(err, "the rewrite is recorded as git reported it");
            return rewrite;
        }
    };
    match kind {
        Kind::Amend => {
            let mut aside = Vec::new();
            for replacement in mem::take(&mut rewrite.replacements) {
                match before_rebase(&state, &replacement.0) {
                    Ok(true) => rewrite.replacements.push(replacement),
                    Ok(false) => aside.push(replacement),
                    Err(err) => {
                        warn(err, "the amend is recorded at once");
                        rewrite.replacements.push(replacement);
                    }
                }
            }
            if let Err(err) = set_aside(&state.join(AMENDED), &aside) {
                warn(err, "the amend is recorded at once");
                rewrite.replacements.extend(aside);
            }
        }
        Kind::Rebase => {
            let amends = match Amends::read(&state.join(AMENDED)) {
                Ok(amends) => amends,
                Err(err) => {
                    warn(err, "the amends made during the rebase are not recorded");
                    return rewrite;
                }
            };
            match amends.follow(&rewrite, on_branch) {
                Ok(followed) => rewrite = followed,
                Err(err) => {
                    warn(
                        err,
                        "every amend made during the rebase is recorded as it was made",
                    );
                    rewrite.replacements.extend(amends.made);
                }
            }
        }
        Kind::FilterRepo => {}
    }
    rewrite
}

/// Says why the rewrite is recorded in a cruder form than the hook would record.
fn warn(err: Error, consequence: &str) {
    let _ = writeln!(io::stderr(), "replaces-index: {err}; {consequence}");
}

/// The file of the rebase's state directory in which the hook sets amends aside.
const AMENDED: &str = "replaces-index-amended";

/// The state directory of the rebase in progress, if any.
fn rebase_state() -> Result<Option<PathBuf>, Error> {
    let state = |dir| runner::rev_parse_path(&["--git-path", dir]).map_err(Error::RebaseState);
    let merge = state("rebase-merge")?;
    let apply = state("rebase-apply")?;
    let state = if apply.join("rebasing").is_file() {
        Some(apply)
    } else if merge.is_dir() {
        Some(merge)
    } else {
        None
    };
    Ok(state)
}

/// Whether `commit` was there before the rebase whose state is in `state` began: an ancestor of
/// the branch it rebases or of the commit it rebases onto.
fn before_rebase(state: &Path, commit: &str) -> Result<bool, Error> {
    let mut args = vec!["rev-list", "-n", "1", commit, "--not"];
    let mut ids = Vec::new();
    for name in ["orig-head", "onto"] {
        let path = state.join(name);
        match fs::read_to_string(&path) {
            Ok(id) => ids.push(id),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(Error::ReadState { path, source }),
        }
    }
    args.extend(ids.iter().map(|id| id.trim()));
    let newer = runner::stdout(&mut runner::git(args)).map_err(Error::BeforeRebase)?;
    Ok(newer.trim().is_empty())
}

/// Whether `commit` is HEAD or one of its ancestors.
fn on_branch(commit: &str) -> Result<bool, Error> {
    let mut command = runner::git(["merge-base", "--is-ancestor", commit, "HEAD"]);
    let output = runner::run(&mut command).map_err(Error::Branch)?;
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    runner::check(&command, &output).map_err(Error::Branch)?;
    Ok(true)
}

/// Adds `amends` to those set aside in `path`.
fn set_aside(path: &Path, amends: &[(String, String)]) -> Result<(), Error> {
    if amends.is_empty() {
        return Ok(());
    }
    let to_error = |source| Error::WriteAmends {
        path: path.to_owned(),
        source,
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(to_error)?;
    let lines: String = amends
        .iter()
        .map(|(old, new)| format!("{old} {new}\n"))
        .collect();
    file.write_all(lines.as_bytes()).map_err(to_error)
}

/// The amends set aside during a rebase: each commit amended, with the commit that replaced it
/// last.
#[derive(Default)]
struct Amends {
    last: HashMap<String, String>,
    /// Every amend, in the order it was made.
    made: Vec<(String, String)>,
}

impl Amends {
    fn read(path: &Path) -> Result<Self, Error> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(Error::ReadAmends {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let mut amends = Self::default();
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let (Some(old), Some(new)) = (words.next(), words.next()) else {
                continue;
            };
            // A commit amended twice is replaced by the later amend.
            amends.last.insert(old.to_owned(), new.to_owned());
            amends.made.push((old.to_owned(), new.to_owned()));
        }
        Ok(amends)
    }

    /// `rewrite` with each new commit followed to the commit that finally took its place.
    fn follow(
        &self,
        rewrite: &Rewrite,
        mut kept: impl FnMut(&str) -> Result<bool, Error>,
    ) -> Result<Rewrite, Error> {
        let mut followed = rewrite.clone();
        for (_, new) in &mut followed.replacements {
            *new = self.last(new, &mut kept)?.to_owned();
        }
        Ok(followed)
    }

    /// The commit that finally took the place of `commit`, following the amends whose commit
    /// `kept` accepts.
    fn last<'a>(
        &'a self,
        mut commit: &'a str,
        mut kept: impl FnMut(&str) -> Result<bool, Error>,
    ) -> Result<&'a str, Error> {
        let mut visited = HashSet::new();
        while let Some(next) = self.last.get(commit) {
            if next == commit || !visited.insert(commit) || !kept(next)? {
                break;
            }
            commit = next;
        }
        Ok(commit)
    }
}
