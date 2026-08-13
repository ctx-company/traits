**Agent Traits & Workflows · typed, versioned & in-sync.**

_Turn your skills into shareable modules, that are easily maintained and orchestrated by you and your team._

How ctx compares → [docs/comparison.md](docs/comparison.md)

![screenshot](/.internal/assets/screenshot.png)

<u>[Website](https://ctx.company/traits)</u> · <u>[Docs](https://ctx.company/traits/docs)</u> · <u>[GitHub](https://github.com/ctx-company/traits)</u>

## Installation

### Homebrew (macOS and Linux)

```sh
brew tap ctx-company/tap https://github.com/ctx-company/homebrew
brew install ctx
```

### Direct download

```sh
curl -fsSL https://raw.githubusercontent.com/ctx-company/traits/main/install.sh | sh
```

Detects your platform, verifies the archive's published sha256, and installs
to `~/.local/bin` — no sudo. `CTX_VERSION=0.1.0` pins a version;
`CTX_INSTALL_DIR=/usr/local/bin` changes the destination.

Prefer fetching by hand? Every release publishes a `ctx` binary for macOS and
Linux, on both Intel and ARM. Set `VERSION` to the
[latest release](https://github.com/ctx-company/traits/releases/latest)
and `TARGET` to one of `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`:

```sh
VERSION=0.1.0 TARGET=aarch64-apple-darwin \
    sh -c 'curl -fsSL "https://github.com/ctx-company/traits/releases/download/v${VERSION}/ctx-v${VERSION}-${TARGET}.tar.gz" \
    | tar -xz -C /usr/local/bin'
```

### From source

```sh
git clone https://github.com/ctx-company/traits \
    && cd traits \
    && just install
```

`ctx` is a single static binary. Running a trait needs only `git` and whichever
coding agent you point it at — no Node, no Python, no runtime to install.
Node is needed only to _author_ your own traits in TypeScript.

## Quick start

```sh
ctx traits init          # scaffold .ctx/ in your repo
ctx traits list          # what's available, including the built-in meta-traits
ctx traits               # the dashboard
```

## License

[Apache-2.0](/LICENSE)
