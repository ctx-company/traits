# Changelog

All notable changes to ctx.traits are documented here. Versions follow the
workspace version in `Cargo.toml`; entries derive from the commit log per the
release contract (`.ctx/traits/authored/release/resources/release.toml`).

## v0.2.2 — 2026-08-14

Completes the 0.2 release line: `@ctx-traits/cli` (the npm binary wrapper)
joins the aligned version — the v0.2.1 npm job published the four authoring
packages and then failed its cli version guard, leaving the wrapper behind.
The release workflow's tag guard now discovers every public package under
`packages/` instead of naming them, so a newly added package can never be
missed by a version sweep again.

## v0.2.1 — 2026-08-14

Release-pipeline patch: the v0.2.0 tag was cut against a tree whose npm
package manifests still said 0.1.1, so the npm job skipped every publish.
All version-bearing files are aligned at 0.2.1, and the release workflow
now refuses a tag whose tree versions disagree with it instead of shipping
a mismatch.

## v0.2.0 — 2026-08-14

Two features and a validator overhaul.

- **Authored terminals**: `flow.error` / `flow.success` (and the fused
  `flow.errorWhen` / `flow.successWhen`) end a run at the exit point — an
  error terminal fails the run with an authored headline and a typed record
  on the reserved `flow-error` output port; a success terminal completes it,
  binding declared output ports at the exit. Declaring any success terminal
  opts the trait into exit-point completion (falling through every exit is
  an authored `no-exit-reached` failure). Failed runs now exit nonzero
  (`EXIT_RUN_FAILED`, 7) even without a merge in play.
- **Git-first trait sharing**: `ctx traits dependency add owner/repo/trait`
  (or a repo URL with repeatable `--trait`, or `--all`) vendors a trait
  straight from a GitHub repository — smart-HTTP ref resolution, tarball
  fetch by resolved commit, sha-pinned lock, per-trait trust digests, and
  zero toolchain on the consume path (no git, no node). A bare collection
  spec lists the contained traits with copyable add commands.
- **Validator**: sibling `flow.when` ladders are legal — produced-before-read
  tracks guaranteed and possible sets separately, branch guards may read
  conditionally produced evidence, guard-implied production carries a
  guarded verdict's whole arm forward, and terminal arms diverge out of the
  branch join. Guards over never-accepted slots are `Unmeasurable` under a
  strong-Kleene algebra, so both polarities route false on absent evidence.
- Authoring ergonomics: `condition.isTrue`/`isFalse`/`notEmpty`; check steps
  type their verdict record (`CheckResultValue`) instead of a bare boolean.
- `ctx traits build <id>` is source-first: it bootstraps a package with no
  generated canonical and reconciles single-trait/family format transitions
  in either direction.
- Hardening: API credentials live in a `Secret` type that redacts every
  incidental surface; command bound-kills take the whole process group;
  schema-version feature gates accept "0.3 or newer" instead of exact
  versions; the release trait ships its release-manifest resource.

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
