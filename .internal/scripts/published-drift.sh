#!/usr/bin/env bash
# Has a published package changed without its version moving?
#
# Every other guard in this repository compares our files to each other. The
# release workflow checks the tag against Cargo.toml and every package.json,
# and they all agreed on 2026-08-15 when 0.2.4 shipped. Then the CDK's API
# changed — `stage` became `step`, the behavior axes moved under `behavior`,
# the agent templates under `agent` — and the number stayed 0.2.4 for six
# days and thirty-two commits. Everything kept passing, because everything
# still agreed with everything else. The only thing that disagreed was npm,
# and nothing asked npm.
#
# So this asks npm. For each publishable package: if our current version is
# ALREADY published, compare what is up there with what we would build now.
# Different means the number is a lie and has to move before this merges.
#
# Passes trivially once the version is bumped, because an unpublished version
# has nothing to compare against. That is the intended shape: it is quiet
# through a release cycle and loud the moment someone edits a shipped
# version's code.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${REPO_ROOT}"

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

status=0
checked=0
skipped=()

for manifest in packages/*/package.json; do
  if [[ "$(jq -r '.private // false' "${manifest}")" == "true" ]]; then
    continue
  fi
  name=$(jq -r .name "${manifest}")
  version=$(jq -r .version "${manifest}")
  directory=$(dirname "${manifest}")

  # Not published at this version: nothing to contradict. This is the state
  # a repository is in for the whole of a release cycle.
  if ! npm view "${name}@${version}" version >/dev/null 2>&1; then
    skipped+=("${name}@${version} (not published)")
    continue
  fi

  checked=$((checked + 1))
  published="${work}/${name//\//-}-published"
  local_pack="${work}/${name//\//-}-local"
  mkdir -p "${published}" "${local_pack}"

  (cd "${published}" && npm pack "${name}@${version}" >/dev/null 2>&1 && tar xzf ./*.tgz)
  # `pnpm pack` here, not a raw tar of the directory: it applies the same
  # `files` allowlist and workspace-range rewriting the release does, so the
  # comparison is against what would actually be uploaded.
  pnpm --dir "${directory}" pack --pack-destination "${local_pack}" >/dev/null 2>&1
  (cd "${local_pack}" && tar xzf ./*.tgz)

  # Contents, not tarball hashes: `pnpm pack` records timestamps, so two
  # packs of identical trees differ byte-for-byte while meaning the same
  # thing. `dist/` is the published artifact; package.json is compared for
  # its dependency ranges, which are part of what a consumer resolves.
  if ! diff -r -q "${published}/package/dist" "${local_pack}/package/dist" >/dev/null 2>&1; then
    echo "::error::${name}@${version} is published, but this tree builds something different."
    echo "  The version number says these are the same package. They are not."
    echo "  Run '.internal/scripts/set-version.sh <next-version>' before merging."
    echo "  Differing files:"
    # Just the file names, relative to the package. Both sides of the diff
    # line carry the same `dist/...` suffix under different temp roots, so
    # pulling that suffix and deduping gives one line per differing file.
    diff -r -q "${published}/package/dist" "${local_pack}/package/dist" 2>&1 \
      | grep -oE "dist/[^ ]*" | sort -u | sed 's/^/    /' | head -20
    status=1
  fi
done

if [[ ${#skipped[@]} -gt 0 ]]; then
  printf 'not yet published, nothing to compare:\n'
  printf '  %s\n' "${skipped[@]}"
fi

if [[ "${status}" -eq 0 ]]; then
  echo "published-drift: ${checked} package(s) compared, no drift"
fi
exit "${status}"
