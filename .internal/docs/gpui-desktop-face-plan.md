# The gpui desktop face — initial plan

Source document for the plan trait (`--task` references this file by path).
Raised 2026-08-25 from the owner brainstorm; rulings R1–R3 are owner
decisions of 2026-08-25. Companion diagram:
`.hidden/2026-08-24-gpui-the-gui-is-a-subscriber.html` (note 05 of the
lifecycle series, gitignored).

## Context

The center (0243.x) exists so that activity happens once: drivers notify
once, the center updates its model once, every subscriber gets the delta.
A gpui window is one more subscriber — the same socket, the same
JSON-lines snapshot-plus-deltas, the same spawn/interrupt/pause verbs the
TUI dashboard uses. There is no GUI backend: if a face needs something the
center does not serve, the center learns to serve it and every face gets
it in the same release. A capability that exists in only one face is a
design defect.

## Rulings (owner, 2026-08-25)

- **R1 — dashboard + live runs first.** The GUI is for the dashboard and
  live runs. Feature parity between GUI and TUI is the goal
  (realistically); the GUI is "only" the visual part — the center carries
  everything else. Parity's mechanism: a feature is something the center
  serves plus per-face rendering, never a per-face feature.
- **R2 — four faces, TUI stays.** The TUI remains a first-class face. A
  mobile GUI is planned and probably a web dashboard — four faces total on
  one center. gpui is the DESKTOP face only (gpui does not target
  mobile/web); the later faces are different shells on the same door.
  Remote transport for mobile/web stays deferred per the 0243 parent
  ruling ("sharing beyond this machine: deferred").
- **R3 — gpui is good enough; drive it.** No research/grounding pass. The
  walking skeleton is small enough to be the probe; if gpui reality
  disappoints there, the loss is one task.

## Gate

Work starts only after **0243.4** (everyone asks the center — the TUI
dashboard's poll loop deleted, the center proven by a real consumer) and
**0243.5** (spawn/interrupt/pause through the center) land. A GUI built
before that would be built against a surface nobody has ever depended on.
The summons work item additionally gates on **0253.4** (ask parks
`awaiting-owner`) and does not block the others.

## Structural decision the charter must carry

**The crate lives OUTSIDE the cargo workspace.** gpui is a git dependency
on Zed's tree; inside the workspace it would put a multi-minute compile
into `just test-full` and every CI lane. Own directory, own `Cargo.toml`
and lockfile, own build lane; path-dependencies on the io/core crates it
consumes. This is a cost ruling, not architecture ceremony.

## Work items

1. **Walking skeleton.** A gpui window that connects to the center's
   socket, subscribes, and renders the live session list from snapshot +
   deltas. Read-only, list-only (groups/structure are rendering, not risk
   — later). Covers center-absent behavior: spawn-on-need or a visibly
   stale view, the same posture as the TUI, never a hard error. This item
   deliberately carries both first-contact risks at once: gpui reality
   (R3's probe) and speaking the center's wire from a fresh process —
   including finding out whether the center's client side (today inside
   `modules/io`) needs extracting into a reusable crate. Done when: an
   externally started run appears in the window without any poll loop in
   the GUI.

2. **Run detail.** Select a row → the ledger read (the store path that
   exists today) plus live-follow from activity deltas; the frame tree
   rendered natively, not as terminal lines. Done when: a live run's
   detail view updates as frames land, and a finished run's view is
   identical after closing and reopening the window.

3. **Act on a run.** Spawn (trait + args form), interrupt, pause/resume —
   through the center's verbs from 0243.5, nothing driver-direct. Done
   when: each verb round-trips from the window and every other subscriber
   observes the effect via deltas.

4. **The summons surface.** `awaiting-owner` rows surfaced as native
   notifications; question plus typed or choice answer; answering resumes
   the run. Gates on 0253.4; lands whenever that arc does without
   blocking ship. Done when: a parked scratch run is answered entirely
   from the notification and continues.

5. **Ship shape.** App bundle + icon, a `ctx gui` launcher, and the
   version-skew posture: the center's socket is version-scoped, so the app
   states what it does when no matching center exists. One task,
   deliberately last.

## Done criteria

- A distributable desktop app shows live runs with parity on the
  dashboard's core verbs (list, detail, spawn, interrupt, pause/resume).
- No poll loop anywhere in the GUI; every update arrives as a center
  delta.
- Nothing is served to the GUI that the TUI and CLI cannot also reach.
- The workspace's gates (`just test-full`, CI lanes) are byte-identical in
  cost — the gpui dependency never enters the workspace build.

## Constraints for the task files

- Decomposition follows these work items, never stack layers; no
  define-first, audit-only, or gate-only tasks (plan doctrine).
- Dependencies: item 1 needs only the landed center family; item 3 needs
  0243.5's verbs; item 4 needs 0253.4; items 2 and 5 need item 1.
- 0252.6 (action registry) is deliberately NOT touched by this plan —
  owner ruling 2026-08-25: too early to pin its surface-neutral shape.
