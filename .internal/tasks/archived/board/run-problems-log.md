# Run problems log

(Full pre-consolidation board with all narrative history: .docs/archive/RUN_PROBLEMS_LOG_PRE_CONSOLIDATION_2026-07-24.md; entries before 2026-07-21: .docs/archive/RUN_PROBLEMS_LOG_FULL_2026-07-17_to_21.md)

### Landed

CERTIFICATION STATUS (P487, 2026-07-27). Nineteen lines below carry a dated verdict verified against the TREE, never against a commit subject — the method matters because the log was wrong in both directions: four commits certified work their diff did not contain (P507, P530, P518, P350), one was empty (P395), and six phases landed under subjects naming no phase at all. Everything demoted by that audit is in the In-flight and Forward queues, not here. The uncertified remainder in this section rests on substantial subject-matching commits and has NOT been artifact-verified one by one — a false certification with a large plausible diff would still be sitting here. Recording that limit is part of the pass.

RULE ADDED BY THIS PASS: a landing whose final verdict is `revise`, or whose diff touches no path its own Done-when names, is not recorded as landed at all. Both failure modes are represented above; neither was caught by the process that produced them.


Group 70:
  Phase P274: `check` step primitive: a command run for its typed pass/fail verdict, condition-readable + re-evaluated per loop iteration
  Phase P275: Ground the review: feed the real diff to reviewers + gate the loop-exit on the `check`
  Phase P276: Richer verdict schema + shared review-rubric knowledge resource **Certified 2026-07-27 (P487), verified against the tree:** 12 hits for `owner-items`/`escalation-reason`/`wall-id` in `packages/agents/src/index.ts`.

Group 74:
  Phase P287: Narrow `ResourceTrigger` to `{on-activation, on-demand}`
  Phase P288: Remove `required` from the resource model
  Phase P289: Add explicit `render` mode `{reference, inline}`, decoupled from source
  Phase P290: Conditional resource inclusion — a `when`-guarded resource input on a step
  Phase P291: Regenerate, rebuild, reinstall

Group 77:
  Phase P311: `ctx.toml` → `.ctx/config.toml` (+ registry), dual-path, layout-owned
  Phase P312: Per-trait sidecar `.ctx/traits/<trait>/config.toml` (budget-only) — implements Group 71 P278
  Phase P313: `session-mode` → `ctx.toml [agent.role.*]` + plumb to the warm gate
  Phase P314: Retire the profiles file mechanism (keep `--assign`/attach)

Group 91:
  Phase P362: refactor family on the axis: refactor:{quick,default,smart,strict} + direct kept annotation-driven
  Phase P377: The variant axis: `quick` / `default` / `smart` / `strict` — shared doctrine
  Phase P396: Single-package variant families: variants map on trait() + variant.import()

Group 92:
  Phase P397: Command output routing, understandable at first glance: `schema:text` receives the command's raw stdout
  Phase P401: `ctx traits merge` skips the LLM merger on a clean fast-forward

Group 97:
  Phase P413: deep-research review fixes: scale polarity, dead resource, final-review cliff, stream-list guard

Group 98:
  Phase P385: `auto-research` trait: the generic experiment loop (first draft)
  Phase P386: Showcase: self-improving traits — auto-research over our own eval harness
  Phase P391: Benchmark-driven refactor loop

Group 99:
  Phase P398: Handle surface = fields: typed field accessors, spread composition, typed guards, schema templates

Group 101:
  Phase P402: Finish P344's concurrency: the pieces its own commit calls absent
  Phase P403: Resolve the import subcommand's `--profile` naming conflict
  Phase P405: Markdown → typed-checklist reconciler (import path)
  Phase P406: Make generated `.map` files repo-relative, not absolute
  Phase P415: Machine-readable Deps: + current paths on every open phase

Group 102:
  Phase P408: The leftover contract: schema + two-question doctrine in the shared agents package
  Phase P409: Leftovers at the boundary: a typed output of the run, plus the adapter pattern documented
  Phase P410: Rendering the third state: one generic aligned-rows renderer + the three surfaces

