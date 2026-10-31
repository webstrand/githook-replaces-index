//! Records rewrites in refs/notes/replaces. When a rewrite makes a commit `new` from a commit
//! `old`, the note of `new` comes to hold a `replaces: <old>` line and every `replaces:` line of
//! the note of `old`: `new` stands in place of `old` and of everything `old` stands in place of.
//! The note of `old` is not changed.
//!
//! "Replaces" holds only between the chains that hold the two commits: `old` may stay on other
//! branches, and in other clones, as a commit in its own right. The notes record which commit was
//! made from which, and a query that compares two chains decides what replaces what.
//!
//! Lines are only ever added to a note, and only to the note of a commit that a rewrite in this
//! clone created, so the notes of two clones merge by taking the union of each note's lines. The
//! lines a note inherits are a copy for convenience: when a commit was rewritten before the note
//! of the commit it was made from arrived from another clone, they are incomplete, and a reader
//! follows the `replaces:` lines from note to note. The lines that git copies to `new` when
//! notes.rewriteRef matches this ref are among those `new` inherits, and are kept.
//!
//! The notes of the commits a rewrite created are written in one commit on the ref. The commit is
//! written to a ref of its own, and the notes ref is moved to it only if it has not moved since it
//! was read; otherwise the notes are read again.

pub mod abandoned;
mod error;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::hash::{BuildHasher, RandomState};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use runner::git;

pub use crate::error::Error;

/// The notes ref that records the rewrites.
pub const REF: &str = "refs/notes/replaces";

/// The prefix of each line of a note.
pub const REPLACES: &str = "replaces: ";

/// The longest wait before the first retry; each retry may wait twice as long as the one before,
/// up to `LONGEST_WAIT`.
const FIRST_WAIT: Duration = Duration::from_millis(50);
const LONGEST_WAIT: Duration = Duration::from_secs(1);

/// The refusals in a row, with no lock seen, after which a writer gives up.
const REFUSALS: u32 = 3;

/// The replacements of a rewrite, each from the commit it was made from to the commit it made, in
/// the order the rewrite reported them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Rewrite {
    pub replacements: Vec<(String, String)>,
}

impl Rewrite {
    /// Reads a rewrite from lines of the form `<old> <new>`, as git passes them to post-rewrite.
    pub fn parse(text: &str) -> Self {
        let replacements = text
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                Some((words.next()?.to_owned(), words.next()?.to_owned()))
            })
            .collect();
        Self { replacements }
    }

    /// The rewrite as `parse` reads it, one `<old> <new>` line per replacement.
    pub fn text(&self) -> String {
        self.replacements
            .iter()
            .map(|(old, new)| format!("{old} {new}\n"))
            .collect()
    }
}

/// The lines of notes, by the commit they annotate.
type Notes = BTreeMap<String, BTreeSet<String>>;

/// Records `rewrite` in the ref, retrying while other writers move it.
pub fn record(rewrite: &Rewrite) -> Result<(), Error> {
    record_then(rewrite, || {})
}

/// Records `rewrite` as `record` does, and calls `written` as soon as the ref holds it.
///
/// A writer retries for as long as other writers move the ref or hold its lock; `progress` tells
/// how it fares, and a process that must end sooner ends it.
pub fn record_then(rewrite: &Rewrite, written: impl FnOnce()) -> Result<(), Error> {
    *progress_mut() = Progress::default();
    if rewrite.replacements.is_empty() {
        written();
        return Ok(());
    }
    let staging = Staging::new();
    let mut ceiling = FIRST_WAIT;
    let mut refusals = 0;
    loop {
        progress_mut().attempt += 1;
        let retry = match attempt(rewrite, &staging.0)? {
            Attempt::Written => {
                written();
                return Ok(());
            }
            Attempt::Moved => {
                refusals = 0;
                Retry::Moved
            }
            Attempt::Locked => {
                refusals = 0;
                Retry::Locked
            }
            // A lock may be let go of between the refusal and the look for it, so that only a
            // refusal that recurs is taken for one that will not end.
            Attempt::Refused(err) => {
                refusals += 1;
                if refusals == REFUSALS {
                    return Err(Error::Write(err));
                }
                Retry::Locked
            }
        };
        progress_mut().retrying = Some(retry);
        // Writers that lost to each other wait for different times before retrying.
        let ceiling_ms = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
        std::thread::sleep(Duration::from_millis(random() % (ceiling_ms + 1)));
        ceiling = (ceiling * 2).min(LONGEST_WAIT);
    }
}

/// How the writer of this process fares.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// The attempts made so far, the one under way included.
    pub attempt: u32,
    /// Why the writer retries, once an attempt has failed.
    pub retrying: Option<Retry>,
}

/// Why a writer retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Another writer moved the ref after it was read.
    Moved,
    /// Another process held the lock of the ref.
    Locked,
}

static PROGRESS: Mutex<Progress> = Mutex::new(Progress {
    attempt: 0,
    retrying: None,
});

