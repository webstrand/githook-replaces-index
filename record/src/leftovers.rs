//! What a process ended by `SIGKILL` or a crash leaves behind: the refs that `git pull-replaces`
//! fetches the notes of a URL or path into, the temporary files of abandoned records, and the
//! files that an index is built in. Each is deleted once it is older than `gc.pruneExpire`. Its age
//! is measured from the newest entry of the reflogs of HEAD and the notes ref, rather than from
//! this host's clock: another host that shares the repository on a network filesystem may be
//! writing it, and its clock may differ.

use std::fs;
use std::path::Path;

use runner::git;

use crate::{Error, REF};

/// The prefix of the refs of each pull, which `<pid>/` follows.
pub const FETCHED: &str = "refs/replaces-index/fetched-";

/// When a leftover is old enough to delete.
pub struct Expiry {
    newest: u64,
    /// None when `gc.pruneExpire` is `never`.
    period: Option<u64>,
}

impl Expiry {
    pub fn read() -> Result<Self, Error> {
        let mut newest = 0;
        for name in ["HEAD", REF] {
            newest = newest.max(newest_entry(name)?.unwrap_or(0));
        }
        Ok(Self {
            newest,
            period: prune_period()?,
        })
    }

    /// Whether a leftover written at `time`, in seconds since the epoch, is old enough to delete.
    pub fn expired(&self, time: u64) -> bool {
        self.period
            .is_some_and(|period| self.newest.saturating_sub(time) >= period)
    }

    /// Deletes the files of `dir` whose names `time_of` gives an expired time, and returns how many
    /// it deleted. A file it cannot delete is left for the next sweep.
    pub fn sweep_files(&self, dir: &Path, time_of: impl Fn(&str) -> Option<u64>) -> usize {
        let Ok(entries) = fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                name.to_str()
                    .and_then(&time_of)
                    .is_some_and(|time| self.expired(time))
            })
            .filter(|entry| fs::remove_file(entry.path()).is_ok())
            .count()
    }
}

/// Deletes the refs that pulls left behind and the temporary files of abandoned records once they
/// are expired, and returns how many it deleted. A ref without a reflog has no age, and is kept.
pub fn sweep() -> Result<usize, Error> {
    let expiry = Expiry::read()?;
    let listed = runner::stdout(&mut git([
        "for-each-ref",
        "--format=%(refname)",
        "refs/replaces-index/",
    ]))
    .map_err(Error::Read)?;
    let mut expired = Vec::new();
    for name in listed.lines().filter(|name| name.starts_with(FETCHED)) {
        if newest_entry(name)?.is_some_and(|time| expiry.expired(time)) {
            expired.push(name);
        }
    }
    if !expired.is_empty() {
        let input: String = expired
            .iter()
            .map(|name| format!("delete {name}\n"))
            .collect();
        runner::stdout_bytes_with_input(&mut git(["update-ref", "--stdin"]), input.as_bytes())
            .map_err(Error::Write)?;
    }
    let dir = crate::abandoned::dir().map_err(Error::Read)?;
    let files = expiry.sweep_files(&dir, crate::abandoned::temporary_time);
    Ok(expired.len() + files)
}

/// The time, in seconds since the epoch, of the newest entry of the reflog of `name`; none when it
/// has no reflog.
fn newest_entry(name: &str) -> Result<Option<u64>, Error> {
    let mut show = git([
        "reflog",
        "show",
        "-n1",
        "--date=unix",
        "--format=%gd",
        name,
        "--",
    ]);
    let output = runner::run(&mut show).map_err(Error::Read)?;
    if !output.status.success() {
        return Ok(None);
    }
    let out = runner::text(&show, output.stdout).map_err(Error::Read)?;
    // `<name>@{<time>}`.
    Ok(out
        .trim()
        .rsplit_once("@{")
        .and_then(|(_, time)| time.strip_suffix('}')?.parse().ok()))
}

/// How long `gc.pruneExpire` keeps an object, in seconds; none when it is `never`.
fn prune_period() -> Result<Option<u64>, Error> {
    let at = runner::stdout(&mut git([
        "config",
        "--type=expiry-date",
        "--default=2.weeks.ago",
        "--get",
        "gc.pruneExpire",
    ]))
    .map_err(Error::Read)?;
    let at: u64 = at.trim().parse().unwrap_or(0);
    if at == 0 {
        return Ok(None);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok(Some(now.saturating_sub(at)))
}
