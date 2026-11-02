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

The programs are written in Rust, each a crate in the directory of its name, in a cargo workspace of their own that shares no crate with other hooks, so that a repository may keep it as a submodule apart from its own build. `skills/replaces-index/` is a Claude Code skill that uses them. `install.sh`, run from inside a checkout, installs them with `cargo install`, which needs a Rust toolchain, into `bin/` in the repository's common git directory, registers `post-rewrite` in the repository's config as `hook.replaces-index`, so that it runs beside any other `post-rewrite` hooks, such as that of session notes, and defines the aliases `git whereis`, `git pull-replaces`, `git compare-replaces`, `git rebase-replaces` and `git gc-replaces`. The hook is installed as `githook-replaces-index-post-rewrite`, the program of each alias as `git-<alias>`, and the others as `replaces-index-<program>`. It links the skill into `.claude/skills/replaces-index` in the checkout it is run from; the project may commit the link, so that every checkout and worktree carries it. A bare repository has no checkout to link it into, and gets the programs, the hook and the aliases alone. The checkout must be at a path that is UTF-8, since cargo builds from no other; it refuses one that is not before it installs anything. The hook and each alias name their binary through `$(git rev-parse --git-common-dir)`, which the shell resolves each time git runs them, so the repository may move. It may be run again to update them. Run from inside a submodule, it refuses, since it would install into the submodule's repository rather than the one that mounts it; `INSTALL_SUBMODULE=1` installs into the submodule nonetheless. When the checkout it is run from is a submodule of the repository it installs into, and the repository's attributes give that submodule no `submodule-roll` attribute, it adds a line that sets it to the repository's `.gitattributes`, quoted so that a path with spaces or glob characters matches the submodule alone, which the operator commits, so that [`githook-submodule-roll`](https://github.com/webstrand/githook-submodule-roll) rolls the submodule forward to its latest compatible release; it recommends that hook where it is not installed. Each clone must run it once; worktrees share the programs and the config of their repository. Configured hooks need a git that supports `hook.<name>.event`; the programs are tested with git 2.56.

`notes.rewriteRef` may match `refs/notes/replaces`, as a glob such as `refs/notes/*` set for instance in a global config does. Git then copies the note of `old` to `new` on every amend and rebase, before the hook runs. The lines it copies are among those that `new` inherits, and the hook adds the rest.

| Program | Role |
| - | - |
| `post-rewrite` | records the commits that each amend and rebase made |
| `record` | writes the notes of the commits that a rewrite made, and of the abandoned records |
| `whereis` | `git whereis <commit>`: prints the commits related to a commit by rewrites, and the refs that hold them |
| `compare` | `git compare-replaces <old> <new>`: tells what one chain holds of each commit of another |
| `pull` | `git pull-replaces <remote>`: merges a remote's replaces notes into the local ones |
| `rebase` | `git rebase-replaces [--keep-base] [<upstream>]`: rebases the current branch onto a rewritten upstream |
| `record-filter-repo` | records the commits that a `git filter-repo` run made |

The library crate `store` keeps the index that the queries read.

## Terms

- An *amend* is a run of `git commit --amend`, by the operator, by an `exec` line of a rebase or by a test, and nothing else.
- A *single-commit rewrite* is a run of `post-rewrite` that git passes the kind `amend`. Git runs one after an amend, and during a rebase also after each fixup or squash step.
- A *replacement* is one `<old> <new>` line that git passes to `post-rewrite`. Its old commit is the *rewritten commit*, and its new commit is the commit that the rewrite made.
- A *deferred replacement* is the replacement of a single-commit rewrite during a rebase that the hook keeps in `replaces-index-deferred` in the rebase's state directory until the rebase ends. A replacement that the hook records when git reports it is *recorded at once*.
- *Git's `rebase-merge/amend` file* is the file of the rebase's state directory in which git names the commit that the rebase stopped at for an `edit`.
- An *`amend!` commit* is a commit whose subject begins with `amend!`, which `git rebase --autosquash` folds into the commit it names.

