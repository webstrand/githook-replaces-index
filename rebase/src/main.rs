//! `git rebase-replaces [<upstream>]`
//!
//! Rebases the current branch onto <upstream>, by default its configured upstream, leaving out
//! the commits that the upstream already holds in another form. `git pull --rebase` leaves out
//! only the commits that the upstream's reflog shows it once had; after the upstream's history was
//! rewritten, the branch still holds the old versions of its commits, and those it cannot match
//! are replayed on top of their own replacements.
//!
//! When <upstream> is a remote-tracking branch, its remote is fetched first and its replaces notes
//! are merged through `pull`. A commit of the branch that the upstream lacks is then left out
//! when:
//!
//! - the upstream holds a commit that was made from it, through any number of rewrites, as
//!   `compare` finds through the index of refs/notes/replaces; or
//! - the upstream's reflog shows that the upstream once held it, and has since dropped or
//!   replaced it, as `git pull --rebase` would.
//!
//! A commit that was rewritten as commits the upstream lacks is replayed, with a warning. The
//! remaining commits are replayed onto the upstream by `git rebase -i`, with this program as its
//! sequence editor, which marks the commits left out as `drop` in the list.

mod error;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_compare::{Status, compare};
use replaces_index_pull::{Pulled, pull};
use runner::git;

use crate::error::Error;

#[derive(Parser)]
#[command(name = "git rebase-replaces", about)]
struct Cli {
    /// The branch to rebase onto; by default the upstream of the current branch.
    upstream: Option<String>,
    /// Marks as `drop` each pick in the todo list TODO of a commit listed in DROP; the rebase
    /// runs this program so as its sequence editor.
    #[arg(long, hide = true, num_args = 2, value_names = ["DROP", "TODO"])]
    edit_todo: Option<Vec<PathBuf>>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.edit_todo.as_deref() {
        Some([drop, todo]) => edit_todo(drop, todo).map(|()| ExitCode::SUCCESS),
        _ => rebase(cli.upstream.as_deref().unwrap_or("@{upstream}")),
    };
    result.unwrap_or_else(|err| {
        eprintln!("rebase-replaces: {err}");
        ExitCode::FAILURE
    })
}

fn rebase(upstream: &str) -> Result<ExitCode, Error> {
    let upstream_error = |source| Error::Upstream {
        upstream: upstream.to_owned(),
        source,
    };
    let full = runner::stdout(&mut git(["rev-parse", "--symbolic-full-name", upstream]))
        .map_err(upstream_error)?
        .trim()
        .to_owned();

    // The remote of a remote-tracking upstream: the longest remote name that prefixes its ref.
    let remotes = runner::stdout(&mut git(["remote"])).map_err(upstream_error)?;
    let remote = remotes
        .lines()
        .filter(|remote| full.starts_with(&format!("refs/remotes/{remote}/")))
        .max_by_key(|remote| remote.len());
    if let Some(remote) = remote {
        runner::status(&mut git(["fetch", "-q", remote])).map_err(|source| Error::Fetch {
            remote: remote.to_owned(),
            source,
        })?;
        let pulled = pull(remote).map_err(|source| Error::Pull {
            remote: remote.to_owned(),
            source,
        })?;
        if pulled == Pulled::Absent {
            eprintln!("pull-replaces: {remote} has no refs/notes/replaces");
        }
    }
    let up = runner::stdout(&mut git([
        "rev-parse",
        "--verify",
        &format!("{upstream}^{{commit}}"),
    ]))
    .map_err(upstream_error)?
    .trim()
    .to_owned();

    let held = held(&full, &up)?;
    let local = compare("HEAD", &up).map_err(Error::Compare)?;
    let mut drop = Vec::new();
    for entry in &local {
        let commit = entry.commit.as_str();
        if held.contains(commit) {
            drop.push(commit);
            continue;
        }
        let rewritten = match &entry.status {
            Status::Held(_) => {
                drop.push(commit);
                continue;
            }
            Status::Rewritten(into) => into,
            Status::Later(_) | Status::Missing => continue,
        };
        let described = runner::stdout(&mut git(["log", "-1", "--format=%h %s", commit]))
            .map_err(Error::List)?;
        let short: String = rewritten
            .iter()
            .map(|c| format!("{} ", &c[..c.len().min(12)]))
            .collect();
        eprintln!(
            "rebase-replaces: {} was rewritten as {short}but {upstream} lacks it; replaying it",
            described.trim_end()
        );
    }
    println!(
        "rebase-replaces: replaying {} commits onto {upstream}, leaving out {} it already holds",
        local.len() - drop.len(),
        drop.len()
    );

    let tmp = tempfile::tempdir().map_err(Error::TempDir)?;
    let drop_path = tmp.path().join("drop");
    let list: String = drop.iter().map(|commit| format!("{commit}\n")).collect();
    fs::write(&drop_path, list).map_err(|source| Error::Write {
        path: drop_path.clone(),
        source,
    })?;
    let exe = std::env::current_exe().map_err(Error::CurrentExe)?;
    // Git runs the editor through the shell, with the path of the todo list appended.
    let editor = format!(
        "{} --edit-todo {}",
        quote(&exe.to_string_lossy()),
        quote(&drop_path.to_string_lossy())
    );
    let status = git(["rebase", "-i", &up])
        .env("GIT_SEQUENCE_EDITOR", editor)
        .status()
        .map_err(Error::Rebase)?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    })
}

