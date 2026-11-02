use replaces_index_record::REF;

/// A failure of a lookup, displayed after `whereis:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} is not a commit of this clone or of {REF}")]
    Unknown(String),
    #[error("{0} is ambiguous in {REF}")]
    Ambiguous(String),
    #[error("could not resolve a commit")]
    Resolve(#[source] runner::Error),
    #[error("could not list the refs that hold a commit")]
    Refs(#[source] runner::Error),
    #[error(transparent)]
    Index(replaces_index_store::Error),
}
