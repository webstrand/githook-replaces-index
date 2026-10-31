# Replaces index

The replaces index records which commit each amend, rebase or `git filter-repo` run made from which, so that the versions of a change can be found from any one of them. The records are git notes in `refs/notes/replaces`: commits are never modified, and the notes travel between clones as a ref.

A rewrite that makes a commit `new` from a commit `old` writes the note of `new` only:

```
Notes (replaces):
    replaces: <id of old>
    replaces: <id of each commit that old's note names>
```

`new` stands in place of `old` and of everything `old` stands in place of. The note of `old` is not changed: `old` may stay on other branches and in other clones as a commit in its own right, for instance when the commits of a remote are rebased onto a local branch while the remote keeps them. Whether `new` replaces `old` holds only between two chains, one holding each, and is decided by comparing them; the notes record only which commit was made from which. The lines of a note are sorted and unique; their order carries no meaning.

Unlike session notes, replaces notes belong to the repository rather than to a session. Every rewrite records them, whether or not a Claude Code session made it, and in every clone where they are installed.

## Installation

The programs are written in Rust, each a crate in the directory of its name, in a cargo workspace of their own that shares no crate with other hooks, so that a repository may keep it as a submodule apart from its own build. `skills/replaces-index/` is a Claude Code skill that uses them. `install.sh`, run from inside a checkout, installs them with `cargo install`, which needs a Rust toolchain, into `bin/` in the repository's common git directory, registers `post-rewrite` in the repository's config as `hook.replaces-index`, so that it runs beside any other `post-rewrite` hooks, such as that of session notes, and defines the aliases `git whereis`, `git pull-replaces`, `git compare-replaces` and `git rebase-replaces`. The hook is installed as `githook-replaces-index-post-rewrite`, the program of each alias as `git-<alias>`, and the others as `replaces-index-<program>`. It links the skill into `.claude/skills/replaces-index` in the checkout it is run from; the project may commit the link, so that every checkout and worktree carries it. The hook and each alias name their binary through `$(git rev-parse --git-common-dir)`, which the shell resolves each time git runs them, so the repository may move. It may be run again to update them. Run from inside a submodule, it refuses, since it would install into the submodule's repository rather than the one that mounts it; `INSTALL_SUBMODULE=1` installs into the submodule nonetheless. When the checkout it is run from is a submodule of the repository it installs into, and the repository's attributes give that submodule no `submodule-roll` attribute, it adds a line that sets it to the repository's `.gitattributes`, which the operator commits, so that [`githook-submodule-roll`](https://github.com/webstrand/githook-submodule-roll) rolls the submodule forward to its latest compatible release; it recommends that hook where it is not installed. Each clone must run it once; worktrees share the programs and the config of their repository. Configured hooks need a git that supports `hook.<name>.event`; the programs are tested with git 2.56.

`notes.rewriteRef` may match `refs/notes/replaces`, as a glob such as `refs/notes/*` set for instance in a global config does. Git then copies the note of `old` to `new` on every amend and rebase, before the hook runs. The lines it copies are among those that `new` inherits, and the hook adds the rest.

| Program | Role |
| - | - |
| `post-rewrite` | records the commits that each amend and rebase made |
| `record` | writes the notes of the commits that a rewrite made |
| `whereis` | `git whereis <commit>`: prints the commits related to a commit by rewrites, and the refs that hold them |
| `compare` | `git compare-replaces <old> <new>`: tells what one chain holds of each commit of another |
| `pull` | `git pull-replaces <remote>`: merges a remote's replaces notes into the local ones |
| `rebase` | `git rebase-replaces [<upstream>]`: rebases the current branch onto a rewritten upstream |
| `record-filter-repo` | records the commits that a `git filter-repo` run made |

The library crate `store` keeps the index that the queries read.

## Recording

Git runs `post-rewrite` after `git commit --amend` and after a rebase, with one line per rewritten commit mapping the old commit to the new one. The hook passes these lines to `record`.

`record` reads the notes of the commits involved, and gives each new commit a `replaces:` line for the commit it was made from and every `replaces:` line of that commit's note, beside the lines it holds already. It writes the notes that changed in a single commit on the ref, through `git fast-import`, and leaves every other note as it was. Since every commit it annotates is one that a rewrite in this clone made, the clone has it, and fast-import keeps the fanout of the notes tree.

`record` writes the commit to a ref of its own, `refs/replaces-index/record-<pid>`, and moves `refs/notes/replaces` to it only if the notes ref still points at the commit it read, then removes its own ref. When git refuses to move the notes ref, `record` reads it again. If another writer, such as a rebase in another worktree, moved it in the meantime, `record` reads the notes again and retries; if it did not move, another process holds its lock, and `record` retries as well. It waits a short and varying time between attempts, and gives up after 20 attempts with an error. No write therefore replaces another.