## Recording

Git runs `post-rewrite` after `git commit --amend` and after a rebase, with one line per rewritten commit mapping the old commit to the new one. The hook passes these lines to `record`.

`record` reads the notes of the commits involved, and only those, through one `git cat-file` that looks for each at every depth of fanout of the notes tree, and gives each new commit a `replaces:` line for the commit it was made from and every `replaces:` line of that commit's note, beside the lines it holds already. It writes the notes that changed in a single commit on the ref, through `git fast-import`, and leaves every other note as it was. Since every commit it annotates is one that a rewrite in this clone made, the clone has it, and fast-import keeps the fanout of the notes tree. fast-import replaces a note only at the fanout it derives from the number of notes, while `git notes` gives the trees it writes a fanout of its own, so `record` first removes each note it rewrites from every path it found it at, and the tree never holds two files for one commit. It records only full commit ids as git writes them, lowercase and of the length of the repository's object format: an abbreviated or uppercase one is refused, since no query would match it.

`record` writes the commit through `git fast-import`, without `--force`, which moves `refs/notes/replaces` to it only if the commit holds the one the ref points at: a writer that moved the ref after `record` read it put there a commit that `record`'s does not hold. When git refuses to move the notes ref, `record` reads it again. If another writer, such as a rebase in another worktree, moved it in the meantime, `record` reads the notes again and retries. If it did not move and its lock is held, `record` retries as well: the lock is `refs/notes/replaces.lock` in a repository that keeps refs as files, and `reftable/tables.list.lock`, which locks every ref at once, in one that keeps them in a reftable. A refusal with no lock in sight is taken for one that will not end once it recurs three times in a row, and `record` then fails. Before each retry, `record` waits a random time up to a ceiling that starts at 50 milliseconds and doubles with each retry up to a second, so that writers that collided spread out. It retries for as long as the ref keeps moving, and while it has watched the same lock held for less than 30 seconds, by its own clock: no write therefore replaces another, and a process that must end sooner is ended by a signal. Git itself gives up on the lock of a ref after 100 milliseconds, and never takes one that a git ended by `SIGKILL` left behind; `record` gives up on such a lock in turn, and the hook keeps the rewrite as an abandoned record, which the next rewrite records once the lock is gone. No git command it runs is given a time limit either, since a repository on a network filesystem may answer slowly.

Once the hook has run for a second, it prints what it waits for every second, until it ends:

```
replaces-index: waiting for git fast-import --quiet (3s; Ctrl-C to abandon)
replaces-index: refs/notes/replaces is locked, retrying (attempt 14, 5s; Ctrl-C to abandon)
replaces-index: refs/notes/replaces keeps moving, retrying (attempt 9, 4s; Ctrl-C to abandon)
```

Git holds the lock of a ref for moments only. Once `record` has watched the lock held for ten seconds, the line names it and says that it may have been left behind, as by a git ended by `SIGKILL`, which git itself then refuses to take; `record` keeps retrying until it has watched it for 30 seconds, and removing a lock that no running git holds lets the write through:

```
replaces-index: refs/notes/replaces is locked, retrying (attempt 21, 12s; Ctrl-C to abandon); <git-dir>/refs/notes/replaces.lock has been held for 12s; if no git command is running, it was left behind, and removing it lets the write through; the write is given up once it has been held for 30s
```

`replaces-index-record`, `record-filter-repo` and `git pull-replaces` print the same lines, which end with `Ctrl-C to give up` instead, since running them again does what they did not finish.

- An amend records that the new commit was made from the commit it rewrote.
- A rebase records the same for each commit it creates.
- A squash or fixup records that the result was made from every commit folded together.
- A commit that git reports as made from itself, as an amend that changes nothing within the same second does, records nothing.
- A commit recreated with the id of a commit it stands in place of, for instance when a commit is amended back to an earlier one, does not name itself. The notes then hold a cycle, which the queries follow without harm.

