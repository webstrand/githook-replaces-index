use std::io;

use crate::REF;

/// A failure to record, displayed after `replaces-index:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read the replacements: {0}")]
    Input(#[source] io::Error),
    #[error("could not read {REF}: {0}")]
    Read(#[source] runner::Error),
    #[error("could not write {REF}: {0}")]
    Write(#[source] runner::Error),
    #[error("{REF} points to {0}, which is not a commit")]
    NotACommit(String),
}
