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

The programs are written in Rust, each a crate in the directory of its name, in a cargo workspace of their own that shares no crate with other hooks, so that a repository may keep it as a submodule apart from its own build. `skills/replaces-index/` is a Claude Code skill that uses them. `install.sh`, run from inside a checkout, installs them with `cargo install`, which needs a Rust toolchain, into `bin/` in the repository's common git directory, registers `post-rewrite` in the repository's config as `hook.replaces-index`, so that it runs beside any other `post-rewrite` hooks, such as that of session notes, and defines the aliases `git whereis`, `git pull-replaces`, `git compare-replaces` and `git rebase-replaces`. The hook is installed as `githook-replaces-index-post-rewrite`, the program of each alias as `git-<alias>`, and the others as `replaces-index-<program>`. It links the skill into `.claude/skills/replaces-index` in the checkout it is run from; the project may commit the link, so that every checkout and worktree carries it. A bare repository has no checkout to link it into, and gets the programs, the hook and the aliases alone. The checkout must be at a path that is UTF-8, since cargo builds from no other; it refuses one that is not before it installs anything. The hook and each alias name their binary through `$(git rev-parse --git-common-dir)`, which the shell resolves each time git runs them, so the repository may move. It may be run again to update them. Run from inside a submodule, it refuses, since it would install into the submodule's repository rather than the one that mounts it; `INSTALL_SUBMODULE=1` installs into the submodule nonetheless. When the checkout it is run from is a submodule of the repository it installs into, and the repository's attributes give that submodule no `submodule-roll` attribute, it adds a line that sets it to the repository's `.gitattributes`, quoted so that a path with spaces or glob characters matches the submodule alone, which the operator commits, so that [`githook-submodule-roll`](https://github.com/webstrand/githook-submodule-roll) rolls the submodule forward to its latest compatible release; it recommends that hook where it is not installed. Each clone must run it once; worktrees share the programs and the config of their repository. Configured hooks need a git that supports `hook.<name>.event`; the programs are tested with git 2.56.

`notes.rewriteRef` may match `refs/notes/replaces`, as a glob such as `refs/notes/*` set for instance in a global config does. Git then copies the note of `old` to `new` on every amend and rebase, before the hook runs. The lines it copies are among those that `new` inherits, and the hook adds the rest.

| Program | Role |
| - | - |
| `post-rewrite` | records the commits that each amend and rebase made |
| `record` | writes the notes of the commits that a rewrite made, and of the abandoned records |
| `whereis` | `git whereis <commit>`: prints the commits related to a commit by rewrites, and the refs that hold them |
| `compare` | `git compare-replaces <old> <new>`: tells what one chain holds of each commit of another |
| `pull` | `git pull-replaces <remote>`: merges a remote's replaces notes into the local ones |
| `rebase` | `git rebase-replaces [<upstream>]`: rebases the current branch onto a rewritten upstream |
| `record-filter-repo` | records the commits that a `git filter-repo` run made |

The library crate `store` keeps the index that the queries read.

## Recording

Git runs `post-rewrite` after `git commit --amend` and after a rebase, with one line per rewritten commit mapping the old commit to the new one. The hook passes these lines to `record`.

`record` reads the notes of the commits involved, and only those, through one `git cat-file` that looks for each at every depth of fanout of the notes tree, and gives each new commit a `replaces:` line for the commit it was made from and every `replaces:` line of that commit's note, beside the lines it holds already. It writes the notes that changed in a single commit on the ref, through `git fast-import`, and leaves every other note as it was. Since every commit it annotates is one that a rewrite in this clone made, the clone has it, and fast-import keeps the fanout of the notes tree. fast-import replaces a note only at the fanout it derives from the number of notes, while `git notes` gives the trees it writes a fanout of its own, so `record` first removes each note it rewrites from every path it found it at, and the tree never holds two files for one commit. It records only full commit ids as git writes them, lowercase and of the length of the repository's object format: an abbreviated or uppercase one is refused, since no query would match it.

