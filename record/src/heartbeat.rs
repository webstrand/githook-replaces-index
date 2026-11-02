//! What a program that writes the notes waits for, said every second once it has waited a second.

use std::io::{self, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::{Lock, REF, Retry, lock_patience, progress};

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
            let line = format!(
                "{REF} {why}, retrying (attempt {}, {seconds}s; {stop})",
                progress.attempt
            );
            match stale(progress.lock.as_ref()) {
                Some(stale) => format!("{line}; {stale}"),
                None => line,
            }
        }
        None => match runner::running().first() {
            Some(command) => format!("waiting for {command} ({seconds}s; {stop})"),
            None => format!("recording ({seconds}s; {stop})"),
        },
    }
}

/// What to do about `lock`, watched held since `since`, once that is a third of `lock_patience`:
/// git holds the lock of a ref for moments only, and one that has been there that long was most
/// likely left by a git ended by `SIGKILL`, which git then refuses to take as well. The writer
/// retries until the whole of `lock_patience`, since a git on a slow filesystem may yet let go
/// of it.
fn stale(lock: Option<&(Lock, Instant)>) -> Option<String> {
    let (lock, since) = lock?;
    let held = since.elapsed();
    (held >= lock_patience() / 3).then(|| {
        format!(
            "{} has been held for {}s; if no git command is running, it was left behind, and removing it lets the write through; the write is given up once it has been held for {}s",
            lock.path.display(),
            held.as_secs(),
            lock_patience().as_secs()
        )
    })
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
