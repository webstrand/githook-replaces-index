//! `git rebase-replaces [--keep-base] [<upstream>]`
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
//! - no <upstream> was named, and the reflog of the configured upstream shows that it once held it,
//!   and has since dropped or replaced it, as `git rebase` does with its fork point when it is
//!   named no upstream either.
//!
//! A commit that was rewritten as commits the upstream lacks is replayed, with a warning. The
//! remaining commits are replayed onto the upstream by `git rebase -i`, with this program as its
//! sequence editor, which marks the commits left out as `drop` in the list. No commit is folded
//! into another, so that the branch keeps the commits it shares with the upstream. With
//! `--keep-base`, they are replayed onto the commit of the upstream that replaces the last commit
//! the branch shared with it, rather than onto the upstream.

mod error;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_compare::{Described, Status, compare, describe};
use replaces_index_pull::{Pulled, fetch_and_pull};
use runner::git;

use crate::error::Error;

#[derive(Parser)]
#[command(name = "git rebase-replaces", about)]
struct Cli {
    /// The branch to rebase onto; by default the upstream of the current branch.
    upstream: Option<String>,
    /// Replay the branch onto the commit of the upstream that replaces the last commit it shared
    /// with the branch, rather than onto the upstream, as `git rebase --keep-base` replays it onto
    /// the merge base.
    #[arg(long)]
    keep_base: bool,
    /// Marks as `drop` each pick in the todo list TODO of a commit listed in DROP; the rebase
    /// runs this program so as its sequence editor.
    #[arg(long, hide = true, num_args = 2, value_names = ["DROP", "TODO"])]
    edit_todo: Option<Vec<PathBuf>>,
    /// Gives HEAD the message in MESSAGE, if it is the commit the rebase made of ORIGINAL; the
    /// todo list runs this program so after a pick to reword.
    #[arg(long, hide = true, num_args = 2, value_names = ["ORIGINAL", "MESSAGE"])]
    reword: Option<Vec<OsString>>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match (cli.edit_todo.as_deref(), cli.reword.as_deref()) {
        (Some([drop, todo]), _) => edit_todo(drop, todo).map(|()| ExitCode::SUCCESS),
        (_, Some([original, message])) => match original.to_str() {
            Some(original) => reword(original, Path::new(message)).map(|()| ExitCode::SUCCESS),
            None => Ok(ExitCode::FAILURE),
        },
        _ => rebase(cli.upstream.as_deref(), cli.keep_base),
    };
    result.unwrap_or_else(|err| {
        eprintln!("rebase-replaces: {err}");
        ExitCode::FAILURE
    })
}

