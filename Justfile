checkout_target_dir := justfile_directory() / "target"
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
	CARGO_TARGET_DIR="{{checkout_target_dir}}" cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	#!/usr/bin/env bash
	set -euo pipefail
	export CARGO_TARGET_DIR="{{checkout_target_dir}}"
	target_dir="$CARGO_TARGET_DIR"
	just sdk-check
	just ts-format-check
	just lint
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
	@crystal run --link-flags="-fuse-ld=/usr/bin/ld" {{justfile_directory()}}/.internal/scripts/implement.cr -- {{quote(tasks)}}

implement-with trait_id tasks:
	@crystal run --link-flags="-fuse-ld=/usr/bin/ld" {{justfile_directory()}}/.internal/scripts/implement.cr -- --trait {{quote(trait_id)}} {{quote(tasks)}}
