//! Records in refs/notes/replaces the replacements that git filter-repo made, since filter-repo
//! does not run post-rewrite. After each run, filter-repo leaves in filter-repo/commit-map in the
//! git directory a header line and one `<old> <new>` line per commit it processed, with the zero
//! id as the new commit of a commit it deleted. The map is passed to post-rewrite without the
//! header and the deleted commits. Running this again records nothing new.
//!
//! A run on a repository that filter-repo already filtered composes the map with the previous
//! one, so that it maps each original commit to its newest replacement and no longer names the
//! result of the previous run. A copy of the map is therefore kept beside it once recorded, and
//! for each original commit that the previous run replaced, the commit that replaced it is
//! recorded as replaced as well.

mod error;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::process::{self, ExitCode};
use std::sync::Mutex;

use clap::Parser;
use replaces_index_post_rewrite::{Kind, post_rewrite};
use replaces_index_record::heartbeat;

use crate::error::Error;

#[derive(Parser)]
#[command(name = "record-filter-repo", about)]
struct Cli {}

fn main() -> ExitCode {
    Cli::parse();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("replaces-index: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Error> {
    let dir = runner::rev_parse_path(&["--git-dir"])
        .map_err(Error::GitDir)?
        .join("filter-repo");
    let map_path = dir.join("commit-map");
    let prev_path = dir.join("replaces-index-commit-map");
    let map = match fs::read_to_string(&map_path) {
        Ok(map) => map,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(Error::NoMap(map_path)),
        Err(source) => {
            return Err(Error::Read {
                path: map_path,
                source,
            });
        }
    };
    let prev = read_or_empty(&prev_path)?;

    let replacements = replacements(&map, &prev);
    heartbeat::while_running("replaces-index", "Ctrl-C to give up", || {
        post_rewrite(Kind::FilterRepo, &replacements, &Mutex::new(None))
    })
    .map_err(|failure| Error::Record(failure.error))?;
    // The copy is kept only once the map is recorded, so that a failed run is retried in full, and
    // through a rename, so that a run that ends while writing it leaves the previous copy whole.
    keep(&prev_path, map.as_bytes()).map_err(|source| Error::Keep {
        path: prev_path,
        source,
    })?;
    Ok(())
}

fn keep(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = path.with_file_name(format!(".replaces-index-commit-map.{}", process::id()));
    let kept = write(&temporary, contents).and_then(|()| fs::rename(&temporary, path));
    if kept.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    kept
}

fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

fn read_or_empty(path: &Path) -> Result<String, Error> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(Error::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

/// The replacements of `map`, as post-rewrite reads them, given the map of the previous run.
fn replacements(map: &str, prev: &str) -> String {
    let deleted = |id: &str| !id.is_empty() && id.bytes().all(|b| b == b'0');
    let was: HashMap<&str, &str> = prev
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            Some((words.next()?, words.next().unwrap_or("")))
        })
        .collect();
    let mut out = String::new();
    for line in map.lines().skip(1) {
        let mut words = line.split_whitespace();
        let (old, new) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        if deleted(new) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
        if let Some(&previous) = was.get(old)
            && previous != new
            && !deleted(previous)
        {
            out.push_str(&format!("{previous} {new}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::replacements;

    const ZERO: &str = "0000000000000000000000000000000000000000";

    #[test]
    fn leaves_out_the_header_and_the_deleted_commits() {
        let map = format!("old new\naaa bbb\nccc {ZERO}\nddd ddd\n");
        assert_eq!(replacements(&map, ""), "aaa bbb\nddd ddd\n");
    }

    #[test]
    fn records_that_the_result_of_the_previous_run_was_replaced() {
        let prev = "old new\naaa bbb\nccc ddd\n";
        let map = "old new\naaa eee\nccc ddd\n";
        assert_eq!(replacements(map, prev), "aaa eee\nbbb eee\nccc ddd\n");
    }

    #[test]
    fn ignores_a_commit_the_previous_run_deleted() {
        let prev = format!("old new\naaa {ZERO}\n");
        assert_eq!(replacements("old new\naaa bbb\n", &prev), "aaa bbb\n");
    }
}
