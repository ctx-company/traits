# 0035 — Three config-resolution invariants, enforced rather than remembered

**Status:** ready to implement · **Raised:** 2026-07-29 · **Applies to:** 0025, 0034, and every future layered config

These three keep appearing as "watch out for" notes on individual tasks. Each
has already cost real time once. Make them properties of the system instead of
things the next author has to recall.

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
