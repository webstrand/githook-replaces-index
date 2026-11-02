use std::io;
use std::path::PathBuf;

use replaces_index_record::REF;

/// A failure to bring the index up to date or to query it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not find the git directory: {0}")]
    GitDir(#[source] runner::Error),
    #[error("could not create {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("could not read {REF}: {0}")]
    Notes(#[source] runner::Error),
    #[error("{} stayed locked by another process", .0.display())]
    Busy(PathBuf),
    #[error("could not use the index {}: {source}", path.display())]
    Index { path: PathBuf, source: redb::Error },
    #[error(
        "the index {} is damaged: {source}. It holds nothing that {REF} does not, so it may be deleted once no git whereis, compare-replaces or rebase-replaces is running, and the next of them builds it anew",
        path.display()
    )]
    Damaged { path: PathBuf, source: redb::Error },
}
