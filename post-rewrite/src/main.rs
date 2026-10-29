//! The post-rewrite hook of replaces notes. Git passes it the command that rewrote the commits,
//! and on standard input one `<old> <new>` line per rewritten commit.

use std::io::{self, Read};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_post_rewrite::{Error, Kind, post_rewrite};

#[derive(Parser)]
#[command(name = "post-rewrite", about)]
struct Cli {
    /// The command that rewrote the commits.
    kind: Kind,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut input = String::new();
    let result = io::stdin()
        .read_to_string(&mut input)
        .map_err(Error::Input)
        .and_then(|_| post_rewrite(cli.kind, &input));
    // Git ignores the exit status of post-rewrite; a failure is reported and nothing else.
    if let Err(err) = result {
        eprintln!("replaces-index: {err}");
        eprintln!("replaces-index: the rewrite was not recorded");
    }
    ExitCode::SUCCESS
}