`record` writes the commit through `git fast-import`, without `--force`, which moves `refs/notes/replaces` to it only if the commit holds the one the ref points at: a writer that moved the ref after `record` read it put there a commit that `record`'s does not hold. When git refuses to move the notes ref, `record` reads it again. If another writer, such as a rebase in another worktree, moved it in the meantime, `record` reads the notes again and retries. If it did not move and its lock is held, `record` retries as well: the lock is `refs/notes/replaces.lock` in a repository that keeps refs as files, and `reftable/tables.list.lock`, which locks every ref at once, in one that keeps them in a reftable. A refusal with no lock in sight is taken for one that will not end once it recurs three times in a row, and `record` then fails. Before each retry, `record` waits a random time up to a ceiling that starts at 50 milliseconds and doubles with each retry up to a second, so that writers that collided spread out. It retries for as long as the ref keeps moving, and while its lock has been held for less than 30 seconds, by the time the lock file was written or since `record` first found it, whichever is longer, so that a clock of a network filesystem that runs ahead does not keep it waiting, while one that lags makes it give up early: no write therefore replaces another, and a process that must end sooner is ended by a signal. Git itself gives up on the lock of a ref after 100 milliseconds, and never takes one that a git ended by `SIGKILL` left behind; `record` gives up on such a lock in turn, and the hook keeps the rewrite as an abandoned record, which the next rewrite records once the lock is gone. No git command it runs is given a time limit either, since a repository on a network filesystem may answer slowly.

Once the hook has run for a second, it prints what it waits for every second, until it ends:

```
replaces-index: waiting for git fast-import --quiet (3s; Ctrl-C to abandon)
replaces-index: refs/notes/replaces is locked, retrying (attempt 14, 5s; Ctrl-C to abandon)
replaces-index: refs/notes/replaces keeps moving, retrying (attempt 9, 4s; Ctrl-C to abandon)
```

Git holds the lock of a ref for moments only. Once the lock has been held for ten seconds, the line names it and says that it may have been left behind, as by a git ended by `SIGKILL`, which git itself then refuses to take; `record` keeps retrying until the lock has been held for 30 seconds, and removing a lock that no running git holds lets the write through:

```
replaces-index: refs/notes/replaces is locked, retrying (attempt 21, 12s; Ctrl-C to abandon); <git-dir>/refs/notes/replaces.lock was taken 14s ago; if no git command is running, it was left behind, and removing it lets the write through; the write is given up once it has been held for 30s
```

`replaces-index-record`, `record-filter-repo` and `git pull-replaces` print the same lines, which end with `Ctrl-C to give up` instead, since running them again does what they did not finish.

- An amend records that the new commit was made from the amended one.
- A rebase records the same for each commit it creates.
- A squash or fixup records that the result was made from every commit folded together.
- A commit that git reports as made from itself, as an amend that changes nothing within the same second does, records nothing.
- A commit recreated with the id of a commit it stands in place of, for instance when a commit is amended back to an earlier one, does not name itself. The notes then hold a cycle, which the queries follow without harm.

During a rebase, git reports each fixup or squash step, and each amend that an `exec` line or a stopped rebase makes, as an amend of a commit that the rebase created itself. Such commits never appear outside the rebase. The hook sets these amends aside in the rebase's state directory, and when the rebase ends, follows each commit that the rebase reports through the chain of its amends to the last commit of the chain that is on the branch as the rebase ends: the commits between were replaced by a later amend, and an amend undone during the rebase, for instance by a `git reset` back to the amended commit, is not followed. No commit that the rebase discarded is therefore named. An amend of a commit that was there before the rebase began, such as one that a pick kept as it was or the commit the rebase is onto, is recorded at once instead: the rebase does not report such a commit as rewritten, and a rebase that rewrites nothing else runs no `post-rewrite` when it ends. An amend of the commit that such an amend made is recorded at once in turn, which names the commit between. The exception is the commit that the rebase stopped at for an `edit`, kept pick or not: the rebase reports it as replaced by the commit it finds when it goes on, and names it in `amend` in its state directory, so its amends are set aside as those of any other commit. Git removes the state directory when the rebase ends or is aborted, so an aborted rebase records nothing. The state directory is found through `git rev-parse --git-path`, so each worktree's rebase keeps its own amends.

