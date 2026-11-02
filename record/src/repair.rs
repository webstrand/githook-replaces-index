//! Puts back the lines that a writer dropped from the notes. Lines are only ever added to them, so
//! a step of the reflog of the notes ref after which a note lacks a line it held was made by a
//! writer that overwrote another, such as `git notes add` in a git that moves the ref without
//! naming the commit it read, or by hand. The lines are written back as `record` writes.
//!
//! The commit up to which the reflog was checked is kept in `replaces-checked` in the common git
//! directory, so that each step is checked once. A clone that keeps no reflog of the notes ref,
//! as a bare one by default, is not checked.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use runner::git;

use crate::{Change, Error, REF, is_id, lines, notes_commit, write};

/// The file of the common git directory that holds the commit up to which the reflog was checked.
const CHECKED: &str = "replaces-checked";

/// What a repair found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Repaired {
    /// The steps of the reflog, from one commit to the next, that dropped lines; their lines are
    /// back.
    pub steps: Vec<(String, String)>,
}

/// Checks the steps of the reflog of the notes ref since the last check, and writes back the
/// lines that any of them dropped.
pub fn repair() -> Result<Repaired, Error> {
    let checked_path = checked_path()?;
    let checked = match fs::read_to_string(&checked_path) {
        Ok(id) => Some(id.trim().to_owned()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(Error::Checked {
                path: checked_path,
                source,
            });
        }
    };
    if notes_commit()?.is_none() {
        return Ok(Repaired::default());
    }
    // The commits the ref was moved to, newest first, back to the one checked last.
    let out =
        runner::stdout(&mut git(["reflog", "show", "--format=%H", REF])).map_err(Error::Read)?;
    let mut states: Vec<&str> = Vec::new();
    for id in out.lines() {
        states.push(id);
        if Some(id) == checked.as_deref() {
            break;
        }
    }
    let Some(&newest) = states.first() else {
        return Ok(Repaired::default());
    };
    let mut repaired = Repaired::default();
    for step in states.windows(2).rev() {
        let (before, after) = (step[1], step[0]);
        if drops_lines(before, after)? {
            repaired.steps.push((before.to_owned(), after.to_owned()));
        }
    }
    for (before, _) in &repaired.steps {
        write(Change::Restore(before), || {})?;
    }
    fs::write(&checked_path, format!("{newest}\n")).map_err(|source| Error::Checked {
        path: checked_path,
        source,
    })?;
    Ok(repaired)
}

fn checked_path() -> Result<PathBuf, Error> {
    let common = runner::rev_parse_path(&["--path-format=absolute", "--git-common-dir"])
        .map_err(Error::Read)?;
    Ok(common.join(CHECKED))
}

/// Whether a note of the notes commit `before` lacks a line in the notes commit `after`.
fn drops_lines(before: &str, after: &str) -> Result<bool, Error> {
    let out = runner::stdout(&mut git([
        "diff-tree",
        "-r",
        "-z",
        "--no-renames",
        before,
        after,
    ]))
    .map_err(Error::Read)?;
    // `:<mode> <mode> <blob> <blob> <status>\0<path>\0` for each file that changed. A note that
    // moved to another fanout, as every note does once their number crosses a power of 256, shows
    // as removed at one path and added at another, and is compared by the commit it annotates.
    let mut fields = out.split('\0');
    let mut pairs = Vec::new();
    let mut removed = BTreeMap::new();
    let mut added = BTreeMap::new();
    while let (Some(meta), Some(path)) = (fields.next(), fields.next()) {
        let words: Vec<&str> = meta.split(' ').collect();
        let [_, _, old, new, status] = words[..] else {
            continue;
        };
        let commit = path.replace('/', "");
        // A file whose path spells no commit id is no note, and holds no lines to drop.
        if !is_id(&commit) {
            continue;
        }
        match status {
            "D" => {
                removed.insert(commit, old);
            }
            "A" => {
                added.insert(commit, new);
            }
            "M" => pairs.push((old, new)),
            _ => {}
        }
    }
    for (commit, old) in removed {
        match added.get(&commit) {
            Some(&new) => pairs.push((old, new)),
            None => return Ok(true),
        }
    }
    let blobs: Vec<&str> = pairs.iter().flat_map(|(old, new)| [*old, *new]).collect();
    let contents = runner::cat_file(&blobs).map_err(Error::Read)?;
    let lines = |blob: &Option<Vec<u8>>| lines(blob.as_deref().unwrap_or_default());
    Ok(contents
        .chunks(2)
        .any(|pair| !lines(&pair[0]).is_subset(&lines(&pair[1]))))
}

/// Repairs the notes, and says on standard error, after `prefix`, what the repair put back, or
/// why it could not check them; a failure to check does not stop the program.
pub fn repair_and_say(prefix: &str) {
    let mut stderr = io::stderr();
    match repair() {
        Ok(repaired) => {
            for (before, after) in &repaired.steps {
                let _ = writeln!(
                    stderr,
                    "{prefix}: a writer dropped lines from {REF} when it moved it from {before} to {after}; they are put back"
                );
            }
        }
        Err(err) => {
            let _ = writeln!(
                stderr,
                "{prefix}: could not check {REF} for lines that a writer dropped: {err}"
            );
        }
    }
}
