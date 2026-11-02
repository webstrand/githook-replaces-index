#!/usr/bin/env bash
# Installs the replaces index into the repository this is run from. cargo install
# puts the programs in bin/ in its common git directory. The repository's config
# registers the post-rewrite hook, so that it runs beside any other post-rewrite
# hooks, and defines the aliases `git whereis`, `git pull-replaces`,
# `git compare-replaces`, `git rebase-replaces` and `git gc-replaces`. The skill
# is linked into .claude/skills/ of the checkout. The commands find the git
# directory each time they run, so the repository may move. Running this again
# updates them. It needs no more than bash 3.2 and the tools that macOS ships.
set -euo pipefail
IFS=$'\n\t'
if [ -n "$(git rev-parse --show-superproject-working-tree)" ] && [ "${INSTALL_SUBMODULE:-}" != 1 ]; then
  echo "replaces-index: $(git rev-parse --show-toplevel) is a submodule; run this from the repository that mounts it, or set INSTALL_SUBMODULE=1 to install into the submodule itself" >&2
  exit 1
fi
src=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd -P)
# cargo builds only from a path that is UTF-8, and would fail once the first
# program is installed.
if ! printf '%s' "$src" | iconv -f UTF-8 -t UTF-8 >/dev/null 2>&1; then
  echo "replaces-index: cargo cannot build from $src, whose path is not UTF-8; move the checkout to one that is" >&2
  exit 1
fi
root=$(git rev-parse --path-format=absolute --git-common-dir)

for crate in post-rewrite whereis pull compare rebase record record-filter-repo; do
  cargo install --locked --no-track --force --quiet --root "$root" --path "$src/$crate"
done
# shellcheck disable=SC2016
bin='"$(git rev-parse --git-common-dir)"/bin'
git config hook.replaces-index.event post-rewrite
git config hook.replaces-index.command "$bin/githook-replaces-index-post-rewrite"
git config alias.whereis "!$bin/git-whereis"
git config alias.pull-replaces "!$bin/git-pull-replaces"
git config alias.compare-replaces "!$bin/git-compare-replaces"
git config alias.rebase-replaces "!$bin/git-rebase-replaces"
git config alias.gc-replaces "!$bin/replaces-index-record --gc"
# A fetch refspec such as +refs/notes/*:refs/notes/* replaces the notes on each
# fetch; it is named, with the command that fetches the notes beside them instead.
"$root/bin/replaces-index-record" --check-refspecs

# The path of directory $2 relative to directory $1, both absolute and free of
# symbolic links.
relative() {
  local from=$1/ to=$2/ up=
  while [ "${to#"$from"}" = "$to" ]; do
    from=${from%/*/}/
    up=../$up
  done
  to=$up${to#"$from"}
  printf '%s\n' "${to%/}"
}

# A gitattributes pattern that matches the path $1, relative to the top of the
# work tree, alone: C-quoted, so that it may hold spaces and bytes that are not
# UTF-8, with its glob characters escaped.
pattern() {
  local LC_ALL=C path=$1 quoted= i char code
  for ((i = 0; i < ${#path}; i++)); do
    char=${path:i:1}
    printf -v code '%d' "'$char"
    case $char in
      # A backslash escapes a glob character, and is itself escaped by quoting.
      '*' | '?' | '[') quoted+='\\'$char ;;
      '\') quoted+='\\\\' ;;
      '"') quoted+='\"' ;;
      *) if ((code < 32 || code > 126)); then
        printf -v char '\\%03o' $((code & 255))
      fi
        quoted+=$char ;;
    esac
  done
  printf '"/%s"\n' "$quoted"
}

if [ "$(git rev-parse --is-inside-work-tree)" != true ]; then
  echo "replaces-index: $root belongs to no work tree, so the skill is not linked into one"
else
  skills="$(git rev-parse --show-toplevel)/.claude/skills"
  mkdir -p "$skills"
  skills=$(cd -- "$skills" && pwd -P)
  link=$skills/replaces-index
  if [ -e "$link" ] && [ ! -L "$link" ]; then
    echo "replaces-index: $link is in the way of the skill's link; remove it and run this again" >&2
    exit 1
  fi
  rm -f "$link"
  ln -s "$(relative "$skills" "$src/skills/replaces-index")" "$link"

  # When this checkout is a submodule of the repository, githook-submodule-roll
  # rolls it forward to its latest compatible release, unless the repository's
  # attributes say otherwise.
  top=$(cd -- "$(git rev-parse --show-toplevel)" && pwd -P)
  super=$(git -C "$src" rev-parse --show-superproject-working-tree 2>/dev/null || true)
  if [ -n "$super" ] && [ "$(cd -- "$super" && pwd -P)" = "$top" ]; then
    path=${src#"$top"/}
    attributes=$top/.gitattributes
    # `<path>\0<attribute>\0<value>\0`, the path unquoted whatever its bytes.
    { IFS= read -r -d '' _ && IFS= read -r -d '' _ && IFS= read -r -d '' value; } \
      < <(git -C "$top" check-attr -z submodule-roll -- "$path")
    if [ "$value" = unspecified ]; then
      if [ -s "$attributes" ] && [ -n "$(tail -c1 "$attributes")" ]; then
        echo >>"$attributes"
      fi
      printf '%s submodule-roll\n' "$(pattern "$path")" >>"$attributes"
      echo "replaces-index: set submodule-roll on $path in .gitattributes; commit it"
    fi
    hooks=$(git hook list post-checkout 2>/dev/null || true)
    if ! grep -qxF submodule-roll-post-checkout <<<"$hooks"; then
      echo "replaces-index: githook-submodule-roll is not installed; it rolls this submodule forward to its latest compatible release: https://github.com/webstrand/githook-submodule-roll"
    fi
  fi
fi
echo "replaces-index: installed into $root/bin"
cat <<'EOF'
replaces-index: rewrites are recorded in this clone only; run 'git pull-replaces <remote>' to merge in those of a remote
replaces-index: for each remote whose rewrites every fetch should bring, configure the refspec:
  git config --add remote.<remote>.fetch '+refs/notes/replaces*:refs/notes/remotes/<remote>/replaces*'
EOF
