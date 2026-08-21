#!/usr/bin/env bash
# The path a user actually takes, start to finish, in an empty folder.
#
#   e2e-cold-start.sh              install from packed local tarballs
#   e2e-cold-start.sh --registry   install from npm (post-publish smoke)
#
# Everything else in this repository tests the workspace against itself. That
# leaves the whole first hour of a user's experience — init, install, create,
# build, activate, trust, add a dependency — covered by nothing, which is how
# a published CDK whose API had moved under an unchanged version number
# reached a state where `create` died on a TypeError inside a scaffolded file.
#
# The default mode needs no registry: `pnpm pack` produces the exact tarball
# the release would upload, and installing that file is byte-identical to
# installing from npm. So this runs on a branch, at a version nobody has
# published, and still proves what a user would get.
#
# `--registry` is the same script pointed at the real thing, for after a
# publish: "we published" becomes "we published something that works".
set -euo pipefail

MODE="local"
[[ "${1:-}" == "--registry" ]] && MODE="registry"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CTX="${CTX_BIN:-${REPO_ROOT}/target/debug/ctx}"
if [[ ! -x "${CTX}" ]]; then
  echo "e2e-cold-start: no ctx binary at ${CTX} — run 'cargo build -p ctx-traits-cli' first" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
HOME_DIR="${WORK}/home"
PROJECT="${WORK}/project"
SIBLING="${WORK}/sibling"
mkdir -p "${HOME_DIR}" "${PROJECT}" "${SIBLING}"

failures=0
step() {
  local label="$1"
  shift
  if "$@" >"${WORK}/out" 2>&1; then
    printf '  ok    %s\n' "${label}"
  else
    printf '  FAIL  %s (exit %s)\n' "${label}" "$?"
    sed 's/^/          /' "${WORK}/out" | tail -12
    failures=$((failures + 1))
  fi
}
# Same, for the cases where a NON-zero exit is the passing outcome. A guard
# nobody exercises is a guard that quietly stops working.
refuses() {
  local label="$1"
  shift
  if "$@" >"${WORK}/out" 2>&1; then
    printf '  FAIL  %s (expected refusal, exited 0)\n' "${label}"
    failures=$((failures + 1))
  else
    printf '  ok    %s (refused, as it should)\n' "${label}"
  fi
}

ctx() {
  local dir="$1"
  shift
  (cd "${dir}" && HOME="${HOME_DIR}" XDG_CONFIG_HOME="${HOME_DIR}" \
    XDG_CACHE_HOME="${HOME_DIR}" "${CTX}" "$@")
}

for dir in "${PROJECT}" "${SIBLING}"; do
  git -C "${dir}" init -q .
  git -C "${dir}" config user.email e2e@local
  git -C "${dir}" config user.name e2e
done

echo "cold start (${MODE})"

# --- the authoring environment -------------------------------------------
step "init scaffolds the project" ctx "${PROJECT}" traits init

if [[ "${MODE}" == "local" ]]; then
  # Pack in dependency order and install the files. `pnpm pack` applies the
  # `files` allowlist and rewrites `workspace:^` to real versions, so these
  # are the release's own artifacts rather than a copy of the source tree.
  STAGE="${WORK}/stage"
  mkdir -p "${STAGE}"
  for package in config cdk toolkit agents; do
    pnpm --dir "${REPO_ROOT}/packages/${package}" pack --pack-destination "${STAGE}" >/dev/null
  done
  tarballs=()
  for package in config cdk toolkit agents; do
    tarballs+=("$(ls "${STAGE}"/ctx-traits-"${package}"-*.tgz)")
  done
  step "install the packed packages" \
    env HOME="${HOME_DIR}" npm install --prefix "${PROJECT}/.ctx/traits" "${tarballs[@]}"
else
  step "install from npm" ctx "${PROJECT}" traits init --install
fi

# --- authoring ------------------------------------------------------------
step "create from a template" ctx "${PROJECT}" traits create work --from implement
step "create from scratch" ctx "${PROJECT}" traits create blank
step "build" ctx "${PROJECT}" traits build work
step "check" ctx "${PROJECT}" traits check work

# --- lifecycle ------------------------------------------------------------
step "activate" ctx "${PROJECT}" traits state --active work
step "trust" ctx "${PROJECT}" traits trust --approved work
step "trust reads back" ctx "${PROJECT}" traits trust work
step "list" ctx "${PROJECT}" traits list

# --- dependencies ---------------------------------------------------------
step "sibling: init" ctx "${SIBLING}" traits init
if [[ "${MODE}" == "local" ]]; then
  step "sibling: install the packed packages" \
    env HOME="${HOME_DIR}" npm install --prefix "${SIBLING}/.ctx/traits" "${tarballs[@]}"
else
  step "sibling: install from npm" ctx "${SIBLING}" traits init --install
fi
step "sibling: create" ctx "${SIBLING}" traits create shared
git -C "${SIBLING}" add -A >/dev/null 2>&1
git -C "${SIBLING}" -c core.hooksPath=/dev/null commit -qm "shared" >/dev/null 2>&1

step "dependency add" ctx "${PROJECT}" traits dependency add "path:../sibling/.ctx/traits/authored/shared"
step "dependency install" ctx "${PROJECT}" traits dependency install
step "dependency outdated" ctx "${PROJECT}" traits dependency outdated

# Move the upstream, then assert --locked refuses it. This is the property
# `--locked` exists for, and it silently passed until 0242's follow-up.
sed -i.bak 's/Describe what this trait is for./Upstream moved on./' \
  "${SIBLING}/.ctx/traits/authored/shared/source/index.ts"
ctx "${SIBLING}" traits build shared >/dev/null 2>&1 || true
git -C "${SIBLING}" add -A >/dev/null 2>&1
git -C "${SIBLING}" -c core.hooksPath=/dev/null commit -qm "edit" >/dev/null 2>&1

refuses "dependency install --locked, upstream moved" \
  ctx "${PROJECT}" traits dependency install --locked
step "dependency update accepts it" ctx "${PROJECT}" traits dependency update
step "dependency remove" ctx "${PROJECT}" traits dependency remove shared

# --- fork -----------------------------------------------------------------
step "dependency add again" ctx "${PROJECT}" traits dependency add "path:../sibling/.ctx/traits/authored/shared"
step "fork" ctx "${PROJECT}" traits fork shared

# --- the guard itself -----------------------------------------------------
# Replace the CDK with something that resolves but exports nothing, and check
# that scaffolding refuses rather than producing a broken package. Without
# this the readiness guard is never exercised by any test.
BROKEN="${PROJECT}/.ctx/traits/node_modules/@ctx-traits/cdk"
if [[ -d "${BROKEN}" ]]; then
  rm -rf "${BROKEN:?}/dist"
  mkdir -p "${BROKEN}/dist"
  printf 'export {};\n' > "${BROKEN}/dist/index.js"
  printf 'export {};\n' > "${BROKEN}/dist/index.d.ts"
  refuses "create refuses a CDK that cannot build" \
    ctx "${PROJECT}" traits create broken-check
fi

echo
if [[ "${failures}" -eq 0 ]]; then
  echo "cold start: every step passed"
  exit 0
fi
echo "cold start: ${failures} step(s) failed"
exit 1
