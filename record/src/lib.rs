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
//! The notes of the commits a rewrite created are written in one commit on the ref, through
//! `git fast-import`, which moves the notes ref to it only if it has not moved since it was read;
//! otherwise the notes are read again.

pub mod abandoned;
mod error;
pub mod heartbeat;
mod repair;

pub use crate::repair::{Repaired, repair, repair_and_say, warn_forcing};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{BuildHasher, RandomState};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

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

/// How long a writer watches a lock of the ref held before it gives up: 30 seconds, or the
/// milliseconds that `REPLACES_INDEX_LOCK_PATIENCE_MS` gives, which the tests set. Git holds it for
/// moments only, and gives up itself after 100 milliseconds, so that one held this long was most
/// likely left behind by a git ended by `SIGKILL`. A writer that gives up keeps its rewrite as an
/// abandoned record, which the next writer records once the lock is gone.
pub fn lock_patience() -> Duration {
    static PATIENCE: OnceLock<Duration> = OnceLock::new();
    *PATIENCE.get_or_init(|| {
        std::env::var_os("REPLACES_INDEX_LOCK_PATIENCE_MS")
            .and_then(|ms| ms.to_str()?.parse().ok())
            .map_or(Duration::from_secs(30), Duration::from_millis)
    })
}

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

/// The lines of notes, by the commit they annotate. A line is the bytes that a note holds, which
/// need not be UTF-8: a note from another clone, or one written by hand, may hold any.
pub(crate) type Notes = BTreeMap<String, BTreeSet<Vec<u8>>>;

/// The lines of the note `text`, each without its `\n`.
pub(crate) fn lines(text: &[u8]) -> BTreeSet<Vec<u8>> {
    let text = text.strip_suffix(b"\n").unwrap_or(text);
    if text.is_empty() {
        return BTreeSet::new();
    }
    text.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect()
}

/// Records `rewrite` in the ref, retrying while other writers move it.
pub fn record(rewrite: &Rewrite) -> Result<(), Error> {
    record_then(rewrite, || {})
}

/// Records `rewrite` as `record` does, and calls `written` as soon as the ref holds it.
///
/// A writer retries for as long as other writers move the ref or hold its lock; `progress` tells
/// how it fares, and a process that must end sooner ends it.
pub fn record_then(rewrite: &Rewrite, written: impl FnOnce()) -> Result<(), Error> {
    // An abbreviated id would be written into the notes as it is, where no query matches it.
    let ids = rewrite
        .replacements
        .iter()
        .flat_map(|(old, new)| [old, new]);
    if let Some(id) = ids.into_iter().find(|id| !is_id(id)) {
        return Err(Error::NotAnId(id.clone()));
    }
    if rewrite.replacements.is_empty() {
        *progress_mut() = Progress::default();
        written();
        return Ok(());
    }
    // fast-import writes the note of a commit only if the clone has it, and fails otherwise, which
    // no retry would change.
    let news: BTreeSet<&str> = rewrite
        .replacements
        .iter()
        .map(|(_, new)| new.as_str())
        .collect();
    let commits = runner::commits(news.iter().copied()).map_err(Error::Read)?;
    if let Some(missing) = news.into_iter().find(|new| !commits.contains(*new)) {
        return Err(Error::NotInClone(missing.to_owned()));
    }
    write(Change::Rewrite(rewrite), written)
}

