# 0051 — Run startup progress belongs in the live pane layout, not in inline text lines

**Status:** ready to implement · **Raised:** 2026-08-02 (owner, watching a dispatch)

## Today

`ctx traits run` prints its pre-drive progress as bare stderr lines before the live view ever
appears:

```
ctx run · initialization
ctx run · creating worktree
ctx run · seeding
ctx run · warming target
ctx run
```

Then the screen switches to the four-pane live view and those lines scroll away as terminal
scrollback. The first thing an owner sees of a run is therefore a different presentation from
everything after it — plain appended text, no panes, no borders, no layout, and it does not
resize, redraw, or survive into the view it precedes.

Emission site: `modules/cli/src/app/command_handlers.rs:793` (`eprintln!("ctx run · …")`) and
the sibling stage prints around it. `drive.rs:1341` already notes that a fresh worktree's
creation/seeding/setup steps are narrated in-pane once the panel exists — startup is the gap
before that.

## Target

Start the paned surface FIRST and render startup inside it, in the same visual language as the
live view: bordered panes, the same two-tone palette, the same title/label treatment. It does
not have to be the exact 2x2 grid the running view uses — startup has less to show, so a
different split is fine (e.g. one wide status pane plus a narrow detail pane, or a 2x1 that
grows into the 2x2 once the first frame dispatches). What must hold is that it reads as the
same surface: an owner should see one continuous view from `ctx traits run` to the first frame,
not text lines followed by a screen takeover.

Each startup stage (initialization, worktree creation, seeding, warm, harness probe, trust
gate) is a row in that surface with its own state — pending, running, done, failed — rather
than a line appended once and never updated. A failing stage stays visible with its reason
instead of scrolling past.

## Watch

- Non-TTY paths must keep working exactly as they do now: `--progress stream|status|none`,
  piped output, and `--json` all still emit plain lines. Only the interactive default changes.
- The startup surface has to hand off to the live view without a flash, a clear, or a second
  alt-screen enter — one terminal session, one surface, panes reflowing as content arrives.
- Errors before the panel exists (trust refusal, unrunnable command preflight, dispatch
  refusal) must still be readable AFTER the process exits — a message drawn only inside an
  alt-screen that is then torn down is a message the owner never gets to read. Decide
  explicitly how those land in scrollback.
- Related: 0048 (all TUI surfaces fill the terminal; dashboard stops flashing "Loading…") —
  same renderer, same layout rules, do them in whatever order suits but keep one layout path.

## Done when

An interactive `ctx traits run` shows its startup stages inside the paned surface in the live
view's style, each stage carrying live state rather than a one-shot line; the transition into
the running view has no clear or flash; non-interactive and `--json` output is byte-identical
to today; and a startup failure remains readable after the process exits.
