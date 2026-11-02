//! Pulls a remote's replaces notes: fetches its refs/notes/replaces into the ref that the remote's
//! fetch refspecs give, and merges that into the local ref as `record` writes it, taking the union
//! of each note's lines and moving the ref only from the commit it read. Lines are only ever added
//! to the notes, so the union is the whole merge. The remote is not changed; it pulls from this
//! clone in turn.

mod error;

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;

use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::Signals;

use replaces_index_record::REF;
use replaces_index_record::fetched::FETCHED;
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
/// remote, whose fetch refspecs must bring its notes into a ref beside the local ones, or, as
/// `git pull` takes, a URL or path, whose notes are fetched into a ref of their own only until they
/// are merged, as `git pull` keeps nothing of such a remote but FETCH_HEAD.
pub fn pull(remote: &str) -> Result<Pulled, Error> {
    let fetch_error = |source| Error::Fetch {
        remote: remote.to_owned(),
        source,
    };
    let remotes = runner::stdout(&mut git(["remote"])).map_err(fetch_error)?;
    if remotes.lines().any(|name| name == remote) {
        let kept = kept(remote)?;
        let refspec = refspec(&kept);
        return fetch_and_merge(
            remote,
            &kept,
            git(["fetch", "--porcelain", "-v", remote, &refspec]),
        );
    }
    let fetched = format!("{FETCHED}{}", std::process::id());
    delete_on_signal(&fetched);
    let refspec = refspec(&format!("{fetched}/replaces"));
    // The reflog dates the refs, so that `git gc-replaces` tells those a pull left behind.
    let pulled = fetch_and_merge(
        remote,
        &format!("{fetched}/replaces"),
        git([
            "-c",
            "core.logAllRefUpdates=always",
            "fetch",
            "--porcelain",
            "-v",
            remote,
            &refspec,
        ]),
    );
    // Every ref the glob fetched, which the merge keeps no need of, as it holds what it merged.
    // Their names are passed on as git printed them, whatever their bytes.
    let deleted = runner::stdout_bytes(&mut git([
        "for-each-ref",
        "--format=delete %(refname)",
        &format!("{fetched}/"),
    ]))
    .and_then(|refs| runner::stdout_bytes_with_input(&mut git(["update-ref", "--stdin"]), &refs));
    // A failed pull is told of rather than the refs it could not delete, which gc deletes later.
    let pulled = pulled?;
    deleted.map_err(fetch_error)?;
    Ok(pulled)
}

/// Starts a thread that, when a signal would end the pull, ends the git commands it runs, deletes
/// the refs under `fetched`, and ends the pull as the signal would have.
fn delete_on_signal(fetched: &str) {
    let Ok(mut signals) = Signals::new([SIGHUP, SIGINT, SIGQUIT, SIGTERM]) else {
        return;
    };
    let fetched = format!("{fetched}/");
    thread::spawn(move || {
        if let Some(signal) = signals.forever().next() {
            runner::stop();
            // Run apart from runner, which starts no command once it is stopping.
            let listed = Command::new("git")
                .args(["for-each-ref", "--format=delete %(refname)", &fetched])
                .stderr(Stdio::null())
                .output();
            if let Ok(listed) = listed
                && let Ok(mut delete) = Command::new("git")
                    .args(["update-ref", "--stdin"])
                    .stdin(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
            {
                if let Some(mut stdin) = delete.stdin.take() {
                    let _ = stdin.write_all(&listed.stdout);
                }
                let _ = delete.wait();
            }
            let _ = signal_hook::low_level::emulate_default_handler(signal);
        }
    });
}

/// Fetches `remote` as `git fetch <remote>` does, and pulls its replaces notes, which its fetch
/// refspecs must bring into a ref beside the local ones. `remote` must be the name of a configured
/// remote.
pub fn fetch_and_pull(remote: &str) -> Result<Pulled, Error> {
    let kept = kept(remote)?;
    fetch_and_merge(remote, &kept, git(["fetch", "--porcelain", "-v", remote]))
}

/// The ref that the fetch refspecs of the configured remote `remote` bring its notes into. A
/// refspec that brings them into the local notes ref itself does not keep them beside it.
fn kept(remote: &str) -> Result<String, Error> {
    let kept = runner::refspec::destination(remote, REF).map_err(|source| Error::Fetch {
        remote: remote.to_owned(),
        source,
    })?;
    kept.filter(|kept| kept != REF)
        .ok_or_else(|| Error::Unconfigured {
            remote: remote.to_owned(),
        })
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
    // `<flag> <old> <new> <ref>` for each ref that the fetch matched, up to date or not; a
    // remote without notes therefore gives no line for them; the flag of a fast-forward is a space.
    // The commit fetched is taken from the line, so that another fetch that moves the ref in the
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