/// Whether `word` is the full id of an object of this repository, as git writes it: lowercase hex
/// digits, 40 of them in a SHA-1 repository and 64 in a SHA-256 one. Another form names the same
/// object to git, but no query matches it, since the notes hold ids as git writes them.
pub(crate) fn is_id(word: &str) -> bool {
    let length_fits = match id_length() {
        Some(length) => word.len() == length,
        None => matches!(word.len(), 40 | 64),
    };
    length_fits && word.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The length of the ids of this repository's objects, once found; none when git cannot tell.
fn id_length() -> Option<usize> {
    static LENGTH: OnceLock<Option<usize>> = OnceLock::new();
    *LENGTH.get_or_init(|| {
        let format = runner::stdout(&mut git(["rev-parse", "--show-object-format"])).ok()?;
        match format.trim() {
            "sha1" => Some(40),
            "sha256" => Some(64),
            _ => None,
        }
    })
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
    /// Lines to add to the notes of the commits they are given for, which a writer dropped.
    Add(&'a Notes),
}

/// Writes `change`, retrying while other writers move the ref, and calls `written` as soon as the
/// ref holds it.
fn write(change: Change, written: impl FnOnce()) -> Result<(), Error> {
    *progress_mut() = Progress::default();
    // Found on the first attempt that writes a commit, and kept for the retries.
    let mut ident = None;
    let mut ceiling = FIRST_WAIT;
    let mut refusals = 0;
    // The lock that the attempts found, and when they first found it.
    let mut seen: Option<(Lock, Instant)> = None;
    loop {
        progress_mut().attempt += 1;
        let mut lock = None;
        let retry = match attempt(change, &mut ident)? {
            Attempt::Written => {
                written();
                return Ok(());
            }
            Attempt::Moved => {
                refusals = 0;
                Retry::Moved
            }
            Attempt::Locked(found) => {
                refusals = 0;
                let since = match &seen {
                    Some((last, since)) if *last == found => *since,
                    _ => Instant::now(),
                };
                // Held for as long as this writer has watched it, by its own clock, which no
                // clock of another machine, as on a network filesystem, puts out. The lock
                // itself is never taken or removed.
                let held = since.elapsed();
                if held >= lock_patience() {
                    return Err(Error::Locked {
                        path: found.path,
                        seconds: held.as_secs(),
                    });
                }
                seen = Some((found.clone(), since));
                lock = Some((found, since));
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
        {
            let mut progress = progress_mut();
            progress.retrying = Some(retry);
            progress.lock = lock;
        }
        // Writers that lost to each other wait for different times before retrying.
        let ceiling_ms = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
        std::thread::sleep(Duration::from_millis(random() % (ceiling_ms + 1)));
        ceiling = (ceiling * 2).min(LONGEST_WAIT);
    }
}

/// How the writer of this process fares.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Progress {
    /// The attempts made so far, the one under way included.
    pub attempt: u32,
    /// Why the writer retries, once an attempt has failed.
    pub retrying: Option<Retry>,
    /// The lock that the last attempt found held, if any, and since when this writer has watched
    /// it.
    pub lock: Option<(Lock, Instant)>,
}

/// A lock file that another process holds, or that a process ended by `SIGKILL` left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub path: PathBuf,
    /// When it was last written, which git does as it takes the lock: a lock with another time is
    /// another lock, taken since.
    pub modified: Option<SystemTime>,
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
    lock: None,
});

fn progress_mut() -> std::sync::MutexGuard<'static, Progress> {
    PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How the writer of this process fares.
pub fn progress() -> Progress {
    progress_mut().clone()
}

/// The outcome of one attempt to write the ref.
enum Attempt {
    Written,
    /// Another writer moved the ref after it was read.
    Moved,
    /// Another process held the lock of the ref.
    Locked(Lock),
    /// git refused to move the ref, which did not move, and no lock was seen.
    Refused(runner::Error),
}

/// A random number, which no other process draws alike.
fn random() -> u64 {
    RandomState::new().hash_one(std::process::id())
}

/// Writes the notes once.
fn attempt(change: Change, ident: &mut Option<Vec<u8>>) -> Result<Attempt, Error> {
    let parent = notes_commit()?;
    let (changed, theirs) = match change {
        Change::Rewrite(rewrite) => {
            let involved: BTreeSet<&str> = rewrite
                .replacements
                .iter()
                .flat_map(|(old, new)| [old.as_str(), new.as_str()])
                .collect();
            let (current, found) = match &parent {
                Some(parent) => read(parent, &involved)?,
                None => (Notes::new(), Paths::new()),
            };
            (Changed::ByCommit(notes_of(&current, rewrite), found), None)
        }
        Change::Merge(theirs) => match parent.as_deref() {
            None => return move_ref(theirs, None),
            Some(ours) if is_ancestor(theirs, ours)? => return Ok(Attempt::Written),
            Some(ours) if is_ancestor(ours, theirs)? => return move_ref(theirs, Some(ours)),
            Some(ours) => (Changed::ByPath(union(ours, theirs)?), Some(theirs)),
        },
        Change::Add(additions) => (Changed::ByPath(add(parent.as_deref(), additions)?), None),
    };
    if changed.is_empty() {
        return Ok(Attempt::Written);
    }

    let ident = match ident {
        Some(ident) => ident,
        None => {
            // The name and email of the committer are bytes of the config, which need not be UTF-8.
            let mut found = runner::stdout_bytes(&mut git(["var", "GIT_COMMITTER_IDENT"]))
                .map_err(Error::Write)?;
            while found.last().is_some_and(u8::is_ascii_whitespace) {
                found.pop();
            }
            ident.insert(found)
        }
    };
    let stream = stream(ident, parent.as_deref(), theirs, &changed);
    // Without --force, fast-import moves the ref only to a commit that holds the one it points at
    // when fast-import moves it, which a writer that came in between has moved it from.
    match runner::stdout_with_input(&mut git(["fast-import", "--quiet"]), &stream) {
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
    if let Some(lock) = refs_lock()? {
        return Ok(Attempt::Locked(lock));
    }
    Ok(Attempt::Refused(err))
}

/// Whether commit `ancestor` is `descendant` or one of its ancestors.
fn is_ancestor(ancestor: &str, descendant: &str) -> Result<bool, Error> {
    runner::is_ancestor(ancestor, descendant).map_err(Error::Read)
}

/// The notes that a writer writes: by the commit they annotate, which the clone has, with the
/// paths each is found at now, or by their path in the notes tree, for the notes of another clone,
/// which annotate commits the clone may lack.
enum Changed {
    ByCommit(Notes, Paths),
    ByPath(Notes),
}

impl Changed {
    fn is_empty(&self) -> bool {
        match self {
            Self::ByCommit(notes, _) | Self::ByPath(notes) => notes.is_empty(),
        }
    }
}

/// The notes of the notes commit `ours` that the union with those of `theirs` changes, by their
/// path in the tree of `ours`. A note that `ours` lacks goes at the fanout of the notes of `ours`
/// rather than at its path in `theirs`, whose fanout may differ, so that the tree keeps one.
fn union(ours: &str, theirs: &str) -> Result<Notes, Error> {
    let our_files = note_files(ours)?;
    let their_files = note_files(theirs)?;
    let depth = fanout(&our_files);
    // Each note of theirs that differs from ours, with ours if any.
    let differing: Vec<(&str, Option<&NoteFile>, &NoteFile)> = their_files
        .iter()
        .map(|(commit, theirs)| (commit.as_str(), our_files.get(commit), theirs))
        .filter(|(_, ours, theirs)| ours.is_none_or(|ours| ours.blob != theirs.blob))
        .collect();
    let blobs: Vec<&str> = differing
        .iter()
        .flat_map(|(_, ours, theirs)| ours.iter().chain([theirs]).map(|file| file.blob.as_str()))
        .collect();
    // The contents of the blobs, in the order of `blobs`.
    let mut contents = runner::cat_file(&blobs).map_err(Error::Read)?.into_iter();
    let mut next_lines = || lines(&contents.next().flatten().unwrap_or_default());
    let mut changed = Notes::new();
    for (commit, ours, theirs) in differing {
        let mut note = match ours {
            Some(_) => next_lines(),
            None => BTreeSet::new(),
        };
        let before = note.len();
        note.extend(next_lines());
        let path = match (ours, depth) {
            (Some(ours), _) => ours.path.clone(),
            (None, Some(depth)) => note_path(commit, depth),
            (None, None) => theirs.path.clone(),
        };
        if ours.is_none() || note.len() != before {
            changed.insert(path, note);
        }
    }
    Ok(changed)
}

/// The notes of the notes commit `ours`, if any, that `additions` changes, by their path in its
/// tree: each with its lines and those that `additions` gives for its commit. A note that `ours`
/// lacks goes at the fanout of the notes of `ours`.
fn add(ours: Option<&str>, additions: &Notes) -> Result<Notes, Error> {
    let our_files = match ours {
        Some(ours) => note_files(ours)?,
        None => BTreeMap::new(),
    };
    let depth = fanout(&our_files).unwrap_or(0);
    let blobs: Vec<&str> = additions
        .keys()
        .filter_map(|commit| our_files.get(commit))
        .map(|file| file.blob.as_str())
        .collect();
    let mut contents = runner::cat_file(&blobs).map_err(Error::Read)?.into_iter();
    let mut changed = Notes::new();
    for (commit, added) in additions {
        let (path, mut note) = match our_files.get(commit) {
            Some(file) => (
                file.path.clone(),
                lines(&contents.next().flatten().unwrap_or_default()),
            ),
            None => (note_path(commit, depth), BTreeSet::new()),
        };
        let before = note.len();
        note.extend(added.iter().cloned());
        if note.len() != before {
            changed.insert(path, note);
        }
    }
    Ok(changed)
}

/// The depth of fanout that most of `files` are at, if there are any.
fn fanout(files: &BTreeMap<String, NoteFile>) -> Option<usize> {
    let mut depths = [0usize; FANOUT + 1];
    for file in files.values() {
        depths[file.path.matches('/').count().min(FANOUT)] += 1;
    }
    (0..=FANOUT)
        .filter(|&d| depths[d] > 0)
        .max_by_key(|&d| depths[d])
}

/// Every note of the notes commit `notes`, by the commit it annotates.
pub(crate) fn all_notes(notes: &str) -> Result<Notes, Error> {
    let files = note_files(notes)?;
    let blobs: Vec<&str> = files.values().map(|file| file.blob.as_str()).collect();
    let contents = runner::cat_file(&blobs).map_err(Error::Read)?;
    Ok(files
        .into_keys()
        .zip(contents)
        .map(|(commit, text)| (commit, lines(&text.unwrap_or_default())))
        .collect())
}

/// A note as a file of the notes tree.
struct NoteFile {
    /// Its path, the annotated commit split by the fanout of the tree.
    path: String,
    blob: String,
}

/// The notes of the notes commit `notes`, by the commit they annotate. A file whose path does not
/// spell a commit id is not a note.
fn note_files(notes: &str) -> Result<BTreeMap<String, NoteFile>, Error> {
    let out = runner::stdout(&mut git(["ls-tree", "-r", "-z", "--full-tree", notes]))
        .map_err(Error::Read)?;
    // `<mode> blob <blob>\t<path>` for each file.
    Ok(out
        .split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            let blob = meta.strip_prefix("100644 blob ")?;
            let commit: String = path.chars().filter(|&c| c != '/').collect();
            if !is_id(&commit) {
                return None;
            }
            let file = NoteFile {
                path: path.to_owned(),
                blob: blob.to_owned(),
            };
            Some((commit, file))
        })
        .collect())
}

