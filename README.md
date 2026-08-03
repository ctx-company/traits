__Agent Traits & Workflows · typed, versioned & in-sync.__

_Turn your skills into shareable modules, that are easily maintained and orchestrated by you and your team._

![screenshot](/.internal/assets/screenshot.png)

<u>[Website](https://ctx.company/traits)</u> · <u>[Docs](https://ctx.company/traits/docs)</u> · <u>[GitHub](https://github.com/ctx-company/traits)</u>

## Installation

### Homebrew (macOS and Linux)
```sh
brew install ctx-company/tap/ctx
```

### Direct download
Every release publishes a `ctx` binary for macOS and Linux, on both Intel and ARM.
Pick your platform's archive from the [latest release](https://github.com/ctx-company/traits/releases/latest):

```sh
curl -fsSL https://github.com/ctx-company/traits/releases/latest/download/ctx-v0.1.0-alpha.1-aarch64-apple-darwin.tar.gz \
    | tar -xz -C /usr/local/bin
```

### From source
```sh
git clone https://github.com/ctx-company/traits \
    && cd traits \
    && just install
```

`ctx` is a single static binary. Running a trait needs only `git` and whichever
coding agent you point it at — no Node, no Python, no runtime to install.
Node is needed only to *author* your own traits in TypeScript.

## Quick start

```sh
ctx traits init          # scaffold .ctx/ in your repo
ctx traits list          # what's available, including the built-in meta-traits
ctx traits               # the dashboard
```

## License

[Apache-2.0](/LICENSE)
