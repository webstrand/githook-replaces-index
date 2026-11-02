//! An index of refs/notes/replaces, kept in `replaces/` in the common git directory, which answers
//! in both directions what a commit stands in place of: the commits a commit was made from,
//! through any number of rewrites, and the commits that were made from it.
//!
//! The index is derived from the notes and never shared: each clone builds its own, and its
//! worktrees share it. It records the notes commit it was built from, and is brought up to date
//! whenever it is opened and the notes ref has moved, whatever moved it. Since lines are only ever
//! added to the notes, bringing it up to date reads only the notes that changed since; a notes ref
//! that lost a note, or that does not descend from the commit indexed, is indexed anew.
//!
//! Every `replaces:` line is an edge of the index, and a query follows the edges from commit to
//! commit, so that the lines a note inherits need not be complete.
//!
//! Queries open the index read-only, which any number of processes may do at once. Bringing it up
//! to date needs it to itself, and waits while other processes have it open.

mod error;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use redb::{
    Database, DatabaseError, MultimapTableDefinition, ReadOnlyDatabase, ReadableDatabase,
    TableDefinition,
};
use replaces_index_record::{REF, REPLACES};
use runner::git;

pub use crate::error::Error;

/// Each commit, with the commits that its note says it replaces.
const REPLACES_TABLE: MultimapTableDefinition<&str, &str> =
    MultimapTableDefinition::new("replaces");
/// Each commit, with the commits whose notes say they replace it.
const REPLACED_BY_TABLE: MultimapTableDefinition<&str, &str> =
    MultimapTableDefinition::new("replaced-by");
/// The notes commit that the index was built from, under `NOTES`.
const META_TABLE: TableDefinition<&str, &str> = TableDefinition::new("meta");
const NOTES: &str = "notes";

/// How long a process waits for others to let go of the index.
const PATIENCE: Duration = Duration::from_secs(10);

/// The index, up to date with the notes ref as it was when it was opened.
pub struct Index {
    db: ReadOnlyDatabase,
    path: PathBuf,
}

