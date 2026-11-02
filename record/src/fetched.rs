//! The refs that `git pull-replaces` fetches the notes of a URL or path into, and deletes once it
//! merged them. A pull ended by `SIGKILL` or a crash leaves them behind; `git gc-replaces` deletes
//! those older than `gc.pruneExpire`. A ref's age is told by the reflog the pull writes for it, and
//! measured from the newest entry of the reflogs read, rather than from this host's clock: another
//! host that shares the repository on a network filesystem may be pulling, and its clock may
//! differ.

use runner::git;

use crate::{Error, REF};

/// The prefix of the refs of each pull, which `<pid>/` follows.
pub const FETCHED: &str = "refs/replaces-index/fetched-";

/// Deletes the refs that pulls left behind once they are older than `gc.pruneExpire`, and returns
/// how many it deleted. A ref without a reflog has no age, and is kept.
pub fn sweep() -> Result<usize, Error> {
    let listed = runner::stdout(&mut git([
        "for-each-ref",
        "--format=%(refname)",
        "refs/replaces-index/",
    ]))
    .map_err(Error::Read)?;
    let fetched: Vec<&str> = listed
        .lines()
        .filter(|name| name.starts_with(FETCHED))
        .collect();
    if fetched.is_empty() {
        return Ok(0);
    }
    let mut written = Vec::new();
    for name in &fetched {
        if let Some(time) = newest_entry(name)? {
            written.push((*name, time));
        }
    }
    let mut newest = written.iter().map(|(_, time)| *time).max().unwrap_or(0);
    for name in ["HEAD", REF] {
        newest = newest.max(newest_entry(name)?.unwrap_or(0));
    }
    let period = prune_period()?;
    let expired: Vec<&str> = written
        .iter()
        .filter(|(_, time)| period.is_some_and(|period| newest.saturating_sub(*time) >= period))
        .map(|(name, _)| *name)
        .collect();
    if expired.is_empty() {
        return Ok(0);
    }
    let input: String = expired
        .iter()
        .map(|name| format!("delete {name}\n"))
        .collect();
    runner::stdout_bytes_with_input(&mut git(["update-ref", "--stdin"]), input.as_bytes())
        .map_err(Error::Write)?;
    Ok(expired.len())
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
