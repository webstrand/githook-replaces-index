//! What a program that writes the notes waits for, said every second once it has waited a second.

use std::io::{self, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::{REF, Retry, progress};

/// How often a program says what it waits for, once it has waited that long.
pub const EVERY: Duration = Duration::from_secs(1);

/// What the process waits for, `elapsed` after it started, and `stop`, how to stop it.
pub fn line(elapsed: Duration, stop: &str) -> String {
    let seconds = elapsed.as_secs();
    let progress = progress();
    match progress.retrying {
        Some(retry) => {
            let why = match retry {
                Retry::Moved => "keeps moving",
                Retry::Locked => "is locked",
            };
            format!(
                "{REF} {why}, retrying (attempt {}, {seconds}s; {stop})",
                progress.attempt
            )
        }
        None => match runner::running().first() {
            Some(command) => format!("waiting for {command} ({seconds}s; {stop})"),
            None => format!("recording ({seconds}s; {stop})"),
        },
    }
}

/// Runs `work`, and says on standard error, after `prefix`, what it waits for every second once
/// it has taken a second.
pub fn while_running<T: Send>(prefix: &str, stop: &str, work: impl FnOnce() -> T + Send) -> T {
    thread::scope(|scope| {
        let (done, finished) = mpsc::channel();
        let worker = scope.spawn(move || {
            let result = work();
            let _ = done.send(());
            result
        });
        let start = Instant::now();
        let mut beat = start + EVERY;
        while let Err(RecvTimeoutError::Timeout) =
            finished.recv_timeout(beat.saturating_duration_since(Instant::now()))
        {
            let _ = writeln!(io::stderr(), "{prefix}: {}", line(start.elapsed(), stop));
            beat += EVERY;
        }
        worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
