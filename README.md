# githook-replaces-index

When a commit is amended, rebased or filtered (e.g. by `git filter-repo`), git makes a new commit in its place. The old commit may still be in use elsewhere, on another branch or in someone else's clone, and nothing relates it to the new one: a commit id quoted in a transcript or an issue stops matching anything on the branch, and after an upstream is force-pushed, git cannot tell the old versions of its commits from the new ones.

This hook records which commit each new commit replaced, so that every version of a change can be found from any one of them. The records are stored in git notes in `refs/notes/replaces`, which travel between clones as a ref. The commits themselves are never touched.

Each note names every prior version of its commit:

```
Notes (replaces):
    replaces: <id of a prior version>
    replaces: <id of another prior version>
```

## Requirements

- A Rust toolchain, for `cargo install`.
- A git that supports `hook.<name>.event`. The programs are tested with git 2.56.
- bash 3.2 or later, for `install.sh`.

## Installation

The hook is installed into a repository by running `install.sh` from a checkout of that repository, usually with githook-replaces-index mounted in it as a submodule:

```sh
git submodule add https://github.com/webstrand/githook-replaces-index.git tools/replaces-index
tools/replaces-index/install.sh
```

The script installs the programs into `bin/` in the repository's common git directory, registers the `post-rewrite` hook as `hook.replaces-index`, beside any other `post-rewrite` hooks, defines the git aliases below, and links the Claude Code skill into `.claude/skills/replaces-index`. Each clone must run it once; worktrees share the programs and the config of their repository. Running it again updates them.

Rewrites made in other clones reach a clone only through `git pull-replaces`. For each remote whose notes the clone shares, the operator should configure a refspec that brings them beside the local ones:

```sh
git config --add remote.<remote>.fetch '+refs/notes/replaces*:refs/notes/remotes/<remote>/replaces*'
```

A refspec that forces a remote's notes onto `refs/notes/replaces` itself, such as `+refs/notes/*:refs/notes/*`, would drop the local records on each fetch; every program refuses to run while one exists, and names the command that replaces it.

## Commands

| Command | Role |
| - | - |
| `git whereis <commit>` | prints the commits related to a commit by rewrites, and the refs that hold them |
| `git compare-replaces <old> <new>` | tells what the chain `new` holds of each commit of the chain `old` |
| `git pull-replaces <remote>` | merges a remote's replaces notes into the local ones |
| `git rebase-replaces [--keep-base] [<upstream>]` | rebases the current branch onto a rewritten upstream, leaving out the commits that the upstream already holds in another form |
| `git gc-replaces` | removes the notes of commits the clone no longer has, and puts back lines that another writer dropped |
| `replaces-index-record-filter-repo` | records the commits that the last `git filter-repo` run made; the operator runs it after each run |

The hook records amends and rebases without being invoked. `git log --notes=replaces` displays the notes beside the log.

`git whereis` prints one line per related commit:

```
$ git whereis 75bce16
from 1f0c2a9e… theirs
self 75bce166… theirs
into f92e7bea… main
into 0d4b8c31… (no ref)
```

`git compare-replaces` prints a line for each commit of `<old>` that `<new>` lacks; nothing of `<old>` is lost in `<new>` when every line is a `held` line.

## Claude Code skill

`skills/replaces-index/SKILL.md` instructs an agent to find the versions of a commit, check whether a rewrite lost work, and bring branches and worktrees onto a rewritten upstream with the commands above.
