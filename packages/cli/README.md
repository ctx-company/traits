# @ctx-traits/cli

Install the `ctx` binary with npm:

```sh
npm install --global @ctx-traits/cli
ctx --version
```

The package downloads the matching checksum-verified GitHub release archive during `postinstall`. It supports macOS and Linux on arm64 and x64. If npm scripts are disabled (for example, with `--ignore-scripts`), reinstall without that flag before running `ctx`.

For unsupported systems or restricted networks, use the curl installer described in the [repository installation guide](https://github.com/ctx-company/traits#installation), or Homebrew:

```sh
brew install ctx-company/tap/ctx
```

## Packaging design

This v1 package fetches the verified release asset in `postinstall` instead of publishing four optional platform packages. npm trusted publishing requires a one-time manual bootstrap for each package identity; a single wrapper keeps that provisioning and release surface small while preserving the existing release archives as the source of truth.