- An amend records that the new commit was made from the amended one.
- A rebase records the same for each commit it creates.
- A squash or fixup records that the result was made from every commit folded together.
- A commit that git reports as made from itself, as an amend that changes nothing within the same second does, records nothing.
- A commit recreated with the id of a commit it stands in place of, for instance when a commit is amended back to an earlier one, does not name itself. The notes then hold a cycle, which the queries follow without harm.

During a rebase, git reports each fixup or squash step, and each amend that an `exec` line or a stopped rebase makes, as an amend of a commit that the rebase created itself. Such commits never appear outside the rebase. The hook sets these amends aside in the rebase's state directory, and when the rebase ends, follows each commit that the rebase reports through them to the commit that took its place. An amend is followed only when the commit it made is on the branch as the rebase ends, so that one undone during the rebase, for instance by a `git reset` back to the amended commit, is not. No commit that the rebase discarded is therefore named. An amend of a commit that was there before the rebase began, such as one that a pick kept as it was or the commit the rebase is onto, is recorded at once instead: the rebase does not report such a commit as rewritten, and a rebase that rewrites nothing else runs no `post-rewrite` when it ends. Git removes the state directory when the rebase ends or is aborted, so an aborted rebase records nothing. The state directory is found through `git rev-parse --git-path`, so each worktree's rebase keeps its own amends.

`git am` keeps its state in `rebase-apply/` as a rebase may, but marks it with an `applying` file rather than `rebasing`, and runs no `post-rewrite` when it ends. An amend while `git am` is stopped is therefore recorded at once, rather than set aside.

The lines a note inherits are a copy, for reading one note alone. A clone may rewrite a commit before the note of that commit arrives from another clone, and the new note then inherits nothing from it. The queries do not rely on the copy: they follow the `replaces:` lines from note to note.

## Abandoned rewrites

Git passes a rewrite to `post-rewrite` once only. When the hook cannot record it, for instance because the notes ref stayed locked or is not a commit, or because `git fast-import` failed, it keeps the replacements that remained to be recorded as an abandoned record, and prints the error and a command that records them by hand:

```
replaces-index: <error>
replaces-index: the rewrite was kept in <path>; to record it by hand:
printf '%s\n' '<old> <new>' … | "$(git rev-parse --git-common-dir)"/bin/replaces-index-record
```

The hook then exits with a failure status, which git ignores: the rewrite itself is complete. During a rebase, the record holds the replacements after they were followed through the amends set aside, or as git reported them when following them failed.

Each record is a file of `<old> <new>` lines in `replaces-abandoned/` in the common git directory, which the worktrees share. It is named after the time it was written and a random suffix, so that the records sort in the order they were written. The hook writes it under a temporary name that begins with `.`, and renames it, so that a record either holds the whole rewrite or does not exist. When no record can be written, for instance because the disk is full, the hook says so and prints the command all the same. A standard error that cannot be written to, for instance once the terminal is gone, does not keep the record from being written.

No program records abandoned records yet: the operator must run the printed command, after which the record may be deleted.

## Index

The queries read an index of the notes, kept in `replaces/index.redb` in the common git directory. Each clone builds its own and its worktrees share it; the index is never committed or pushed, and may be deleted at any time, after which the next query builds it anew. It is a redb database that holds each `replaces:` line as an edge in both directions, so that both what a commit stands in place of and what was made from it are found without reading the notes.

The index records the notes commit it was built from. Each query opens it, and when the notes ref has moved, whatever moved it, brings it up to date first. Since lines are only ever added to the notes, it reads only the notes that `git diff-tree` reports added or changed since the commit indexed. A notes ref that lost a note, or that does not descend from the commit indexed, is indexed anew.

Queries open the database read-only, which any number of processes may do at once. Bringing it up to date needs it alone, and waits up to ten seconds while other processes hold it.

## Queries

`git whereis <commit>` prints every commit related to a commit by rewrites, one per line, as `<relation> <commit> <where>`:

| Relation | Commit |
| - | - |
| `from` | a commit that it was made from, through any number of rewrites |
| `self` | the commit itself |
| `into` | a commit that was made from it, through any number of rewrites |

Where it is lists the branches, remote-tracking branches and tags of the clone that hold the commit, `(no ref)` when none does, or `(not in this clone)`. A commit the clone does not have, such as one rewritten in another clone, may be given by its full id or by any prefix that names exactly one commit of the index.

`git compare-replaces <old> <new>` prints, for each commit of `old` that `new` lacks, oldest first, what `new` holds of it:

| Line | Meaning |
| - | - |
| `held <commit> as <commits>` | `new` holds commits made from it |
| `later <commit> than <commits>` | `new` holds commits it was made from; `old` holds a later version of them |
| `rewritten <commit> as <commits>` | it was made into commits, none of which `new` holds |
| `missing <commit>` | nothing related to it is in `new` |

Each line ends with the commit's subject. Nothing of `old` is lost in `new` when every line is a `held` line.

