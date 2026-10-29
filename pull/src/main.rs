//! `git pull-replaces <remote>`: merges a remote's replaces notes into the local ones.

use std::process::ExitCode;

use clap::Parser;
use replaces_index_pull::{Pulled, pull};
use replaces_index_record::REF;

#[derive(Parser)]
#[command(name = "git pull-replaces", about)]
struct Cli {
    /// The remote whose notes to merge.
    remote: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match pull(&cli.remote) {
        Ok(Pulled::Merged) => ExitCode::SUCCESS,
        Ok(Pulled::Absent) => {
            eprintln!("pull-replaces: {} has no {REF}", cli.remote);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("pull-replaces: {err}");
            ExitCode::FAILURE
        }
    }
}
