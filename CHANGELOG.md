# Changelog

All notable changes to ctx.traits are documented here. Versions follow the
workspace version in `Cargo.toml`; entries derive from the commit log per the
release contract (`.ctx/traits/authored/release/resources/release.toml`).

## v0.1.1 — 2026-08-13

Release-infrastructure hardening, following the first 0.1.0 cut.

- npm: all four authoring packages publish from CI via OIDC trusted
  publishing (no registry tokens anywhere); `@ctx-traits/toolkit` and
  `@ctx-traits/agents` are public for the first time alongside
  `@ctx-traits/cdk` and `@ctx-traits/config`.
- Homebrew: the tap lives at `ctx-company/homebrew-tap` —
  `brew install ctx-company/tap/ctx` is a one-liner.
- New `install.sh`: platform detection, sha256 verification against the
  release's published checksums, sudo-free install to `~/.local/bin`.
- Release builds move to current runners (`macos-15-intel` replaces the
  retired `macos-13`) and node24-generation Actions; re-tagging a release is
  idempotent end to end; tags run only the Release workflow.

## v0.1.0 — 2026-08-13

First public release.

- `ctx` CLI and runtime: typed, digest-locked agent procedures — authored in
  TypeScript (`@ctx-traits/cdk`), compiled to canonical TOML, validated
  path-sensitively, and executed through configured harnesses with worktree
  isolation, budgets, merge gates, and a full run ledger.
- Authoring packages on npm: `@ctx-traits/config`, `@ctx-traits/cdk`,
  `@ctx-traits/toolkit`, `@ctx-traits/agents`.
- Trait package distribution: publish trait packages to npm
  (`ctx traits dependency publish`) and consume them with pure-Rust fetch,
  integrity verification, vendoring, and per-machine trust decisions
  (`ctx traits dependency add/install/update`).
- First-party trait families: implement, plan, refactor, release, research,
  review, optimize.
- Interactive dashboard, run history, and repository task-board integration.
- Binary distribution: GitHub release archives (curl), Homebrew tap
  (`brew install ctx-company/tap/ctx`).