`git am` keeps its state in `rebase-apply/` as a rebase may, but marks it with an `applying` file rather than `rebasing`, and runs no `post-rewrite` when it ends. An amend while `git am` is stopped is therefore recorded at once, rather than set aside.

When the hook cannot keep track of the amends of a rebase, it says why and records a cruder form instead, in which each replacement holds all the same:

| Failure | What the hook records |
| - | - |
| The state directory cannot be found, or whether the amended commit was there before the rebase cannot be told | the amend, at once |
| The amend cannot be set aside | the amend, at once |
| The state directory cannot be found when the rebase ends | the replacements as git reported them; the amends set aside are not recorded |
| The amends set aside cannot be read when the rebase ends | the replacements as git reported them; the amends set aside are not recorded |
| Whether a commit is on the branch cannot be told when the rebase ends | the replacements as git reported them, and every amend set aside, as it was made |

A cruder form names commits that the rebase discarded, or leaves out the commit that took the place of one it reported. `record-filter-repo` passes on no rebase, and the hook looks for none when it runs.

The lines a note inherits are a copy, for reading one note alone. A clone may rewrite a commit before the note of that commit arrives from another clone, and the new note then inherits nothing from it. The queries do not rely on the copy: they follow the `replaces:` lines from note to note.

## Abandoned rewrites

Git passes a rewrite to `post-rewrite` once only. When the hook cannot record it, for instance because the notes ref is not a commit, its lock was left behind, git refuses to move it for another reason than a lock, or `git fast-import` failed, it keeps the replacements that remained to be recorded as an abandoned record, and prints the error and a command that records them by hand:

```
replaces-index: <error>
replaces-index: the rewrite was kept in <path>; to record it by hand:
printf '%s\n' '<old> <new>' … | "$(git rev-parse --git-common-dir)"/bin/replaces-index-record
replaces-index: the next rewrite records every abandoned record, as does "$(git rev-parse --git-common-dir)"/bin/replaces-index-record --abandoned
```

The hook then exits with a failure status, which git ignores: the rewrite itself is complete. At the end of a rebase, the record holds the replacements as the hook would have recorded them, after they were followed through the amends set aside. A signal that comes while the hook follows them, which no git command can then help it finish, keeps the cruder form instead: the replacements as git reported them, and every amend set aside as it was made.

Each record is a file of `<old> <new>` lines in `replaces-abandoned/` in the common git directory, which the worktrees share. It is named after the time it was written and a random suffix, so that the records sort in the order they were written. The hook writes it under a temporary name that begins with `.`, and renames it, so that a record either holds the whole rewrite or does not exist. When no record can be written, for instance because the disk is full, the hook says so and prints the command all the same. A standard error that cannot be written to, for instance once the terminal is gone, does not keep the record from being written.

A signal that ends the hook, `SIGINT`, `SIGTERM`, `SIGHUP` or `SIGQUIT`, before the notes ref holds the rewrite makes it end the git commands it runs, start no other, and keep what remained to be recorded as an abandoned record. It says so, `unable to note replaces, abandoning state, waiting for disk (60s timeout; signal again to skip)`, waits at most a minute for the record to be written, and prints the command; a second signal ends the wait at once. It then ends as the first signal would have ended it. A signal once the notes ref holds the rewrite ends it at once. A signal that stops the hook, such as that of Ctrl-Z, stops it and its git commands as usual, and they carry on once continued.

Each run of the hook, `record-filter-repo` included, records the abandoned records before its own rewrite, oldest first, so that a later replacement inherits what an earlier one records, and removes them once the notes ref holds them, or held them already. `replaces-index-record` does the same before the rewrite on its standard input, so that the printed command removes the record it came from, and `replaces-index-record --abandoned` records the abandoned records alone. A line that is not a replacement, and a replacement whose new commit the clone lacks, for instance once garbage collection removed it, could never be recorded: they are left out with a warning, and removed with their record. A file whose name begins with `.` is one being written, and is left alone. When the rewrite cannot be recorded, the records stay as they are, and the rewrite is kept in a record of its own. Two runs may record the same records at once: recording a replacement again changes nothing, and a record that the other run removed already is not missed.

