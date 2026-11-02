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
//! The commit up to which the reflog was checked is kept in the ref `refs/replaces-index/checked`,
//! with the time of its entry, so that each step is checked once, and so that garbage collection
//! keeps the commit: once the
//! reflog no longer holds it, the step from it to the oldest commit the reflog holds is checked
//! as one. A clone that keeps no reflog of the notes ref, as a bare one by default, is not checked.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use runner::git;
use runner::refspec::Refspec;

use crate::{Change, Error, Notes, REF, REPLACES, all_notes, is_id, lines, notes_commit, write};

/// The ref that holds the commit up to which the reflog was checked.
const CHECKED: &str = "refs/replaces-index/checked";

/// The ref that holds a blob of the time of the entry of the reflog up to which it was checked,
/// which tells that entry from a later one that moved the ref back to the same commit.
const CHECKED_AT: &str = "refs/replaces-index/checked-at";

/// What a repair found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Repaired {
    /// The steps of the reflog, from one commit to the next, that dropped lines of notes of
    /// commits the clone has, or that no note names; their lines are back.
    pub steps: Vec<(String, String)>,
    /// Each commit that the clone no longer has whose note lost lines, with the commits whose
    /// notes name it, which now hold them.
    pub folded: Vec<(String, Vec<String>)>,
}

/// Checks the steps of the reflog of the notes ref since the last check, and writes back the
/// lines that any of them dropped, or adds them to the notes that name a commit the clone lacks.
pub fn repair() -> Result<Repaired, Error> {
    let checked = read_ref(CHECKED)?;
    let checked_at: Option<u64> = match read_ref(CHECKED_AT)? {
        Some(blob) => runner::cat_file(&[blob])
            .map_err(Error::Read)?
            .pop()
            .flatten()
            .and_then(|time| String::from_utf8(time).ok()?.trim().parse().ok()),
        None => None,
    };
    let Some(current) = notes_commit()? else {
        return Ok(Repaired::default());
    };
    // The commits the ref was moved to, newest first, back to the one checked last. Each entry is
    // `<id> <ref>@{<time>}`.
    let out = runner::stdout(&mut git([
        "reflog",
        "show",
        "--date=unix",
        "--format=%H %gd",
        REF,
    ]))
    .map_err(Error::Read)?;
    let mut states: Vec<&str> = Vec::new();
    let mut newest_at = None;
    let mut found = false;
    for line in out.lines() {
        let Some((id, entry)) = line.split_once(' ') else {
            continue;
        };
        let at: Option<u64> = entry
            .rsplit_once("@{")
            .and_then(|(_, time)| time.strip_suffix('}')?.parse().ok());
        if states.is_empty() {
            newest_at = at;
        }
        states.push(id);
        // A check that kept no time is that of an entry of the same commit.
        if Some(id) == checked.as_deref() && (checked_at.is_none() || checked_at == at) {
            found = true;
            break;
        }
    }
    let Some(&newest) = states.first() else {
        return Ok(Repaired::default());
    };
    // The steps from the commit checked last to the oldest that the reflog holds expired; they are
    // checked as one.
    if !found && let Some(checked) = checked.as_deref() {
        states.push(checked);
    }
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
    let mut update = format!("update {CHECKED} {newest}\n");
    if let Some(at) = newest_at {
        let blob = runner::stdout_with_input(
            &mut git(["hash-object", "-w", "--stdin"]),
            at.to_string().as_bytes(),
        )
        .map_err(Error::Write)?;
        update.push_str(&format!("update {CHECKED_AT} {}\n", blob.trim()));
    }
    runner::stdout_with_input(&mut git(["update-ref", "--stdin"]), update.as_bytes())
        .map_err(Error::Write)?;
    Ok(repaired)
}