/// The lock of the ref, if another process holds it: its own lock file in a repository that keeps
/// refs as files, or the lock of the list of tables in one that keeps them in a reftable.
fn refs_lock() -> Result<Option<Lock>, Error> {
    let lock = format!("{REF}.lock");
    let paths = runner::rev_parse_paths(&[
        "--path-format=absolute",
        "--git-path",
        &lock,
        "--git-common-dir",
    ])
    .map_err(Error::Write)?;
    let mut paths = paths.into_iter();
    let ref_lock = paths.next();
    let reftable = paths
        .next()
        .map(|common| common.join("reftable/tables.list.lock"));
    Ok([ref_lock, reftable].into_iter().flatten().find_map(|path| {
        let metadata = std::fs::metadata(&path).ok()?;
        Some(Lock {
            modified: metadata.modified().ok(),
            path,
        })
    }))
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

/// The deepest fanout of a notes tree that a lookup tries: each level splits two more hex digits
/// off the path of a note, and git adds one for every factor of 256 in the number of notes.
const FANOUT: usize = 3;

/// The paths in the notes tree at which the note of each commit was found.
type Paths = BTreeMap<String, Vec<String>>;

/// The notes of the commits `wanted` in the notes commit `notes`, those that have one, and the
/// paths they were found at. Each is looked for at each depth of fanout, by one `git cat-file`,
/// rather than by listing every note. A note found at more than one depth holds the lines of all.
fn read(notes: &str, wanted: &BTreeSet<&str>) -> Result<(Notes, Paths), Error> {
    let specs: Vec<(&str, String)> = wanted
        .iter()
        .flat_map(|commit| (0..=FANOUT).map(move |depth| (*commit, note_path(commit, depth))))
        .collect();
    let names: Vec<String> = specs
        .iter()
        .map(|(_, path)| format!("{notes}:{path}"))
        .collect();
    let contents = runner::cat_file(&names).map_err(Error::Read)?;
    let mut found = Notes::new();
    let mut paths = Paths::new();
    for ((commit, path), text) in specs.into_iter().zip(contents) {
        let Some(text) = text else {
            continue;
        };
        found
            .entry(commit.to_owned())
            .or_default()
            .extend(lines(&text));
        paths.entry(commit.to_owned()).or_default().push(path);
    }
    Ok((found, paths))
}

/// The path of the note of `commit`, a full id, in a notes tree of fanout `depth`: each of its
/// first `depth` pairs of hex digits names a directory.
fn note_path(commit: &str, depth: usize) -> String {
    let (dirs, rest) = commit.split_at(2 * depth);
    let mut path = String::new();
    for pair in dirs.as_bytes().chunks(2) {
        path.push_str(std::str::from_utf8(pair).unwrap_or_default());
        path.push('/');
    }
    path.push_str(rest);
    path
}

/// The notes that `rewrite` changes, given the `current` notes of the commits it involves, each
/// with all of its lines.
fn notes_of(current: &Notes, rewrite: &Rewrite) -> Notes {
    let mut notes: HashMap<&str, BTreeSet<Vec<u8>>> = HashMap::new();
    for (old, new) in &rewrite.replacements {
        if old == new {
            continue;
        }
        // A commit made earlier in the same rewrite passes on what it inherited there.
        let inherited: Vec<Vec<u8>> = notes
            .get(old.as_str())
            .or_else(|| current.get(old))
            .into_iter()
            .flatten()
            .filter(|line| line.starts_with(REPLACES.as_bytes()))
            .cloned()
            .collect();
        let note = notes
            .entry(new.as_str())
            .or_insert_with(|| current.get(new).cloned().unwrap_or_default());
        note.insert(format!("{REPLACES}{old}").into_bytes());
        note.extend(inherited);
        // A commit recreated with the id of one it stands in place of does not replace itself.
        note.remove(format!("{REPLACES}{new}").as_bytes());
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
///
/// `N` replaces a note only at the fanout that fast-import derives from the number of notes, while
/// `git notes` chooses the fanout of the trees it writes otherwise, and a merge may have left a
/// note at another. Each note written by commit is therefore removed first from every path it was
/// found at, so that the tree never holds two files for one commit.
fn stream(ident: &[u8], parent: Option<&str>, theirs: Option<&str>, notes: &Changed) -> Vec<u8> {
    let message = if theirs.is_some() {
        "replaces-index: merge\n"
    } else {
        "replaces-index: record\n"
    };
    let mut stream = format!("commit {REF}\ncommitter ").into_bytes();
    stream.extend_from_slice(ident);
    let _ = write!(stream, "\ndata {}\n{message}", message.len());
    if let Some(parent) = parent {
        let _ = writeln!(stream, "from {parent}");
    }
    if let Some(theirs) = theirs {
        let _ = writeln!(stream, "merge {theirs}");
    }
    let data = |stream: &mut Vec<u8>, lines: &BTreeSet<Vec<u8>>| {
        let text: Vec<u8> = lines
            .iter()
            .flat_map(|line| line.iter().copied().chain(*b"\n"))
            .collect();
        let _ = writeln!(stream, "data {}", text.len());
        stream.extend_from_slice(&text);
        stream.push(b'\n');
    };
    match notes {
        Changed::ByCommit(notes, found) => {
            for (commit, lines) in notes {
                for path in found.get(commit).into_iter().flatten() {
                    let _ = writeln!(stream, "D {path}");
                }
                let _ = writeln!(stream, "N inline {commit}");
                data(&mut stream, lines);
            }
        }
        Changed::ByPath(notes) => {
            for (path, lines) in notes {
                let _ = writeln!(stream, "M 100644 inline {path}");
                data(&mut stream, lines);
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
                let lines = lines.iter().map(|line| line.as_bytes().to_vec()).collect();
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
