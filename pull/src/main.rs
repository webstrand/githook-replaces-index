//! `git pull-replaces <remote>`: merges a remote's replaces notes into the local ones.

use std::process::ExitCode;

use clap::Parser;
use replaces_index_pull::{Pulled, pull};
use replaces_index_record::{REF, heartbeat};

#[derive(Parser)]
#[command(name = "git pull-replaces", about)]
struct Cli {
    /// The remote whose notes to merge: a configured remote's name, or a URL or path.
    remote: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if !replaces_index_record::check_refspecs("pull-replaces") {
        return ExitCode::FAILURE;
    }
    let pulled =
        heartbeat::while_running("pull-replaces", "Ctrl-C to give up", || pull(&cli.remote));
    match pulled {
        Ok(Pulled::Merged) => {
            replaces_index_record::repair_and_say("pull-replaces");
            ExitCode::SUCCESS
        }
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
