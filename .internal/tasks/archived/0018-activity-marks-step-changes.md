# 0018 — Current activity marks each step change, and the timestamp goes back to HH:MM:SS

**Status:** ready to implement · **Raised:** 2026-07-28

Three small changes to the same lines.

## 1. Mark the step change

Activity is a continuous stream of narration and model text with no boundary between steps, so a reader scrolling it cannot tell where one step ended and the next began. When the step changes, emit a row:

```
00:20:10 [Building Produce] (Initialization)
```

The step name, and the phase it is entering. It is a stream row like any other (`push_stream_row`), so it scrolls, ages and clips with everything around it — not a header, not chrome.

## 2. One space, not two

Current rows separate the elapsed prefix from the text with two spaces. Use one. Owner call, and it matches the bullet-and-single-space rhythm the footer and journey lines now use.

## 3. Back to `HH:MM:SS`

The prefix returns to the plain clock — `00:20:10`, not `00h 20m 10s`. `elapsed_text` (`tui.rs`) already produces exactly this from P548; the change is to use it rather than a unit-suffixed variant, and to keep the human-units formatter for relative ages ("3m ago"), which P548 deliberately kept separate.

This reverses part of an earlier ask (task 0007 / P552 discussed `00h 19m 13s`). The clock form is the decision: fixed width, sorts, and reads as a timestamp rather than a duration sentence. **Apply it everywhere the elapsed prefix appears — activity, history, journey — so the three panes agree.**

## Watch

- Derive the step-change row from the ledger's own step transition, not from a narration string containing a step name. The panel already receives accepted-frame refreshes (`push_step_summary`, `refresh`); use that seam, or a run whose narrator is off gets no markers.
- Emit it once per transition. A refresh that re-reports the same step must not add a second row — key on the step's identity plus its iteration, since the same step id recurs every round.
- The parenthetical is the phase being entered (`Initialization`, and whatever the activity vocabulary names). If no phase is known, print the step alone rather than an empty `()`.
- Check `story` uses the same prefix format once this lands; a run should read the same way live and afterwards (task 0016).

## Done when

Each step change appears in current activity as `HH:MM:SS [Step Name] (Phase)`; every elapsed prefix in the live view is `HH:MM:SS` separated by one space; relative-age text is unaffected; a run with no narrator still gets step markers.