Group 104:
  Phase P430: Protected resources: content digests inside the trust boundary
  Phase P431: Projection literals: constants without a process spawn
  Phase P433: Auto-research quick fixes: toolchain cleanup kill, target relocation, by-kind source split
  Phase P435: deep-research: wire or delete the manifest-streams orphan

Group 105:
  Phase P422: ratatui foundation + live session view at parity
  Phase P423: session-manager dashboard: bare `ctx traits` opens the TUI
  Phase P454: Narration sidebar v2: strict no-overlap layout bounds + ctrl-c-only input on the live view

Group 106:
  Phase P416: Command finalization: obvious names, owner-curated visibility
  Phase P460: Merge-on-completion: `run --worktree --merge[=standard|deep]` — the product absorbs the Justfile's temporary orchestration

Group 107:
  Phase P426: Global state store: runs/debug/cache → `~/.config/ctx/<area>/<repo-key>/`
  Phase P445: Ledger budget/token evidence + narrator usage accounting (P378 salvage, re-based on current substrate)

Group 108:
  Phase P438: `install` / `remove` / `update` / `outdated` / `info`: project-scope porcelain + pure-Rust registry client

Group 109:
  Phase P441: Host-install lifecycle: place, track, update, remove — plus archive export
  Phase P444: Competitive dossiers: sx 2.0 and Slate, loop-runnable

Group 110:
  Phase P448: `sequence.flow` constants (CDK-only)
  Phase P459: CDK surface pruning + typed rule/signal/dependency

Group 111:
  Phase P453: `-h` regroup, renames, and visibility pass (completes P416)

Group 104:
  Phase P436: Build lints from the review: escaping imports and orphan slots


Group 107:
  Phase P427: Zero-config harnesses: built-in registry + PATH detection
  Phase P428: Generalization sweep: no tech-specific behavior in generic verbs

Group 82:
  Phase P412: Bare-leaf vocabulary aliases: `intent.ScopeCreep` works without naming the facet
Group 80:
  Phase P326: CDK surface + shape. `session(id, opts)` constructor + `session.PerFrame`/`session.Persistent` constants
Group 56:
  Phase P242: DX pre-launch gates: placeholder commands, grouped help, prebuilt binaries, first-run quickstart
Group 102:
  Phase P411: "The agent that tells you what it didn't do" — the truthful-agents post
Group 55:
  Phase P240: Native==WASM parity check: assert the `modules/wasm-core` ABI output byte-matches native for the pure-core paths (deferred today)
Group 113:
  Phase P465: One output kit: shared panel renderer (ctx.gate style) + compact-by-default / full-under---verbose gate, --json untouched. **Certified 2026-07-27 against 9340e02 and e4b11d6:** `cargo test -p ctx-traits-cli --test proof_output_style` passed (21 behavioral panel/degrade assertions); `cargo fmt --check` and `just implement-phase-gates` passed.
Group 92:
  Phase P464: Rejection retries submit against a fresh frame and resume the same conversation — a correction is a quick reshape, never a cold full redo
Group 109:
  Phase P443: Manifest `extends`: team base-sets without silent propagation
Group 91:
  Phase P378: Variant-aware profiles + recipes
Group 116:
  Phase P481: Canonical document model generated from Rust — toDraftJson and Meta.declaration get real types
  Phase P482: trait() fields stop taking unknown: every declaration field typed to its handle
  Phase P483: Guards are branded handles; arbitrary object literals no longer typecheck as guards
  Phase P484: Schema literal preservation, handle assignability, resource field lockdown
  Phase P485: Builder namespaces satisfy their interfaces: overloads checked, cast count collapses

Group 118:
  Phase P496: Narrator fallback diet: narration works on redacted-thinking (Claude 5) streams
  Phase P497: Trust gates the render family: prompt/export/host-install refuse what the trust store refuses

Group 83:
  Phase P334: Recurrence breaker: park after N unresolved recurrences instead of looping to exhaustion

Group 120:
  Phase P498: Session context ledger + one-call `context plan`: the engine both hook families call
  Phase P499: claude-code hook adapter: `ctx traits hook`, deterministic activation on the primary harness
  Phase P500: opencode plugin v2: re-assert the trait set per call in the system prompt
  Phase P501: The second harness of each family: codex hook + pi extension
  Phase P502: Static delivery: skill stubs, real skill directories, and the four host rows

