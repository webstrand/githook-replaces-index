//! What `git rebase-replaces` says besides its failures, after `rebase-replaces:`.

#[derive(Debug, thiserror::Error)]
pub enum Notice {
    #[error("{commit} was rewritten as {into} but {upstream} lacks it; replaying it")]
    Lacked {
        commit: String,
        into: String,
        upstream: String,
    },
    #[error("leaving out {commit}: {why}")]
    LeavingOut { commit: String, why: String },
    #[error(
        "replaying {replayed} commits onto {onto}, keeping the base, leaving out {left_out} {upstream} already holds"
    )]
    ReplayingKeepingBase {
        replayed: usize,
        onto: String,
        left_out: usize,
        upstream: String,
    },
    #[error(
        "replaying {replayed} commits onto {upstream}, leaving out {left_out} it already holds"
    )]
    Replaying {
        replayed: usize,
        upstream: String,
        left_out: usize,
    },
    #[error(
        "{this} names {left_out}, which is left out, and no single commit replaces it; replaying it as it is"
    )]
    NoReplacement { this: String, left_out: String },
    #[error("making {this} name {short} {subject}, which replaces {left_out}")]
    Retargeting {
        this: String,
        short: String,
        subject: String,
        left_out: String,
    },
}

/// Why a commit is left out.
#[derive(Debug, thiserror::Error)]
pub enum Why {
    #[error("the reflog of {upstream} shows that it held it")]
    Reflog { upstream: String },
    #[error("{upstream} holds {into} made from it")]
    Held { upstream: String, into: String },
}
