//! The fetch refspecs of the remotes, as the config of the repository holds them.

use crate::{Error, check, git, run, text};

/// A fetch refspec of a remote: `[+]<src>:<dst>`, or `^<src>`, which excludes the refs that
/// `<src>` matches from the other refspecs of the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refspec {
    pub remote: String,
    /// The refspec as the config holds it.
    pub text: String,
    /// Whether it moves its destinations whether or not they fast-forward: a leading `+`.
    pub force: bool,
    pub negative: bool,
    pub src: String,
    /// None for a negative refspec, and for one that fetches into no ref.
    pub dst: Option<String>,
}

impl Refspec {
    fn parse(remote: &str, text: &str) -> Self {
        let (force, rest) = match text.strip_prefix('+') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (negative, rest) = match rest.strip_prefix('^') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let (src, dst) = match rest.split_once(':') {
            Some((src, dst)) if !negative && !dst.is_empty() => (src, Some(dst.to_owned())),
            Some((src, _)) => (src, None),
            None => (rest, None),
        };
        Self {
            remote: remote.to_owned(),
            text: text.to_owned(),
            force,
            negative,
            src: src.to_owned(),
            dst,
        }
    }

    /// The ref of the remote that this refspec fetches into the ref `full`, if it fetches into it.
    pub fn source_of(&self, full: &str) -> Option<String> {
        let dst = self.dst.as_deref()?;
        let star = matched(dst, full)?;
        Some(self.src.replacen('*', star, 1))
    }

    /// Whether this is a negative refspec that excludes the ref `src` of the remote.
    pub fn excludes(&self, src: &str) -> bool {
        self.negative && matched(&self.src, src).is_some()
    }
}

/// What the `*` of `pattern` stands for in `name`, or `""` for a pattern without one that is
/// `name`; none when `pattern` does not match `name`. A `*` stands for one character at least.
fn matched<'a>(pattern: &str, name: &'a str) -> Option<&'a str> {
    match pattern.split_once('*') {
        Some((before, after)) => {
            let star = name.strip_prefix(before)?.strip_suffix(after)?;
            (!star.is_empty()).then_some(star)
        }
        None => (pattern == name).then_some(""),
    }
}

/// Every fetch refspec of every remote, in the order the config holds them.
pub fn fetch_refspecs() -> Result<Vec<Refspec>, Error> {
    let mut config = git(["config", "-z", "--get-regexp", r"^remote\..*\.fetch$"]);
    let output = run(&mut config)?;
    // git config exits with 1 when no remote has a fetch refspec.
    if output.status.code() == Some(1) {
        return Ok(Vec::new());
    }
    check(&config, &output)?;
    let out = text(&config, output.stdout)?;
    // `remote.<name>.fetch\n<refspec>\0` for each refspec.
    Ok(out
        .split('\0')
        .filter_map(|entry| {
            let (key, text) = entry.split_once('\n')?;
            let remote = key.strip_prefix("remote.")?.strip_suffix(".fetch")?;
            Some(Refspec::parse(remote, text))
        })
        .collect())
}

/// The remote that a fetch brings the ref `full` from: the first whose fetch refspecs map a ref of
/// it to `full`. The ref's name alone does not tell, since a remote may keep its refs anywhere.
pub fn remote_of(full: &str) -> Result<Option<String>, Error> {
    Ok(fetch_refspecs()?
        .into_iter()
        .find(|refspec| refspec.source_of(full).is_some())
        .map(|refspec| refspec.remote))
}

/// The refspecs whose fetch moves the ref `full` whether or not it fast-forwards, and so may drop
/// what the ref held: those with a `+` that map a ref of their remote to `full`, unless a negative
/// refspec of the same remote excludes that ref.
pub fn forcing(full: &str) -> Result<Vec<Refspec>, Error> {
    let refspecs = fetch_refspecs()?;
    Ok(refspecs
        .iter()
        .filter(|refspec| refspec.force)
        .filter(|refspec| {
            refspec.source_of(full).is_some_and(|src| {
                !refspecs
                    .iter()
                    .any(|other| other.remote == refspec.remote && other.excludes(&src))
            })
        })
        .cloned()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glob_maps_a_ref_through_its_star() {
        let refspec = Refspec::parse("origin", "+refs/notes/*:refs/notes/*");
        assert!(refspec.force);
        assert_eq!(
            refspec.source_of("refs/notes/replaces").as_deref(),
            Some("refs/notes/replaces")
        );
        assert_eq!(refspec.source_of("refs/heads/main"), None);
    }

    #[test]
    fn a_negative_refspec_excludes_and_maps_nothing() {
        let refspec = Refspec::parse("origin", "^refs/notes/replaces");
        assert!(refspec.negative);
        assert_eq!(refspec.source_of("refs/notes/replaces"), None);
        assert!(refspec.excludes("refs/notes/replaces"));
        assert!(!refspec.excludes("refs/notes/commits"));
    }

    #[test]
    fn a_star_stands_for_one_character_at_least() {
        assert_eq!(matched("refs/remotes/*", "refs/remotes/"), None);
        assert_eq!(
            matched("refs/remotes/*", "refs/remotes/o/main"),
            Some("o/main")
        );
    }
}
