//! Rewrites that could not be recorded, each kept as a file of `<old> <new>` lines in
//! `replaces-abandoned/` in the common git directory, so that they may be recorded later.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Error, Rewrite, is_id};

/// The directory of the common git directory that holds the abandoned rewrites.
pub const DIR: &str = "replaces-abandoned";

/// The directory that holds the abandoned rewrites of the repository the process runs in.
pub fn dir() -> Result<PathBuf, runner::Error> {
    let common = runner::rev_parse_path(&["--path-format=absolute", "--git-common-dir"])?;
    Ok(common.join(DIR))
}

/// Keeps `rewrite` in a file of its own in `dir`, and returns its path. The file is written under
/// a temporary name and renamed, so that it either holds the whole rewrite or does not exist.
pub fn keep(dir: &Path, rewrite: &Rewrite) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let name = name();
    let temporary = dir.join(format!(".{name}.tmp"));
    let path = dir.join(&name);
    let kept =
        write(&temporary, rewrite.text().as_bytes()).and_then(|()| fs::rename(&temporary, &path));
    if kept.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    kept.map(|()| path)
}

fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = File::create_new(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// A name that sorts by the time it was made, and that no other process makes.
fn name() -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let random = crate::random();
    format!(
        "{:012}.{:09}-{random:016x}",
        since.as_secs(),
        since.subsec_nanos()
    )
}

/// The program that records rewrites, as a shell names it from inside the repository.
const RECORD: &str = "\"$(git rev-parse --git-common-dir)\"/bin/replaces-index-record";

/// A shell command that records `rewrite` by hand, from inside the repository.
pub fn command(rewrite: &Rewrite) -> String {
    let lines: Vec<String> = rewrite
        .replacements
        .iter()
        .map(|(old, new)| format!("'{old} {new}'"))
        .collect();
    format!("printf '%s\\n' {} | {RECORD}", lines.join(" "))
}

/// A shell command that records every abandoned record, from inside the repository.
pub fn command_all() -> String {
    format!("{RECORD} --abandoned")
}

/// The abandoned records in a directory, read to be recorded.
#[derive(Debug, Default)]
pub struct Waiting {
    /// The records read, to be removed once recorded.
    pub files: Vec<PathBuf>,
    /// Their replacements, oldest record first, without those that cannot be recorded.
    pub rewrite: Rewrite,
    /// Why lines of the records were left out.
    pub dropped: Vec<String>,
}

impl Waiting {
    /// Removes the records read, once their replacements are recorded. A record that another
    /// process removed already is not missed.
    pub fn remove(&self) -> Result<(), Error> {
        for path in &self.files {
            match fs::remove_file(path) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => {
                    return Err(Error::Abandoned {
                        path: path.clone(),
                        source: err,
                    });
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// The abandoned records in `dir`, oldest first. A file whose name begins with `.` is one being
/// written, and is left alone. Lines that are no replacement, and replacements whose new commit
/// the clone lacks, which could never be recorded, are left out. A record that cannot be read, as
/// a directory or a file without read permission, is left out where it is, so that it keeps no
/// other record from being recorded.
pub fn waiting(dir: &Path) -> Result<Waiting, Error> {
    let to_error = |source| Error::Abandoned {
        path: dir.to_owned(),
        source,
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Waiting::default()),
        Err(err) => return Err(to_error(err)),
    };
    let mut names = Vec::new();
    let mut waiting = Waiting::default();
    for entry in entries {
        let name = entry.map_err(to_error)?.file_name();
        match name.to_str() {
            Some(name) if name.starts_with('.') => {}
            Some(name) => names.push(name.to_owned()),
            // It is left where it is.
            None => waiting.dropped.push(format!(
                "{}, whose name is not UTF-8, is no abandoned record",
                dir.join(&name).display()
            )),
        }
    }
    names.sort();
    for name in names {
        let path = dir.join(&name);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            // Another process recorded it and removed it in the meantime.
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                waiting
                    .dropped
                    .push(format!("{} could not be read: {err}", path.display()));
                continue;
            }
        };
        let mut lines = Vec::new();
        for line in bytes.split(|&b| b == b'\n') {
            match std::str::from_utf8(line) {
                Ok(line) => lines.push(line),
                Err(_) => waiting.dropped.push(format!(
                    "{}: `{}` is not UTF-8, and so no replacement",
                    path.display(),
                    line.escape_ascii()
                )),
            }
        }
        for line in lines.into_iter().filter(|line| !line.trim().is_empty()) {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words[..] {
                [old, new] if is_id(old) && is_id(new) => {
                    waiting
                        .rewrite
                        .replacements
                        .push((old.to_owned(), new.to_owned()));
                }
                _ => waiting
                    .dropped
                    .push(format!("{}: `{line}` is not a replacement", path.display())),
            }
        }
        waiting.files.push(path);
    }
    let news: Vec<&str> = waiting
        .rewrite
        .replacements
        .iter()
        .map(|(_, new)| new.as_str())
        .collect();
    let commits = runner::commits(news).map_err(Error::Read)?;
    waiting.rewrite.replacements.retain(|(old, new)| {
        let kept = commits.contains(new);
        if !kept {
            waiting.dropped.push(format!(
                "{new} is not in this clone, so `{old} {new}` cannot be recorded"
            ));
        }
        kept
    });
    Ok(waiting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_rewrite_reads_back_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let rewrite = Rewrite::parse("a b\nc d\n");
        let path = keep(dir.path(), &rewrite).unwrap();
        let text = fs::read_to_string(path).unwrap();
        assert_eq!(Rewrite::parse(&text), rewrite);
        // Nothing but the record is left in the directory.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn records_kept_one_after_the_other_sort_in_that_order() {
        let dir = tempfile::tempdir().unwrap();
        let first = keep(dir.path(), &Rewrite::parse("a b\n")).unwrap();
        let second = keep(dir.path(), &Rewrite::parse("c d\n")).unwrap();
        assert!(first.file_name() < second.file_name());
    }

    #[test]
    fn the_command_prints_each_line_of_the_rewrite() {
        let rewrite = Rewrite::parse("a b\nc d\n");
        assert_eq!(
            command(&rewrite),
            "printf '%s\\n' 'a b' 'c d' | \"$(git rev-parse --git-common-dir)\"/bin/replaces-index-record"
        );
    }
}