`git log --notes=replaces` and `git show --notes=replaces` display the notes, alongside those of `GIT_NOTES_REF` when `--notes` is also given, and `git notes --ref=replaces show <commit>` prints the note of one commit.

## Remotes

Each clone records the rewrites made in it. `git pull-replaces <remote>` merges a remote's notes into the local ones:

1. It fetches the remote's `refs/notes/replaces` into `refs/notes/remotes/<remote>/replaces`. The fetch names its refspec itself, so the remote needs no configuration.
2. It merges that into the local `refs/notes/replaces` with the `cat_sort_uniq` strategy, which takes the union of the notes' lines and never conflicts.

Lines are only ever added to notes, so the union is the whole merge. The remote is not changed. Two clones that each record rewrites hold the same notes once each has pulled from the other, after the last rewrite in either; a host that cannot reach the operator's clone receives no notes from it. A clone may pull from several remotes in turn.

Nothing merges a remote's notes until a pull runs: the installer cannot know which remotes share them, and says so when it finishes. The replaces notes of a remote may also be fetched by every `git fetch`, for reading between pulls, by adding a refspec to the remote's configuration:

```
git config --add remote.<remote>.fetch '+refs/notes/replaces*:refs/notes/remotes/<remote>/replaces*'
```

The refspec is a glob so that a remote without replaces notes matches nothing: a refspec that names `refs/notes/replaces` exactly makes every `git fetch` of such a remote fail. The `*` matches the empty string, so the remote's `refs/notes/replaces` is fetched into `refs/notes/remotes/<remote>/replaces`, where the pull fetches it. `--add` keeps the remote's existing refspecs. The target lies under `refs/notes/` so that `git notes` and `git log --notes` accept it, for instance `git log --notes=remotes/<remote>/replaces`. Fetched notes affect the local ref only once a pull merges them.

## Rebasing onto a rewritten upstream

`git pull --rebase` replays the commits of the current branch that its upstream lacks, leaving out those that the upstream once held: commits reachable from the upstream's remote-tracking branch as it was before the fetch, or as its reflog shows it. When the upstream's history was rewritten after the branch was built on it, and neither shows the commits it was built on, for instance because an earlier `git fetch` already moved the remote-tracking branch and its reflog does not reach back far enough, the branch's old versions of the upstream's commits are replayed on top of their own replacements. A replacement that changed a commit's content then conflicts with its old version.

`git rebase-replaces [<upstream>]` rebases the current branch onto `<upstream>`, by default its configured upstream, and leaves out every commit of the branch that the upstream already holds in another form:

1. When `<upstream>` is a remote-tracking branch, it fetches the remote and merges its replaces notes, as `git pull-replaces` does.
2. Of the commits of the branch that the upstream lacks, it leaves out each one for which `git compare-replaces HEAD <upstream>` prints a `held` line, and each one that the upstream's reflog shows it once held.
3. It replays the others onto the upstream through `git rebase -i`, with the commits left out marked `drop` in the list.

A commit that was rewritten as commits the upstream lacks, for instance in a clone whose notes were not pulled, is replayed, and the command says so. A rebase that stops on a conflict continues and ends as any rebase does, and the hook records its rewrites. The command does not fetch when `<upstream>` is a local branch, and uses the notes already in the clone.

## git filter-repo

`git filter-repo` does not run `post-rewrite`, and it passes the commits of notes refs through unchanged. After each run, it leaves in `filter-repo/commit-map` in the git directory the id of each commit it processed and the id of the commit it made from it. `record-filter-repo`, run from inside the checkout after `git filter-repo`, passes that map to `post-rewrite`. Commits that filter-repo left unchanged or deleted record nothing, and running the program again records nothing new.

When filter-repo runs on a repository it has already filtered, it composes the new map with the previous one, mapping each original commit to its newest commit without naming the result of the previous run. `record-filter-repo` keeps a copy of each map once it is recorded, `filter-repo/replaces-index-commit-map`, and from it also records that the newest commit was made from the previous run's result. The operator should therefore run it after each run of filter-repo; otherwise the previous run's results are not related to the newest commits except through the original ones. The copy is not kept when the map could not be recorded, so that running the program again records the whole map; it keeps no abandoned record.

## Known limitations

- Rewrites by tools other than `git commit --amend`, `git rebase` and `git filter-repo` are not recorded, among them `git replay`, `git cherry-pick` and a rewrite made by hand, such as `git reset --soft HEAD~` followed by `git commit`.
- `git rebase --quit` ends a rebase without running `post-rewrite`, so the rewrites the rebase had made are not recorded.
- Notes name commits by id only. A commit that is no longer reachable is eventually removed by garbage collection, after which its id can still be looked up but its contents cannot be read.
- `git notes --ref=replaces prune` must not be run: it removes the notes of every commit the clone lacks, including those of commits made in other clones.
