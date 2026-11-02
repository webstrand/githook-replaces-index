//! `git rebase-replaces [<upstream>]`
//!
//! Rebases the current branch onto <upstream>, by default its configured upstream, leaving out
//! the commits that the upstream already holds in another form. `git pull --rebase` leaves out
//! only the commits that the upstream's reflog shows it once had; after the upstream's history was
//! rewritten, the branch still holds the old versions of its commits, and those it cannot match
//! are replayed on top of their own replacements.
//!
//! When <upstream> is a remote-tracking branch, its remote is fetched first, its replaces notes
//! with it in the same fetch, and they are merged through `pull`. A commit of the branch that the
//! upstream lacks is then left out when:
//!
//! - the upstream holds a commit that was made from it, through any number of rewrites, as
//!   `compare` finds through the index of refs/notes/replaces; or
//! - no <upstream> was named, and the reflog of the configured upstream shows that it once held
//!   it, and has since dropped or replaced it, as `git rebase` does with its fork point when it is
//!   named no upstream either.
//!
//! A commit that was rewritten as commits the upstream lacks is replayed, with a warning. The
//! remaining commits are replayed onto the upstream by `git rebase -i`, with this program as its
//! sequence editor, which marks the commits left out as `drop` in the list.

mod error;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_compare::{Status, compare, describe};
use replaces_index_pull::{Pulled, fetch_and_pull};
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
        _ => rebase(cli.upstream.as_deref()),
    };
    result.unwrap_or_else(|err| {
        eprintln!("rebase-replaces: {err}");
        ExitCode::FAILURE
    })
}

