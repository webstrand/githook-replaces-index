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

mod error;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::time::Duration;

use runner::git;

pub use crate::error::Error;

/// The notes ref that records the rewrites.
pub const REF: &str = "refs/notes/replaces";

/// The prefix of each line of a note.
pub const REPLACES: &str = "replaces: ";

/// The attempts to write the ref before a writer gives up.
const ATTEMPTS: u32 = 20;

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
}

/// The lines of notes, by the commit they annotate.
type Notes = BTreeMap<String, BTreeSet<String>>;

/// Records `rewrite` in the ref, retrying while other writers move it.
pub fn record(rewrite: &Rewrite) -> Result<(), Error> {
    if rewrite.replacements.is_empty() {
        return Ok(());
    }
    let staging = Staging(format!("refs/replaces-index/record-{}", std::process::id()));
    for n in 1..=ATTEMPTS {
        if attempt(rewrite, &staging.0)? {
            return Ok(());
        }
        // Writers that lost to each other wait for different times before retrying.
        let tenths = (std::process::id() * 7 + n * 13) % 9 + 1;
        std::thread::sleep(Duration::from_millis(u64::from(tenths) * 100));
    }
    Err(Error::KeptMoving)
}

/// The ref that a writer writes its commit to before it moves the notes ref, removed when the
/// writer is done.
struct Staging(String);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = runner::run(&mut git(["update-ref", "-d", &self.0]));
    }
}

/// Writes the notes once. Returns false when the ref moved after it was read.
fn attempt(rewrite: &Rewrite, staging: &str) -> Result<bool, Error> {
    let parent = parent()?;
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
        return Ok(true);
    }

    let ident = runner::stdout(&mut git(["var", "GIT_COMMITTER_IDENT"])).map_err(Error::Write)?;
    let stream = stream(staging, ident.trim_end(), parent.as_deref(), &changed);
    runner::stdout_with_input(
        &mut git(["fast-import", "--quiet", "--force"]),
        stream.as_bytes(),
    )
    .map_err(Error::Write)?;
    let commit = runner::stdout(&mut git([
        "rev-parse",
        "-q",
        "--verify",
        &format!("{staging}^{{commit}}"),
    ]))
    .map_err(Error::Write)?;
    // An empty old value requires that the ref does not exist yet.
    let moved = runner::run(&mut git([
        "update-ref",
        "-m",
        "replaces-index: record",
        REF,
        commit.trim_end(),
        parent.as_deref().unwrap_or(""),
    ]))
    .map_err(Error::Write)?;
    Ok(moved.status.success())
}

/// The commit that the ref points to, if it exists.
fn parent() -> Result<Option<String>, Error> {
    let output = runner::run(&mut git([
        "rev-parse",
        "-q",
        "--verify",
        &format!("{REF}^{{commit}}"),
    ]))
    .map_err(Error::Read)?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((output.status.success() && !id.is_empty()).then_some(id))
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
