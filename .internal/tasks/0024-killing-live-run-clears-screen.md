# 0024 — Killing a live run leaves the terminal dirty

**Status:** implemented, pending review · **Raised:** 2026-07-29

Killing a run mid-flight does not properly restore the screen: the alternate
screen buffer and/or raw mode are left behind, so the shell prompt comes back
into a corrupted terminal.

## Where to look

Teardown must run on every exit path, not just the clean one — normal
completion, `q`/detach, SIGINT, and a panic. A restore that only happens at the
bottom of the happy path is skipped by exactly the case that matters.

## Watch

- Restoring twice must be harmless; several paths may race to it.
- A panic mid-render must restore before the payload prints, or the backtrace
  is unreadable — that is precisely when the terminal state matters most.
- This is separate from the run's own shutdown. The child harness may still be
  dying; the terminal must not wait on it.

## Done when

`ctrl-c`, detach, normal completion, and a panic all return a usable terminal
with the alternate screen exited and raw mode off.