During a rebase, git reports each fixup or squash step, and each amend that an `exec` line or the operator at a stop of the rebase runs, as a single-commit rewrite of a commit that the rebase created itself. Such commits never appear outside the rebase. The hook defers the replacements of these rewrites in the rebase's state directory, and when the rebase ends, follows each commit that the rebase reports through the chain of its deferred replacements to the last commit of the chain that is on the branch as the rebase ends: the commits between were replaced in turn, and a replacement undone during the rebase, for instance by a `git reset` back to the rewritten commit, is not followed. No commit that the rebase discarded is therefore named. A single-commit rewrite of a commit that was there before the rebase began, such as one that a pick kept as it was or the commit the rebase is onto, is recorded at once instead: the rebase does not report such a commit as rewritten, and a rebase that rewrites nothing else runs no `post-rewrite` when it ends. A single-commit rewrite of the commit that such a rewrite made is recorded at once in turn, which names the commit between. The exception is the commit that the rebase stopped at for an `edit`, kept pick or not: the rebase reports it as replaced by the commit it finds when it goes on, and names it in git's `rebase-merge/amend` file, so its replacements are deferred as those of any other commit. Git removes the state directory when the rebase ends or is aborted, so an aborted rebase records nothing. The state directory is found through `git rev-parse --git-path`, so each worktree's rebase keeps its own deferred replacements.

`git am` keeps its state in `rebase-apply/` as a rebase may, but marks it with an `applying` file rather than `rebasing`, and runs no `post-rewrite` when it ends. An amend while `git am` is stopped is therefore recorded at once, rather than deferred.

When the hook cannot keep track of the deferred replacements of a rebase, it says why and records a cruder form instead, in which each replacement holds all the same:

| Failure | What the hook records |
| - | - |
| The state directory cannot be found, or whether the rewritten commit was there before the rebase cannot be told | the replacement, at once |
| The replacement cannot be deferred | the replacement, at once |
| The state directory cannot be found when the rebase ends | the replacements as git reported them; the deferred replacements are not recorded |
| The deferred replacements cannot be read when the rebase ends | the replacements as git reported them; the deferred replacements are not recorded |
| Whether a commit is on the branch cannot be told when the rebase ends | the replacements as git reported them, and every deferred replacement, as it was made |

A cruder form names commits that the rebase discarded, or leaves out the commit that took the place of one it reported. `record-filter-repo` passes on no rebase, and the hook looks for none when it runs.

The lines a note inherits are a copy, for reading one note alone. A clone may rewrite a commit before the note of that commit arrives from another clone, and the new note then inherits nothing from it. The queries do not rely on the copy: they follow the `replaces:` lines from note to note.

## Abandoned rewrites

Git passes a rewrite to `post-rewrite` once only. When the hook cannot record it, for instance because the notes ref is not a commit, its lock was left behind, git refuses to move it for another reason than a lock, `git fast-import` failed, or the hook panicked, it keeps the replacements that remained to be recorded as an abandoned record, and prints the error and a command that records them by hand:

```
replaces-index: <error>
replaces-index: the rewrite was kept in <path>; to record it by hand:
printf '%s\n' '<old> <new>' … | "$(git rev-parse --git-common-dir)"/bin/replaces-index-record
replaces-index: the next rewrite records every abandoned record, as does "$(git rev-parse --git-common-dir)"/bin/replaces-index-record --abandoned
```

The hook then exits with a failure status, which git ignores: the rewrite itself is complete. At the end of a rebase, the record holds the replacements as the hook would have recorded them, after they were followed through the deferred replacements. A signal that comes while the hook follows them, which no git command can then help it finish, keeps the cruder form instead: the replacements as git reported them, and every deferred replacement as it was made.

Each record is a file of `<old> <new>` lines in `replaces-abandoned/` in the common git directory, which the worktrees share. It is named after the time it was written and a random suffix, so that the records sort in the order they were written. The hook writes it under a temporary name that begins with `.`, and renames it, so that a record either holds the whole rewrite or does not exist. When no record can be written, for instance because the disk is full, the hook says so and prints the command all the same. A standard error that cannot be written to, for instance once the terminal is gone, does not keep the record from being written.

