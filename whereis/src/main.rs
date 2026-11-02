//! `git whereis <commit>`: prints the commits related to a commit by rewrites, one per line, as
//! `<relation> <commit> <where>`. The relation is `from` for a commit it was made from, `self`
//! for the commit itself, and `into` for a commit made from it; where it is, the refs of the clone
//! that hold the commit, `(no ref)` when none does, or `(not in this clone)`.

use std::process::ExitCode;

use clap::Parser;
use replaces_index_whereis::{Held, whereis};

#[derive(Parser)]
#[command(name = "git whereis", about)]
struct Cli {
    /// The commit, or a prefix of the id of a commit that the clone lacks.
    commit: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !replaces_index_record::check_refspecs("whereis") {
        return ExitCode::FAILURE;
    }
    match whereis(&cli.commit) {
        Ok(related) => {
            for r in related {
                let held = match r.held {
                    Held::On(refs) => refs.join(" "),
                    Held::Unreferenced => "(no ref)".to_owned(),
                    Held::Missing => "(not in this clone)".to_owned(),
                };
                println!("{} {} {held}", r.relation.label(), r.commit);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            replaces_index_record::notice::say("whereis", &err);
            ExitCode::FAILURE
        }
    }
}
