//! Records in refs/notes/replaces the commits that `git commit --amend` or a rebase made, through
//! `record`: the note of each new commit comes to stand in place of the commit it was made from.
//!
//! During a rebase, git reports each fixup or squash step, and each `git commit --amend` that an
//! `exec` line or the operator at a stop of the rebase runs, as a single-commit rewrite, which git
//! calls `amend`, of a commit the rebase created itself. The replacements of such rewrites are
//! deferred instead of recorded, and when the rebase ends, each replacement it reports is followed
//! through them to the last commit that took its place and is still on the branch, so that no
//! commit the rebase discarded is named. A deferred replacement whose commit is no longer on the
//! branch, because it was undone, for instance by a `git reset` back to the rewritten commit, is
//! not followed. The deferred replacements are kept in the rebase's state directory, which git
//! removes when the rebase ends or is aborted. `git am` keeps its state in rebase-apply/ as well,
//! but marks it with `applying` rather than `rebasing`, and runs no post-rewrite when it ends, so a
//! single-commit rewrite while it is stopped is recorded at once.
//!
//! A single-commit rewrite during a rebase of a commit that was there before the rebase began, such
//! as one that a pick kept as it was or the commit the rebase is onto, is recorded at once as well:
//! the rebase does not report such a commit as rewritten, so nothing would lead to the
//! replacement, and a rebase that rewrites nothing else runs no post-rewrite when it ends. The
//! exception is the commit that the rebase stopped at for an `edit`, which it names in git's
//! `rebase-merge/amend` file: the rebase reports it as replaced by the commit it finds when it goes
//! on, kept pick or not, so its replacements are deferred as well. A replacement of a commit that a
//! replacement recorded at once made is recorded at once in turn, since nothing the rebase reports
//! leads to it either.

mod error;

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use replaces_index_record::Rewrite;
use replaces_index_record::abandoned::{self, Waiting};

pub use crate::error::Error;

/// The command that rewrote the commits, as git passes it to the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    /// A single-commit rewrite, which git calls `amend`.
    #[value(name = "amend")]
    Single,
    Rebase,
    /// A run of git filter-repo, which `record-filter-repo` passes on.
    FilterRepo,
}

/// Records the rewrite that `input` reports, one `<old> <new>` line per rewritten commit.
///
/// Until the ref holds it, `outstanding` holds what of the rewrite remains to be recorded, so that
/// a thread that must end the process first may keep it; then it holds nothing.
pub fn post_rewrite(
    kind: Kind,
    input: &str,
    outstanding: &Mutex<Option<Rewrite>>,
) -> Result<(), Failure> {
    let hold = |rewrite: Option<Rewrite>| {
        *outstanding
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = rewrite;
    };
    let reported = Rewrite::parse(input);
    hold(Some(reported.clone()));
    let rewrite = resolve(kind, reported, outstanding);
    hold(Some(rewrite.clone()));
    // The abandoned records are recorded first, so that a rewrite that came later inherits what
    // they record.
    let waiting = waiting();
    let mut all = waiting.rewrite.clone();
    all.replacements
        .extend(rewrite.replacements.iter().cloned());
    replaces_index_record::record_then(&all, || hold(None)).map_err(|error| Failure {
        error: Error::Record(error),
        rewrite,
    })?;
    if let Err(err) = waiting.remove() {
        tell(format_args!(
            "{err}; they are recorded again by the next rewrite"
        ));
    }
    replaces_index_record::repair_and_say("replaces-index");
    Ok(())
}

/// The abandoned records of the repository, read to be recorded; none when they cannot be read,
/// which is said.
fn waiting() -> Waiting {
    let read = abandoned::dir()
        .map_err(|err| err.to_string())
        .and_then(|dir| abandoned::waiting(&dir).map_err(|err| err.to_string()));
    match read {
        Ok(waiting) => {
            for dropped in &waiting.dropped {
                tell(format_args!("{dropped}; it is left out"));
            }
            waiting
        }
        Err(err) => {
            tell(format_args!(
                "{err}; the abandoned records wait for a later rewrite"
            ));
            Waiting::default()
        }
    }
}

