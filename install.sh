#!/usr/bin/env bash
# Installs the replaces index into the repository this is run from: cargo install
# puts the programs in bin/ in its common git directory, and the repository's
# config registers the post-rewrite hook, so that it runs beside any other
# post-rewrite hooks. The command finds the git directory each time it runs, so
# the repository may move. Running this again updates them.
set -euo pipefail
shopt -s lastpipe
IFS=$'\n\t'
if [ -n "$(git rev-parse --show-superproject-working-tree)" ] && [ "${INSTALL_SUBMODULE:-}" != 1 ]; then
  echo "replaces-index: $(git rev-parse --show-toplevel) is a submodule; run this from the repository that mounts it, or set INSTALL_SUBMODULE=1 to install into the submodule itself" >&2
  exit 1
fi
src=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd -P)
root=$(git rev-parse --path-format=absolute --git-common-dir)

for crate in post-rewrite record record-filter-repo; do
  cargo install --locked --no-track --force --quiet --root "$root" --path "$src/$crate"
done
# shellcheck disable=SC2016
bin='"$(git rev-parse --git-common-dir)"/bin'
git config hook.replaces-index.event post-rewrite
git config hook.replaces-index.command "$bin/githook-replaces-index-post-rewrite"

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
