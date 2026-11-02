use std::io;
use std::path::PathBuf;

/// A failure to record a run of git filter-repo, displayed after `replaces-index:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not find the git directory")]
    GitDir(#[source] runner::Error),
    #[error("{} does not exist; git filter-repo has not run here", .0.display())]
    NoMap(PathBuf),
    #[error("could not read {path}")]
    Read { path: PathBuf, source: io::Error },
    #[error("could not keep a copy of the map in {path}")]
    Keep { path: PathBuf, source: io::Error },
    #[error(transparent)]
    Record(replaces_index_post_rewrite::Error),
}