impl Index {
    /// Opens the index of the repository the process runs in, and brings it up to date first.
    pub fn open() -> Result<Self, Error> {
        let dir = runner::rev_parse_path(&["--path-format=absolute", "--git-common-dir"])
            .map_err(Error::GitDir)?
            .join("replaces");
        fs::create_dir_all(&dir).map_err(|source| Error::CreateDir {
            path: dir.clone(),
            source,
        })?;
        let path = dir.join("index.redb");
        let notes = notes_commit()?;
        let mut deadline = Instant::now() + PATIENCE;
        loop {
            match open_current(&path, notes.as_deref())? {
                Opened::Current(db) => return Ok(Self { db, path }),
                Opened::Locked => {}
                Opened::Stale if !path.exists() => {
                    build(&path, notes.as_deref())?;
                    deadline = Instant::now() + PATIENCE;
                    continue;
                }
                // Opened rather than created, so that a file deleted in the meantime is not
                // created empty, as a process that opens it then would take for damaged.
                Opened::Stale => match Database::open(&path) {
                    Ok(db) => {
                        update(&db, &path, notes.as_deref())?;
                        // Reopened read-only at once, so that other processes may share it. The
                        // time the update took was not spent waiting for them, and the wait for
                        // them starts anew.
                        deadline = Instant::now() + PATIENCE;
                        continue;
                    }
                    Err(DatabaseError::DatabaseAlreadyOpen) => {}
                    Err(_) if !path.exists() => continue,
                    Err(source) => return Err(index_error(&path, source)),
                },
            }
            if Instant::now() > deadline {
                return Err(Error::Busy(path));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Every commit that `commit` stands in place of, through any number of rewrites.
    pub fn replaces(&self, commit: &str) -> Result<BTreeSet<String>, Error> {
        self.reach(REPLACES_TABLE, commit)
    }

    /// Every commit that was made from `commit`, through any number of rewrites.
    pub fn replaced_by(&self, commit: &str) -> Result<BTreeSet<String>, Error> {
        self.reach(REPLACED_BY_TABLE, commit)
    }

    /// Every commit that the index names whose id begins with `prefix`.
    pub fn with_prefix(&self, prefix: &str) -> Result<BTreeSet<String>, Error> {
        let txn = self.db.begin_read().map_err(|e| self.error(e))?;
        let mut found = BTreeSet::new();
        for definition in [REPLACES_TABLE, REPLACED_BY_TABLE] {
            let table = match txn.open_multimap_table(definition) {
                Ok(table) => table,
                Err(redb::TableError::TableDoesNotExist(_)) => continue,
                Err(err) => return Err(self.error(err)),
            };
            for entry in table.range(prefix..).map_err(|e| self.error(e))? {
                let (key, _) = entry.map_err(|e| self.error(e))?;
                let key = key.value();
                if !key.starts_with(prefix) {
                    break;
                }
                found.insert(key.to_owned());
            }
        }
        Ok(found)
    }

    /// Every commit reachable from `start` along the edges of `definition`, without `start`
    /// unless it lies on a cycle.
    fn reach(
        &self,
        definition: MultimapTableDefinition<&str, &str>,
        start: &str,
    ) -> Result<BTreeSet<String>, Error> {
        let txn = self.db.begin_read().map_err(|e| self.error(e))?;
        let table = match txn.open_multimap_table(definition) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(BTreeSet::new()),
            Err(err) => return Err(self.error(err)),
        };
        let mut found = BTreeSet::new();
        let mut stack = vec![start.to_owned()];
        while let Some(commit) = stack.pop() {
            for next in table.get(commit.as_str()).map_err(|e| self.error(e))? {
                let next = next.map_err(|e| self.error(e))?.value().to_owned();
                if found.insert(next.clone()) {
                    stack.push(next);
                }
            }
        }
        Ok(found)
    }

    fn error(&self, source: impl Into<redb::Error>) -> Error {
        index_error(&self.path, source)
    }
}

/// The failure `source` of the index at `path`. A file that redb cannot read as a database of its
/// format is told apart, so that the operator learns that the index may be deleted. It is not
/// deleted unasked: the damage most likely comes of something outside the programs, such as a
/// network filesystem or a lock that failed to keep another writer out, which the operator should
/// see.
fn index_error(path: &std::path::Path, source: impl Into<redb::Error>) -> Error {
    let source = source.into();
    let path = path.to_owned();
    match &source {
        redb::Error::Corrupted(_) | redb::Error::UpgradeRequired(_) => {
            Error::Damaged { path, source }
        }
        // As an empty file, which no database ever is.
        redb::Error::Io(err) if err.kind() == std::io::ErrorKind::InvalidData => {
            Error::Damaged { path, source }
        }
        _ => Error::Index { path, source },
    }
}

/// Builds the index at `path` from the notes commit `notes`, under a name of its own first, and
/// renames it into place once built: a database that redb creates is an empty file for a moment,
/// which another process that opened it then would take for damaged.
fn build(path: &std::path::Path, notes: Option<&str>) -> Result<(), Error> {
    let building = path.with_file_name(format!(".index.redb.{}", std::process::id()));
    let built = Database::create(&building)
        .map_err(|source| index_error(&building, source))
        .and_then(|db| update(&db, &building, notes))
        .and_then(|()| {
            fs::rename(&building, path).map_err(|source| Error::Replace {
                path: path.to_owned(),
                source,
            })
        });
    if built.is_err() {
        let _ = fs::remove_file(&building);
    }
    built
}

/// The outcome of an attempt to open the index read-only.
enum Opened {
    /// The index is up to date with the notes commit.
    Current(ReadOnlyDatabase),
    /// The index is missing, was left unclean, or was built from another notes commit.
    Stale,
    /// Another process is bringing the index up to date.
    Locked,
}

fn open_current(path: &std::path::Path, notes: Option<&str>) -> Result<Opened, Error> {
    if !path.exists() {
        return Ok(Opened::Stale);
    }
    let db = match ReadOnlyDatabase::open(path) {
        Ok(db) => db,
        Err(DatabaseError::DatabaseAlreadyOpen) => return Ok(Opened::Locked),
        // A writer repairs a database that was not closed cleanly.
        Err(DatabaseError::RepairAborted) => return Ok(Opened::Stale),
        // Deleted since it was found, which the operator may do at any time.
        Err(_) if !path.exists() => return Ok(Opened::Stale),
        Err(source) => return Err(index_error(path, source)),
    };
    let indexed = indexed_notes(&db).map_err(|e| index_error(path, e))?;
    let current = match (indexed.as_deref(), notes) {
        (indexed, notes) if indexed == notes => true,
        // Lines are only ever added to the notes, so an index built from notes that descend from
        // these holds every line of them: a process that read the ref just before another moved
        // it and brought the index up to date uses the index as it is, rather than index the
        // older notes anew.
        (Some(indexed), Some(notes)) => is_ancestor(notes, indexed)?,
        _ => false,
    };
    Ok(if current {
        Opened::Current(db)
    } else {
        Opened::Stale
    })
}

/// Whether the notes commit `ancestor` is `descendant` or one of its ancestors; not when the clone
/// no longer has either, as once garbage collection removed a notes commit indexed, which indexes
/// the notes anew. Any other failure of git is one.
fn is_ancestor(ancestor: &str, descendant: &str) -> Result<bool, Error> {
    let present = runner::commits([ancestor, descendant]).map_err(Error::Notes)?;
    if !present.contains(ancestor) || !present.contains(descendant) {
        return Ok(false);
    }
    runner::is_ancestor(ancestor, descendant).map_err(Error::Notes)
}

/// The notes commit that `db` was built from, if any.
fn indexed_notes(db: &impl ReadableDatabase) -> Result<Option<String>, redb::Error> {
    let txn = db.begin_read()?;
    let meta = match txn.open_table(META_TABLE) {
        Ok(meta) => meta,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    Ok(meta.get(NOTES)?.map(|id| id.value().to_owned()))
}

/// The commit of the notes ref, if it exists.
fn notes_commit() -> Result<Option<String>, Error> {
    let output = runner::run(&mut git([
        "rev-parse",
        "-q",
        "--verify",
        &format!("{REF}^{{commit}}"),
    ]))
    .map_err(Error::Notes)?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((output.status.success() && !id.is_empty()).then_some(id))
}

/// Brings `db` up to date with the notes commit `notes`.
fn update(db: &Database, path: &std::path::Path, notes: Option<&str>) -> Result<(), Error> {
    let err = |source: redb::Error| index_error(path, source);
    let indexed = indexed_notes(db).map_err(err)?;
    if indexed.as_deref() == notes {
        return Ok(());
    }
    let added = match (indexed.as_deref(), notes) {
        (Some(from), Some(to)) => added_since(from, to)?,
        _ => None,
    };
    let (rebuild, changed) = match added {
        Some(changed) => (false, changed),
        None => (true, notes.map(all_notes).transpose()?.unwrap_or_default()),
    };
    let (commits, blobs): (Vec<String>, Vec<String>) = changed.into_iter().unzip();
    let contents = runner::cat_file(&blobs).map_err(Error::Notes)?;

    let txn = db.begin_write().map_err(|e| err(e.into()))?;
    {
        if rebuild {
            txn.delete_multimap_table(REPLACES_TABLE)
                .map_err(|e| err(e.into()))?;
            txn.delete_multimap_table(REPLACED_BY_TABLE)
                .map_err(|e| err(e.into()))?;
        }
        let mut replaces = txn
            .open_multimap_table(REPLACES_TABLE)
            .map_err(|e| err(e.into()))?;
        let mut replaced_by = txn
            .open_multimap_table(REPLACED_BY_TABLE)
            .map_err(|e| err(e.into()))?;
        for (commit, text) in commits.iter().zip(&contents) {
            let text = String::from_utf8_lossy(text.as_deref().unwrap_or_default());
            for old in text.lines().filter_map(|line| line.strip_prefix(REPLACES)) {
                let old = old.trim();
                if old.is_empty() || old == commit {
                    continue;
                }
                replaces
                    .insert(commit.as_str(), old)
                    .map_err(|e| err(e.into()))?;
                replaced_by
                    .insert(old, commit.as_str())
                    .map_err(|e| err(e.into()))?;
            }
        }
        let mut meta = txn.open_table(META_TABLE).map_err(|e| err(e.into()))?;
        match notes {
            Some(notes) => meta.insert(NOTES, notes).map(drop),
            None => meta.remove(NOTES).map(drop),
        }
        .map_err(|e| err(e.into()))?;
    }
    txn.commit().map_err(|e| err(e.into()))
}

/// The notes that were added or changed between the notes commits `from` and `to`, as pairs of
/// the commit annotated and the note's blob, or `None` when the index must be built anew: when
/// `to` does not descend from `from`, or a note was removed.
fn added_since(from: &str, to: &str) -> Result<Option<Vec<(String, String)>>, Error> {
    if !is_ancestor(from, to)? {
        return Ok(None);
    }
    // `:<mode> <mode> <blob> <blob> <status>\0<path>\0` for each note that changed.
    let diff = runner::stdout(&mut git([
        "diff-tree",
        "-r",
        "-z",
        "--no-renames",
        from,
        to,
    ]))
    .map_err(Error::Notes)?;
    // A note that moved to another fanout shows as removed at one path and added at another, and
    // was removed only if it was added nowhere.
    let mut fields = diff.split('\0').filter(|field| !field.is_empty());
    let mut changed = Vec::new();
    let mut removed = BTreeSet::new();
    while let (Some(header), Some(path)) = (fields.next(), fields.next()) {
        let words: Vec<&str> = header.split(' ').collect();
        let (Some(blob), Some(status)) = (words.get(3), words.get(4)) else {
            return Ok(None);
        };
        let commit = path.replace('/', "");
        match *status {
            "A" | "M" => changed.push((commit, (*blob).to_owned())),
            "D" => {
                removed.insert(commit);
            }
            _ => return Ok(None),
        }
    }
    for (commit, _) in &changed {
        removed.remove(commit);
    }
    Ok(removed.is_empty().then_some(changed))
}

/// Every note of the notes commit `notes`, as pairs of the commit annotated and the note's blob.
fn all_notes(notes: &str) -> Result<Vec<(String, String)>, Error> {
    // `<mode> blob <blob>\t<path>\0` for each note.
    let tree = runner::stdout(&mut git(["ls-tree", "-r", "-z", "--full-tree", notes]))
        .map_err(Error::Notes)?;
    Ok(tree
        .split('\0')
        .filter_map(|entry| {
            let (header, path) = entry.split_once('\t')?;
            let blob = header.split(' ').nth(2)?;
            Some((path.replace('/', ""), blob.to_owned()))
        })
        .collect())
}
