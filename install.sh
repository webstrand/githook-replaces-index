#!/usr/bin/env bash
# Installs the replaces index into the repository this is run from: cargo install
# puts the programs in bin/ in its common git directory, the repository's config
# registers the post-rewrite hook, so that it runs beside any other post-rewrite
# hooks, and defines the aliases `git whereis`, `git pull-replaces`,
# `git compare-replaces` and `git rebase-replaces`; the skill is linked into
# .claude/skills/ of the checkout. The commands find the git directory each time
# they run, so the repository may move. Running this again updates them.
set -euo pipefail
shopt -s lastpipe
IFS=$'\n\t'
if [ -n "$(git rev-parse --show-superproject-working-tree)" ] && [ "${INSTALL_SUBMODULE:-}" != 1 ]; then
  echo "replaces-index: $(git rev-parse --show-toplevel) is a submodule; run this from the repository that mounts it, or set INSTALL_SUBMODULE=1 to install into the submodule itself" >&2
  exit 1
fi
src=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd -P)
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

skills="$(git rev-parse --show-toplevel)/.claude/skills"
mkdir -p "$skills"
ln -sfT "$(realpath --relative-to="$skills" "$src/skills/replaces-index")" "$skills/replaces-index"

# When this checkout is a submodule of the repository, githook-submodule-roll
# rolls it forward to its latest compatible release, unless the repository's
# attributes say otherwise.
top=$(cd -- "$(git rev-parse --show-toplevel)" && pwd -P)
super=$(git -C "$src" rev-parse --show-superproject-working-tree 2>/dev/null || true)
if [ -n "$super" ] && [ "$(cd -- "$super" && pwd -P)" = "$top" ]; then
  path=${src#"$top"/}
  attributes=$top/.gitattributes
  if [ "$(git -C "$top" check-attr submodule-roll -- "$path")" = "$path: submodule-roll: unspecified" ]; then
    if [ -s "$attributes" ] && [ -n "$(tail -c1 "$attributes")" ]; then
      echo >>"$attributes"
    fi
    printf '/%s submodule-roll\n' "$path" >>"$attributes"
    echo "replaces-index: set submodule-roll on $path in .gitattributes; commit it"
  fi
  hooks=$(git hook list post-checkout 2>/dev/null || true)
  if ! grep -qxF submodule-roll-post-checkout <<<"$hooks"; then
    echo "replaces-index: githook-submodule-roll is not installed; it rolls this submodule forward to its latest compatible release: https://github.com/webstrand/githook-submodule-roll"
  fi
fi
echo "replaces-index: installed into $root/bin"
cat <<'EOF'
replaces-index: rewrites are recorded in this clone only; run 'git pull-replaces <remote>' to merge in those of a remote
replaces-index: for each remote whose rewrites every fetch should bring, configure the refspec:
  git config --add remote.<remote>.fetch '+refs/notes/replaces*:refs/notes/remotes/<remote>/replaces*'
EOF