/// The object that the ref `name` points to, if it exists.
fn read_ref(name: &str) -> Result<Option<String>, Error> {
    let mut rev_parse = git(["rev-parse", "--verify", "--quiet", name]);
    let output = runner::run(&mut rev_parse).map_err(Error::Read)?;
    // rev-parse --verify --quiet exits with 1 when the ref does not exist.
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    runner::check(&rev_parse, &output).map_err(Error::Read)?;
    let id = runner::text(&rev_parse, output.stdout).map_err(Error::Read)?;
    Ok(Some(id.trim().to_owned()))
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
        let notes = notes.as_ref().expect("read for a commit the clone lacks");
        let mut lacked = false;
        for namer in &naming {
            // A commit that a cycle of rewrites names does not come to replace itself.
            let own = format!("{REPLACES}{namer}").into_bytes();
            let held = &notes[namer];
            let missing: Vec<_> = lines
                .iter()
                .filter(|line| **line != own && !held.contains(*line))
                .cloned()
                .collect();
            lacked |= !missing.is_empty();
            additions.entry(namer.clone()).or_default().extend(missing);
        }
        // A removal that folded the note into its namers first, as gc's does, lost nothing.
        if lacked {
            folded.push((commit, naming));
        }
    }
    additions.retain(|_, lines| !lines.is_empty());
    Ok((additions, restored))
}

/// The lines of the notes of the notes commit `before` that the notes commit `after` lacks, by
/// the commit whose note held them.
fn dropped(before: &str, after: &str) -> Result<Notes, Error> {
    // A note that moved to another fanout, as every note does once their number crosses a power
    // of 256, shows as removed at one path and added at another, and is compared by the commit it
    // annotates.
    let mut olds = BTreeMap::new();
    let mut news = BTreeMap::new();
    for file in runner::diff_tree(before, after).map_err(Error::Read)? {
        // A file whose path spells no commit id is no note, and holds no lines to drop.
        let commit = file.path.replace('/', "");
        if !is_id(&commit) {
            continue;
        }
        if let Some(old) = file.old {
            olds.insert(commit.clone(), old);
        }
        if let Some(new) = file.new {
            news.insert(commit, new);
        }
    }
    let (commits, blobs): (Vec<&String>, Vec<&str>) = olds
        .iter()
        .flat_map(|(commit, old)| {
            let new = news.get(commit).map(String::as_str);
            [(commit, old.as_str())]
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

/// Says on standard error, after `prefix`, each fetch refspec that forces the notes ref, with the
/// configuration that fetches the remote's notes beside the local ones instead. False when it
/// named any, or could not read the refspecs.
pub fn check_refspecs(prefix: &str) -> bool {
    let mut stderr = io::stderr();
    let forcing = match runner::refspec::forcing(REF) {
        Ok(forcing) => forcing,
        Err(err) => {
            let _ = writeln!(stderr, "{prefix}: could not read the fetch refspecs: {err}");
            return false;
        }
    };
    for refspec in &forcing {
        let _ = writeln!(
            stderr,
            "{prefix}: remote.{}.fetch {} replaces {REF} on each fetch, dropping what this clone recorded since; {}",
            refspec.remote,
            refspec.text,
            instead(refspec)
        );
    }
    forcing.is_empty()
}

/// How the forcing `refspec` comes to fetch the remote's notes beside the local ones: with its
/// destinations under refs/notes/remotes/<remote>/, where `git pull-replaces` merges them from.
fn instead(refspec: &Refspec) -> String {
    let remote = &refspec.remote;
    match refspec
        .dst
        .as_deref()
        .and_then(|dst| dst.strip_prefix("refs/notes/"))
    {
        Some(rest) => format!(
            "to fetch the remote's notes beside this clone's: git config --replace-all --fixed-value {} {} {}",
            runner::quote(format!("remote.{remote}.fetch").as_ref()).display(),
            runner::quote(format!("+{}:refs/notes/remotes/{remote}/{rest}", refspec.src).as_ref())
                .display(),
            runner::quote(refspec.text.as_ref()).display(),
        ),
        None => format!(
            "it must fetch {} into another ref, such as refs/notes/remotes/{remote}/replaces",
            refspec.source_of(REF).unwrap_or_default()
        ),
    }
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
