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
pub mod heartbeat;
mod repair;

pub use crate::repair::{Repaired, repair, repair_and_say};

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
    if rewrite.replacements.is_empty() {
        *progress_mut() = Progress::default();
        written();
        return Ok(());
    }
    write(Change::Rewrite(rewrite), written)
}

/// Merges the notes of the notes commit `theirs` into the ref, as `record` records a rewrite: the
/// union of the lines of each note, written from the commit the ref was read at. The merge keeps
/// `theirs` as a parent, so that merging it again changes nothing.
pub fn merge(theirs: &str) -> Result<(), Error> {
    write(Change::Merge(theirs), || {})
}

/// What a writer writes on top of the notes it reads.
#[derive(Clone, Copy)]
enum Change<'a> {
    Rewrite(&'a Rewrite),
    /// The notes of another notes commit.
    Merge(&'a str),
    /// The lines of the notes of an earlier notes commit, which a writer dropped.
    Restore(&'a str),
}

/// Writes `change`, retrying while other writers move the ref, and calls `written` as soon as the
/// ref holds it.
fn write(change: Change, written: impl FnOnce()) -> Result<(), Error> {
    *progress_mut() = Progress::default();
    let mut ceiling = FIRST_WAIT;
    let mut refusals = 0;
    loop {
        progress_mut().attempt += 1;
        let retry = match attempt(change)? {
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
    /// git refused to move the ref, which did not move, and no lock was seen.
    Refused(runner::Error),
}

/// A random number, which no other process draws alike.
fn random() -> u64 {
    RandomState::new().hash_one(std::process::id())
}

/// Writes the notes once.
fn attempt(change: Change) -> Result<Attempt, Error> {
    let parent = notes_commit()?;
    let (changed, theirs) = match change {
        Change::Rewrite(rewrite) => {
            let involved: BTreeSet<&str> = rewrite
                .replacements
                .iter()
                .flat_map(|(old, new)| [old.as_str(), new.as_str()])
                .collect();
            let current = match &parent {
                Some(_) => read(&involved)?,
                None => Notes::new(),
            };
            (Changed::ByCommit(notes_of(&current, rewrite)), None)
        }
        Change::Merge(theirs) => match parent.as_deref() {
            None => return move_ref(theirs, None),
            Some(ours) if is_ancestor(theirs, ours)? => return Ok(Attempt::Written),
            Some(ours) if is_ancestor(ours, theirs)? => return move_ref(theirs, Some(ours)),
            Some(ours) => (Changed::ByPath(union(ours, theirs)?), Some(theirs)),
        },
        Change::Restore(earlier) => match parent.as_deref() {
            Some(ours) => (Changed::ByPath(union(ours, earlier)?), None),
            None => return move_ref(earlier, None),
        },
    };
    if changed.is_empty() {
        return Ok(Attempt::Written);
    }

    let ident = runner::stdout(&mut git(["var", "GIT_COMMITTER_IDENT"])).map_err(Error::Write)?;
    let stream = stream(ident.trim_end(), parent.as_deref(), theirs, &changed);
    // Without --force, fast-import moves the ref only to a commit that holds the one it points at
    // when fast-import moves it, which a writer that came in between has moved it from.
    match runner::stdout_with_input(&mut git(["fast-import", "--quiet"]), stream.as_bytes()) {
        Ok(_) => Ok(Attempt::Written),
        Err(err) => refused(parent.as_deref(), err),
    }
}

/// Moves the ref to `new`, only if it still points at `parent`, or does not exist when `parent` is
/// `None`.
fn move_ref(new: &str, parent: Option<&str>) -> Result<Attempt, Error> {
    // An empty old value requires that the ref does not exist yet.
    let mut update = git([
        "update-ref",
        "-m",
        "replaces-index: record",
        REF,
        new,
        parent.unwrap_or(""),
    ]);
    let output = runner::run(&mut update).map_err(Error::Write)?;
    match runner::check(&update, &output) {
        Ok(()) => Ok(Attempt::Written),
        Err(err) => refused(parent, err),
    }
}

/// Why git refused, with `err`, to move the ref from `parent`. git refuses alike a ref that moved,
/// one whose lock another process holds, and one it cannot write at all; only reading the ref
/// again, and looking for its lock, tells them apart.
fn refused(parent: Option<&str>, err: runner::Error) -> Result<Attempt, Error> {
    if notes_commit()?.as_deref() != parent {
        return Ok(Attempt::Moved);
    }
    if refs_locked()? {
        return Ok(Attempt::Locked);
    }
    Ok(Attempt::Refused(err))
}

/// Whether commit `ancestor` is `descendant` or one of its ancestors.
fn is_ancestor(ancestor: &str, descendant: &str) -> Result<bool, Error> {
    let mut command = git(["merge-base", "--is-ancestor", ancestor, descendant]);
    let output = runner::run(&mut command).map_err(Error::Read)?;
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    runner::check(&command, &output).map_err(Error::Read)?;
    Ok(true)
}

/// The notes that a writer writes: by the commit they annotate, which the clone has, or by their
/// path in the notes tree, for the notes of another clone, which annotate commits the clone may
/// lack.
enum Changed {
    ByCommit(Notes),
    ByPath(Notes),
}

impl Changed {
    fn is_empty(&self) -> bool {
        match self {
            Self::ByCommit(notes) | Self::ByPath(notes) => notes.is_empty(),
        }
    }
}

/// The notes of the notes commit `ours` that the union with those of `theirs` changes, by their
/// path in the tree of `ours`, or of `theirs` for a note that `ours` lacks.
fn union(ours: &str, theirs: &str) -> Result<Notes, Error> {
    let our_files = note_files(ours)?;
    let their_files = note_files(theirs)?;
    // Each note of theirs that differs from ours, with ours if any.
    let differing: Vec<(Option<&NoteFile>, &NoteFile)> = their_files
        .iter()
        .map(|(commit, theirs)| (our_files.get(commit), theirs))
        .filter(|(ours, theirs)| ours.is_none_or(|ours| ours.blob != theirs.blob))
        .collect();
    let blobs: Vec<&str> = differing
        .iter()
        .flat_map(|(ours, theirs)| ours.iter().chain([theirs]).map(|file| file.blob.as_str()))
        .collect();
    // The contents of the blobs, in the order of `blobs`.
    let mut contents = runner::cat_file(&blobs).map_err(Error::Read)?.into_iter();
    let mut next_lines = || -> BTreeSet<String> {
        let text = contents.next().flatten().unwrap_or_default();
        String::from_utf8_lossy(&text)
            .lines()
            .map(str::to_owned)
            .collect()
    };
    let mut changed = Notes::new();
    for (ours, theirs) in differing {
        let mut note = match ours {
            Some(_) => next_lines(),
            None => BTreeSet::new(),
        };
        let before = note.len();
        note.extend(next_lines());
        if ours.is_none() || note.len() != before {
            let path = ours.unwrap_or(theirs).path.clone();
            changed.insert(path, note);
        }
    }
    Ok(changed)
}

/// A note as a file of the notes tree.
struct NoteFile {
    /// Its path, the annotated commit split by the fanout of the tree.
    path: String,
    blob: String,
}

/// The notes of the notes commit `notes`, by the commit they annotate.
fn note_files(notes: &str) -> Result<BTreeMap<String, NoteFile>, Error> {
    let out = runner::stdout(&mut git(["ls-tree", "-r", "-z", notes])).map_err(Error::Read)?;
    // `<mode> blob <blob>\t<path>` for each file.
    Ok(out
        .split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            let blob = meta.strip_prefix("100644 blob ")?;
            let commit: String = path.chars().filter(|&c| c != '/').collect();
            let file = NoteFile {
                path: path.to_owned(),
                blob: blob.to_owned(),
            };
            Some((commit, file))
        })
        .collect())
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

/// A fast-import stream for one commit on the ref that sets `notes` on top of `parent`, and that
/// merges `theirs`, if given. Notes by commit are written with `N`, which leaves the fanout of the
/// notes tree to fast-import; every commit they annotate is one that a rewrite in this clone
/// created, which the clone therefore has. Notes by path are written as files at that path.
fn stream(ident: &str, parent: Option<&str>, theirs: Option<&str>, notes: &Changed) -> String {
    let message = if theirs.is_some() {
        "replaces-index: merge\n"
    } else {
        "replaces-index: record\n"
    };
    let mut stream = format!(
        "commit {REF}\ncommitter {ident}\ndata {}\n{message}",
        message.len()
    );
    if let Some(parent) = parent {
        let _ = writeln!(stream, "from {parent}");
    }
    if let Some(theirs) = theirs {
        let _ = writeln!(stream, "merge {theirs}");
    }
    let text = |lines: &BTreeSet<String>| -> String {
        lines.iter().map(|line| format!("{line}\n")).collect()
    };
    match notes {
        Changed::ByCommit(notes) => {
            for (commit, lines) in notes {
                let text = text(lines);
                let _ = write!(stream, "N inline {commit}\ndata {}\n{text}\n", text.len());
            }
        }
        Changed::ByPath(notes) => {
            for (path, lines) in notes {
                let text = text(lines);
                let _ = write!(
                    stream,
                    "M 100644 inline {path}\ndata {}\n{text}\n",
                    text.len()
                );
            }
        }
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
