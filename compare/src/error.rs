/// A failure of a comparison, displayed after `compare-replaces:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not resolve {name}: {source}")]
    Resolve { name: String, source: runner::Error },
    #[error("could not list the commits to compare: {0}")]
    List(#[source] runner::Error),
    #[error("{0}")]
    Index(#[source] replaces_index_store::Error),
    #[error("could not tell which commits {tip} holds: {source}")]
    Holds { tip: String, source: runner::Error },
    #[error("could not read the subjects of the commits: {0}")]
    Describe(#[source] runner::Error),
}