A signal that ends the hook, `SIGINT`, `SIGTERM`, `SIGHUP` or `SIGQUIT`, before the notes ref holds the rewrite makes it end the git commands it runs, start no other, and keep what remained to be recorded as an abandoned record. It says so, `unable to note replaces, abandoning state, waiting for disk (60s timeout; signal again to skip)`, waits at most a minute for the record to be written, and prints the command; a second signal ends the wait at once. It then ends as the first signal would have ended it. A signal once the notes ref holds the rewrite ends it at once. A signal that stops the hook, such as that of Ctrl-Z, stops it and its git commands as usual, and they carry on once continued.

Each run of the hook, `record-filter-repo` included, records the abandoned records before its own rewrite, oldest first, so that a later replacement inherits what an earlier one records, and removes them once the notes ref holds them, or held them already. `replaces-index-record` does the same before the rewrite on its standard input, so that the printed command removes the record it came from, and `replaces-index-record --abandoned` records the abandoned records alone. A line that is not a replacement, and a replacement whose new commit the clone lacks, for instance once garbage collection removed it, could never be recorded: they are left out with a warning, and removed with their record. A record that cannot be read, such as a directory or a file without read permission, is left out with a warning and left where it is, and keeps no other record from being recorded. A file whose name begins with `.` is one being written, and is left alone. When the rewrite cannot be recorded, the records stay as they are, and the rewrite is kept in a record of its own. Two runs may record the same records at once: recording a replacement again changes nothing, and a record that the other run removed already is not missed.

## Repair

A recorded rewrite holds for as long as its commits do, so the note of a commit the clone has never loses a line. A step of the reflog of `refs/notes/replaces` after which such a note lacks a line it held before was therefore made by a writer that overwrote another, such as a fetch through a refspec that forces the ref, `git notes add -f`, a `git notes merge` that keeps one side, or `git notes add` in a git that moves the ref without naming the commit it read, which builds its commit on the notes as they were when it started; or the note was removed by hand. Such a step cannot be told from the history of the ref, since the overwriting commit has the one it overwrote as its parent; only the notes themselves tell.

`git gc-replaces` checks the steps of the reflog since the last check, through `git diff-tree` between the commits of each step, and writes back, as `record` writes, the lines of each such note that a step dropped. It says so:

```
gc-replaces: a writer dropped lines from refs/notes/replaces when it moved it from <commit> to <commit>; they are put back
```

The commit up to which the reflog was checked is kept in the ref `refs/replaces-index/checked`, so that each step is checked once. The ref keeps the commit from garbage collection: once the reflog expired and no longer holds it, the steps from it to the oldest commit the reflog holds are checked as one, which finds what they dropped but not a line that came and went between the two. A note removed with `git notes remove` is put back as well.

The note of a commit that the clone no longer has, once garbage collection removed the commit, may go, as `git notes prune` removes it. Its lines are then added to the notes that name the commit in a `replaces:` line, those of the commits made from it, which may lack some, for instance when its note arrived from another clone after they were made; what it recorded is thereby kept, and the note stays removed:

```
gc-replaces: the note of <commit>, which this clone no longer has, lost lines; the notes of <commit> …, which name it, hold them
```

A note of a commit that the clone lacks and that no note names is the only record of what it holds, such as one pulled from a clone whose commits were never fetched, and is written back as that of a commit the clone has.

A note that a step moved to another fanout, as a writer moves every note once their number crosses a power of 256, is compared with itself by the commit it annotates, and was not dropped. A clone that keeps no reflog of the notes ref, as a bare clone does unless `core.logAllRefUpdates` says otherwise, is not checked.

