# 0035 — Three config-resolution invariants, enforced rather than remembered

**Status:** done · **Raised:** 2026-07-29 · **Closed:** 2026-08-05 · **Applies to:** 0025, 0034, and every future layered config

These three keep appearing as "watch out for" notes on individual tasks. Each
has already cost real time once. Make them properties of the system instead of
things the next author has to recall.

All three invariants were already enforced at the runtime seams before this
change closed the remaining gaps:

- **Invariant 1** (merge once): `merge_built_in_harness_overrides` has run
  exactly once inside `resolve_runtime_config` since P568
  (`modules/io/src/harness_config.rs:3552-3559`). This change adds the
  regression tests the invariant named as missing, in
  `modules/io/src/harness_config.rs` `mod config_tests`:
  `raw_lookup_on_resolved_registry_equals_merged_value` (a raw `.get(id)` on
  the resolved registry equals the merged value — the narrator casualty),
  `empty_harness_section_materializes_every_built_in` (an empty `[harness]`
  section still yields every built-in id — the "unknown harness id"
  casualty), and `explicit_unset_survives_resolution_and_is_not_reinherited`
  (locks the Watch item: an explicit `flag = ""` resolves to `None`, and a
  second merge pass over the same map re-inherits the built-in's value —
  proof that lookup sites must never "merge defensively").
- **Invariant 2** (provenance-bearing resolved view): `ConfigReport.winners`
  and `doctor --config` already rendered every layered table including
  `trait.<id>.*` seats (0034, commit `58b91f3a`). This change adds the
  end-to-end proof that was missing: `proof_config.rs`'s
  `layered_doctor_reports_exact_leaf_provenance_and_additive_contributors`
  now declares a `[trait.layered-trait.agent.role.worker]` seat and asserts
  its `doctor --config --json` value, winning layer, and source — the
  "new scope appears in the resolved view" property pinned through the CLI,
  not just the internal winners map (which `merge_machine_config_records_trait_agent_winners`
  already covered).
- **Invariant 3** (one mechanism per decision): `--assign`, run-profile
  `[assign.<role>]`, and `[agent.role.*]`/scope tables converge on
  `resolved_assignment_for_role`'s single layering
  (`modules/io/src/harness_config.rs:5920-5995`); `--master` is a hard error
  routing to `--assign` (P567 alias pattern,
  `modules/cli/src/app/run.rs:350-353`); the Justfile bakes no `--assign`.
  No residual dual mechanism was found in this audit; none was added.

Verified: `CARGO_TARGET_DIR=target cargo test -p ctx-traits-io` (new tests
pass among 78 in `config_tests`), `CARGO_TARGET_DIR=target cargo test -p
ctx-traits-cli --test proof_config` (all 35 pass, including the extended
fixture), `cargo fmt --check`, and `cargo clippy -p ctx-traits-io -p
ctx-traits-cli --all-targets --all-features -- -D warnings` (clean).

---

## 1. Merge once, at config resolution

Layered config must be collapsed to its effective form at ONE point — where the
runtime document is finalized — never at the lookup sites.

**Why:** P568 merged `[harness.*]` over the built-ins at the lookup sites it
knew about. Dispatch and narration used a raw `registry.harness.get(id)` and so
saw a half-defined harness: the narrator lost `--model` and runs failed with
"progress narration harness has a resolved model but no CLI model flag". Doctor
looked correct at the same moment, because doctor was one of the sites that had
been updated. A second instance followed immediately — with the harness section
emptied, built-ins were never materialized at all and every lookup failed with
"unknown harness id".

**Enforce:** resolution collapses layers before the document is returned, so a
plain `.get(id)` is always correct. A test that asserts a raw lookup on a
resolved document equals the merged value would have caught both.

## 2. The effective value must be visible, with its provenance

Every resolved knob must be renderable with the layer that won. If a value can
change behavior, an operator must be able to see what it is and where it came
from without reading code.

**Why:** the `--assign` overrides baked into Justfile recipes work, but they are
invisible to `doctor --config` — so the config says one thing and the run does
another, with nothing reconciling them. The same trap is available to 0034's
trait scoping the moment a winning scope is not named.

**Enforce:** a resolved-config surface exists for every layered table, showing
value plus winning layer. Any new scope must appear there in the same change
that introduces it, not later.

## 3. One mechanism per decision

When two mechanisms can decide the same thing, they will eventually disagree
and the less visible one will win silently.

**Why:** `--assign` in a recipe versus `[agent.role.*]` in config decide the
same fact today. After 0034 lands there would be three. The recipe wins, is
unversioned, and contradicts the file an operator would check first.

**Enforce:** when a config mechanism supersedes an ad-hoc one, the ad-hoc one is
REMOVED in the same change. If it must survive for compatibility, it becomes a
hidden alias routed through the same resolution path — the P567 pattern, where
the retired verbs and their replacements converge on one handler so an alias
cannot drift from what it aliases.

---

## Watch

- These are not style rules. Each names a specific failure that reached a run:
  a silent narrator, an unknown harness, a recipe overriding config invisibly.
  Keep the failure in the text — a rule without its casualty gets optimized away.
- Invariant 1 has a sharp edge: merging is NOT always idempotent. An explicit
  unset (`field = ""`) resolves to `None`, and a second merge pass would
  re-inherit the built-in's value. Merge exactly once; do not "merge defensively"
  at a lookup site to be safe.

## Done when

Layered config is collapsed at one resolution point and a raw lookup is always
correct; every layered table has a provenance-bearing resolved view; no decision
has two live mechanisms, and superseded ones are removed or routed through the
same handler.
