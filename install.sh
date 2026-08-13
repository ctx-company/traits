#!/bin/sh
# ctx installer — https://github.com/ctx-company/traits
#
#   curl -fsSL https://raw.githubusercontent.com/ctx-company/traits/main/install.sh | sh
#
# Downloads the release archive for this machine, verifies its sha256 against
# the checksum the release publishes, and installs the single `ctx` binary.
# No sudo: the default destination is ~/.local/bin (override with
# CTX_INSTALL_DIR). Installs the latest release unless CTX_VERSION is set to
# a specific version (digits-and-dots form, no leading "v").
#
# POSIX sh only — this runs under dash on Debian/Ubuntu.

set -eu

REPO="ctx-company/traits"
INSTALL_DIR="${CTX_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
    printf 'install.sh: %s\n' "$1" >&2
    exit 1
}

# --- platform -> release target triple -------------------------------------
os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux) os_part="unknown-linux-gnu" ;;
    *) fail "unsupported OS '$os' — see https://github.com/$REPO#install for other options" ;;
esac
case "$arch" in
    arm64 | aarch64) arch_part="aarch64" ;;
    x86_64 | amd64) arch_part="x86_64" ;;
    *) fail "unsupported architecture '$arch' — see https://github.com/$REPO#install for other options" ;;
esac
target="${arch_part}-${os_part}"

# --- version ----------------------------------------------------------------
version="${CTX_VERSION:-}"
if [ -z "$version" ]; then
    # tag_name is the first-and-only such key in the latest-release document;
    # parsed with grep/cut so the script needs no jq.
    version=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | grep '"tag_name"' | head -n 1 | cut -d '"' -f 4) || true
    version="${version#v}"
    [ -n "$version" ] || fail "could not resolve the latest release; set CTX_VERSION explicitly"
fi

archive="ctx-v${version}-${target}.tar.gz"
base="https://github.com/$REPO/releases/download/v${version}"

# --- download + verify + install -------------------------------------------
workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

printf 'downloading %s ...\n' "$archive"
curl -fsSL -o "$workdir/$archive" "$base/$archive" \
    || fail "download failed: $base/$archive"
curl -fsSL -o "$workdir/$archive.sha256" "$base/$archive.sha256" \
    || fail "checksum download failed: $base/$archive.sha256"

if command -v sha256sum >/dev/null 2>&1; then
    (cd "$workdir" && sha256sum -c "$archive.sha256" >/dev/null) \
        || fail "sha256 verification failed for $archive"
elif command -v shasum >/dev/null 2>&1; then
    (cd "$workdir" && shasum -a 256 -c "$archive.sha256" >/dev/null) \
        || fail "sha256 verification failed for $archive"
else
    fail "neither sha256sum nor shasum found; refusing to install unverified binaries"
fi

tar -xzf "$workdir/$archive" -C "$workdir"
[ -f "$workdir/ctx" ] || fail "archive did not contain a ctx binary at its root"

mkdir -p "$INSTALL_DIR"
# install(1), never cp: overwriting a running binary in place delivers
# SIGKILL to its processes on macOS; install replaces the file atomically.
install -m 755 "$workdir/ctx" "$INSTALL_DIR/ctx"

printf 'installed ctx %s to %s\n' "$("$INSTALL_DIR/ctx" --version 2>/dev/null || printf 'v%s' "$version")" "$INSTALL_DIR/ctx"
case ":$PATH:" in
    *:"$INSTALL_DIR":*) ;;
    *) printf 'note: %s is not on your PATH — add it, e.g.:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR" "$INSTALL_DIR" ;;
esac