/// Every commit that the reflog of the upstream's ref `full` shows it held, and that `up` lacks.
fn held(full: &str, up: &str) -> Result<HashSet<String>, Error> {
    if full.is_empty() {
        return Ok(HashSet::new());
    }
    let exists = runner::run(&mut git(["reflog", "exists", full])).map_err(Error::List)?;
    if !exists.status.success() {
        return Ok(HashSet::new());
    }
    let reflog = runner::stdout(&mut git(["reflog", "show", "--format=%H", full, "--"]))
        .map_err(Error::List)?;
    let entries: HashSet<&str> = reflog.lines().collect();
    if entries.is_empty() {
        return Ok(HashSet::new());
    }
    let mut revs = format!("^{up}\n");
    for entry in entries {
        revs.push_str(entry);
        revs.push('\n');
    }
    let held = runner::stdout_with_input(
        &mut git(["rev-list", "--ignore-missing", "--stdin"]),
        revs.as_bytes(),
    )
    .map_err(Error::List)?;
    Ok(held.lines().map(str::to_owned).collect())
}

/// Marks as `drop` each pick in the todo list `todo` of a commit listed in `drop`.
fn edit_todo(drop: &Path, todo: &Path) -> Result<(), Error> {
    let read = |path: &Path| {
        fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })
    };
    let dropped = read(drop)?;
    let dropped: HashSet<&str> = dropped.lines().collect();
    let mut edited = String::new();
    for line in read(todo)?.lines() {
        let mut words = line.split_whitespace();
        let picked = match (words.next(), words.next()) {
            (Some("pick" | "p"), Some(commit)) => resolve(commit),
            _ => None,
        };
        match (picked, line.split_once(' ')) {
            (Some(commit), Some((_, rest))) if dropped.contains(commit.as_str()) => {
                edited.push_str("drop ");
                edited.push_str(rest);
            }
            _ => edited.push_str(line),
        }
        edited.push('\n');
    }
    let new = todo.with_extension("new");
    fs::write(&new, edited).map_err(|source| Error::Write {
        path: new.clone(),
        source,
    })?;
    fs::rename(&new, todo).map_err(|source| Error::Write {
        path: todo.to_owned(),
        source,
    })
}

/// The full id of the commit that `name` names, if any.
fn resolve(name: &str) -> Option<String> {
    let output = runner::run(&mut git([
        "rev-parse",
        "-q",
        "--verify",
        &format!("{name}^{{commit}}"),
    ]))
    .ok()?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !id.is_empty()).then_some(id)
}

/// A word that the shell reads back as `word`.
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}
