/// A failure of a pull, displayed after `pull-replaces:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not fetch {remote}: {source}")]
    Fetch {
        remote: String,
        source: runner::Error,
    },
    #[error("could not merge the notes of {remote}: {source}")]
    Merge {
        remote: String,
        source: replaces_index_record::Error,
    },
}