Group 113:
  Phase P466: `doctor` on the kit: counts up front, only actionable findings, each row names its fix
  Phase P467: All-commands output sweep onto the kit + a structural gate so no command drifts off-style

Group 107:
  Phase P475: Per-role budgets: frame time, idle time, and retries declared per role in config — merger/narrator hardcoded limits become overridable defaults **Certified 2026-07-27 (P487), verified against the tree:** landed as `1a7d9e3` (subject names no phase) — `validate_role_budget` and per-role frame/idle/retry limits in `harness_config.rs`.

Group 114:
  Phase P468: TUI kit v2: scrollable lists/viewports everywhere, master-detail split, modal confirm/input dialogs, one-keypress actions, $EDITOR round-trip that restores the view
  Phase P469: Sessions screen: list + live preview right; enter, resume, kill, delete — each one keypress through a confirm modal; exit always confirmed
  Phase P470: Session live view v2: done steps collapse to one narrated line each, the current step shows its full stream, the pane uses all available space
  Phase P471: Traits screen: live preview right, approve/deny with reason in a modal (no $EDITOR temp file), edit source in $EDITOR and return to the same view
  Phase P472: Merges screen: every stopped merge explained in a plain sentence plus its exact next action
  Phase P473: Trust screen: what moved and why in plain words, approve one or a whole block with a reason typed in a modal
  Phase P474: The live view is the default: `--progress tui` without asking on a real terminal, today's line output everywhere non-interactive

Group 106:
  Phase P419: Trust UX unification: one verb, never paste a digest
  Phase P446: Committed `.ctx/.gitignore` + doctor gitignore diagnostics
  Phase P457: Optional TypeScript config authoring (`config.ts` → `config.toml`) with a source-drift guard
  Phase P476: One agent namespace: every seat is `[agent.role.<name>]`, `master` renamed to `default` (driver + fallback), bare `[agent]` scalars deleted
  Phase P477: The merge gate is declared in config, never assumed — no repo tool named in the product; a repo without a Justfile merges fine

Group 57:
  Phase P244: ratatui for the two rich interactive surfaces (feature-gated)
  Phase P245: Reconcile the accent identity into one canonical token system: gold = page chrome, named-ANSI green/red/amber = status

Group 80:
  Phase P327: Core model + validation
  Phase P328: Runtime wiring: drive keys harness sessions off the declared session graph

Group 100:
  Phase P399: `onExhausted` owns loop exhaustion end to end

Group 104:
  Phase P434: Auto-research unification: one trait; self-improving-traits folded in as protected resources **Certified 2026-07-27 (P487), verified against the tree:** landed as `2085b4f` — `auto-research` is one package; no `self-improving-*` package remains.

Group 110:
  Phase P449: The process kit: `guardedProduction` + `commitTail` in @ctx-traits/agents
  Phase P450: Restructure the implement family onto the kit and the P396 variants map (absorbs P437)
  Phase P451: Variant-qualified (and repo-qualified) agent assignment in config
  Phase P452: Blog: graph-engineering frame + the composition act **Certified 2026-07-27 (P487), verified against the tree:** landed as `5184881` (subject names no phase) — the scaffold carries 8 `## ACT` sections.
  Phase P458: CDK sequence grammar v2: uniform `(id, options)` constructors, `input.*`/`output.*` step model, gate-branch + typed match


Group 115:
  Phase P478: Harness-native write denies injected per spawn — out-of-worktree edits rejected by claude-code/opencode themselves

Group 94:
  Phase P383: `ctx traits story <run-id>`: the run, told straight

Group 118:
  Phase P488: Default branch discovered, never assumed — `main` leaves the merge machinery
  Phase P489: Silent-truncation sweep: captures that feed state complete verifiably or fail loudly
  Phase P490: Identity scrub: owner paths out of the shipped binary, model lists out of core
  Phase P491: Honest errors: full reasons reach the user, typed exits everywhere, no Debug leaks
  Phase P492: Env-var hygiene: fixture hooks disarmed in release, registry override documented