/// A rewrite that could not be recorded, and why.
#[derive(Debug)]
pub struct Failure {
    pub error: Error,
    /// What remains to be recorded: every replacement of the rewrite that was not deferred.
    pub rewrite: Rewrite,
}

/// What of `rewrite` is to be recorded now: during a rebase, the replacements of commits that the
/// rebase made are deferred, and the replacements it reports when it ends are followed through
/// them.
///
/// When the hook cannot keep track of the deferred replacements, it says why and records a cruder
/// form, which names commits that it would otherwise leave out, or which leaves out deferred
/// replacements it cannot read: each replacement it records holds all the same, and a rewrite is
/// not lost to a failure that recording it would not meet.
///
/// The replacements are deferred while `outstanding` is locked, and it is made to hold what
/// remains before it is let go of, so that a thread that ends the process keeps each replacement
/// either as git reported it or deferred, and never both. When the rebase ends, `outstanding`
/// holds the cruder form while the replacements are followed, so that a thread that ends the
/// process meanwhile keeps every deferred replacement as well.
fn resolve(kind: Kind, mut rewrite: Rewrite, outstanding: &Mutex<Option<Rewrite>>) -> Rewrite {
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
        Kind::Single => {
            let mut deferred = Vec::new();
            let edited = edited(&state).unwrap_or_else(|err| {
                warn(
                    err,
                    "a replacement of the commit stopped at for an edit is recorded at once",
                );
                String::new()
            });
            // The commits that replacements recorded at once made: nothing that the rebase reports
            // leads to them either, so their own replacements are recorded at once as well, at
            // the cost of naming the commit between.
            let made_at_once: HashSet<String> = match Replacements::read(&state.join(AT_ONCE)) {
                Ok(at_once) => at_once.made.into_iter().map(|(_, new)| new).collect(),
                Err(err) => {
                    warn(err, "a replacement of a commit it names is deferred");
                    HashSet::new()
                }
            };
            for replacement in mem::take(&mut rewrite.replacements) {
                let before = if replacement.0 == edited {
                    Ok(false)
                } else if made_at_once.contains(&replacement.0) {
                    Ok(true)
                } else {
                    before_rebase(&state, &replacement.0)
                };
                match before {
                    Ok(true) => rewrite.replacements.push(replacement),
                    Ok(false) => deferred.push(replacement),
                    Err(err) => {
                        warn(err, "the replacement is recorded at once");
                        rewrite.replacements.push(replacement);
                    }
                }
            }
            let mut held = outstanding
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Err(err) = append(&state.join(DEFERRED), &deferred) {
                warn(err, "the replacement is recorded at once");
                rewrite.replacements.extend(deferred);
            }
            if let Err(err) = append(&state.join(AT_ONCE), &rewrite.replacements) {
                warn(err, "a later replacement of the commit it made is deferred");
            }
            *held = Some(rewrite.clone());
        }
        Kind::Rebase => {
            let deferred = match Replacements::read(&state.join(DEFERRED)) {
                Ok(deferred) => deferred,
                Err(err) => {
                    warn(err, "the deferred replacements are not recorded");
                    return rewrite;
                }
            };
            // Kept, should a signal end the hook before the replacements are followed, as the
            // cruder form that a failure to follow them records: once a signal arrives, no git
            // command runs to tell which deferred replacements to follow.
            let mut crude = rewrite.clone();
            crude.replacements.extend(deferred.made.iter().cloned());
            *outstanding
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(crude);
            match deferred.follow(&rewrite, on_branch) {
                Ok(followed) => rewrite = followed,
                Err(err) => {
                    warn(err, "every deferred replacement is recorded as it was made");
                    rewrite.replacements.extend(deferred.made);
                }
            }
        }
        Kind::FilterRepo => {}
    }
    rewrite
}

