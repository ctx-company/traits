#!/usr/bin/env bash
# Write one version number everywhere it lives.
#
#   set-version.sh 0.3.0
#
# Six files carried it by hand: the Cargo workspace and five package.json
# manifests. The release workflow refuses a tag whose tree disagrees with it,
# which catches a missed one — but at tag time, which means retagging. This
# removes the arithmetic instead of checking it.
#
# DISCOVERED, not enumerated, for the same reason release.yml discovers: a
# hardcoded list is how the @ctx-traits/cli wrapper was missed by the 0.2.1
# sweep. Every non-private manifest under packages/ gets the number.
set -euo pipefail

version="${1:-}"
if [[ -z "${version}" ]]; then
  echo "usage: set-version.sh <version>   e.g. set-version.sh 0.3.0" >&2
  exit 2
fi
if [[ ! "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "set-version.sh: ${version} is not a semantic version" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${REPO_ROOT}"

# The workspace version, which every crate inherits with `version.workspace`.
# Anchored to the [workspace.package] table's own key so a dependency's
# `version = "..."` elsewhere in the file is never touched.
python3 - "${version}" <<'PY'
import pathlib, re, sys
version = sys.argv[1]
path = pathlib.Path("Cargo.toml")
text = path.read_text()
updated, count = re.subn(
    r'(\[workspace\.package\][^\[]*?\nversion = ")[^"]+(")',
    lambda m: m.group(1) + version + m.group(2),
    text,
    count=1,
    flags=re.S,
)
if count != 1:
    raise SystemExit("set-version.sh: no [workspace.package] version in Cargo.toml")
path.write_text(updated)
print(f"  Cargo.toml -> {version}")
PY

for manifest in packages/*/package.json; do
  if [[ "$(jq -r '.private // false' "${manifest}")" == "true" ]]; then
    continue
  fi
  name=$(jq -r .name "${manifest}")
  # A TEXT edit of the one line, not a jq rewrite. jq emits a normalized
  # document, which decodes escapes it did not need to touch — rewriting
  # config's manifest turned `\u2014` into a literal em dash, a content
  # change disguised as a version bump. Only the version line moves here.
  python3 - "${manifest}" "${version}" <<'PYEOF'
import pathlib, re, sys
path, version = pathlib.Path(sys.argv[1]), sys.argv[2]
text = path.read_text()
updated, count = re.subn(r'("version":\s*")[^"]+(")', lambda m: m.group(1) + version + m.group(2), text, count=1)
if count != 1:
    raise SystemExit(f"set-version.sh: no version key in {path}")
path.write_text(updated)
PYEOF
  echo "  ${name} -> ${version}"
done

echo
echo "Next: commit, tag v${version}, push the tag. The release workflow"
echo "re-checks every one of these against the tag before it publishes."
