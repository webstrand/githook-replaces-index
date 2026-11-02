use std::io;
use std::path::PathBuf;

/// A failure of the rebase or of its sequence editor, displayed after `rebase-replaces:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not resolve {upstream}: {source}")]
    Upstream {
        upstream: String,
        source: runner::Error,
    },
    #[error("could not pull the replaces notes of {remote}: {source}")]
    Pull {
        remote: String,
        source: replaces_index_pull::Error,
    },
    #[error("could not list the commits to replay: {0}")]
    List(#[source] runner::Error),
    #[error("{0}")]
    Compare(#[source] replaces_index_compare::Error),
    #[error("could not create a temporary directory: {0}")]
    TempDir(#[source] io::Error),
    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("could not find this program's executable: {0}")]
    CurrentExe(#[source] io::Error),
    #[error(
        "the commits of {upstream} that replace those left out end in {tips}, apart, so that no base can be kept; nothing was rebased"
    )]
    NoBase { upstream: String, tips: String },
    #[error("could not run git rebase: {0}")]
    Rebase(#[source] io::Error),
}
