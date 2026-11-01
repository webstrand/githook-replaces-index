//! Pulls a remote's replaces notes: fetches its refs/notes/replaces into
//! refs/notes/remotes/<remote>/replaces, and merges that into the local ref as `record` writes it,
//! taking the union of each note's lines and moving the ref only from the commit it read. Lines are
//! only ever added to the notes, so the union is the whole merge. The remote is not changed; it pulls from this
//! clone in turn.

mod error;

use std::process::Command;

use runner::git;

pub use crate::error::Error;

/// What a pull found at the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pulled {
    /// The remote's notes were merged into the local ones.
    Merged,
    /// The remote has no replaces notes.
    Absent,
}

/// Pulls the replaces notes of `remote`, fetching them alone.
pub fn pull(remote: &str) -> Result<Pulled, Error> {
    let refspec = refspec(remote);
    fetch_and_merge(
        remote,
        git(["fetch", "--porcelain", "-v", remote, &refspec]),
    )
}

/// Fetches `remote` as `git fetch <remote>` does, and pulls its replaces notes, which the same
/// fetch brings. `remote` must be the name of a configured remote.
pub fn fetch_and_pull(remote: &str) -> Result<Pulled, Error> {
    // Added to the refspecs that the remote's configuration gives, which a refspec named on the
    // command line would replace.
    let config = format!("remote.{remote}.fetch={}", refspec(remote));
    fetch_and_merge(
        remote,
        git(["-c", &config, "fetch", "--porcelain", "-v", remote]),
    )
}

/// The refspec that fetches the remote's notes. It is a glob, so that a remote without notes
/// matches nothing rather than failing the fetch; the `*` matches the empty string.
fn refspec(remote: &str) -> String {
    format!("+refs/notes/replaces*:refs/notes/remotes/{remote}/replaces*")
}

/// Runs `fetch`, a `git fetch --porcelain -v` that fetches the remote's notes, and merges what it
/// fetched.
fn fetch_and_merge(remote: &str, mut fetch: Command) -> Result<Pulled, Error> {
    let theirs = format!("refs/notes/remotes/{remote}/replaces");
    let out = runner::stdout(&mut fetch).map_err(|source| Error::Fetch {
        remote: remote.to_owned(),
        source,
    })?;
    // `<flag> <old> <new> <ref>` for each ref that the fetch matched, up to date or not, so that
    // a remote without notes gives no line for them; the flag of a fast-forward is a space. The
    // commit fetched is taken from the line, so that another fetch that moves the ref in the
    // meantime does not change what is merged.
    let fetched = out.lines().find_map(|line| {
        let (flag, rest) = (line.get(..1)?, line.get(2..)?);
        let mut fields = rest.split(' ');
        let (_, new, local) = (fields.next()?, fields.next()?, fields.next()?);
        (local == theirs && !matches!(flag, "-" | "!")).then_some(new)
    });
    let Some(fetched) = fetched else {
        return Ok(Pulled::Absent);
    };
    replaces_index_record::merge(fetched).map_err(|source| Error::Merge {
        remote: remote.to_owned(),
        source,
    })?;
    Ok(Pulled::Merged)
}
