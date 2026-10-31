//! The post-rewrite hook of replaces notes. Git passes it the command that rewrote the commits,
//! and on standard input one `<old> <new>` line per rewritten commit.
//!
//! A rewrite that cannot be recorded is kept as an abandoned record, and printed as a command that
//! records it by hand: git passes the rewrite to the hook once only. The hook then fails, which
//! git ignores.

use std::fmt::Display;
use std::io::{self, Read, Write};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_post_rewrite::{Error, Failure, Kind, post_rewrite};
use replaces_index_record::{Rewrite, abandoned};

#[derive(Parser)]
#[command(name = "post-rewrite", about)]
struct Cli {
    /// The command that rewrote the commits.
    kind: Kind,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut input = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut input) {
        say(Error::Input(err));
        return ExitCode::FAILURE;
    }
    match post_rewrite(cli.kind, &input) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure { error, rewrite }) => {
            say(error);
            abandon(&rewrite);
            ExitCode::FAILURE
        }
    }
}

/// Keeps `rewrite` as an abandoned record, and prints the command that records it by hand.
fn abandon(rewrite: &Rewrite) {
    let kept = abandoned::dir()
        .map_err(|err| err.to_string())
        .and_then(|dir| abandoned::keep(&dir, rewrite).map_err(|err| err.to_string()));
    match kept {
        Ok(path) => say(format_args!(
            "the rewrite was kept in {}; to record it by hand:",
            path.display()
        )),
        Err(err) => say(format_args!(
            "the rewrite could not be kept ({err}); to record it by hand:"
        )),
    }
    // A standard error that cannot be written to, for instance once the terminal is gone, leaves
    // the record kept all the same.
    let _ = writeln!(io::stderr(), "{}", abandoned::command(rewrite));
}

fn say(message: impl Display) {
    let _ = writeln!(io::stderr(), "replaces-index: {message}");
}
