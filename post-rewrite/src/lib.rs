//! Records in refs/notes/replaces the commits that an amend or a rebase made, through `record`:
//! the note of each new commit comes to stand in place of the commit it was made from.
//!
//! During a rebase, git reports each fixup or squash step, and each amend that an `exec` line or
//! a stopped rebase makes, as an amend of a commit the rebase created itself. Such amends are set
//! aside instead of recorded, and when the rebase ends, each replacement it reports is followed
//! through them to the commit that took its place, so that no commit the rebase discarded is
//! named. They are set aside in the rebase's state directory, which git removes when the rebase
//! ends or is aborted. `git am` keeps its state in rebase-apply/ as well, but marks it with
//! `applying` rather than `rebasing`, and runs no post-rewrite when it ends, so an amend while it
//! is stopped is recorded at once.

mod error;

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
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
pub fn post_rewrite(kind: Kind, input: &str) -> Result<(), Error> {
    let amended = amended()?;
    if let (Kind::Amend, Some(path)) = (kind, &amended) {
        return append(path, input);
    }
    let amends = match (kind, &amended) {
        (Kind::Rebase, Some(path)) => Amends::read(path)?,
        _ => Amends::default(),
    };

    let mut rewrite = Rewrite::parse(input);
    for (_, new) in &mut rewrite.replacements {
        *new = amends.last(new).to_owned();
    }
    replaces_index_record::record(&rewrite).map_err(Error::Record)
}

/// The file in which the rebase in progress, if any, sets its amends aside.
fn amended() -> Result<Option<PathBuf>, Error> {
    let state = |dir| runner::rev_parse_path(&["--git-path", dir]).map_err(Error::RebaseState);
    let merge = state("rebase-merge")?;
    let apply = state("rebase-apply")?;
    let amended = if apply.join("rebasing").is_file() {
        Some(apply.join("replaces-index-amended"))
    } else if merge.is_dir() {
        Some(merge.join("replaces-index-amended"))
    } else {
        None
    };
    Ok(amended)
}

fn append(path: &Path, input: &str) -> Result<(), Error> {
    let to_error = |source| Error::WriteAmends {
        path: path.to_owned(),
        source,
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(to_error)?;
    writeln!(file, "{}", input.trim_end_matches('\n')).map_err(to_error)
}

/// The amends set aside during a rebase: each commit amended, with the commit that replaced it
/// last.
#[derive(Default)]
struct Amends {
    last: HashMap<String, String>,
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
        }
        Ok(amends)
    }

    /// The commit that finally took the place of `commit`, following the amends.
    fn last<'a>(&'a self, mut commit: &'a str) -> &'a str {
        let mut visited = HashSet::new();
        while let Some(next) = self.last.get(commit) {
            if next == commit || !visited.insert(commit) {
                break;
            }
            commit = next;
        }
        commit
    }
}