/// Says why the rewrite is recorded in a cruder form than the hook would record.
fn warn(err: Error, consequence: &str) {
    tell(format_args!("{err}; {consequence}"));
}

fn tell(message: impl std::fmt::Display) {
    let _ = writeln!(io::stderr(), "replaces-index: {message}");
}

/// The file of the rebase's state directory in which the hook keeps the deferred replacements.
const DEFERRED: &str = "replaces-index-deferred";

/// The file of the rebase's state directory in which the hook keeps the replacements of
/// single-commit rewrites that it recorded at once.
const AT_ONCE: &str = "replaces-index-at-once";

/// The state directory of the rebase in progress, if any.
fn rebase_state() -> Result<Option<PathBuf>, Error> {
    let paths =
        runner::rev_parse_paths(&["--git-path", "rebase-merge", "--git-path", "rebase-apply"])
            .map_err(Error::RebaseState)?;
    let mut paths = paths.into_iter();
    let (Some(merge), Some(apply)) = (paths.next(), paths.next()) else {
        return Ok(None);
    };
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

/// The commit that the rebase whose state is in `state` stopped at for an `edit`, which git names
/// in its `rebase-merge/amend` file and reports as replaced by the commit it finds when it goes
/// on; empty when it did not stop for one.
fn edited(state: &Path) -> Result<String, Error> {
    let path = state.join("amend");
    match fs::read_to_string(&path) {
        Ok(id) => Ok(id.trim().to_owned()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(Error::ReadState { path, source }),
    }
}

fn on_branch(commit: &str) -> Result<bool, Error> {
    runner::is_ancestor(commit, "HEAD").map_err(Error::Branch)
}

/// Adds `replacements` to those kept in `path`.
fn append(path: &Path, replacements: &[(String, String)]) -> Result<(), Error> {
    if replacements.is_empty() {
        return Ok(());
    }
    let to_error = |source| Error::WriteReplacements {
        path: path.to_owned(),
        source,
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(to_error)?;
    let lines: String = replacements
        .iter()
        .map(|(old, new)| format!("{old} {new}\n"))
        .collect();
    file.write_all(lines.as_bytes()).map_err(to_error)
}

/// The replacements kept in a file of the rebase's state directory, the deferred ones or those
/// recorded at once: each rewritten commit, with the commit that its last replacement made.
#[derive(Default)]
struct Replacements {
    last: HashMap<String, String>,
    /// Every replacement, in the order it was made.
    made: Vec<(String, String)>,
}

impl Replacements {
    fn read(path: &Path) -> Result<Self, Error> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(Error::ReadReplacements {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let made = Rewrite::parse(&text).replacements;
        // A commit rewritten twice is replaced by the commit that the later rewrite made.
        let last = made.iter().cloned().collect();
        Ok(Self { last, made })
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

    /// The commit that finally took the place of `commit`: the last commit of the chain of
    /// replacements from it that `kept` accepts, or `commit` when it accepts none. The commits
    /// between were replaced in turn, and are on the branch no more than the commit of an undone
    /// replacement, so only the end of the chain tells whether it was undone.
    fn last<'a>(
        &'a self,
        commit: &'a str,
        mut kept: impl FnMut(&str) -> Result<bool, Error>,
    ) -> Result<&'a str, Error> {
        let mut chain = vec![commit];
        // The visited commits end a cycle, as a replacement that made a commit again
        // forms.
        let mut visited = HashSet::from([commit]);
        let mut at = commit;
        while let Some(next) = self.last.get(at) {
            if !visited.insert(next.as_str()) {
                break;
            }
            chain.push(next);
            at = next;
        }
        for &made in chain[1..].iter().rev() {
            if kept(made)? {
                return Ok(made);
            }
        }
        Ok(commit)
    }
}
