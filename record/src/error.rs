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
    #[error("{0} is not the full id of a commit, in lowercase, as git writes it")]
    NotAnId(String),
    #[error("{0} is not a commit of this clone, so no note can be written for it")]
    NotInClone(String),
    #[error(
        "could not write {REF}: {} has been held for {seconds}s; if no git command is running, it was left behind, and removing it lets the write through",
        path.display()
    )]
    Locked {
        path: std::path::PathBuf,
        seconds: u64,
    },
    #[error("could not read or write {path}: {source}")]
    Checked {
        path: std::path::PathBuf,
        source: io::Error,
    },
    #[error("could not read or remove the abandoned records in {path}: {source}")]
    Abandoned {
        path: std::path::PathBuf,
        source: io::Error,
    },
}
