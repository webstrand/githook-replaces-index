//! What the programs say besides their failures, each after the program's prefix.

use std::io::{self, Write};

use runner::refspec::Refspec;

use crate::{Error, REF};

/// Something the programs say, on standard error unless told otherwise.
#[derive(Debug, thiserror::Error)]
pub enum Notice {
    #[error("`{0}` of the rewrite is not UTF-8, and so no replacement; it is left out")]
    NotUtf8(String),
    #[error(
        "remote.{}.fetch {} replaces {REF} on each fetch, dropping what this clone recorded since; {}",
        .0.remote,
        .0.text,
        Instead::of(.0)
    )]
    Forcing(Refspec),
    #[error("could not read the fetch refspecs")]
    RefspecsUnreadable(#[source] runner::Error),
    #[error(
        "a writer dropped lines from {REF} when it moved it from {before} to {after}; they are put back"
    )]
    PutBack { before: String, after: String },
    #[error(
        "the note of {commit}, which this clone no longer has, lost lines; the notes of {}, which name it, hold them",
        .naming.join(" ")
    )]
    Folded { commit: String, naming: Vec<String> },
    #[error("could not check {REF} for lines that a writer dropped")]
    Unchecked(#[source] Error),
    /// On standard output.
    #[error(
        "removed {removed} notes of commits this clone no longer has, whose lines the notes that name them hold; kept {kept} that no note names"
    )]
    Collected { removed: usize, kept: usize },
    /// On standard output.
    #[error("deleted {0} refs and files that ended processes left behind")]
    Swept(usize),
}

/// How a forcing refspec comes to fetch the remote's notes beside the local ones: with its
/// destinations under refs/notes/remotes/<remote>/, where `git pull-replaces` merges them from.
#[derive(Debug, thiserror::Error)]
enum Instead {
    #[error(
        "to fetch the remote's notes beside this clone's: git config --replace-all --fixed-value {key} {new} {old}"
    )]
    Move {
        key: String,
        new: String,
        old: String,
    },
    #[error("it must fetch {src} into another ref, such as refs/notes/remotes/{remote}/replaces")]
    Elsewhere { src: String, remote: String },
}

impl Instead {
    fn of(refspec: &Refspec) -> Self {
        let remote = &refspec.remote;
        let quote = |word: String| runner::quote(word.as_ref()).display().to_string();
        match refspec
            .dst
            .as_deref()
            .and_then(|dst| dst.strip_prefix("refs/notes/"))
        {
            Some(rest) => Instead::Move {
                key: quote(format!("remote.{remote}.fetch")),
                new: quote(format!(
                    "+{}:refs/notes/remotes/{remote}/{rest}",
                    refspec.src
                )),
                old: quote(refspec.text.clone()),
            },
            None => Instead::Elsewhere {
                src: refspec.source_of(REF).unwrap_or_default(),
                remote: remote.clone(),
            },
        }
    }
}

/// Says `notice` on standard error, after `prefix`. A standard error that cannot be written to,
/// as once the terminal is gone, stops nothing.
pub fn say(prefix: &str, notice: &dyn std::error::Error) {
    let _ = writeln!(io::stderr(), "{prefix}: {}", runner::chain(notice));
}
