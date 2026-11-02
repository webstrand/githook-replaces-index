//! Puts back the lines that a writer dropped from the notes. A recorded rewrite holds for as long
//! as the commits do, so a note of a commit the clone has never loses a line: a step of the reflog
//! of the notes ref after which such a note lacks a line it held was made by a writer that
//! overwrote another, such as a fetch through `+refs/notes/*:refs/notes/*`, `git notes add -f`
//! or a `git notes merge` that keeps one side, and the line is written back.
//!
//! A note of a commit that the clone no longer has, once garbage collection removed it, may go,
//! as `git notes prune` removes it: its lines are then added to the notes that name the commit,
//! those of the commits made from it, so that what it recorded is kept. A note that no other
//! names is the only record of what it holds, and is written back as well.
//!
//! The commit up to which the reflog was checked is kept in `replaces-checked` in the common git
//! directory, so that each step is checked once. A clone that keeps no reflog of the notes ref,
//! as a bare one by default, is not checked.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use runner::git;

use crate::{Change, Error, Notes, REF, REPLACES, all_notes, is_id, lines, notes_commit, write};

/// The file of the common git directory that holds the commit up to which the reflog was checked.
const CHECKED: &str = "replaces-checked";

/// What a repair found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Repaired {
    /// The steps of the reflog, from one commit to the next, that dropped lines of notes of
    /// commits the clone has, or that no note names; their lines are back.
    pub steps: Vec<(String, String)>,
    /// Each commit that the clone no longer has whose note lost lines, with the commits whose notes
    /// name it, which now hold them.
    pub folded: Vec<(String, Vec<String>)>,
}

/// Checks the steps of the reflog of the notes ref since the last check, and writes back the
/// lines that any of them dropped, or adds them to the notes that name a commit the clone lacks.
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
    let Some(current) = notes_commit()? else {
        return Ok(Repaired::default());
    };
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
    // The lines that each step dropped, by the commit whose note held them.
    let mut steps: Vec<(&str, &str, Notes)> = Vec::new();
    for step in states.windows(2).rev() {
        let (before, after) = (step[1], step[0]);
        let lost = dropped(before, after)?;
        if !lost.is_empty() {
            steps.push((before, after, lost));
        }
    }
    let mut repaired = Repaired::default();
    if !steps.is_empty() {
        let (additions, restored) = placed(&current, &steps, &mut repaired.folded)?;
        repaired.steps = steps
            .iter()
            .filter(|(_, _, lost)| lost.keys().any(|commit| restored.contains(commit)))
            .map(|(before, after, _)| ((*before).to_owned(), (*after).to_owned()))
            .collect();
        if !additions.is_empty() {
            write(Change::Add(&additions), || {})?;
        }
    }
    fs::write(&checked_path, format!("{newest}\n")).map_err(|source| Error::Checked {
        path: checked_path,
        source,
    })?;
    Ok(repaired)
}

/// Where the lines that `steps` dropped go, as lines to add by commit, and the commits whose own
/// notes get theirs back: a commit the clone has, or one that no note of the notes commit
/// `current` names. The lines of a note of any other commit go to the notes that name it, which
/// `folded` is told of.
fn placed(
    current: &str,
    steps: &[(&str, &str, Notes)],
    folded: &mut Vec<(String, Vec<String>)>,
) -> Result<(Notes, BTreeSet<String>), Error> {
    let mut lost = Notes::new();
    for (_, _, dropped) in steps {
        for (commit, lines) in dropped {
            lost.entry(commit.clone())
                .or_default()
                .extend(lines.iter().cloned());
        }
    }
    let present = runner::commits(lost.keys().map(String::as_str)).map_err(Error::Read)?;
    // Read only when a note of a commit the clone lacks lost lines, which is rare.
    let mut notes: Option<Notes> = None;
    let mut additions = Notes::new();
    let mut restored = BTreeSet::new();
    for (commit, lines) in lost {
        let naming: Vec<String> = if present.contains(&commit) {
            Vec::new()
        } else {
            let notes = match &mut notes {
                Some(notes) => notes,
                None => notes.insert(all_notes(current)?),
            };
            let line = format!("{REPLACES}{commit}").into_bytes();
            notes
                .iter()
                .filter(|(_, note)| note.contains(&line))
                .map(|(namer, _)| namer.clone())
                .collect()
        };
        if naming.is_empty() {
            additions.entry(commit.clone()).or_default().extend(lines);
            restored.insert(commit);
            continue;
        }
        for namer in &naming {
            // A commit that a cycle of rewrites names does not come to replace itself.
            let own = format!("{REPLACES}{namer}").into_bytes();
            additions
                .entry(namer.clone())
                .or_default()
                .extend(lines.iter().filter(|line| **line != own).cloned());
        }
        folded.push((commit, naming));
    }
    additions.retain(|_, lines| !lines.is_empty());
    Ok((additions, restored))
}

