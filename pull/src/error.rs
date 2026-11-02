/// A failure of a pull, displayed after `pull-replaces:`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not fetch {remote}")]
    Fetch {
        remote: String,
        source: runner::Error,
    },
    #[error(
        "{remote} has no fetch refspec that brings its {} beside this clone's; to add one: git config --add 'remote.{remote}.fetch' '+refs/notes/replaces*:refs/notes/remotes/{remote}/replaces*'",
        replaces_index_record::REF
    )]
    Unconfigured { remote: String },
    #[error("could not merge the notes of {remote}")]
    Merge {
        remote: String,
        source: replaces_index_record::Error,
    },
}