## Repair

Lines are only ever added to the notes. A step of the reflog of `refs/notes/replaces` after which a note lacks a line it held before was therefore made by a writer that overwrote another, such as `git notes add` in a git that moves the ref without naming the commit it read, which builds its commit on the notes as they were when it started, or by a note removed by hand. Such a step cannot be told from the history of the ref, since the overwriting commit has the one it overwrote as its parent; only the notes themselves tell.

After each write, the hook, `replaces-index-record` and `git pull-replaces` check the steps of the reflog since the last check, through `git diff-tree` between the commits of each step, and write back, as `record` writes, the lines of each note that a step dropped. They say so:

```
replaces-index: a writer dropped lines from refs/notes/replaces when it moved it from <commit> to <commit>; they are put back
```

The commit up to which the reflog was checked is kept in `replaces-checked` in the common git directory, so that each step is checked once; a run checks the step of its own write, and any of other writers since. A note removed with `git notes remove` is put back as well. A note that a step moved to another fanout, as a writer moves every note once their number crosses a power of 256, is compared with itself by the commit it annotates, and was not dropped. A clone that keeps no reflog of the notes ref, as a bare clone does unless `core.logAllRefUpdates` says otherwise, is not checked, and neither is a step that the reflog no longer holds once it expired.

## Index

The queries read an index of the notes, kept in `replaces/index.redb` in the common git directory. Each clone builds its own and its worktrees share it; the index is never committed or pushed, and may be deleted at any time, after which the next query builds it anew. It is a redb database that holds each `replaces:` line as an edge in both directions, so that both what a commit stands in place of and what was made from it are found without reading the notes.

The index records the notes commit it was built from. Each query opens it, and when the notes ref has moved, whatever moved it, brings it up to date first. Since lines are only ever added to the notes, it reads only the notes that `git diff-tree` reports added or changed since the commit indexed. A notes ref that lost a note, or that does not descend from the commit indexed, is indexed anew. An index built from a notes commit that descends from the one a query read, as when another process moved the ref and brought the index up to date in the meantime, holds every line of it, and is used as it is.

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

1. It fetches the remote's `refs/notes/replaces` into `refs/notes/remotes/<remote>/replaces`, through one `git fetch`, whose output also tells whether the remote has any. The fetch names its refspec itself, so the remote needs no configuration.
2. It merges that into the local `refs/notes/replaces` as `record` writes it: the merge takes the union of each note's lines, which never conflicts, keeps the remote's notes commit as a parent, and moves the ref only from the commit it read, retrying as `record` does. A note that the local ref lacks is written at the fanout of the local notes, rather than at its path in the remote's, whose fanout may differ. When either notes commit holds the other, the ref is left as it is, or moved to the remote's.

Lines are only ever added to notes, so the union is the whole merge. The remote is not changed. Two clones that each record rewrites hold the same notes once each has pulled from the other, after the last rewrite in either; a host that cannot reach the operator's clone receives no notes from it. A clone may pull from several remotes in turn.

Nothing merges a remote's notes until a pull runs: the installer cannot know which remotes share them, and says so when it finishes. The replaces notes of a remote may also be fetched by every `git fetch`, for reading between pulls, by adding a refspec to the remote's configuration:

```
git config --add remote.<remote>.fetch '+refs/notes/replaces*:refs/notes/remotes/<remote>/replaces*'
```

The refspec is a glob so that a remote without replaces notes matches nothing: a refspec that names `refs/notes/replaces` exactly makes every `git fetch` of such a remote fail. The `*` matches the empty string, so the remote's `refs/notes/replaces` is fetched into `refs/notes/remotes/<remote>/replaces`, where the pull fetches it. `--add` keeps the remote's existing refspecs. The target lies under `refs/notes/` so that `git notes` and `git log --notes` accept it, for instance `git log --notes=remotes/<remote>/replaces`. Fetched notes affect the local ref only once a pull merges them.

## Rebasing onto a rewritten upstream