/// Rebases onto `upstream`, the configured upstream when none is named.
fn rebase(named: Option<&str>) -> Result<ExitCode, Error> {
    let upstream = named.unwrap_or("@{upstream}");
    let upstream_error = |source| Error::Upstream {
        upstream: upstream.to_owned(),
        source,
    };
    let full = runner::stdout(&mut git(["rev-parse", "--symbolic-full-name", upstream]))
        .map_err(upstream_error)?
        .trim()
        .to_owned();

    if let Some(remote) = remote_of(&full).map_err(upstream_error)? {
        let remote = remote.as_str();
        let pulled = fetch_and_pull(remote).map_err(|source| Error::Pull {
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

    // A named upstream is taken as it is, as `git rebase` takes it without a fork point.
    let held = match named {
        Some(_) => HashSet::new(),
        None => held(&full, &up)?,
    };
    let local = compare("HEAD", &up).map_err(Error::Compare)?;
    // Each commit left out, with why, and each replayed although it was rewritten, with into what.
    let mut drop: Vec<(&str, String)> = Vec::new();
    let mut replayed: Vec<(&str, &[String])> = Vec::new();
    for entry in &local {
        let commit = entry.commit.as_str();
        match &entry.status {
            _ if held.contains(commit) => drop.push((
                commit,
                format!("the reflog of {upstream} shows that it held it"),
            )),
            Status::Held(into) => drop.push((
                commit,
                format!("{upstream} holds {} made from it", shorts(into)),
            )),
            Status::Rewritten(into) => replayed.push((commit, into)),
            Status::Later(_) | Status::Missing => {}
        }
    }
    let named = drop
        .iter()
        .map(|(c, _)| *c)
        .chain(replayed.iter().map(|(c, _)| *c));
    let described = describe(named).map_err(Error::Compare)?;
    let describe = |commit: &str| {
        described
            .get(commit)
            .map_or(commit.to_owned(), |d| format!("{} {}", d.short, d.subject))
    };
    for (commit, into) in &replayed {
        eprintln!(
            "rebase-replaces: {} was rewritten as {} but {upstream} lacks it; replaying it",
            describe(commit),
            shorts(into)
        );
    }
    for (commit, why) in &drop {
        println!("rebase-replaces: leaving out {}: {why}", describe(commit));
    }
    println!(
        "rebase-replaces: replaying {} commits onto {upstream}, leaving out {} it already holds",
        local.len() - drop.len(),
        drop.len()
    );

    let tmp = tempfile::tempdir().map_err(Error::TempDir)?;
    let drop_path = tmp.path().join("drop");
    // `<commit> <why>` for each commit left out.
    let list: String = drop
        .iter()
        .map(|(commit, why)| format!("{commit} {why}\n"))
        .collect();
    fs::write(&drop_path, list).map_err(|source| Error::Write {
        path: drop_path.clone(),
        source,
    })?;
    let exe = std::env::current_exe().map_err(Error::CurrentExe)?;
    // Git runs the editor through the shell, with the path of the todo list appended.
    let mut editor = quote(exe.as_os_str());
    editor.push(" --edit-todo ");
    editor.push(quote(drop_path.as_os_str()));
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

/// The remote that a fetch brings the ref `full` from: the first whose fetch refspecs map a ref of
/// it to `full`. The ref's name alone does not tell, since a remote may keep its refs anywhere.
fn remote_of(full: &str) -> Result<Option<String>, runner::Error> {
    let mut config = git(["config", "-z", "--get-regexp", r"^remote\..*\.fetch$"]);
    let output = runner::run(&mut config)?;
    // git config exits with 1 when no remote has a fetch refspec.
    if output.status.code() != Some(1) {
        runner::check(&config, &output)?;
    }
    let out = String::from_utf8_lossy(&output.stdout);
    // `remote.<name>.fetch\n<refspec>\0` for each refspec.
    for entry in out.split('\0') {
        let Some((key, refspec)) = entry.split_once('\n') else {
            continue;
        };
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|k| k.strip_suffix(".fetch"))
        else {
            continue;
        };
        let refspec = refspec.strip_prefix('+').unwrap_or(refspec);
        // A negative refspec excludes refs, and maps none.
        if refspec.starts_with('^') {
            continue;
        }
        let Some((_, destination)) = refspec.split_once(':') else {
            continue;
        };
        let maps = match destination.split_once('*') {
            Some((before, after)) => {
                full.len() > before.len() + after.len()
                    && full.starts_with(before)
                    && full.ends_with(after)
            }
            None => destination == full,
        };
        if maps {
            return Ok(Some(name.to_owned()));
        }
    }
    Ok(None)
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

/// Abbreviations of `commits`, for messages.
fn shorts(commits: &[String]) -> String {
    // TODO: a note that arrived from another clone may name anything, not only an ASCII commit
    // id; this and the other places that slice or print such names need to handle any bytes.
    let shorts: Vec<&str> = commits.iter().map(|c| &c[..c.len().min(12)]).collect();
    shorts.join(" ")
}

/// Marks as `drop` each pick in the todo list `todo` of a commit listed in `drop`, as `<commit>
/// <why>` lines.
///
/// A rebase with `rebase.autoSquash` lists each `fixup!` or `squash!` commit after the commit it
/// folds into, as a `fixup` or `squash`. One that is left out itself is dropped as well. One that
/// is not, but would fold into a commit left out, has nowhere to go: it is no part of the
/// upstream, and folded into the commit before it, it would change another. The rebase is then
/// refused, by a failure that git takes for the editor's, which ends it before it starts.
fn edit_todo(drop: &Path, todo: &Path) -> Result<(), Error> {
    let read = |path: &Path| {
        fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })
    };
    let dropped = read(drop)?;
    let dropped: HashMap<&str, &str> = dropped
        .lines()
        .filter_map(|line| line.split_once(' '))
        .collect();
    let text = read(todo)?;
    let lines: Vec<Option<Command>> = text.lines().map(Command::parse).collect();
    let names: Vec<&str> = lines
        .iter()
        .flatten()
        .map(|command| command.commit)
        .collect();
    let ids = resolve(&names)?;
    let mut edited = String::new();
    // The commit left out that a `fixup` or `squash` on the next lines would fold into.
    let mut folding: Option<(&str, &str)> = None;
    let mut stranded = Vec::new();
    for (line, command) in text.lines().zip(&lines) {
        let Some(command) = command else {
            // A command that makes or moves to another commit ends what a fixup folds into.
            if matches!(
                line.split_whitespace().next(),
                Some("reset" | "t" | "merge" | "m")
            ) {
                folding = None;
            }
            edited.push_str(line);
            edited.push('\n');
            continue;
        };
        let id = ids.get(command.commit).map(String::as_str);
        let why = id.and_then(|id| dropped.get(id));
        match (command.folds, why, folding) {
            (_, Some(&why), _) => {
                if !command.folds {
                    folding = Some((command.rest, why));
                }
                edited.push_str("drop ");
                edited.push_str(command.rest);
            }
            (true, None, Some((into, why))) => {
                stranded.push(format!(
                    "{} would fold into {}, which is left out since {why}, but the upstream holds nothing made from {} itself",
                    shown(command.rest),
                    shown(into),
                    command.commit
                ));
                edited.push_str(line);
            }
            (false, None, _) => {
                folding = None;
                edited.push_str(line);
            }
            (true, None, None) => edited.push_str(line),
        }
        edited.push('\n');
    }
    if !stranded.is_empty() {
        return Err(Error::Stranded(stranded.join("; ")));
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

/// The commit and subject of a line of a todo list, for messages, without the `#` that git puts
/// between them.
fn shown(rest: &str) -> String {
    rest.replacen(" # ", " ", 1)
}

/// A line of a todo list that names a commit to pick, or to fold into the one before.
struct Command<'a> {
    /// Whether it folds the commit into the one before: a `fixup` or `squash`.
    folds: bool,
    /// The commit, as the todo list abbreviates it.
    commit: &'a str,
    /// The commit and what follows it, which a `drop` keeps.
    rest: &'a str,
}

impl<'a> Command<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let line = line.trim_start();
        let (verb, mut rest) = line.split_once(' ')?;
        let folds = match verb {
            "pick" | "p" => false,
            "fixup" | "f" | "squash" | "s" => true,
            _ => return None,
        };
        // `fixup -C` and `fixup -c` take the message of the commit they fold.
        if folds && let Some(after) = rest.strip_prefix("-C ").or(rest.strip_prefix("-c ")) {
            rest = after;
        }
        let rest = rest.trim_start();
        let commit = rest.split_whitespace().next()?;
        Some(Self {
            folds,
            commit,
            rest,
        })
    }
}

/// The full id of each of `names` that names a commit, by one `git cat-file`.
fn resolve<'a>(names: &[&'a str]) -> Result<HashMap<&'a str, String>, Error> {
    if names.is_empty() {
        return Ok(HashMap::new());
    }
    let input: String = names
        .iter()
        .map(|name| format!("{name}^{{commit}}\n"))
        .collect();
    let mut check = git(["cat-file", "--batch-check=%(objectname)"]);
    let out = runner::stdout_with_input(&mut check, input.as_bytes()).map_err(Error::List)?;
    // One line for each name, in their order: the id, or `<name> missing` or `ambiguous`.
    Ok(names
        .iter()
        .zip(out.lines())
        .filter(|(_, line)| !line.contains(' '))
        .map(|(name, id)| (*name, id.to_owned()))
        .collect())
}

/// A word that the shell reads back as `word`, whatever its bytes.
fn quote(word: &OsStr) -> OsString {
    let mut quoted = vec![b'\''];
    for &byte in word.as_bytes() {
        if byte == b'\'' {
            quoted.extend_from_slice(br"'\''");
        } else {
            quoted.push(byte);
        }
    }
    quoted.push(b'\'');
    OsString::from_vec(quoted)
}
