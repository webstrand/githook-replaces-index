use std::io;
use std::path::PathBuf;

/// A failure to record a rewrite, displayed after `replaces-index:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read the rewritten commits")]
    Input(#[source] io::Error),
    #[error("could not find the state of a rebase")]
    RebaseState(#[source] runner::Error),
    #[error("could not read {path}")]
    ReadState { path: PathBuf, source: io::Error },
    #[error("could not tell whether a commit was there before the rebase")]
    BeforeRebase(#[source] runner::Error),
    #[error("could not tell whether a commit is on the branch")]
    Branch(#[source] runner::Error),
    #[error("could not read {path}")]
    ReadReplacements { path: PathBuf, source: io::Error },
    #[error("could not add the replacements to {path}")]
    WriteReplacements { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Record(replaces_index_record::Error),
}
