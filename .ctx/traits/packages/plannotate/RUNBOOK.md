# plannotate — one-take run script

One unedited take. Read this once before recording; do not improvise the annotation wording — say
roughly what's below so the annotations line up with what the viewer is told to watch for.

## Before you start

- `command -v plannotator` succeeds on this machine (the run itself checks this first and fails
  loudly if not, but confirm ahead of time so the take doesn't die on step one).
- Never rebuild the package (`ctx traits build plannotate`) while the run is live.
- Confirm the effective idle bound on every plannotator step (`plan-owner-review`,
  `implement-owner-review`, `display-brief`) is longer than `command-idle-seconds = 600` from
  `.ctx/traits/runtime.toml` — time a real annotation pass once before recording, don't trust the
  config number.

## What to type

Start the run with an assignment a few sentences long — something with an obvious, checkable
scope (e.g. "add a `--dry-run` flag to the `foo` command that prints what it would do without
doing it"). Small enough to build in a few rounds; big enough that there's something real to
annotate.

## The run, gate by gate

1. **`preflight-binary`** — no interaction; the viewer sees this pass silently and fast. If
   plannotator is missing this is where it stops, before any model call.
2. **`plan-draft`** — the smart agent drafts the plan. Silent while it works.
3. **`plan-owner-review`** — plannotator opens in plan mode. **Deny it once**, with a concrete,
   checkable annotation (e.g. "also update the README's flag table"). This is the moment to narrate:
   the run is now silently waiting on a human, not polling or guessing.
4. **`plan-review`** — the smart agent refines the draft against your denial. Point out, in the
   refined plan it produces, the sentence that visibly answers your annotation — this is the "a
   rejected annotation is provably present in the refined plan" proof from the task's Done-when.
5. **`implement-work` → `implement-review`** (round 1) — the worker implements the refined plan;
   the reviewer reviews it. **Have the reviewer land on `revise`** the first time through (easy to
   arrange: ask for something the plan didn't cover, or just let a real gap surface) — narrate that
   the owner is *not* summoned on this round. That's the escalation ladder: the machine reviewer
   has to be satisfied before the run will ever ask you for anything.
6. **`implement-work` → `implement-review`** (round 2) — reviewer now sets `approved`. Narrate that
   *this* is what triggers the owner gate — nothing else does.
7. **`round-briefing` → `implement-owner-review`** — a fresh smart frame writes the round briefing
   from the work summary, the verdict, and the actual working tree; then plannotator opens it in
   gate mode. **Reject it once**, with a concrete annotation. Narrate: the loop is not done — a
   reviewer approval alone never ends it.
8. **`implement-work` → `implement-review`** (round 3) — the worker addresses your rejection; the
   reviewer's prompt treats your unaddressed note as a blocker until it visibly is one, so point out
   in this round's `blockers` field that it named your note by content, not just "apply owner
   feedback".
9. **`implement-owner-review`** (second time) — **approve it**. Narrate: only now does the loop
   exit — both the reviewer and you had to say yes, and nothing forces a re-ask.
10. **`summary-brief`** — a smart frame writes the brief and the commit message, grounded in the
    diff; plannotator displays the brief with no gate. Narrate that this window carries no
    decision — it's a record, not another checkpoint.
11. **`git-stage` → `git-commit`** — the run stages and commits, brief included. Show the commit:
    the brief under `.internal/briefs/<slug>.md` is inside it.

## What the viewer should walk away noticing

- The run goes **silent** at every plannotator step — that silence is a human reading, not the run
  stalling. (This is why 0084's per-step idle budget exists: without it the demo dies mid-read with
  no explanation on screen.)
- The owner is asked **twice**, and both times **only after the reviewer already said yes**. A
  reviewer `revise` round never reaches you.
- The loop has **no round counter** anywhere on screen, because there isn't one — it ends when both
  verdicts are `approved`, however many rounds that takes.
- A denial doesn't vanish: it reappears, by name, until it's actually addressed.
