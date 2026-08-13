# Release 0.1.0 checklist

Live document — tick as we go. Raised 2026-08-13 from the owner's release
plan: npm for cdk/toolkit/agents/config, `ctx` via curl/brew (npm wrapper
deferred), authored traits published to npm/git, vendoring exercised, and a
virgin-repo consume → upstream update → pull-update loop.

## Phase 0 — accounts, secrets, decisions

- [x] 0.1 Release version: **0.1.0** everywhere (owner ruling 2026-08-13).
- [x] 0.2 `@ctx-traits` npm org exists and is owned.
- [x] 0.3 `ctx-company/traits` public at tag time.
- [x] 0.4 `ctx-company/homebrew-tap` repo created.
- [x] 0.5 Actions secrets set: `NPM_TOKEN`, and the tap PAT under
      **`GH_HOMEBREW_PAT_TOKEN`** — NOTE: release.yml referenced
      `HOMEBREW_TAP_TOKEN`; the workflow is updated to the actual name.

## Phase 1 — the four npm packages

- [x] 1.1 Remove `private: true` from toolkit and agents.
- [x] 1.2 `publishConfig.access: "public"` on all four.
- [x] 1.3 All four versions → 0.1.0; inter-package deps on `workspace:` ranges.
- [x] 1.4 README/LICENSE present wherever `files:` promises them.
- [x] 1.5 Topological publish order determined from real dependency edges.
- [x] 1.6 release.yml npm job publishes all four in order via `pnpm publish`
      (workspace-range rewrite) with the already-published skip guard.
- [x] 1.7 Local `publish --dry-run` per package; tarball listings reviewed.

## Phase 2 — ctx binary channels

- [x] 2.1 Build matrix covers darwin arm64/x86_64 + linux x86_64/arm64.
- [x] 2.2 npm wrapper for `ctx`: DEFERRED (owner ruling 2026-08-13) — filed
      as task 0190.
- [x] 2.3 Hand-cut the release: workspace at 0.1.0 (done), CHANGELOG.md drafted
      (done) — remaining: OWNER runs `git tag v0.1.0 && git push origin v0.1.0`.
- [ ] 2.4 Actions run green end-to-end: archives, formula in tap, npm ×4, no
      skip-warnings.
- [ ] 2.5 Verify from a clean machine: README curl one-liner;
      `brew install ctx-company/tap/ctx`; `ctx --version` from both.

## Phase 3 — publish authored traits (npm + git)

- [ ] 3.1 First trait chosen (suggest: refactor) before sweeping all.
- [ ] 3.2 `ctx traits dependency init` per package ([publish] name/registry/
      access + npm wrapper).
- [ ] 3.3 build + check clean, lock fresh and committed.
- [ ] 3.4 `dependency publish --trait <id> --dry-run` pack listing reviewed.
- [ ] 3.5 Publish; verify from outside the repo with `dependency info`.
- [ ] 3.6 Verify a `git:` source declaration resolves the same package.

## Phase 4 — vendoring rehearsal (scratch clone)

- [ ] 4.1 `dependency add @ctx-traits/<trait>` in a scratch repo; trust-approve
      flow walked deliberately; vendored tree + lock evidence verified.
- [ ] 4.2 `ctx traits vendor --locked` and `ctx traits check` green.

## Phase 5 — virgin repo, full lifecycle

- [ ] 5.1 Fresh `git init`, no JS project: `ctx traits init` (authoring packages
      into .ctx/node_modules; zero-Node TOML path holds).
- [ ] 5.2 `dependency add` + trust + `list` + a run/explain smoke.
- [ ] 5.3 Upstream: edit trait, bump trait.toml version, rebuild, republish.
- [ ] 5.4 Virgin repo: `dependency outdated` → `update` → digests move,
      re-trust only where content changed, run again.
- [ ] 5.5 Same loop over the `git:` transport.

## Known blockers (all addressed in phase steps)

toolkit/agents private (1.1) · restricted access (1.2) · npm job cdk-only
(1.6) · secret-name mismatch (0.5→1.6) · npm binary wrapper nonexistent (2.2,
deferred).
