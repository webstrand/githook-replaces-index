//! Records the rewrite given on standard input, one `<old> <new>` line for each commit made from
//! another, in refs/notes/replaces: the note of each new commit comes to stand in place of the
//! commit it was made from. The abandoned records of the repository are recorded first, and
//! removed once recorded.

use std::io::{self, Read};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_record::{Error, Rewrite, abandoned, gc, heartbeat, record};

#[derive(Parser)]
#[command(name = "record", about)]
struct Cli {
    /// Records the abandoned records alone, without reading a rewrite.
    #[arg(long)]
    abandoned: bool,
    /// Says which fetch refspecs would replace refs/notes/replaces on each fetch, and records
    /// nothing.
    #[arg(long, conflicts_with = "abandoned")]
    check_refspecs: bool,
    /// Removes the notes of commits the clone no longer has, keeping their lines in the notes that
    /// name them, and records nothing; `git gc-replaces` runs this.
    #[arg(long, conflicts_with_all = ["abandoned", "check_refspecs"])]
    gc: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.check_refspecs {
        replaces_index_record::check_refspecs("replaces-index");
        return ExitCode::SUCCESS;
    }
    let prefix = if cli.gc {
        "gc-replaces"
    } else {
        "replaces-index"
    };
    if !replaces_index_record::check_refspecs(prefix) {
        return ExitCode::FAILURE;
    }
    if cli.gc {
        return match heartbeat::while_running("gc-replaces", "Ctrl-C to give up", gc) {
            Ok(collected) => {
                println!(
                    "gc-replaces: removed {} notes of commits this clone no longer has, whose lines the notes that name them hold; kept {} that no note names",
                    collected.removed, collected.kept
                );
                replaces_index_record::repair_and_say("gc-replaces");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("gc-replaces: {err}");
                ExitCode::FAILURE
            }
        };
    }
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("replaces-index: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), Error> {
    let mut input = Vec::new();
    if !cli.abandoned {
        io::stdin().read_to_end(&mut input).map_err(Error::Input)?;
    }
    let (read, left_out) = Rewrite::parse_bytes(&input);
    for line in left_out {
        eprintln!("replaces-index: `{line}` is not UTF-8, and so no replacement; it is left out");
    }
    let dir = abandoned::dir().map_err(Error::Read)?;
    let waiting = abandoned::waiting(&dir)?;
    for dropped in &waiting.dropped {
        eprintln!("replaces-index: {dropped}; it is left out");
    }
    let mut rewrite = waiting.rewrite.clone();
    rewrite.replacements.extend(read.replacements);
    heartbeat::while_running("replaces-index", "Ctrl-C to give up", || record(&rewrite))?;
    waiting.remove()?;
    replaces_index_record::repair_and_say("replaces-index");
    Ok(())
}
