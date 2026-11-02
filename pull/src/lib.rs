//! Pulls a remote's replaces notes: fetches its refs/notes/replaces into
//! refs/notes/remotes/<remote>/replaces, and merges that into the local ref as `record` writes it,
//! taking the union of each note's lines and moving the ref only from the commit it read. Lines are
//! only ever added to the notes, so the union is the whole merge. The remote is not changed; it
//! pulls from this clone in turn.

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

/// Pulls the replaces notes of `remote`, fetching them alone. `remote` is the name of a configured
/// remote, whose notes are kept in refs/notes/remotes/<remote>/replaces, or, as `git pull` takes,
/// a URL or path, whose notes are fetched into a ref of their own only until they are merged, as
/// `git pull` keeps nothing of such a remote but FETCH_HEAD.
pub fn pull(remote: &str) -> Result<Pulled, Error> {
    let fetch_error = |source| Error::Fetch {
        remote: remote.to_owned(),
        source,
    };
    let remotes = runner::stdout(&mut git(["remote"])).map_err(fetch_error)?;
    if remotes.lines().any(|name| name == remote) {
        let kept = kept(remote);
        let refspec = refspec(&kept);
        return fetch_and_merge(
            remote,
            &kept,
            git(["fetch", "--porcelain", "-v", remote, &refspec]),
        );
    }
    let fetched = format!("refs/replaces-index/fetched-{}", std::process::id());
    let refspec = refspec(&format!("{fetched}/replaces"));
    let pulled = fetch_and_merge(
        remote,
        &format!("{fetched}/replaces"),
        git(["fetch", "--porcelain", "-v", remote, &refspec]),
    );
    // Every ref the glob fetched, which the merge keeps no need of, as it holds what it merged.
    // Their names are passed on as git printed them, whatever their bytes.
    let refs = runner::stdout_bytes(&mut git([
        "for-each-ref",
        "--format=delete %(refname)",
        &format!("{fetched}/"),
    ]))
    .map_err(fetch_error)?;
    runner::stdout_bytes_with_input(&mut git(["update-ref", "--stdin"]), &refs)
        .map_err(fetch_error)?;
    pulled
}

/// Fetches `remote` as `git fetch <remote>` does, and pulls its replaces notes, which the same
/// fetch brings. `remote` must be the name of a configured remote.
pub fn fetch_and_pull(remote: &str) -> Result<Pulled, Error> {
    let kept = kept(remote);
    // Added to the refspecs that the remote's configuration gives, which a refspec named on the
    // command line would replace.
    let config = format!("remote.{remote}.fetch={}", refspec(&kept));
    fetch_and_merge(
        remote,
        &kept,
        git(["-c", &config, "fetch", "--porcelain", "-v", remote]),
    )
}

/// The ref that keeps the notes of the configured remote `remote`.
fn kept(remote: &str) -> String {
    format!("refs/notes/remotes/{remote}/replaces")
}

/// The refspec that fetches the remote's notes into `theirs`. It is a glob, so that a remote
/// without notes matches nothing rather than failing the fetch; the `*` matches the empty string.
fn refspec(theirs: &str) -> String {
    format!("+refs/notes/replaces*:{theirs}*")
}

/// Runs `fetch`, a `git fetch --porcelain -v` that fetches the notes of `remote` into `theirs`,
/// and merges what it fetched.
fn fetch_and_merge(remote: &str, theirs: &str, mut fetch: Command) -> Result<Pulled, Error> {
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
