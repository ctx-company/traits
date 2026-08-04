# 0006 — Scrolling tracks the key, not the frame: no batching, no overshoot

**Status:** ready to implement · **Raised:** 2026-07-28

## The behaviour

Holding a scroll key does nothing, then jumps a long way, then keeps going past where the key was released. It reads as "aggregating during frames with delay, then running to infinity."

## Why

`apply_keys` (`modules/cli/src/app/run_view.rs`) drains **every** queued key in one pass and applies each as its own delta. The pump thread reads crossterm every 100 ms and pushes into an mpsc channel; the panel drains that channel when it repaints. So a held key produces auto-repeat events at the terminal's own rate, they queue while no repaint happens, and then thirty of them land as thirty single-row deltas in one frame.

The overshoot has the same cause from the other end: events generated before release are still in the queue after it, so the view keeps moving through a backlog the owner has already stopped producing.

This is adjacent to but NOT the same as the 1 Hz drain fixed earlier — that was *when* the queue is emptied; this is *what emptying it does*.

## What to build

Collapse a burst instead of replaying it. On each drain, fold consecutive same-direction scroll deltas into one movement sized to the render that is about to happen — one repaint, one visible step — rather than N steps applied invisibly. A held key then moves at the repaint rate, which is a rate the eye can follow and the release can stop.

Two properties to preserve:
- `Home`/`End`/`PageUp`/`PageDown` keep their existing single-shot semantics; only the repeating arrow/`j`/`k` case is folded.
- Follow-mode derivation stays as P470 left it: derived from the RESULTING position, never the key's direction.

## Watch

- Do not fix this by throttling the pump. The pump reading promptly is what keeps ctrl-c responsive; the problem is downstream of it.
- Do not drop events silently either — folding a burst into one movement is different from discarding a backlog. Fold the magnitude, do not truncate it to a fixed step, or a genuine page-length hold becomes a single row.
- Test with a real held key on a real terminal, not a synthetic burst: auto-repeat rate is a terminal setting and the failure only shows at the rates a person actually produces.

## Done when

Holding a scroll key moves the view smoothly at the repaint rate, stops within one frame of release, and does not overshoot; the jump keys are unaffected.
