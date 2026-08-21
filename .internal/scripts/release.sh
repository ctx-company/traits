#!/usr/bin/env bash
# Cut a release: set the version, verify, commit, tag. Stops before pushing.
#
#   release.sh 0.3.0
#
# Pushing the tag is what publishes to npm, and npm publishes are permanent —
# a version number can be deprecated but never reused. So this does every
# reversible step and prints the one irreversible command for a person to run
# deliberately, rather than burying it in a recipe.
#
# What it refuses, and why each one has to be checked BEFORE the tag exists:
#   - a dirty tree, because the tag would name a commit that is not what you
#     tested
#   - a tag that already exists, because moving one is how the v0.2.0 cut came
#     to point at a pre-bump tree
#   - a version already on npm, because the publish job would skip every
#     package and the release would look successful having shipped nothing
set -euo pipefail

version="${1:-}"
if [[ -z "${version}" ]]; then
  echo "usage: release.sh <version>   e.g. release.sh 0.3.0" >&2
  exit 2
fi
if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "release.sh: ${version} is not a semantic version" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${REPO_ROOT}"
tag="v${version}"

say() { printf '\n== %s\n' "$1"; }
refuse() { printf '\nrelease.sh: %s\n' "$1" >&2; exit 1; }

say "checks"

if [[ -n "$(git status --porcelain)" ]]; then
  git status --short | sed 's/^/    /'
  refuse "the working tree has changes — commit or stash them, so the tag names what you tested"
fi

branch="$(git rev-parse --abbrev-ref HEAD)"
if [[ "${branch}" != "main" ]]; then
  refuse "on branch ${branch}, not main — releases are cut from main"
fi

if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
  refuse "tag ${tag} already exists — pick the next version rather than moving it"
fi

# Any already-published package means the publish job would skip it, and a
# release that skips every package still reports success.
for manifest in packages/*/package.json; do
  [[ "$(jq -r '.private // false' "${manifest}")" == "true" ]] && continue
  name=$(jq -r .name "${manifest}")
  if npm view "${name}@${version}" version >/dev/null 2>&1; then
    refuse "${name}@${version} is already on npm — that version is spent, pick the next one"
  fi
done
echo "  tree clean, on main, ${tag} unused, nothing published at ${version}"

say "version"
./.internal/scripts/set-version.sh "${version}"

say "verify"
# The gates that speak to a RELEASE specifically. The full suite runs in CI on
# the tag; re-running all of it here would double a twenty-minute wait for a
# signal that has not changed since the last push. What has changed is the
# version, so: does the tree still build, and does a user's path still work
# with these exact packages?
just ts-build
cargo build --workspace --bins
just builtins-check
just e2e

say "commit and tag"
if [[ -n "$(git status --porcelain)" ]]; then
  git add -A
  git commit -q -m "release: ${tag}"
  echo "  committed the version bump"
else
  echo "  versions already said ${version}; tagging the current commit"
fi
git tag -a "${tag}" -m "${tag}"
echo "  tagged ${tag} at $(git rev-parse --short HEAD)"

cat <<EOF

Everything local is done. Pushing the tag publishes to npm, which cannot be
undone — so it is yours to run:

    git push origin main
    git push origin ${tag}

The release workflow then re-checks ${tag} against Cargo.toml and every
package manifest, builds the binaries, publishes the packages, and finishes
by running a cold start against what it just published.

Changed your mind before pushing?  git tag -d ${tag}
EOF