Group 131:
  Phase P537: Earned exit — carrying a leftover costs rounds, and the round is sealed against a mid-body exit **Certified 2026-07-27 (P487), verified against the tree:** `carry`/`minRounds` present in `implement-default/source/shared.ts`. NOTE: deliberately removed from `implement-quick` on 2026-07-27, so Group 131 covers default/smart/strict/phase only.
  Phase P538: `out-of-budget` leaves the type; the doctrine describes the guard instead of arguing with the model **Certified 2026-07-27 (P487), verified against the tree:** `out-of-budget` returns zero hits in `packages/agents/src/index.ts`; enum is `needs-unlanded | needs-human`.
  Phase P539: The implement family adopts the floor (minRounds 3), and the digests move once

Group 132:
  Phase P540: The correction carries the contract on every harness — schema survives a resumed retry, escalation ladder replaces the accidental cold start
  Phase P541: Corrections state the defect and the fix, never the validator's internal string
  Phase P542: A rejection the model cannot act on never spends a model retry

Group 119:
  Phase P493: Render v2, full level: XML behavioral render with imperative directives replaces the markdown model view **Certified 2026-07-27 (P487), verified against the tree:** `10cafa4`/`2ca5143` — tagged `<trait>` envelope verified live: `ctx traits prompt guarded-change` emits `<trait id version model-view>` with `<intent group id>`/`<behavior axis id>`/`<resource id digest render>`.
  Phase P494: Summary level made real: the resolver's middle rung renders, prices, and round-trips **Certified 2026-07-27 (P487), verified against the tree:** `24cdf58` — `prompt --level` present, `--level summary` emits `level="summary"` with intents collapsed per group.

Group 122:
  Phase P504: The normalized activity model: typed session state + per-frame activity, one vocabulary every harness maps into (absorbs P496) **Certified 2026-07-27 (P487), verified against the tree:** `1780206` — `modules/core/src/procedure/activity.rs` and `modules/io/src/harness_activity.rs` exist.

Group 125:
  Phase P510: A local liveness index + a scan that stops re-reading everything **Certified 2026-07-27 (P487), verified against the tree:** `e28a350` — liveness index, mtime-gated parse cache and summary sidecar present.
  Phase P511: Retention: caps on regenerable things, tiered pruning, never a timer on evidence
  Phase P512: The reclaim surface: one honest list, one command, and the prune bug that hides 34 GB


Group 126:
  Phase P513: The gate is green again: four red checks, none of them in `just test`
  Phase P514: Owner migration: config to `[agent.role.*]` + re-declare the merge gate before the next install
  Phase P515: Repo-trait drift: rebuild implement-quick's canonical, refresh locks, gate `.ctx/traits/*`
  Phase P516: Deferred riders that lost their home: six orphans with no phase
  Phase P517: Confinement completion: the codex renderer and the opencode edit-permission question
  Phase P520: Unblock the longest chain (P432 → P458 → P459 → sdk-check) and finish P485's target

Group 123:
  Phase P505: Pane tree + chrome kit: named panes, border titles, focus ring, real tabs, scrollbars, per-pane scroll **Certified 2026-07-27 (P487), verified against the tree:** `221bd1f` — `tui_panes.rs` carries `PaneTree`, `FocusRing`, `render_pane`, `render_scrollbar`, `PaneScrolls`.
  Phase P506: Every dashboard screen on the tree: state grouping, short ids, in-pane modal input
  Phase P507: The live run view moves onto the tree, and the Rect math dies (premise re-measured 2026-07-27: keys exist but are bespoke and invisible — no scrollbar, no focus ring) **Certified 2026-07-27 (P487), verified against the tree:** landed as `4eaf1a0` (subject names no phase) — `run_view.rs` imports the P505 kit; the `b21331d` commit bearing the P507 subject was Justfile-only and certified nothing.
  Phase P543: The live view's viewport — inline by design and cannot grow; owner decision, gates P507 **Certified 2026-07-27 (P487), verified against the tree:** `4bd5f7b` — `apply_resize` transactionally replaces the inline terminal; 110 insertions.

