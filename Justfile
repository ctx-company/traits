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
	cargo run -p ctx-traits-cli -- traits schema --protocol sdk-types > "{{target_dir}}/code_review_group_5_6/sdk_types_schema.json"

sdk-generate:
	cargo run -p ctx-traits-cli -- traits sdk-generate

sdk-check:
	cargo run -p ctx-traits-cli -- traits sdk-generate --check

ts-build:
	pnpm --dir packages/cdk run build
	pnpm --dir packages/agents run build
	pnpm --dir packages/rust run build
	pnpm --dir packages/config run build

ts-typecheck:
	pnpm --dir packages/cdk run typecheck && pnpm --dir packages/cdk run typecheck:test
	pnpm --dir packages/config run typecheck && pnpm --dir packages/config run typecheck:test
	pnpm --dir packages/agents run typecheck
	pnpm --dir packages/rust run typecheck
	pnpm --dir packages/harness-client run typecheck

ts-lint:
	pnpm --dir packages/cdk run lint
	pnpm --dir packages/config run lint
	pnpm --dir packages/agents run lint
	pnpm --dir packages/rust run lint
	pnpm --dir packages/harness-client run lint

ts-format-check:
	pnpm --dir packages/cdk run format:check
	pnpm --dir packages/config run format:check
	pnpm --dir packages/agents run format:check
	pnpm --dir packages/rust run format:check
	pnpm --dir packages/harness-client run format:check

ts-test:
	pnpm --dir packages/cdk run test
	pnpm --dir packages/config run test

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
	just ts-format-check
	cargo fmt --check
	cargo clippy --workspace --all-features -- -D warnings
	cargo test --workspace --lib

# The LANDING gate (0056): everything `test` proves, plus the full proof
# suites, run ONCE before a branch touches main. Declared as `[merge] gate` in
# .ctx/traits/runtime.toml, so nothing lands unproven even though rounds stop
# paying for the whole suite.
test-full:
	#!/usr/bin/env bash
	set -euo pipefail
	export CARGO_TARGET_DIR="{{gate_target_dir}}"
	target_dir="$CARGO_TARGET_DIR"
	just sdk-check
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

deep-research topic quality="standard" depth="standard":
	ctx traits run deep-research --worktree --progress tui -- --topic={{quote(topic)}} --quality={{quote(quality)}} --depth={{quote(depth)}}

auto-research input:
	ctx traits run auto-research --worktree --progress tui -- --input={{quote(input)}}

plan task:
    ctx traits run plan --worktree --progress tui -- --task={{quote(task)}}

refactor target:
    ctx traits run refactor --worktree --progress tui -- --target={{quote(target)}}

refactor-direct:
	ctx traits run refactor:direct --worktree --progress tui

implement tasks:
	@{{justfile_directory()}}/.internal/scripts/implement.sh {{quote(tasks)}}

implement-with trait_id tasks:
	@{{justfile_directory()}}/.internal/scripts/implement.sh --trait {{quote(trait_id)}} {{quote(tasks)}}