fn progress_mut() -> std::sync::MutexGuard<'static, Progress> {
    PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How the writer of this process fares.
pub fn progress() -> Progress {
    *progress_mut()
}

/// The outcome of one attempt to write the ref.
enum Attempt {
    Written,
    /// Another writer moved the ref after it was read.
    Moved,
    /// Another process held the lock of the ref.
    Locked,
    /// git refused to write the ref, or the staging ref, and no lock was seen.
    Refused(runner::Error),
}

/// The ref that a writer writes its commit to before it moves the notes ref, removed when the
/// writer is done.
struct Staging(String);

/// The staging ref of the writer of this process, if one is writing.
static STAGING: Mutex<Option<String>> = Mutex::new(None);

impl Staging {
    fn new() -> Self {
        let name = format!("refs/replaces-index/record-{:016x}", random());
        *STAGING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(name.clone());
        Self(name)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = runner::run(&mut git(["update-ref", "-d", &self.0]));
        *STAGING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// The ref that this process stages the notes in while it writes them, which a process that
/// ends before the writer is done must remove itself.
pub fn staging() -> Option<String> {
    STAGING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// A random number, which no other process draws alike.
fn random() -> u64 {
    RandomState::new().hash_one(std::process::id())
}

/// Writes the notes once.
fn attempt(rewrite: &Rewrite, staging: &str) -> Result<Attempt, Error> {
    let parent = notes_commit()?;
    let involved: BTreeSet<&str> = rewrite
        .replacements
        .iter()
        .flat_map(|(old, new)| [old.as_str(), new.as_str()])
        .collect();
    let current = match &parent {
        Some(_) => read(&involved)?,
        None => Notes::new(),
    };
    let changed = notes_of(&current, rewrite);
    if changed.is_empty() {
        return Ok(Attempt::Written);
    }

    let ident = runner::stdout(&mut git(["var", "GIT_COMMITTER_IDENT"])).map_err(Error::Write)?;
    let stream = stream(staging, ident.trim_end(), parent.as_deref(), &changed);
    // fast-import writes the staging ref, which a reftable repository locks along with every
    // other ref.
    if let Err(err) = runner::stdout_with_input(
        &mut git(["fast-import", "--quiet", "--force"]),
        stream.as_bytes(),
    ) {
        return if refs_locked()? {
            Ok(Attempt::Locked)
        } else {
            Ok(Attempt::Refused(err))
        };
    }
    let commit = runner::stdout(&mut git([
        "rev-parse",
        "-q",
        "--verify",
        &format!("{staging}^{{commit}}"),
    ]))
    .map_err(Error::Write)?;
    // An empty old value requires that the ref does not exist yet.
    let mut update = git([
        "update-ref",
        "-m",
        "replaces-index: record",
        REF,
        commit.trim_end(),
        parent.as_deref().unwrap_or(""),
    ]);
    let output = runner::run(&mut update).map_err(Error::Write)?;
    if output.status.success() {
        return Ok(Attempt::Written);
    }
    // git refuses alike a ref that moved, one whose lock another process holds, and one it cannot
    // write at all; only reading the ref again, and looking for its lock, tells them apart.
    if notes_commit()? != parent {
        return Ok(Attempt::Moved);
    }
    if refs_locked()? {
        return Ok(Attempt::Locked);
    }
    let refused = runner::check(&update, &output).expect_err("update-ref failed");
    Ok(Attempt::Refused(refused))
}

/// Whether another process holds the lock of the ref: its own lock file in a repository that keeps
/// refs as files, or the lock of the list of tables in one that keeps them in a reftable.
fn refs_locked() -> Result<bool, Error> {
    let out = runner::stdout(&mut git([
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        &format!("{REF}.lock"),
        "--git-common-dir",
    ]))
    .map_err(Error::Write)?;
    let mut lines = out.lines().map(std::path::Path::new);
    let ref_lock = lines.next().map(Path::to_owned);
    let reftable = lines
        .next()
        .map(|common| common.join("reftable/tables.list.lock"));
    Ok([ref_lock, reftable]
        .into_iter()
        .flatten()
        .any(|lock| lock.exists()))
}

/// The commit that the ref points to, if it exists.
fn notes_commit() -> Result<Option<String>, Error> {
    let format = "--format=%(objectname) %(objecttype)";
    let out = runner::stdout(&mut git(["for-each-ref", format, REF])).map_err(Error::Read)?;
    let Some((id, kind)) = out.trim().split_once(' ') else {
        return Ok(None);
    };
    if kind != "commit" {
        return Err(Error::NotACommit(id.to_owned()));
    }
    Ok(Some(id.to_owned()))
}

/// The notes of the commits `wanted`, those that have one.
fn read(wanted: &BTreeSet<&str>) -> Result<Notes, Error> {
    // `<blob> <annotated commit>` for every note.
    let list = runner::stdout(&mut git(["notes", &format!("--ref={REF}"), "list"]))
        .map_err(Error::Read)?;
    let (commits, blobs): (Vec<&str>, Vec<&str>) = list
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(blob, commit)| (commit, blob))
        .filter(|(commit, _)| wanted.contains(commit))
        .unzip();
    let contents = runner::cat_file(&blobs).map_err(Error::Read)?;
    let mut notes = Notes::new();
    for (commit, text) in commits.into_iter().zip(contents) {
        let text = String::from_utf8_lossy(text.as_deref().unwrap_or_default()).into_owned();
        notes.insert(commit.to_owned(), text.lines().map(str::to_owned).collect());
    }
    Ok(notes)
}

/// The notes that `rewrite` changes, given the `current` notes of the commits it involves, each
/// with all of its lines.
fn notes_of(current: &Notes, rewrite: &Rewrite) -> Notes {
    let mut notes: HashMap<&str, BTreeSet<String>> = HashMap::new();
    for (old, new) in &rewrite.replacements {
        if old == new {
            continue;
        }
        // A commit made earlier in the same rewrite passes on what it inherited there.
        let inherited: Vec<String> = notes
            .get(old.as_str())
            .or_else(|| current.get(old))
            .into_iter()
            .flatten()
            .filter(|line| line.starts_with(REPLACES))
            .cloned()
            .collect();
        let note = notes
            .entry(new.as_str())
            .or_insert_with(|| current.get(new).cloned().unwrap_or_default());
        note.insert(format!("{REPLACES}{old}"));
        note.extend(inherited);
        // A commit recreated with the id of one it stands in place of does not replace itself.
        note.remove(&format!("{REPLACES}{new}"));
    }
    notes
        .into_iter()
        .filter(|(commit, note)| current.get(*commit) != Some(note))
        .map(|(commit, note)| (commit.to_owned(), note))
        .collect()
}

/// A fast-import stream for one commit on `staging` that sets `notes` on top of `parent`. The
/// notes are written with `N`, which leaves the fanout of the notes tree to fast-import; every
/// commit annotated is one that a rewrite in this clone created, which the clone therefore has.
fn stream(staging: &str, ident: &str, parent: Option<&str>, notes: &Notes) -> String {
    let message = "replaces-index: record\n";
    let mut stream = format!(
        "commit {staging}\ncommitter {ident}\ndata {}\n{message}",
        message.len()
    );
    if let Some(parent) = parent {
        let _ = writeln!(stream, "from {parent}");
    }
    for (commit, lines) in notes {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let _ = write!(stream, "N inline {commit}\ndata {}\n{text}\n", text.len());
    }
    stream
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes(entries: &[(&str, &[&str])]) -> Notes {
        entries
            .iter()
            .map(|(commit, lines)| {
                let lines = lines.iter().map(|line| line.to_string()).collect();
                (commit.to_string(), lines)
            })
            .collect()
    }

    #[test]
    fn a_new_commit_inherits_what_the_old_one_replaces() {
        let current = notes(&[("b", &["replaces: a"])]);
        let expected = notes(&[("c", &["replaces: a", "replaces: b"])]);
        assert_eq!(notes_of(&current, &Rewrite::parse("b c\n")), expected);
    }

    #[test]
    fn a_chain_within_one_rewrite_is_inherited_along_it() {
        let expected = notes(&[
            ("b", &["replaces: a"]),
            ("c", &["replaces: a", "replaces: b"]),
        ]);
        assert_eq!(
            notes_of(&Notes::new(), &Rewrite::parse("a b\nb c\n")),
            expected
        );
    }

    #[test]
    fn a_squash_replaces_every_commit_folded_together() {
        let current = notes(&[("b", &["replaces: x"])]);
        let expected = notes(&[("c", &["replaces: a", "replaces: b", "replaces: x"])]);
        assert_eq!(notes_of(&current, &Rewrite::parse("a c\nb c\n")), expected);
    }

    #[test]
    fn the_lines_git_copied_to_the_new_commit_are_kept() {
        let current = notes(&[("b", &["replaces: a"]), ("c", &["replaces: a"])]);
        let expected = notes(&[("c", &["replaces: a", "replaces: b"])]);
        assert_eq!(notes_of(&current, &Rewrite::parse("b c\n")), expected);
    }

    #[test]
    fn a_rewrite_recorded_already_changes_nothing() {
        let current = notes(&[("b", &["replaces: a"])]);
        assert_eq!(notes_of(&current, &Rewrite::parse("a b\n")), Notes::new());
        assert_eq!(notes_of(&current, &Rewrite::parse("a a\n")), Notes::new());
    }

    #[test]
    fn a_commit_recreated_with_an_earlier_id_does_not_replace_itself() {
        let current = notes(&[("b", &["replaces: a"])]);
        let expected = notes(&[("a", &["replaces: b"])]);
        assert_eq!(notes_of(&current, &Rewrite::parse("b a\n")), expected);
    }
}
