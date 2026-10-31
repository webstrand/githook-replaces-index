//! The post-rewrite hook of replaces notes. Git passes it the command that rewrote the commits,
//! and on standard input one `<old> <new>` line per rewritten commit.
//!
//! A rewrite that cannot be recorded is kept as an abandoned record, and printed as a command that
//! records it by hand: git passes the rewrite to the hook once only. The hook then fails, which
//! git ignores. A signal that ends the hook before the rewrite is recorded ends the git commands it
//! runs, keeps the rewrite as an abandoned record in the same way, waiting at most a minute for it
//! to be written, and then ends the hook as the signal would have. A signal that stops the hook,
//! such as that of Ctrl-Z, stops it and its git commands as usual, and they carry on once
//! continued.

use std::fmt::Display;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{self, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use replaces_index_post_rewrite::{Error, Failure, Kind, post_rewrite};
use replaces_index_record::{Rewrite, abandoned};
use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::Signals;

#[derive(Parser)]
#[command(name = "post-rewrite", about)]
struct Cli {
    /// The command that rewrote the commits.
    kind: Kind,
}

/// The signals that end the hook, after it keeps the rewrite.
const SIGNALS: [i32; 4] = [SIGINT, SIGTERM, SIGHUP, SIGQUIT];

/// How long the hook waits for an abandoned record to be written once a signal ends it.
const PATIENCE: Duration = Duration::from_secs(60);

enum Event {
    Signal(i32),
    /// The rewrite was recorded, or could not be.
    Done(Result<(), Failure>),
    /// The abandoned record was written, or could not be; nothing when there was none to write.
    Kept(Option<Result<PathBuf, String>>),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut input = String::new();
    if let Err(err) = io::stdin().read_to_string(&mut input) {
        say(Error::Input(err));
        return ExitCode::FAILURE;
    }
    let (events, received) = mpsc::channel();
    match Signals::new(SIGNALS) {
        Ok(mut signals) => {
            let events = events.clone();
            thread::spawn(move || {
                for signal in signals.forever() {
                    if events.send(Event::Signal(signal)).is_err() {
                        break;
                    }
                }
            });
        }
        Err(err) => say(format_args!("could not handle signals: {err}")),
    }
    // Found at once, since once a signal arrives no git command starts.
    let dir = abandoned::dir().map_err(|err| err.to_string());
    let outstanding = Arc::new(Mutex::new(None));
    {
        let events = events.clone();
        let outstanding = Arc::clone(&outstanding);
        thread::spawn(move || {
            let result = post_rewrite(cli.kind, &input, &outstanding);
            let _ = events.send(Event::Done(result));
        });
    }
    let hook = Hook {
        events,
        received,
        patience: PATIENCE,
        keep: Arc::new(move |rewrite| {
            let dir = dir.clone()?;
            abandoned::keep(&dir, rewrite).map_err(|err| err.to_string())
        }),
    };
    loop {
        match hook.received.recv().expect("the hook holds a sender") {
            Event::Done(Ok(())) => return ExitCode::SUCCESS,
            Event::Done(Err(Failure { error, rewrite })) => {
                say(error);
                return match hook.abandon(Some(rewrite), None) {
                    (Some(signal), _) => die(signal),
                    (None, _) => ExitCode::FAILURE,
                };
            }
            Event::Signal(signal) => {
                runner::stop();
                let rewrite = outstanding
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take();
                let (signal, _) = hook.abandon(rewrite, Some(signal));
                die(signal.expect("the hook was signalled"));
            }
            Event::Kept(_) => {}
        }
    }
}

/// The events that the hook waits for, and how it keeps a rewrite.
struct Hook {
    events: Sender<Event>,
    received: Receiver<Event>,
    /// How long to wait for a record once a signal has arrived.
    patience: Duration,
    keep: Arc<Keep>,
}

/// Writes an abandoned record, and returns its path.
type Keep = dyn Fn(&Rewrite) -> Result<PathBuf, String> + Send + Sync;

/// How the hook was done with an abandoned rewrite.
#[derive(Debug, PartialEq, Eq)]
enum Ending {
    Kept,
    NotKept,
    /// The record was not written in time, or a second signal ended the wait.
    GaveUp,
}

impl Hook {
    /// Keeps `rewrite`, if any, as an abandoned record, and prints the command that records it.
    /// Once a signal has arrived, it waits for the record at most `patience`, and a second signal
    /// ends the wait; the first signal is returned. A signal also has the staging ref of the
    /// writer removed, since the writer will not remove it.
    fn abandon(&self, rewrite: Option<Rewrite>, mut signal: Option<i32>) -> (Option<i32>, Ending) {
        {
            let events = self.events.clone();
            let keep = Arc::clone(&self.keep);
            let rewrite = rewrite.clone();
            let signalled = signal.is_some();
            thread::spawn(move || {
                if signalled {
                    remove_staging();
                }
                let kept = rewrite.map(|rewrite| keep(&rewrite));
                let _ = events.send(Event::Kept(kept));
            });
        }
        let waiting = || {
            if rewrite.is_some() {
                say(
                    "unable to note replaces, abandoning state, waiting for disk (60s timeout; signal again to skip)",
                );
            }
        };
        let mut deadline = signal.map(|_| Instant::now() + self.patience);
        if signal.is_some() {
            waiting();
        }
        loop {
            let event = match deadline {
                Some(deadline) => self
                    .received
                    .recv_timeout(deadline.saturating_duration_since(Instant::now())),
                None => self
                    .received
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match event {
                Ok(Event::Kept(None)) => return (signal, Ending::Kept),
                Ok(Event::Kept(Some(Ok(path)))) => {
                    let place = format!("was kept in {}", path.display());
                    print_command(&place, rewrite.as_ref());
                    return (signal, Ending::Kept);
                }
                Ok(Event::Kept(Some(Err(err)))) => {
                    print_command(&format!("could not be kept ({err})"), rewrite.as_ref());
                    return (signal, Ending::NotKept);
                }
                Ok(Event::Signal(again)) if signal.is_none() => {
                    runner::stop();
                    signal = Some(again);
                    deadline = Some(Instant::now() + self.patience);
                    waiting();
                }
                Ok(Event::Signal(_)) => {
                    let reason = "could not be kept (a second signal ended the wait)";
                    print_command(reason, rewrite.as_ref());
                    return (signal, Ending::GaveUp);
                }
                Ok(Event::Done(_)) => {}
                Err(_) => {
                    let reason = "could not be kept (it was not written within 60s)";
                    print_command(reason, rewrite.as_ref());
                    return (signal, Ending::GaveUp);
                }
            }
        }
    }
}

/// Has the ref that the writer staged the notes in removed, if it was writing. The git that removes
/// it is not waited for, and carries on once the hook has ended.
fn remove_staging() {
    if let Some(staging) = replaces_index_record::staging() {
        // Started outside runner, which starts nothing once the hook is stopping.
        let _ = Command::new("git")
            .args(["update-ref", "-d", &staging])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// Prints what became of `rewrite`, and the command that records it by hand.
fn print_command(what: &str, rewrite: Option<&Rewrite>) {
    let Some(rewrite) = rewrite else {
        return;
    };
    say(format_args!("the rewrite {what}; to record it by hand:"));
    // A standard error that cannot be written to, for instance once the terminal is gone, leaves
    // the record kept all the same.
    let _ = writeln!(io::stderr(), "{}", abandoned::command(rewrite));
}

/// Ends the process as `signal` would have.
fn die(signal: i32) -> ! {
    let _ = io::stderr().flush();
    let _ = signal_hook::low_level::emulate_default_handler(signal);
    process::exit(128 + signal);
}

fn say(message: impl Display) {
    let _ = writeln!(io::stderr(), "replaces-index: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(keep: impl Fn(&Rewrite) -> Result<PathBuf, String> + Send + Sync + 'static) -> Hook {
        let (events, received) = mpsc::channel();
        Hook {
            events,
            received,
            patience: Duration::from_millis(200),
            keep: Arc::new(keep),
        }
    }

    fn rewrite() -> Option<Rewrite> {
        Some(Rewrite::parse("a b\n"))
    }

    /// A record that is never written, as on a disk that does not answer.
    fn never(_: &Rewrite) -> Result<PathBuf, String> {
        loop {
            thread::park();
        }
    }

    #[test]
    fn a_record_written_in_time_is_kept() {
        let hook = hook(|_| Ok(PathBuf::from("record")));
        assert_eq!(
            hook.abandon(rewrite(), Some(SIGINT)),
            (Some(SIGINT), Ending::Kept)
        );
    }

    #[test]
    fn after_a_signal_the_hook_waits_for_the_record_only_so_long() {
        let hook = hook(never);
        let start = Instant::now();
        assert_eq!(
            hook.abandon(rewrite(), Some(SIGTERM)),
            (Some(SIGTERM), Ending::GaveUp)
        );
        assert!(start.elapsed() >= Duration::from_millis(200));
    }

    #[test]
    fn a_second_signal_ends_the_wait() {
        let mut hook = hook(never);
        hook.patience = Duration::from_secs(3600);
        hook.events.send(Event::Signal(SIGINT)).unwrap();
        let start = Instant::now();
        assert_eq!(
            hook.abandon(rewrite(), Some(SIGTERM)),
            (Some(SIGTERM), Ending::GaveUp)
        );
        assert!(start.elapsed() < Duration::from_secs(60));
    }

    #[test]
    fn without_a_signal_the_hook_waits_for_the_record() {
        let hook = hook(|_| {
            thread::sleep(Duration::from_millis(400));
            Ok(PathBuf::from("record"))
        });
        assert_eq!(hook.abandon(rewrite(), None), (None, Ending::Kept));
    }

    #[test]
    fn a_signal_while_the_hook_waits_bounds_the_wait() {
        let hook = hook(never);
        hook.events.send(Event::Signal(SIGHUP)).unwrap();
        assert_eq!(
            hook.abandon(rewrite(), None),
            (Some(SIGHUP), Ending::GaveUp)
        );
    }
}
