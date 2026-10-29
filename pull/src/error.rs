use replaces_index_record::REF;

/// A failure of a pull, displayed after `pull-replaces:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not list the refs of {remote}: {source}")]
    List {
        remote: String,
        source: runner::Error,
    },
    #[error("could not fetch {REF} from {remote}: {source}")]
    Fetch {
        remote: String,
        source: runner::Error,
    },
    #[error("could not merge the notes of {remote}: {source}")]
    Merge {
        remote: String,
        source: runner::Error,
    },
}
