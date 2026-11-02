//! What the hook says besides its failures, after `replaces-index:`.

use crate::Error;

/// Why the hook records a rewrite in a cruder form than it would, with the error that made it.
#[derive(Debug, thiserror::Error)]
pub enum Fallback {
    #[error("the rewrite is recorded as git reported it")]
    AsReported(#[source] Error),
    #[error("a replacement of the commit stopped at for an edit is recorded at once")]
    EditedAtOnce(#[source] Error),
    #[error("a replacement of a commit it names is deferred")]
    NamedDeferred(#[source] Error),
    #[error("the replacement is recorded at once")]
    AtOnce(#[source] Error),
    #[error("a later replacement of the commit it made is deferred")]
    LaterDeferred(#[source] Error),
    #[error("the deferred replacements are not recorded")]
    DeferredLost(#[source] Error),
    #[error("every deferred replacement is recorded as it was made")]
    AsMade(#[source] Error),
}

/// What became of the abandoned records, when it is not what the hook means to do.
#[derive(Debug, thiserror::Error)]
pub enum Abandoned {
    #[error("the abandoned records wait for a later rewrite")]
    Unread(#[source] replaces_index_record::Error),
    #[error("the abandoned records are recorded again by the next rewrite")]
    Unremoved(#[source] replaces_index_record::Error),
}

/// Says `notice` on standard error, after `replaces-index:`.
pub fn tell(notice: &dyn std::error::Error) {
    replaces_index_record::notice::say("replaces-index", notice);
}
