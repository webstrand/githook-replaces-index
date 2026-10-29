use std::io;
use std::path::PathBuf;

/// A failure to record a rewrite, displayed after `replaces-index:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read the rewritten commits: {0}")]
    Input(#[source] io::Error),
    #[error("could not find the state of a rebase: {0}")]
    RebaseState(#[source] runner::Error),
    #[error("could not read {path}: {source}")]
    ReadAmends { path: PathBuf, source: io::Error },
    #[error("could not set the amend aside in {path}: {source}")]
    WriteAmends { path: PathBuf, source: io::Error },
    #[error("{0}")]
    Record(#[source] replaces_index_record::Error),
}