/// Rebases onto `upstream`, the configured upstream when none is named.
fn rebase(named: Option<&str>, keep_base: bool) -> Result<ExitCode, Error> {
    let upstream = named.unwrap_or("@{upstream}");
    let upstream_error = |source| Error::Upstream {
        upstream: upstream.to_owned(),
        source,
    };
    let full = runner::stdout(&mut git(["rev-parse", "--symbolic-full-name", upstream]))
        .map_err(upstream_error)?
        .trim()
        .to_owned();

    if let Some(remote) = runner::refspec::remote_of(&full).map_err(upstream_error)? {
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
    // The commit of the upstream that replaces a commit left out, when a single one does.
    let mut replacements: HashMap<&str, &str> = HashMap::new();
    // Every commit of the upstream that replaces a commit left out.
    let mut replacing: Vec<&str> = Vec::new();
    let mut replayed: Vec<(&str, &[String])> = Vec::new();
    for entry in &local {
        let commit = entry.commit.as_str();
        match &entry.status {
            _ if held.contains(commit) => drop.push((
                commit,
                format!("the reflog of {upstream} shows that it held it"),
            )),
            Status::Held(into) => {
                if let [replacement] = &into[..] {
                    replacements.insert(commit, replacement);
                }
                replacing.extend(into.iter().map(String::as_str));
                drop.push((
                    commit,
                    format!("{upstream} holds {} made from it", shorts(into)),
                ));
            }
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
    let onto = if keep_base {
        let onto = kept_base(&up, &replacing, upstream)?;
        let shown = describe(&onto);
        println!(
            "rebase-replaces: replaying {} commits onto {shown}, keeping the base, leaving out {} {upstream} already holds",
            local.len() - drop.len(),
            drop.len()
        );
        onto
    } else {
        println!(
            "rebase-replaces: replaying {} commits onto {upstream}, leaving out {} it already holds",
            local.len() - drop.len(),
            drop.len()
        );
        up
    };

    let tmp = tempfile::tempdir().map_err(Error::TempDir)?;
    let drop_path = tmp.path().join("drop");
    // `<commit> <replacement> <why>` for each commit left out, `-` for no single replacement.
    let list: String = drop
        .iter()
        .map(|(commit, why)| {
            let replacement = replacements.get(commit).unwrap_or(&"-");
            format!("{commit} {replacement} {why}\n")
        })
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
    let status = git(["rebase", "-i", "--no-autosquash", &onto])
        .env("GIT_SEQUENCE_EDITOR", editor)
        .status()
        .map_err(Error::Rebase)?;
    Ok(match status.code() {
        Some(0) => ExitCode::SUCCESS,
        Some(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        None => ExitCode::FAILURE,
    })
}

/// The commit of the upstream `up` to keep as the base: the newest of the merge base of HEAD and
/// `up` and of the commits `replacing`, which replace the commits left out. It must descend from
/// all the others: replacements on lines of history apart leave no base to keep.
fn kept_base(up: &str, replacing: &[&str], upstream: &str) -> Result<String, Error> {
    let base = runner::stdout(&mut git(["merge-base", "HEAD", up])).map_err(Error::List)?;
    let mut independent = git(["merge-base", "--independent", base.trim()]);
    independent.args(replacing);
    let tips = runner::stdout(&mut independent).map_err(Error::List)?;
    match tips.split_whitespace().collect::<Vec<_>>()[..] {
        [tip] => Ok(tip.to_owned()),
        ref tips => Err(Error::NoBase {
            upstream: upstream.to_owned(),
            tips: shorts(&tips.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>()),
        }),
    }
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
    // The index holds only commit ids, which are ASCII; a name that were not is shown whole rather
    // than cut inside a character.
    let shorts: Vec<&str> = commits.iter().map(|c| c.get(..12).unwrap_or(c)).collect();
    shorts.join(" ")
}

/// Marks as `drop` each pick in the todo list `todo` of a commit listed in `drop`, as `<commit>
/// <replacement> <why>` lines, with `-` for a commit that no single commit of the upstream
/// replaces.
///
/// The rebase runs without `--autosquash`, so that a `fixup!`, `squash!` or `amend!` commit is
/// replayed as it is and folds into nothing: folding one into a commit of the upstream would make
/// the branch part from the upstream again. One that names a commit left out, by its subject or an
/// abbreviation of its id, is made to name what replaces that commit instead, by an `exec` line
/// after its pick, so that a later `git rebase --autosquash` finds what it folds into.
fn edit_todo(drop: &Path, todo: &Path) -> Result<(), Error> {
    let read = |path: &Path| {
        fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })
    };
    let dropped = read(drop)?;
    let dropped: HashMap<&str, Option<&str>> = dropped
        .lines()
        .filter_map(|line| {
            let mut words = line.splitn(3, ' ');
            let commit = words.next()?;
            let replacement = words.next()?;
            Some((commit, (replacement != "-").then_some(replacement)))
        })
        .collect();
    let text = read(todo)?;
    let picks: Vec<Option<Pick>> = text.lines().map(Pick::parse).collect();
    let names: Vec<&str> = picks.iter().flatten().map(|pick| pick.commit).collect();
    let ids = resolve(&names)?;
    let named = dropped
        .keys()
        .copied()
        .chain(dropped.values().flatten().copied());
    let described = describe(named).map_err(Error::Compare)?;
    let exe = std::env::current_exe().map_err(Error::CurrentExe)?;
    let mut edited = Vec::new();
    let mut reworded = 0;
    for (line, pick) in text.lines().zip(&picks) {
        let Some((pick, id)) = pick
            .as_ref()
            .and_then(|pick| Some((pick, ids.get(pick.commit)?.as_str())))
        else {
            edited.extend_from_slice(line.as_bytes());
            edited.push(b'\n');
            continue;
        };
        if dropped.contains_key(id) {
            edited.extend_from_slice(b"drop ");
            edited.extend_from_slice(pick.rest.as_bytes());
            edited.push(b'\n');
            continue;
        }
        edited.extend_from_slice(line.as_bytes());
        edited.push(b'\n');
        let Some(message) = retargeted(id, pick, &dropped, &described)? else {
            continue;
        };
        reworded += 1;
        // The state directory of the rebase, which git removes once the rebase ends.
        let path = todo.with_file_name(format!("replaces-message-{reworded}"));
        fs::write(&path, message).map_err(|source| Error::Write {
            path: path.clone(),
            source,
        })?;
        edited.extend_from_slice(b"exec ");
        edited.extend_from_slice(quote(exe.as_os_str()).as_bytes());
        edited.extend_from_slice(format!(" --reword {id} ").as_bytes());
        edited.extend_from_slice(quote(path.as_os_str()).as_bytes());
        edited.push(b'\n');
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

/// The message of the `fixup!`, `squash!` or `amend!` commit `id`, made to name what replaces the
/// commit left out that it names; nothing when it names none, or one that no single commit
/// replaces.
fn retargeted(
    id: &str,
    pick: &Pick,
    dropped: &HashMap<&str, Option<&str>>,
    described: &HashMap<String, Described>,
) -> Result<Option<Vec<u8>>, Error> {
    let prefixed = |subject: &str| {
        ["fixup! ", "squash! ", "amend! "]
            .iter()
            .find_map(|prefix| subject.strip_prefix(prefix))
            .map(str::len)
    };
    if prefixed(shown(pick.rest).split_once(' ').map_or("", |(_, s)| s)).is_none() {
        return Ok(None);
    }
    let message =
        runner::stdout_bytes(&mut git(["log", "-1", "--format=%B", id])).map_err(Error::List)?;
    let end = message
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(message.len());
    // TODO(EuString): a subject that is not UTF-8 names no commit here.
    let Ok(subject) = std::str::from_utf8(&message[..end]) else {
        return Ok(None);
    };
    // `fixup! fixup! <reference>` names what `fixup! <reference>` names.
    let mut reference = subject;
    while let Some(len) = prefixed(reference) {
        reference = &reference[reference.len() - len..];
    }
    let subject_of = |commit: &str| described.get(commit).map(|d| d.subject.as_str());
    let by_id = reference.len() >= 4 && reference.bytes().all(|b| b.is_ascii_hexdigit());
    // As git's autosquash finds what a commit folds into: by its whole subject, then by its id,
    // then by the start of its subject; a reference that names more than one commit names none.
    let lookups: [&dyn Fn(&str) -> bool; 3] = [
        &|commit| subject_of(commit) == Some(reference),
        &|commit| by_id && commit.starts_with(reference),
        &|commit| subject_of(commit).is_some_and(|s| s.starts_with(reference)),
    ];
    let Some((lookup, target)) = lookups.iter().enumerate().find_map(|(i, lookup)| {
        let found: Vec<&str> = dropped.keys().copied().filter(|c| lookup(c)).collect();
        (found.len() == 1).then(|| (i, found[0]))
    }) else {
        return Ok(None);
    };
    let left_out = described
        .get(target)
        .map_or(target.to_owned(), |d| format!("{} {}", d.short, d.subject));
    let Some(replacement) = dropped[target].and_then(|r| Some((r, described.get(r)?))) else {
        println!(
            "rebase-replaces: {} names {left_out}, which is left out, and no single commit replaces it; replaying it as it is",
            shown(pick.rest)
        );
        return Ok(None);
    };
    let (_, new) = replacement;
    let named = if lookup == 1 {
        &new.short
    } else {
        &new.subject
    };
    if named == reference {
        return Ok(None);
    }
    println!(
        "rebase-replaces: making {} name {} {}, which replaces {left_out}",
        shown(pick.rest),
        new.short,
        new.subject
    );
    let mut reworded = message[..end - reference.len()].to_vec();
    reworded.extend_from_slice(named.as_bytes());
    reworded.extend_from_slice(&message[end..]);
    Ok(Some(reworded))
}

/// Gives HEAD the message in `path`, when HEAD is the commit that the rebase made of `original`,
/// as its message tells: a pick that became empty and was dropped leaves HEAD at another commit.
fn reword(original: &str, path: &Path) -> Result<(), Error> {
    let message = |commit: &str| {
        runner::stdout_bytes(&mut git(["log", "-1", "--format=%B", commit])).map_err(Error::List)
    };
    if message("HEAD")? != message(original)? {
        return Ok(());
    }
    let mut amend = git([
        "commit",
        "--amend",
        "--only",
        "--no-verify",
        "--allow-empty",
        "-q",
    ]);
    amend.arg("-F").arg(path);
    runner::status(&mut amend).map_err(Error::List)
}

/// The commit and subject of a line of a todo list, for messages, without the `#` that git puts
/// between them.
fn shown(rest: &str) -> String {
    rest.replacen(" # ", " ", 1)
}

/// A line of a todo list that picks a commit.
struct Pick<'a> {
    /// The commit, as the todo list abbreviates it.
    commit: &'a str,
    /// The commit and what follows it, which a `drop` keeps.
    rest: &'a str,
}

impl<'a> Pick<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let line = line.trim_start();
        let (verb, rest) = line.split_once(' ')?;
        if !matches!(verb, "pick" | "p") {
            return None;
        }
        let rest = rest.trim_start();
        let commit = rest.split_whitespace().next()?;
        Some(Self { commit, rest })
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
