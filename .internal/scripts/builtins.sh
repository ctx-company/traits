#!/usr/bin/env bash
# Rebuild every embedded built-in trait and scaffold template from source.
#
#   builtins.sh build   rebuild in place
#   builtins.sh check   rebuild into a scratch tree and fail on any difference
#
# Internal tooling: these packages ship inside the binary, so nothing a user
# runs can rebuild them, and nothing else notices when a source edit never
# reaches its committed canonical. critique-trait sat unbuildable for an
# unknown stretch because of exactly that.
#
# Why the scratch root: a package under modules/builtin is unreachable
# by id (`ctx traits build <id>` resolves through .ctx/traits/authored), and
# building it by source path mis-resolves its `../spec` dependency relative to
# source/ instead of the package root (task 0230). Copying the tree into a
# throwaway authored root sidesteps both until that is fixed.
set -euo pipefail

MODE="${1:-build}"
case "$MODE" in
  build | check) ;;
  *)
    echo "usage: builtins.sh [build|check]" >&2
    exit 2
    ;;
esac

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BUILTINS="$REPO_ROOT/modules/builtin"
CTX="${CTX_BIN:-$REPO_ROOT/target/debug/ctx}"

if [[ ! -x "$CTX" ]]; then
  echo "builtins.sh: no ctx binary at $CTX — run 'cargo build -p ctx-traits-cli' first" >&2
  exit 1
fi

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/ctx-builtins-XXXXXX")"
trap 'rm -rf "$SCRATCH"' EXIT

git -C "$SCRATCH" init -q .
ln -s "$REPO_ROOT/node_modules" "$SCRATCH/node_modules"
(cd "$SCRATCH" && "$CTX" traits init >/dev/null)

AUTHORED="$SCRATCH/.ctx/traits/authored"
for package in "$BUILTINS"/traits/*/ "$BUILTINS"/templates/*/ "$BUILTINS"/shared/*/; do
  cp -R "${package%/}" "$AUTHORED/"
done

# spec first: the other built-ins declare it as a path dependency, and a
# dependency has to be resolvable before its dependents build.
ORDER=(spec)
for package in "$AUTHORED"/*/; do
  id="$(basename "$package")"
  [[ "$id" == "spec" ]] || ORDER+=("$id")
done

status=0
for id in "${ORDER[@]}"; do
  if ! (cd "$SCRATCH" && "$CTX" traits build "$id" >/dev/null 2>&1); then
    echo "builtins: $id FAILED TO BUILD" >&2
    (cd "$SCRATCH" && "$CTX" traits build "$id" 2>&1 | tail -20) >&2
    status=1
    continue
  fi

  for bucket in traits templates shared; do
    if [[ -d "$BUILTINS/$bucket/$id" ]]; then
      dest="$BUILTINS/$bucket/$id"
      break
    fi
  done

  # Only the canonical and the lock are compared. A source map records the
  # repo-relative path it was built from, so it differs between this scratch
  # root and the real tree — a difference about where the build ran, not about
  # whether the artifact is stale. The canonical is what ships and what a
  # source edit must reach. (Task 0230 removes the scratch root, and then the
  # map becomes comparable too.)
  for artifact in generated/index.toml trait.lock; do
    src="$AUTHORED/$id/$artifact"
    [[ -e "$src" ]] || continue
    if [[ "$MODE" == "build" ]]; then
      rm -rf "${dest:?}/$artifact"
      cp -R "$src" "$dest/$artifact"
    elif ! diff -rq "$src" "$dest/$artifact" >/dev/null 2>&1; then
      echo "builtins: $id/$artifact is stale — rebuild with 'just builtins-build'" >&2
      status=1
    fi
  done
done

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi
echo "builtins: ${#ORDER[@]} packages clean (${MODE})"
