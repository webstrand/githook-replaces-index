//! `git pull-replaces <remote>`: merges a remote's replaces notes into the local ones.

use std::process::ExitCode;

use clap::Parser;
use replaces_index_pull::{Notice, Pulled, pull};
use replaces_index_record::heartbeat;
use replaces_index_record::notice::say;

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
        Ok(Pulled::Merged) => ExitCode::SUCCESS,
        Ok(Pulled::Absent) => {
            say("pull-replaces", &Notice::Absent(cli.remote));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("pull-replaces: {}", runner::chain(&err));
            ExitCode::FAILURE
        }
    }
}