Each rewrite leaves the notes of the commits it replaced, which garbage collection removes once nothing reaches them, and each note copies the lines it inherits, so that the notes of a branch rebased many times grow with every rebase. `git gc-replaces`, which the operator runs, removes the notes of the commits that the clone no longer has, in one commit on the ref, written as `record` writes. The lines of each go first to the notes of the commits the clone has that name it, directly or through other notes removed, so that nothing it recorded is lost, and the repair that follows the removal takes it for one that loses nothing. A note that no such note names is the only record of what it holds, and stays. The index is built anew at the next query, without the notes removed.

A fetch refspec with a `+` that maps a ref of a remote to `refs/notes/replaces`, as `+refs/notes/*:refs/notes/*` set to share other notes does, replaces the local notes with the remote's on each fetch, and drops every line that the clone recorded since it last pulled. While such a refspec exists, every command refuses to run, and names it on standard error with the command that maps its destinations under `refs/notes/remotes/<remote>/` instead, where the remote's notes are kept beside the local ones; for `+refs/notes/*:refs/notes/*`, `git config --replace-all --fixed-value 'remote.<remote>.fetch' '+refs/notes/*:refs/notes/remotes/<remote>/*' '+refs/notes/*:refs/notes/*'`. The hook, which runs once the rewrite is made, records it all the same and names the refspec. `install.sh` names it as well. Nothing is changed unasked. A negative refspec such as `^refs/notes/replaces` does not serve: git applies it to the remote's ref, which the refspec that fetches the notes beside the local ones fetches as well.

## Index

The queries read an index of the notes, kept in `replaces/index.redb` in the common git directory. Each clone builds its own and its worktrees share it; the index is never committed or pushed, and may be deleted at any time, after which the next query builds it anew. A query that cannot read it as a database fails, and says that no data has been lost and that deleting the file and running the command again rebuilds it. It does not delete the file itself, since the damage most likely comes of something outside the programs, such as a network filesystem or a lock that failed to keep another writer out, which the operator should see. It is a redb database that holds each `replaces:` line as an edge in both directions, so that both what a commit stands in place of and what was made from it are found without reading the notes.

The index records the notes commit it was built from. Each query opens it, and when the notes ref has moved, whatever moved it, brings it up to date first. Since lines are only ever added to the notes, it reads only the notes that `git diff-tree` reports added or changed since the commit indexed. A notes ref that lost a note, or that does not descend from the commit indexed, is indexed anew. An index built from a notes commit that descends from the one a query read, as when another process moved the ref and brought the index up to date in the meantime, holds every line of it, and is used as it is.

Queries open the database read-only, which any number of processes may do at once. Bringing it up to date needs it alone, and waits up to ten seconds while other processes hold it. A query that finds no index builds one under a name of its own, `.index.redb.<pid>`, and renames it into place, so that another never opens one half made; a query that has the file open when it is deleted goes on with it.

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

1. It fetches the remote's `refs/notes/replaces`, alone, into the ref that the remote's fetch refspecs bring it into, through one `git fetch`, whose output also tells whether the remote has any. A remote whose refspecs bring the notes into no ref, or into `refs/notes/replaces` itself, is refused, with the command that configures the refspec below.
2. It merges that into the local `refs/notes/replaces` as `record` writes it: the merge takes the union of each note's lines, which never conflicts, keeps the remote's notes commit as a parent, and moves the ref only from the commit it read, retrying as `record` does. A note that the local ref lacks is written at the fanout of the local notes, rather than at its path in the remote's, whose fanout may differ. When either notes commit holds the other, the ref is left as it is, or moved to the remote's.

`<remote>` may also be a URL or a path, as `git pull` takes: the notes are then fetched into a ref of their own, which is deleted once they are merged, so that nothing of such a remote is kept, as `git pull` keeps nothing of it but `FETCH_HEAD`. A signal that ends the pull, `SIGINT`, `SIGTERM`, `SIGHUP` or `SIGQUIT`, ends the git commands it runs and deletes the ref first. A pull ended by `SIGKILL` or a crash leaves the ref behind; `git gc-replaces` deletes it once it is older than `gc.pruneExpire`. Its age is told by the reflog that the pull writes for it, and measured from the newest entry of that reflog and those of `HEAD` and `refs/notes/replaces` rather than from the clock of the host that runs `git gc-replaces`, since a host that shares the repository on a network filesystem may be pulling, and its clock may differ.

