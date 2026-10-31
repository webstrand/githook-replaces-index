//! Records the rewrite given on standard input, one `<old> <new>` line for each commit made from
//! another, in refs/notes/replaces: the note of each new commit comes to stand in place of the
//! commit it was made from. The abandoned records of the repository are recorded first, and
//! removed once recorded.

use std::io::{self, Read};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_record::{Error, Rewrite, abandoned, heartbeat, record};

#[derive(Parser)]
#[command(name = "record", about)]
struct Cli {
    /// Records the abandoned records alone, without reading a rewrite.
    #[arg(long)]
    abandoned: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("replaces-index: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), Error> {
    let mut input = String::new();
    if !cli.abandoned {
        io::stdin()
            .read_to_string(&mut input)
            .map_err(Error::Input)?;
    }
    let dir = abandoned::dir().map_err(Error::Read)?;
    let waiting = abandoned::waiting(&dir)?;
    for dropped in &waiting.dropped {
        eprintln!("replaces-index: {dropped}; it is left out");
    }
    let mut rewrite = waiting.rewrite.clone();
    rewrite
        .replacements
        .extend(Rewrite::parse(&input).replacements);
    heartbeat::while_running("replaces-index", "Ctrl-C to give up", || record(&rewrite))?;
    waiting.remove()?;
    replaces_index_record::repair_and_say("replaces-index");
    Ok(())
}
