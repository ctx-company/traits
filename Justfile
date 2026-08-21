# Where the gates build. An inherited CARGO_TARGET_DIR wins, falling back to
# this checkout's own target. The fallback is why gates in a run's worktree
# used to be cold every time: `justfile_directory()` is the WORKTREE inside a
# run, so each run built into a brand-new path and every toolchain treats a
# moved cache as no cache. Runs now inherit a leased, stable slot via
# `[worktree.env] CARGO_TARGET_DIR = "{cache-slot}"` (0057) — leased, so two
# concurrent runs still never share one, which is what the hard override here
# was originally defending against.
gate_target_dir := env_var_or_default("CARGO_TARGET_DIR", justfile_directory() / "target")
target_dir := env_var_or_default("CARGO_TARGET_DIR", "target")

install:
	mkdir -p "$HOME/.local/bin"
	cargo build --release -p ctx-traits-cli --bin ctx
	install -m 755 "{{target_dir}}/release/ctx" "$HOME/.local/bin/ctx"

sdk-schema:
	mkdir -p "{{target_dir}}/code_review_group_5_6"
	cargo run -p ctx-traits-cli -- traits internal schema --protocol sdk-types > "{{target_dir}}/code_review_group_5_6/sdk_types_schema.json"

sdk-generate:
	cargo run -p ctx-traits-cli -- traits internal sdk-generate

sdk-check:
	cargo run -p ctx-traits-cli -- traits internal sdk-generate --check

# Write one version number into the Cargo workspace and every publishable
# package manifest. Run this instead of editing six files by hand; the release
# workflow re-checks all of them against the tag.
set-version version:
	./.internal/scripts/set-version.sh {{version}}

# Cut a release: set the version everywhere, verify, commit, tag. Stops before
# pushing — pushing the tag is what publishes to npm, and npm versions cannot
# be reused, so that one command stays a deliberate human act.
release version:
	./.internal/scripts/release.sh {{version}}

# Has a package we already published changed without its version moving?
# Quiet through a release cycle (an unpublished version has nothing to compare
# against), loud the moment someone edits a shipped version's code.
published-drift:
	just ts-build
	./.internal/scripts/published-drift.sh

# The path a user takes in an empty folder: init, install, create, build,
# activate, trust, and the whole dependency lifecycle — against the tarballs
# the release would upload, so it needs no registry and no published version.
e2e:
	cargo build -p ctx-traits-cli
	just ts-build
	./.internal/scripts/e2e-cold-start.sh

# The same run against real npm, for after a publish.
e2e-registry:
	cargo build -p ctx-traits-cli
	./.internal/scripts/e2e-cold-start.sh --registry

# Rebuild the embedded built-in traits and scaffold templates from source.
# Internal only: these ship inside the binary, so no user command rebuilds
# them and nothing else notices when a source edit never reaches its
# committed canonical.
builtins-build:
	cargo build -p ctx-traits-cli
	./.internal/scripts/builtins.sh build

builtins-check:
	cargo build -p ctx-traits-cli
	./.internal/scripts/builtins.sh check

ts-build:
	pnpm --dir packages/config run build
	pnpm --dir packages/cdk run build
	pnpm --dir packages/toolkit run build
	pnpm --dir packages/agents run build
	pnpm --dir packages/rust run build

ts-typecheck:
	pnpm --dir packages/cdk run typecheck && pnpm --dir packages/cdk run typecheck:test
	pnpm --dir packages/config run typecheck && pnpm --dir packages/config run typecheck:test
	pnpm --dir packages/toolkit run typecheck
	pnpm --dir packages/agents run typecheck
	pnpm --dir packages/rust run typecheck
	pnpm --dir packages/harness-client run typecheck

ts-lint:
	pnpm --dir packages/cdk run lint
	pnpm --dir packages/config run lint
	pnpm --dir packages/toolkit run lint
	pnpm --dir packages/agents run lint
	pnpm --dir packages/rust run lint
	pnpm --dir packages/harness-client run lint

ts-format-check:
	pnpm --dir packages/cdk run format:check
	pnpm --dir packages/config run format:check
	pnpm --dir packages/toolkit run format:check
	pnpm --dir packages/agents run format:check
	pnpm --dir packages/rust run format:check
	pnpm --dir packages/harness-client run format:check

ts-test:
	pnpm --dir packages/cdk run test
	pnpm --dir packages/config run test
	pnpm --dir packages/toolkit run test

lint:
	cargo fmt --check
	CARGO_TARGET_DIR="{{gate_target_dir}}" cargo clippy --workspace --all-targets --all-features -- -D warnings

