---
name: replaces-index
description: Finds the other versions of a commit through the replaces notes, and checks whether a rewrite lost work. For a commit id that is no longer on the branch (from a transcript, a TODO or a note), a remote whose branch was rewritten or force-pushed, or a rebase or rewrite whose result should be checked against what it replaced.
---

# Replaces index

The hooks of this repository record most rewrites: each commit that an amend, a rebase or a filter-repo run made carries a note naming the commits it was made from. The commit it was made from is not thereby gone: it may stay on other branches and in other clones. Whether one commit replaces another holds only between two chains, and `git compare-replaces` decides it.

The records are best effort. Rewrites by other means (`git cherry-pick`, `git replay`, a reset and commit by hand) are not recorded, so the absence of a record does not show that no other version exists. In a clone without the hooks there are no records at all: when `git config hook.replaces-index.command` prints nothing, the agent tells the operator that the hooks are not installed, rather than concluding anything from their absence. Setting them up is the operator's.

## Rewrites made in another clone

A clone knows the rewrites made in another clone once their notes are merged in:

```sh
git pull-replaces <remote>
```

The pull fetches the notes itself and changes nothing on the remote.

## Finding the versions of a commit

`git whereis <commit>` prints every commit related to a commit by rewrites, one per line, as `<relation> <commit> <where>`:

```
$ git whereis 75bce16
from 1f0c2a9e… theirs
self 75bce166… theirs
into f92e7bea… main
into 0d4b8c31… (no ref)
```

- `from`: a commit it was made from; `self`: the commit asked about; `into`: a commit made from it, through any number of rewrites.
- The last field lists the branches, remote-tracking branches and tags that hold the commit, `(no ref)` when none does, or `(not in this clone)`.

The ids are printed in full; the example shortens them. It shows a commit that was rebased onto `main` and is still on `theirs`: both versions are live. A commit the clone lacks may be named by any prefix of its id that names one commit of the notes.

## Checking whether a rewrite lost work

`<old>` is the chain whose work must survive, `<new>` the chain that is meant to hold it, for instance a branch before and after a rebase, or a local branch and its rewritten upstream:

```sh
git compare-replaces <old> <new>
```

prints a line for each commit of `<old>` that `<new>` lacks, oldest first:

| Line | Meaning | What follows |
| - | - | - |
| `held <c> as <commits>` | `<new>` holds commits made from it | nothing: the work survives |
| `later <c> than <commits>` | `<new>` holds an earlier version of it | `<new>` lacks the change as `<c>` made it; `git range-diff <commit>^! <c>^!` compares the two versions |
| `rewritten <c> as <commits>` | it was made into commits that `<new>` lacks | its change went elsewhere; `git whereis <c>` shows where |
| `missing <c>` | nothing related to it is in `<new>` | work that `<new>` lacks, or a rewrite the notes did not record; compared by hand |

Nothing of `<old>` is lost in `<new>` when every line is a `held` line, or when there are no lines.

## Bringing a branch onto a rewritten upstream

Two operations, with different results:

- **Replaying the branch's own commits onto the upstream.** `git rebase-replaces [<upstream>]` rebases the current branch onto `<upstream>`, by default its configured upstream, and leaves out each commit that `git compare-replaces HEAD <upstream>` reports as `held`, so that the old versions of the upstream's commits are not replayed onto their own replacements. It fetches a remote-tracking upstream and pulls its notes first. It says which commits it replays although they were rewritten elsewhere. With `--keep-base`, it replays them onto what replaces the history the branch shared with the upstream, without what the upstream added since.
- **Taking the upstream as the branch.** When `git compare-replaces HEAD <upstream>` prints only `held` lines, the branch holds nothing of its own, and may be reset to the upstream once the operator confirms. Tracked changes, and untracked files that the upstream tracks, would be overwritten, so the agent checks for them first. The listing is of the whole work tree, with paths from its top as `<upstream>:` takes them, and separated by NULs, so that no name is quoted or split; ignored files, which git overwrites as it does in any checkout, are left out:

  ```sh
  git status --porcelain | grep -v '^??'
  git ls-files -z -o --exclude-standard --full-name :/ | while IFS= read -r -d '' f; do git cat-file -e "<upstream>:$f" 2>/dev/null && printf '%s\n' "$f"; done
  old=$(git rev-parse HEAD)
  git reset --hard <upstream>
  ```

  Each branch built on the old tip (`git branch --contains $old`, and `git worktree list` for where it is checked out) is then moved with its own commits only:

  ```sh
  git -C <worktree> rebase --onto <upstream> $old
  ```

## Bringing worktrees onto a rewritten branch

After a local branch such as `locus` is rewritten, each worktree whose branch was built on its old history holds the old versions of its commits. `git worktree list` names the worktrees, and for each one:

```sh
git -C <worktree> compare-replaces HEAD <branch>
```

tells what the rewritten branch holds of the worktree's commits. The upstream is a local branch, so nothing is fetched, and the clone's own notes decide.

- **Every line `held`:** the worktree has nothing of its own. Its work was finished and folded in, and the worktree may be removed once the operator confirms.
- **`held` lines, then a few others:** the others are the worktree's own commits. `git -C <worktree> rebase-replaces <branch>` replays them onto the rewritten branch and leaves the rest out.
- **Every line `missing`:** the worktree was built on history that the notes do not cover, such as one rewritten before the hooks were installed, or whose notes were removed. `rebase-replaces` would replay all of it. The worktree's own commits are found instead from the old branch it forked from, an old ref or a backup branch that holds all but the last few of its commits:

  ```sh
  for r in $(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes); do
    echo "$(git rev-list --count $r..<worktree branch>) $r"
  done | sort -n | head -3
  ```

  The ref with the lowest count is the fork; `git log <ref>..<worktree branch>` lists the worktree's own commits, which are moved alone:

  ```sh
  git -C <worktree> rebase --onto <branch> <ref>
  ```

  The hook records that rebase, so the notes cover the worktree from then on.

A worktree with uncommitted changes is left alone until the operator decides what becomes of them.

## After git filter-repo

`git filter-repo` records nothing by itself. An agent that ran it records its rewrites right after, before anything else rewrites the repository:

```sh
"$(git rev-parse --git-common-dir)/bin/replaces-index-record-filter-repo"
```

## Cautions

- `git notes --ref=replaces prune` must never be run: it deletes the notes of commits made in other clones.
- `git rebase --quit` records nothing; an interrupted rebase is continued or aborted instead. The exception is a rebase interrupted while its `post-rewrite` hooks run, which has rewritten the branch already but leaves its state behind: it is ended with `git rebase --quit`, since `git rebase --abort` would reset the branch to where it was before the rebase.
