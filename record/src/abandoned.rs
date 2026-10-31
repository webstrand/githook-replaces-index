//! Rewrites that could not be recorded, each kept as a file of `<old> <new>` lines in
//! `replaces-abandoned/` in the common git directory, so that they may be recorded later.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Rewrite;

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

/// A shell command that records `rewrite` by hand, from inside the repository.
pub fn command(rewrite: &Rewrite) -> String {
    let lines: Vec<String> = rewrite
        .replacements
        .iter()
        .map(|(old, new)| format!("'{old} {new}'"))
        .collect();
    format!(
        "printf '%s\\n' {} | \"$(git rev-parse --git-common-dir)\"/bin/replaces-index-record",
        lines.join(" ")
    )
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
