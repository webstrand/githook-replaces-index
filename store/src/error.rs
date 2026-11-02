use std::io;
use std::path::PathBuf;

use replaces_index_record::REF;

/// A failure to bring the index up to date or to query it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not find the git directory")]
    GitDir(#[source] runner::Error),
    #[error("could not create {path}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("could not read {REF}")]
    Notes(#[source] runner::Error),
    #[error(transparent)]
    Ref(replaces_index_record::Error),
    #[error("could not move the index built into place at {}", path.display())]
    Replace { path: PathBuf, source: io::Error },
    #[error("{} stayed locked by another process", .0.display())]
    Busy(PathBuf),
    #[error("could not use the index {}", path.display())]
    Index { path: PathBuf, source: redb::Error },
    #[error(
        "unable to read the replaces index at {}: {source}. No data has been lost: the index is damaged, but it is rebuilt from {REF}; delete {} and retry the command to rebuild it",
        path.display(),
        path.display()
    )]
    Damaged { path: PathBuf, source: redb::Error },
}
