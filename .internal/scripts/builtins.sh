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
# Every package builds by source path, in the tree it actually lives in.
# There used to be a flat scratch root here, because the five dependents
# declared `path = "../spec"` — true of the flat runtime store they shipped
# into, false of this bucketed tree. 0232 made the store bucketed too, so the
# declared path is now true in both places and the flattening has nothing
# left to do. (0230 removed the other reason: a path build used to resolve
# dependencies from source/ rather than the package root.)
#
# `check` still builds into a sandbox, so a gate never writes to the tree it
# is judging — but the sandbox MIRRORS the repo-relative layout rather than
# flattening it. That is what makes the source map comparable: a map records
# the repo-relative path it was built from, so an identical layout yields an
# identical map, and all three artifacts are compared instead of two.
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

# The sandbox mirrors the repository's own relative layout, so a path built
# inside it is spelled exactly as it is here.
MIRROR="$SCRATCH/modules/builtin"
mkdir -p "$MIRROR"
for bucket in traits templates shared; do
  cp -R "$BUILTINS/$bucket" "$MIRROR/$bucket"
done

# spec first: the other built-ins declare it as a path dependency, and a
# dependency has to be resolvable before its dependents build.
ORDER=("shared/spec")
for package in "$MIRROR"/traits/*/ "$MIRROR"/templates/*/ "$MIRROR"/shared/*/; do
  relative="${package#"$MIRROR/"}"
  relative="${relative%/}"
  [[ "$relative" == "shared/spec" ]] || ORDER+=("$relative")
done

status=0
for relative in "${ORDER[@]}"; do
  source_path="modules/builtin/$relative/source/index.ts"
  if ! (cd "$SCRATCH" && "$CTX" traits build "$source_path" >/dev/null 2>&1); then
    echo "builtins: $relative FAILED TO BUILD" >&2
    (cd "$SCRATCH" && "$CTX" traits build "$source_path" 2>&1 | tail -20) >&2
    status=1
    continue
  fi

  dest="$BUILTINS/$relative"

  # All three artifacts, the source map included. The map was excluded while
  # the sandbox was flat, because it records the repo-relative path it was
  # built from and so differed by construction — which also meant the
  # committed maps pointed at authored paths that do not exist in this
  # repository, and nothing noticed. A mirrored sandbox makes the map mean
  # what it says, so it is now held to the same standard as the canonical.
  for artifact in generated/index.toml generated/index.map trait.lock; do
    src="$MIRROR/$relative/$artifact"
    [[ -e "$src" ]] || continue
    if [[ "$MODE" == "build" ]]; then
      # mkdir -p, because a package added since the last build has no
      # `generated/` yet and `cp` will not create the parent for you.
      mkdir -p "$(dirname "${dest:?}/$artifact")"
      rm -rf "${dest:?}/$artifact"
      cp -R "$src" "$dest/$artifact"
    elif ! diff -rq "$src" "$dest/$artifact" >/dev/null 2>&1; then
      echo "builtins: $relative/$artifact is stale — rebuild with 'just builtins-build'" >&2
      status=1
    fi
  done
done

if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi
echo "builtins: ${#ORDER[@]} packages clean (${MODE})"
