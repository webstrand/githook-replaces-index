//! `git compare-replaces <old> <new>`: prints, for each commit of `old` that `new` lacks, oldest
//! first, what `new` holds of it, one line each:
//!
//! - `held <commit> as <commits>`: `new` holds commits made from it;
//! - `later <commit> than <commits>`: `new` holds commits it was made from, and `old` a later
//!   version of them;
//! - `rewritten <commit> as <commits>`: it was made into commits that `new` lacks;
//! - `missing <commit>`: nothing related to it is in `new`.
//!
//! Each line ends with the subject of the commit. Nothing of `old` is lost in `new` when every
//! line is a `held` one.

use std::process::ExitCode;

use clap::Parser;
use replaces_index_compare::{Status, compare, describe};

#[derive(Parser)]
#[command(name = "git compare-replaces", about)]
struct Cli {
    /// The chain whose commits are looked for.
    old: String,
    /// The chain they are looked for in.
    new: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !replaces_index_record::check_refspecs("compare-replaces") {
        return ExitCode::FAILURE;
    }
    let entries = match compare(&cli.old, &cli.new) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("compare-replaces: {err}");
            return ExitCode::FAILURE;
        }
    };
    let described = match describe(entries.iter().map(|entry| entry.commit.as_str())) {
        Ok(described) => described,
        Err(err) => {
            eprintln!("compare-replaces: {err}");
            return ExitCode::FAILURE;
        }
    };
    for entry in entries {
        let subject = described
            .get(&entry.commit)
            .map_or("", |described| described.subject.as_str());
        let commit = &entry.commit;
        let line = match &entry.status {
            Status::Held(by) => format!("held {commit} as {}", by.join(" ")),
            Status::Later(than) => format!("later {commit} than {}", than.join(" ")),
            Status::Rewritten(as_) => format!("rewritten {commit} as {}", as_.join(" ")),
            Status::Missing => format!("missing {commit}"),
        };
        println!("{line}: {subject}");
    }
    ExitCode::SUCCESS
}