# The PER-ROUND gate (0056). The implement trait's check step runs `just test`
# every build round, in a fresh worktree, for every run — so this must stay
# cheap enough to pay ten times over without dominating the run. It proves the
# round did not break the build: formatting, generated-SDK drift, lint over the
# shipped code, and the unit tests.
#
# Deliberately NOT here: compiling and linking the ~40 integration proof
# binaries and running them. That is the slow half, and several of those proofs
# drive real runs under tight time budgets, so under concurrent dispatches they
# fail for load rather than for defects — the loop then gets told its work is
# broken when it is not. Those run once, at merge, via `test-full`.
test:
	#!/usr/bin/env bash
	set -euo pipefail
	export CARGO_TARGET_DIR="{{gate_target_dir}}"
	target_dir="$CARGO_TARGET_DIR"
	just sdk-check
	just builtins-check
	just ts-format-check
	# ts-typecheck in the PER-ROUND gate (owner ruling 2026-08-11): twice now
	# a run iterated green for its whole build loop while carrying a
	# TS-only defect the landing gate then caught (tone.Blunt in
	# functional.test.ts; 0162's seats() signature) — the worker gets told
	# at merge time about a defect it could have fixed in round 1. tsc is
	# ~15s warm, well inside the per-round budget. ts-test stays
	# landing-only: vitest suites are the slow half.
	just ts-typecheck
	cargo fmt --check
	cargo clippy --workspace --all-features -- -D warnings
	# `builtin-trait-packages` is named rather than inherited. Core's whole
	# builtin_trait_packages module is gated on it, so without the feature its
	# tests do not exist — `cargo test -p ctx-traits-core` reports a confident
	# pass having asserted nothing about the embedded set. Feature unification
	# through io and cli happens to enable it here today; saying so keeps the
	# coverage from depending on a dependency-graph side effect.
	cargo test --workspace --lib --features ctx-traits-core/builtin-trait-packages

# The LANDING gate (0056): everything `test` proves, plus the full proof
# suites, run ONCE before a branch touches main. Declared as `[merge] gate` in
# .ctx/traits/runtime.toml, so nothing lands unproven even though rounds stop
# paying for the whole suite.
test-full:
	#!/usr/bin/env bash
	set -euo pipefail
	export CARGO_TARGET_DIR="{{gate_target_dir}}"
	target_dir="$CARGO_TARGET_DIR"
	# The gate runs wherever the merge does — usually a run worktree created
	# before the latest landings. A landing that adds a workspace package (or
	# bumps JS deps) leaves that worktree's node_modules stale, and the gate
	# then dies on missing tools ("sh: dprint: command not found", observed
	# 2026-08-10 when packages/toolkit landed). Frozen install is a no-op
	# (~1s) when already current, and exactly what CI runs before its gate.
	# CI=true is load-bearing (run-1da991df): when the worktree's modules dir
	# predates a workspace-layout change, headless pnpm shows a purge prompt,
	# auto-answers it, and EXITS 0 WITHOUT INSTALLING — the placebo that
	# parked a merge on "vitest: command not found". CI mode purges and
	# reinstalls for real instead of prompting.
	CI=true pnpm install --frozen-lockfile
	just sdk-check
	just builtins-check
	just ts-format-check
	# CI runs ts-typecheck/ts-test as separate steps after test-full; the
	# landing gate must prove the same surface, or a TS-breaking change lands
	# locally green and reds CI (observed: tone.Blunt in functional.test.ts
	# sat untypechecked for three days behind an earlier test-full failure).
	just ts-typecheck
	just ts-test
	just lint
	cargo build --workspace --bins
	cargo test --workspace

# 0151: these five recipes deliberately dispatch without --merge — the run
# lands via `ctx traits merge <run-id>` after the report's own NOT-merged
# line (`landing_state`), not automatically. Add --merge here if that
# posture should change; the report fix stands either way.
research topic:
	ctx traits run research --worktree --progress tui -- --topic={{quote(topic)}}

research-quick topic:
	ctx traits run research:quick --worktree --progress tui -- --topic={{quote(topic)}}

research-deep topic:
	ctx traits run research:deep --worktree --progress tui -- --topic={{quote(topic)}}

plan task:
    ctx traits run plan --worktree --progress tui -- --task={{quote(task)}}

refactor target:
    ctx traits run refactor --worktree --progress tui -- --target={{quote(target)}}

refactor-guided:
	ctx traits run refactor:guided --worktree --progress tui

optimize objective:
	ctx traits run optimize:experiment --worktree --progress tui -- --objective={{quote(objective)}}

optimize-benchmark target:
	ctx traits run optimize:benchmark --worktree --progress tui -- --target={{quote(target)}}

implement tasks:
	@{{justfile_directory()}}/.internal/scripts/implement.sh {{quote(tasks)}}

implement-with trait_id tasks:
	@{{justfile_directory()}}/.internal/scripts/implement.sh --trait {{quote(trait_id)}} {{quote(tasks)}}