Group 124:
  Phase P508: The `ask` step: a declared, signal-gated human frame with a dynamic reason **Certified 2026-07-27 (P487), verified against the tree:** `82ef2a7` — `kind=ask` primitive with signal guard and waiting-on-human status, 548 insertions.
  Phase P536: The approval gate: see it before it lands — approve, edit, or send back, on the ask primitive **Certified 2026-07-27 (P487), verified against the tree:** `4cbf073` — approval gate over existing procedure primitives, 240 insertions.

  Phase P544: The live view drains input once per second — scrolling is literally 1 FPS; split the input cadence from the repaint heartbeat **Certified 2026-07-27 (P487), verified against the tree:** the 1s `last_tick` gate is gone from `modules/io/src/harness.rs`.
  Phase P545: The live view onto the pane tree — focus and scroll become visible (P507's rendering half; LANDED as 4eaf1a0 under a non-P545 subject)
  Phase P547: Live view scrolling — follow-mode rebuilds the scroll from zero every frame (thumb jitters/snaps to top), slim the scrollbar, and decide the top-of-window gap (reopens P543) **Certified 2026-07-27 (P487), verified against the tree:** `*scroll = ViewportScroll::new()` no longer appears in `run_view.rs`; follow no longer rebuilds the offset per frame.
  Phase P548: Activity durations as clock time — `2m 8s` becomes `00:02:08`; relative-age text stays in human units **Certified 2026-07-27 (P487), verified against the tree:** `tui.rs:56` emits `{hours:02}:{minutes:02}:{seconds:02}` with a test, and the human-units formatter survives separately as the phase required.


Group 127:
  Phase P522: The reload storm: ~4s of blocking work every 2s on the render thread
  Phase P523: Display hygiene: short ids, reasons before ids, columns that clip, no Debug leaks
  Phase P524: Stop and delete actually work, and a failed action does not kill the TUI
  Phase P525: The preview is genuinely live
  Phase P526: Traits show the source you wrote, and can explain themselves (cached per digest)
  Phase P527: Trust: stop saying "orphaned" twice, and stop rendering 208 dead rows
  Phase P528: Focus you can see, and scroll that stays where you left it **Certified 2026-07-27 (P487), verified against the tree:** landed as `686dda7` (subject names no phase) — 272 insertions in dashboard.rs covering focus, clamping and persistent screen state.

Group 128:
  Phase P529: Prune first: delete what we are not shipping, so nothing dead gets refactored

Phase P518: PRODUCT.md reconciliation — RE-SCOPE FIRST (adds §2793 leftover classes, §2827 onExhausted, and the storage-layout/family-axis sections `89f7d22` declared stale). Its commit `2037b8c` touched only the Justfile; PRODUCT.md still carries 15 pre-P493 markdown-render occurrences. Highest leverage: every run's clerk extracts house rules from this file, so today every run is briefed from a stale contract. Run non-worktree (doc-phase harvest caveat).

Phase P462: Merge/worktree hygiene — park prunes regenerable caches, gate disk preflight, doctor debris sweep. Verified absent: `doctor` has no debris/disk options, no disk-space probing in modules/io. Dispatch as-is.

Phase P521: `ctx traits story <run-id>` at three depths. Never dispatched (`story --help` has no `--level`). Now also a dependency of P550.
Phase P509: Answering an ask from the dashboard. `3518b1b` says "blocked by review"; the dashboard has no answer or modal flow.
Phase P546: `check` counts 23 warnings and prints 7 — 16 dropped by a section-name filter `--verbose` does not lift.
Phase P549: The merge joins the run's journey. `merge.rs:3029` still emits the 60s dim stderr line and no P504 activity.
Phase P550: The story is a surface. `run --help` has no `--story`. Run after F6 and after P521.
Phase P533: The trait authoring pattern becomes doctrine plus a structural check. No commit, no doctrine in PRODUCT.md. Run after the fold chain lands, since the folds are what prove the rules.
Phase P530: fold the implement family — SPLIT INTO THREE, do not re-dispatch as one: (a) native-family build pipeline (CDK emitter → Rust decoder → per-leaf synth → atomic publish), (b) canonical family/variant identity + package topology + shared logical-selector model, (c) the structural fold itself + the dprint format gate. Evidence: the run exhausted 10 rounds against exactly these four blockers. `a2ae6a3` landed ~30 lines of (a) as a base. Gates P531, P532, P535.
Phase P534: Trust mechanism — append-only history, start-time pinning, guarded approve. `867d82d` says "review blocked"; `resolve_trust_verdict` (io/lifecycle.rs:71) still does the old strict digest lookup with no supersession. Three incidents in two days came from this.
Phase P551: Live view — `q` opens a Y/N confirm, ctrl-c kills instantly, the procedure paints before worktree setup, no pane draws a scrollbar.
Phase P531: fold refactor and plan, using what P530 learned. Verified absent 2026-07-27: there is no `.ctx/traits/refactor` package and the refactor-* / plan-* siblings all remain. Blocked on item 3.
Phase P532: rebuild, relock, re-approve — REMAINING HALF ONLY; the drift gate landed as `f658391` (`repo-traits-drift`). Blocked on item 3.
Phase P535: one canonical home for shared dogfood traits. Blocked on item 3 (fold before sharing); part of it belongs in ctx-gate's own plan.

### In flight ▶

-- 1
F3. Phase P552: Live view reads like a log — one ellipsized line per event, 2x2 grid (progress/journey | history/current), bold narrated session title persisted on the session. Run after F2: same startup path. ABSORBS the attach ask: one renderer, three surfaces — live (4 panes), dashboard preview (progress+journey only, ledger-sourced), attached (full screen, identical to `ctx traits run`, ending on the same story).

-- 2

-- 3

-- 4

-- 5

-- 6

### Forward queue

Not dispatched, not blocked. Ordered; nothing here waits on anything above it.

  1. Phase P238: Fuzz the audit detectors and the parser (audit.rs, sanitize, canonical decode). Verified absent 2026-07-27 — no fuzz targets anywhere in the tree. Self-contained, and it is the evidence behind the security claims.
  2. Phase P350: Release-day sweep — REMAINING HALF ONLY. The doc clause landed 2026-07-28 (`.docs/README.md` names the authority, the two verified-stale files, and the historical set). Still open: the two `packages/cdk` test failures and the stale-worktree cleanup, then the tag. Runs last by definition.
  3. Phase P395: deep-research live validation — BLOCKED ON SETUP, not on the phase. `probe`, `synthesis` and `reviewer` are unconfigured and fall back to `[agent.role.default]`, so P394's web-capable/cheap-probe tiering does not exist; and the trait has no trust record, so the first run refuses. Configure the three seats and approve, then dispatch. Zero deep-research sessions exist in a store of 400.

Group 134 — packages worth shipping (owner-ordered 2026-07-28; dispatch in this order):
  4. Phase P555: `@ctx-traits/toolkit` — one home for the reusable roles, schemas, sequence factories and the parallel patterns the store has never exercised. FIRST because P553 and P554 should be cut from it rather than retrofitted into it. Land it before or after the family fold (P530/P531), never during — both rewrite the same imports.
  5. Phase P553: `@ctx-traits/rust` — capture `cargo check`/`clippy` `--message-format json`, reduce the NDJSON to a deduplicated file/line/column/code list, and loop an agent over it until a fresh capture is clean. The package is currently two role factories against a README promising a complete trait package.
  6. Phase P554: the task-authoring trait — writes `tasks/NNNN-<slug>.md`, TRACKED, so a description of work survives the worktree merge and a later `implement` run can work from it with no further input.

Group 135 — running out is a first-class outcome (owner-ordered 2026-07-28):
  7. Phase P556: subscription pressure as a run signal. `claude-code` already streams `rate_limit_event` with utilization, limit type and `resetsAt` on nearly every turn — 960 allowed_warning / 684 allowed / 41 rejected in this repo's own traces, utilization up to 0.99 — and `rg rate_limit modules/` returns NOTHING. Decode it into P504 activity, pause on `rejected` with the reset time recorded, never mid-frame.
  8. Phase P557: per-seat credential routing, so two subscriptions mean two usable seats. Today every seat inherits ambient login state; switching accounts means a logout that kills every in-flight run's next frame (observed 2026-07-28). Establish per-harness support first and report `unsupported` honestly — silent fallback to ambient credentials looks like isolation and is not. Rotation-on-exhaustion is deliberately NOT in scope.

Group 136 — the reviewer's blind spot (diagnosed 2026-07-28 from run-3d5ef0a1ff; dispatch in this order):
  9. Phase P558: `slot.optional()` — optionality is already a per-site wrapper in the model, and the core validator already accepts an optional self-read (proven by patching a canonical and running `check`: passed). Only the authoring surface cannot express one, because every `${ref}` in a template mints a required input.
  10. Phase P559: the reviewer reads its own last verdict — one ref, no prose. Round 2 of run-3d5ef0a1ff raised `implement-fold-not-landed`; round 3's reviewer could not see it and never re-raised it; round 4 approved a run whose deliverable does not exist.

Group 137 — frame render v2 (owner-designed 2026-07-28; dispatch in this order):
  11. Phase P560: delete the input JSON Schema block — 4,725 chars/frame describing types the model never produces. Independent, landable alone.
  12. Phase P561: the `<input>` envelope — `<prompt>` + `<data>`, guards/digests/duplicated ref list dropped; absent optional inputs simply have no element, which finally makes `slot.optional()` usable in prompt text.
  13. Phase P562: the `<output>` envelope — `<format>` skeleton plus only the `<schema>` types it references; also closes the P540 gap where opencode/pi carry no shape contract at all.
  14. Phase P563: system prompt carries the stable half (role, goal, standing discipline); per-frame body carries only what changes.

Group 138 — vocabulary (owner-ordered 2026-07-28):
  15. Phase P564: approval says `family: implement` / `variants: default, quick, smart, strict, phase` instead of `leaves approved: 5`. User-facing text only — the `[family.leaf.*]` manifest key and internal identifiers stay, so no digest moves.

Group 54 — benchmark corpus (unordered among themselves):
  Phase P235: Fetch a dated, digest-pinned ≥1,000-skill marketplace corpus, run the audit harness, commit the digest manifest + results
  Phase P236: Detector precision/recall: a labeled known-good/known-bad fixture set to characterize false-negative/false-positive rates
  Phase P237: Comparison arm + secondary benchmarks: markdown-lint / native-review baseline, activation precision/recall, drift %, perf numbers

Group 97:
  Phase P400: Full-fidelity deep-research riders (was P286) — wave parallelism, numeric quality guards, worktree-sandboxed outputs

Group 101:
  Phase P404: Checklist coverage under `for-each` (or a stated refusal)
  Phase P407: P329's live TUI smoke

COMPLETED BY HAND 2026-07-28, not by a run — these have gitignored deliverables (`/.plans/`, `/.docs/*`) that a `--worktree` run cannot land at all, so they were done directly and are awaiting a move to Landed:
  Phase P518: PRODUCT.md reconciled — render v2 contract replaces the dead markdown regime, built-in snippet tables marked indicative with the authority named, leftovers scope corrected, the broken `onExhausted` default recorded with what it cost.
  Phase P486: status convention settled — the board is authoritative, the checkbox is owner-maintained, and the implement family's scribe no longer writes a plan mark it can never deliver.
  Phase P487: certification pass — nineteen Landed lines carry dated tree-verified verdicts, plus the rule that a `revise` verdict or a diff touching no Done-when path is not a landing.
  Phase P533 (doctrine half): the `refactor-direct` authoring layout and the pragmatism rule are written into PRODUCT.md. The structural lint half stays open.

DECISION OPEN: P537/P539 landed the carry-floor across the implement family, then `implement-quick` was deliberately stripped of it (2026-07-27 — no carry, no minRounds, no leftovers, no park report; 10 rounds, `onExhausted: "block"`). Group 131 therefore covers default/smart/strict/phase only. Re-scope it or accept the split.

OPERATING NOTES, current as of 2026-07-28:
  - Worker seat: `session-mode = "per-frame"` and `budget.frame-seconds = 3600`. Persistent sessions made the worker restate its own prior refusal (44 tool calls cold → 0 across eight resumed rounds); the 1800s ceiling killed a frame mid-work and discarded it.
  - `implement-quick` has NO recurrence breaker by design — the reviewer says "not yet" and the loop grinds until approved or the budget is spent. Exhaustion blocks and commits nothing.
  - A phase whose deliverable lands under `/.plans/` or `/.docs/*` cannot be completed by a worktree run. Either run it non-worktree, do it by hand, or add a `!`-negation to `.gitignore` first (three already exist).
  - Truncation, not time, is now the binding limit on long worker frames: `COMMAND_CAPTURE_LIMIT` is 256 KB against a `--include-partial-messages` stream, and it is not exposed as a config key.

### Owner-gated / blocked ⏸

Each row names what unblocks it. Nothing here is dispatchable as-is.

RELEASE MECHANICS — the actual shipping path, ordered:
  Phase P345: Publish `@ctx-traits/cdk` to npm (alpha). Gate: an owner decision to publish and an npm account/token. `ctx traits publish` exists and runs its preflight; nothing else blocks it.
  Phase P346: Shareable trait package on npm, end to end — the reuse demo. Gate: P345 first, since it proves the transport.

LAUNCH CONTENT — owner-voice, claim-gated:
  Phase P246: Capture REAL demo media and first-party brand assets. Gate: nothing visual is real today — zero image assets in the repo — and only the owner can record a real session.
  Phase P247: Public, HONEST feature matrix + per-competitor rebuttals. Gate: needs the claim gate settled — every row must be provable against the shipped product.
  Phase P248: Owner-voice rewrite of the flagship launch content. Gate: owner voice, by definition.
  Phase P249: Execute the phased HN launch sequence with claim-gate discipline. Gate: P246-P248, and an owner decision on timing.

EXTERNAL / EVIDENCE:
  Phase P282: Pilot + pre-release standards sweep. Gate: needs a pilot user, which is an owner relationship, not a run.
  Phase P503: Adherence evals — prove a trait changes behavior, per directive. Gate: owner-ordered; this is the measurement that would make the product claim provable rather than argued.

DESIGN DECISION PENDING:
  Phase P277: Escalation branch on exhaustion, and drop the commit file-hop. Gate: an owner call on what a run should DO when it exhausts — today it blocks and commits nothing, which may be the whole answer. The file-hop half (`.git/CTX_COMMITMSG` as a scribe→command handoff) is independent and mechanical.

### In plan, not yet owner-ordered

Group 54:
  Phase P235: Fetch a dated, digest-pinned ≥1,000-skill marketplace corpus, run the audit harness, commit the digest manifest + results

Group 115:
  Phase P479: Out-of-tree mutation tripwire: snapshot the invocation repo around each frame, park with the offending paths named
  Phase P480: OS-level spawn sandbox: sandbox-exec / landlock write confinement generated per worktree

Group 57:
  Phase P243: Product landing page: hero, one-liner, install, GitHub CTA, real screenshot

Group 94:
  Phase P380: `ctx traits do "<task>"`: ephemeral trait, full provenance
  Phase P381: Skeleton-constrained synthesis: families as typed templates
  Phase P382: `ctx traits distill <run-id>`: session ledger → trait package draft
  Phase P383: `ctx traits story <run-id>`: the run, told straight
  Phase P384: Positioning: the Slate/typed-process Q&A + docs line

Group 96:
  Phase P388: Trait-invokes-trait: typed sub-procedure composition
  Phase P389: Run handles: background sessions, steering, cancellation
  Phase P390: Cost/token budgets per run (P374 sibling) — per-model split + subscription-aware estimation

Group 117:
  Phase P486: Plan checkbox + id reconciliation against the git-verified landed list
  Phase P487: Certification-debt clearance: five landed-unapproved payloads reviewed against their contracts

Group 113:
  Phase P546: `check` counts 23 warnings and prints 7 — rendering must be total; 16 of 23 are dropped by a section-name filter `--verbose` does not lift

Unphased, recorded in Group 132's preamble (both killed a run 2026-07-26):
  - The prompt-limit band: COMMAND_CAPTURE_LIMIT (262,144) is 2x DEFAULT_MAX_INLINE_PROMPT_BYTES (131,072), so a captured value between them assembles a prompt that can never dispatch (run-73b73cc4)
  - The persistent-session ceiling: `session-mode=persistent` seats accumulate context across frames with no reset, budget, or honest park; they fail as garbled output (run-7619031a, 370k tokens)
