/// A failure of a comparison, displayed after `compare-replaces:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not resolve {name}")]
    Resolve { name: String, source: runner::Error },
    #[error("could not list the commits to compare")]
    List(#[source] runner::Error),
    #[error(transparent)]
    Index(replaces_index_store::Error),
    #[error("could not tell which commits {tip} holds")]
    Holds { tip: String, source: runner::Error },
    #[error("could not read the subjects of the commits")]
    Describe(#[source] runner::Error),
}
