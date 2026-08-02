# 0053 — Live view: restore the scroll key hints, and drop the duplicated ask line

**Status:** ready to implement · **Raised:** 2026-08-02 (owner, after the 0009 ask-pane work landed)

## 1. The footer stops advertising scrolling whenever the ask pane exists

`modules/cli/src/app/run_view.rs:1659` picks one of two footer strings:

```rust
if ask_lines.is_some() { "[?] ask · [q] exit · [ctrl-c] kill · [tab] cycle pane" }
else { "[q] exit · [ctrl-c] kill · [up/down] scroll · [pgup/pgdn] page · [home/end] jump · [tab] cycle pane" }
```

So the moment a run can be asked questions — which is every live run now — `[up/down] scroll`,
`[pgup/pgdn] page` and `[home/end] jump` vanish from the footer. The keys THEMSELVES still work
while the ask pane is collapsed (`apply_ask_presentation_key` consumes only `?` in that phase
and returns not-consumed for everything else, so keys fall through to the normal handling, and
`tui_kit.rs:55-58` still maps PageUp/PageDown/Home/End plus `g`/`G`). Only the advertisement was
lost — an owner watching a run is told the surface no longer scrolls.

Restore the full hint set with the ask entry added rather than substituted: one footer listing
ask, scroll, page, jump, cycle, exit and kill. Keep it inside the footer's width budget at the
narrow breakpoint — shorten labels before dropping entries, and if something genuinely cannot
fit, drop the least-used entry deliberately rather than a whole class of navigation.

Decide explicitly, and write down, what page/jump keys do WHILE a question is being composed
(phase `Editing`/`Waiting`): today they reach the text input, so panes cannot be scrolled
mid-question. Either that is the intended trade (say so in a comment) or the pane keys stay
live during editing and only text-editing keys are consumed.

## 2. `[?] Ask the guide` is rendered twice

A collapsed ask pane renders exactly one line — `"[?] Ask the guide"`
(`run_view.rs:1601`) — directly above a footer that already says `[?] ask`. The same
affordance is announced twice, and because the ask region is sized `lines + 1`
(`run_view.rs:1655`), that duplicate costs two terminal rows on every live run.

Drop the collapsed line and let the footer own the affordance: when the pane is collapsed it
should contribute no rows at all, and the panes above should reclaim that height. The pane
still expands normally on `?` for editing, waiting and the answer.

Tests pin the old text — `run_view.rs:6009` and `:6038` assert
`ask_lines(&ask) == ["[?] Ask the guide"]` — and must move with the change.

## Watch

- Both edits are in the shared renderer, so verify all three surfaces (live, dashboard preview,
  attached) and both width breakpoints, per the one-renderer contract.
- Reclaiming the collapsed ask rows changes every pane's height budget; check the narrow
  fallback and `CURRENT_MIN_OUTER_ROWS`-style minimums still hold.
- Related and same files: 0048 (TUIs fill the terminal, dashboard stops flashing "Loading…"),
  0044 (pane ids/rename/duplicate journey label), 0051 (startup progress in the paned surface).
  All four touch the same renderer — batch them if convenient, but each keeps its own contract.

## Done when

The live view's footer advertises ask, scroll, page, jump, cycle, exit and kill together at
both breakpoints; a collapsed ask pane contributes no rows and no second `[?]` hint, with the
freed height going to the panes above; expanding on `?` still works through editing, waiting
and answer; the behaviour of page/jump keys during composition is decided and documented in
place; and the pinned tests reflect the new contract.
