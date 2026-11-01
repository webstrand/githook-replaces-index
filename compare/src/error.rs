/// A failure of a comparison, displayed after `compare-replaces:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not resolve {name}: {source}")]
    Resolve { name: String, source: runner::Error },
    #[error("could not list the commits to compare: {0}")]
    List(#[source] runner::Error),
    #[error("{0}")]
    Index(#[source] replaces_index_store::Error),
    #[error("could not tell whether {tip} holds {commit}: {source}")]
    Holds {
        tip: String,
        commit: String,
        source: runner::Error,
    },
}
