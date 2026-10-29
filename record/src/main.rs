//! Records the rewrite given on standard input, one `<old> <new>` line for each commit made from
//! another, in refs/notes/replaces: the note of each new commit comes to stand in place of the
//! commit it was made from.

use std::io::{self, Read};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_record::{Error, Rewrite, record};

#[derive(Parser)]
#[command(name = "record", about)]
struct Cli {}

fn main() -> ExitCode {
    Cli::parse();
    let mut input = String::new();
    let result = io::stdin()
        .read_to_string(&mut input)
        .map_err(Error::Input)
        .and_then(|_| record(&Rewrite::parse(&input)));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("replaces-index: {err}");
            ExitCode::FAILURE
        }
    }
}
