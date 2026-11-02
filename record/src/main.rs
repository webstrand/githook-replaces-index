//! Records the rewrite given on standard input, one `<old> <new>` line for each commit made from
//! another, in refs/notes/replaces: the note of each new commit names the commit it was made from,
//! in whose place the new commit stands. The abandoned records of the repository are recorded
//! first, and removed once recorded.

use std::io::{self, Read};
use std::process::ExitCode;

use clap::Parser;
use replaces_index_record::notice::{Notice, say};
use replaces_index_record::{Error, Rewrite, abandoned, gc, heartbeat, leftovers, record};

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
                let notice = Notice::Collected {
                    removed: collected.removed,
                    kept: collected.kept,
                };
                println!("gc-replaces: {notice}");
                replaces_index_record::repair_and_say("gc-replaces");
                match leftovers::sweep() {
                    Ok(0) => ExitCode::SUCCESS,
                    Ok(swept) => {
                        println!("gc-replaces: {}", Notice::Swept(swept));
                        ExitCode::SUCCESS
                    }
                    Err(err) => {
                        replaces_index_record::notice::say("gc-replaces", &err);
                        ExitCode::FAILURE
                    }
                }
            }
            Err(err) => {
                replaces_index_record::notice::say("gc-replaces", &err);
                ExitCode::FAILURE
            }
        };
    }
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            replaces_index_record::notice::say("replaces-index", &err);
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
        say("replaces-index", &Notice::NotUtf8(line));
    }
    let dir = abandoned::dir().map_err(Error::Read)?;
    let waiting = abandoned::waiting(&dir)?;
    for dropped in &waiting.dropped {
        say("replaces-index", dropped);
    }
    let mut rewrite = waiting.rewrite.clone();
    rewrite.replacements.extend(read.replacements);
    heartbeat::while_running("replaces-index", "Ctrl-C to give up", || record(&rewrite))?;
    waiting.remove()?;
    Ok(())
}