fn checked_path() -> Result<PathBuf, Error> {
    let common = runner::rev_parse_path(&["--path-format=absolute", "--git-common-dir"])
        .map_err(Error::Read)?;
    Ok(common.join(CHECKED))
}

/// The lines of the notes of the notes commit `before` that the notes commit `after` lacks, by
/// the commit whose note held them.
fn dropped(before: &str, after: &str) -> Result<Notes, Error> {
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
    let mut olds = BTreeMap::new();
    let mut news = BTreeMap::new();
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
        if matches!(status, "D" | "M") {
            olds.insert(commit.clone(), old);
        }
        if matches!(status, "A" | "M") {
            news.insert(commit, new);
        }
    }
    let (commits, blobs): (Vec<&String>, Vec<&str>) = olds
        .iter()
        .flat_map(|(commit, old)| {
            let new = news.get(commit).copied();
            [(commit, *old)]
                .into_iter()
                .chain(new.map(|new| (commit, new)))
        })
        .unzip();
    let contents = runner::cat_file(&blobs).map_err(Error::Read)?;
    let mut contents = commits.into_iter().zip(contents);
    let mut lost = Notes::new();
    while let Some((commit, old)) = contents.next() {
        let old = lines(&old.unwrap_or_default());
        let new = if news.contains_key(commit) {
            contents
                .next()
                .map(|(_, new)| lines(&new.unwrap_or_default()))
                .unwrap_or_default()
        } else {
            BTreeSet::new()
        };
        let gone: BTreeSet<Vec<u8>> = old.difference(&new).cloned().collect();
        if !gone.is_empty() {
            lost.insert(commit.clone(), gone);
        }
    }
    Ok(lost)
}

/// Says on standard error, after `prefix`, which fetch refspecs move the ref whether or not it
/// fast-forwards, as `+refs/notes/*:refs/notes/*` does, and so drop on each fetch what the clone
/// recorded since, and how to keep the ref out of them; a failure to read them is said as well.
pub fn warn_forcing(prefix: &str) {
    let mut stderr = io::stderr();
    match runner::refspec::forcing(REF) {
        Ok(forcing) => {
            for refspec in forcing {
                let src = refspec.source_of(REF).unwrap_or_default();
                let _ = writeln!(
                    stderr,
                    "{prefix}: remote.{}.fetch {} replaces {REF} on each fetch, dropping what this clone recorded since; to keep {REF} out of it: git config --add {} {}",
                    refspec.remote,
                    refspec.text,
                    shell_quote(&format!("remote.{}.fetch", refspec.remote)),
                    shell_quote(&format!("^{src}"))
                );
            }
        }
        Err(err) => {
            let _ = writeln!(stderr, "{prefix}: could not read the fetch refspecs: {err}");
        }
    }
}

/// `word` as the shell reads it back.
fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Repairs the notes, and says on standard error, after `prefix`, what the repair put back, or
/// why it could not check them; a failure to check does not stop the program. It then says which
/// fetch refspecs would drop lines again, as `warn_forcing` does.
pub fn repair_and_say(prefix: &str) {
    warn_forcing(prefix);
    let mut stderr = io::stderr();
    match repair() {
        Ok(repaired) => {
            for (before, after) in &repaired.steps {
                let _ = writeln!(
                    stderr,
                    "{prefix}: a writer dropped lines from {REF} when it moved it from {before} to {after}; they are put back"
                );
            }
            for (commit, naming) in &repaired.folded {
                let _ = writeln!(
                    stderr,
                    "{prefix}: the note of {commit}, which this clone no longer has, lost lines; the notes of {}, which name it, hold them",
                    naming.join(" ")
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