Lines are only ever added to notes, so the union is the whole merge. The remote is not changed. Two clones that each record rewrites hold the same notes once each has pulled from the other, after the last rewrite in either; a host that cannot reach the operator's clone receives no notes from it. A clone may pull from several remotes in turn.

Nothing merges a remote's notes until a pull runs: the installer cannot know which remotes share them, and says so when it finishes. The operator must configure, for each remote whose notes the clone shares, a refspec that brings them beside the local ones, so that every `git fetch` brings them as well:

```
git config --add remote.<remote>.fetch '+refs/notes/replaces*:refs/notes/remotes/<remote>/replaces*'
```

The refspec is a glob so that a remote without replaces notes matches nothing: a refspec that names `refs/notes/replaces` exactly makes every `git fetch` of such a remote fail. The `*` matches the empty string, so the remote's `refs/notes/replaces` is fetched into `refs/notes/remotes/<remote>/replaces`, where the pull fetches it. `--add` keeps the remote's existing refspecs. The target lies under `refs/notes/` so that `git notes` and `git log --notes` accept it, for instance `git log --notes=remotes/<remote>/replaces`. Fetched notes affect the local ref only once a pull merges them.

## Rebasing onto a rewritten upstream

`git pull --rebase` replays the commits of the current branch that its upstream lacks, leaving out those that the upstream once held: commits reachable from the upstream's remote-tracking branch as it was before the fetch, or as its reflog shows it. When the upstream's history was rewritten after the branch was built on it, and neither shows the commits it was built on, for instance because an earlier `git fetch` already moved the remote-tracking branch and its reflog does not reach back far enough, the branch's old versions of the upstream's commits are replayed on top of their own replacements. A replacement that changed a commit's content then conflicts with its old version.

`git rebase-replaces [<upstream>]` rebases the current branch onto `<upstream>`, by default its configured upstream, and leaves out every commit of the branch that the upstream already holds in another form:

1. When `<upstream>` is a remote-tracking branch, it fetches the remote as `git fetch <remote>` does, which brings its replaces notes through the refspec of [Remotes](#remotes), and merges them as `git pull-replaces` does. A remote without that refspec is refused, as `git pull-replaces` refuses it.
2. Of the commits of the branch that the upstream lacks, it leaves out each one for which `git compare-replaces HEAD <upstream>` prints a `held` line, and, when no `<upstream>` is named, each one that the configured upstream's reflog shows it once held, as `git rebase` uses its fork point only when it is named no upstream either: a named upstream is taken as it is.
3. It names each commit it leaves out, and why: the upstream commits made from it, or the upstream's reflog.
4. It replays the others onto the upstream through `git rebase -i`, with the commits left out marked `drop` in the list.

`git rebase-replaces --keep-base [<upstream>]` restores the history that the branch shared with the upstream, without taking what the upstream added since: it replays the others onto the newest of the merge base and the upstream commits that replace the commits left out, as `git rebase --keep-base` replays them onto the merge base. That commit must descend from all the others; when the replacements lie on lines of history apart, the command refuses, and changes nothing.

The rebase runs with `--no-autosquash`, whatever `rebase.autoSquash` says: a `fixup!`, `squash!` or `amend!` commit is replayed as a commit of its own, since folding it into a commit that the upstream holds would part the branch from the upstream again. One that the upstream holds in another form is left out as any other. One that names a commit left out, by its subject or an abbreviation of its id, as autosquash finds it, is reworded to name the commit of the upstream that replaces it, in the same form, so that the operator may fold it in later with `git rebase -i --autosquash`. When no single commit replaces it, it is replayed as it is, and the command says so.

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