`git pull --rebase` replays the commits of the current branch that its upstream lacks, leaving out those that the upstream once held: commits reachable from the upstream's remote-tracking branch as it was before the fetch, or as its reflog shows it. When the upstream's history was rewritten after the branch was built on it, and neither shows the commits it was built on, for instance because an earlier `git fetch` already moved the remote-tracking branch and its reflog does not reach back far enough, the branch's old versions of the upstream's commits are replayed on top of their own replacements. A replacement that changed a commit's content then conflicts with its old version.

`git rebase-replaces [<upstream>]` rebases the current branch onto `<upstream>`, by default its configured upstream, and leaves out every commit of the branch that the upstream already holds in another form:

1. When `<upstream>` is a remote-tracking branch, it fetches the remote, and its replaces notes in the same fetch, and merges them as `git pull-replaces` does.
2. Of the commits of the branch that the upstream lacks, it leaves out each one for which `git compare-replaces HEAD <upstream>` prints a `held` line, and each one that the upstream's reflog shows it once held.
3. It names each commit it leaves out, and why: the upstream commits made from it, or the upstream's reflog.
4. It replays the others onto the upstream through `git rebase -i`, with the commits left out marked `drop` in the list.

With `rebase.autoSquash`, `git rebase -i` lists each `fixup!` or `squash!` commit after the commit it folds into. One that the upstream holds in another form is left out as well. One that it does not, but that would fold into a commit left out, has nowhere to go: it is no part of the upstream, and folded into the commit before it, it would change another. The command then refuses the rebase before it starts, names the commits, and changes nothing.

A commit that was rewritten as commits the upstream lacks, for instance in a clone whose notes were not pulled, is replayed, and the command says so. A rebase that stops on a conflict continues and ends as any rebase does, and the hook records its rewrites. The command does not fetch when `<upstream>` is a local branch, and uses the notes already in the clone.

## git filter-repo

`git filter-repo` does not run `post-rewrite`, and it passes the commits of notes refs through unchanged. After each run, it leaves in `filter-repo/commit-map` in the git directory the id of each commit it processed and the id of the commit it made from it. `record-filter-repo`, run from inside the checkout after `git filter-repo`, passes that map to `post-rewrite`. Commits that filter-repo left unchanged or deleted record nothing, and running the program again records nothing new.

When filter-repo runs on a repository it has already filtered, it composes the new map with the previous one, mapping each original commit to its newest commit without naming the result of the previous run. `record-filter-repo` keeps a copy of each map once it is recorded, `filter-repo/replaces-index-commit-map`, and from it also records that the newest commit was made from the previous run's result. The operator should therefore run it after each run of filter-repo; otherwise the previous run's results are not related to the newest commits except through the original ones. The copy is not kept when the map could not be recorded, so that running the program again records the whole map; it keeps no abandoned record.

## Known limitations

- Rewrites by tools other than `git commit --amend`, `git rebase` and `git filter-repo` are not recorded, among them `git replay`, `git cherry-pick` and a rewrite made by hand, such as `git reset --soft HEAD~` followed by `git commit`.
- `git rebase --quit` ends a rebase without running `post-rewrite`, so the rewrites the rebase had made are not recorded.
- Notes name commits by id only. A commit that is no longer reachable is eventually removed by garbage collection, after which its id can still be looked up but its contents cannot be read.
- A rebase interrupted while its `post-rewrite` hooks run, for instance by Ctrl-C, has already rewritten the branch, but leaves `rebase-merge/` behind, so that `git status` reports a rebase under way. The operator must end it with `git rebase --quit`, which keeps the branch as it is: `git rebase --abort` would reset the branch to where it was before the rebase.
- A hook ended by `SIGKILL`, which cannot be handled, loses the rewrite it was recording.
- `git notes` moves a notes ref without naming the commit it moved it from, so that a `git notes` command on `refs/notes/replaces`, or a note that git copies to it on a rewrite when `notes.rewriteRef` matches it, can drop a note that the hook writes at the same time. Every program of the replaces index moves the ref only from the commit it read, and the next one to write puts dropped lines back, as long as the reflog of the notes ref holds the step that dropped them.
- `git notes --ref=replaces prune` must not be run: it removes the notes of every commit the clone lacks, including those of commits made in other clones.
