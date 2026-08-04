# 0096 — The guide modal earns its place: bigger, dimmed behind, and actually informed

**Status:** ready to implement · **Depends on:** nothing; the modal itself landed in `4926b24d` · **Raised:** 2026-08-04 (owner, after using it)

Five changes to the guide. Four are presentation. The fifth is the one that
matters: **the guide does not know what the run is about.**

## Decisions

### It knows the run (the important one)

- **Ask it "what is this run about?" today and it cannot answer.** Not because
  the model is weak — because nothing in its context says. `guide::evidence`
  (`guide.rs:35`) assembles: current step, sequence statuses, up to six
  accepted beats with verdicts/blockers/activity, and whether activity
  evidence exists. All real, all about mechanics. None of it says what the run
  was ASKED to do.
- **The assignment goes in.** The trait being run, and the run's own inputs —
  the task text, the port values it was dispatched with. That is the first
  thing a person asks about and the only thing currently absent.
- **The evidence budget has to grow with the content.** `MAX_EVIDENCE_CHARS`
  is 2000 and each field is bounded to 360 (`guide.rs:14`, `:43`). Adding the
  assignment to an unchanged budget just evicts the verdicts. Size the budget
  to what the seat can actually take, and if something must be dropped, drop
  routine activity before verdicts and blockers — the existing sort already
  knows that order.
- **Keep the honesty instruction.** The prompt tells the guide to say unknown
  rather than guess. That is right and stays; it is currently just saying
  unknown to almost everything, which is a context problem wearing an honesty
  costume.

### It is big enough to use

- **Roughly 3:2, about 1.5× its current width and height**, capped by the
  terminal. A conversation in a strip is a conversation you scroll instead of
  read.
- **A ratio, not a fixed size.** It must degrade to the available area on a
  small terminal rather than being clipped.

### The rest of the screen recedes

- **Dim what is behind it — do not cover it.** The point is focus, not
  occlusion; the run stays legible underneath.
- **As a reusable component**, because this is the third place that dims:
  unfocused terminal (`draw`), inactive panes, and now a modal backdrop.
  `tui_kit` already carries `Modifier::DIM` styling in several spots
  (`tui_kit.rs:728`, `:748`, `:866`). One helper, used by all three.

### Two smaller ones

- **Clearing the chat.** A conversation that can only grow is one you abandon
  rather than reuse. It is in-memory (`4926b24d`), so clearing is local — no
  ledger involvement.
- **Hints match the main view exactly:** `[enter] send · [esc] close`, bullet
  separated, same shape as `"[d] dashboard · [q] exit · [ctrl-c] kill · …"`
  (`run_view.rs:1856`). If 0095 has landed, the bullet spacing follows it.

## Scope

`guide::evidence` and its budget; the modal's geometry; a shared dim-backdrop
helper in `tui_kit` with the existing dim sites moved onto it; a clear action
and its hint; the guide's hint line.

## Watch

- **The guide seat is one-shot and bounded.** More context means more tokens
  per question on a seat whose timeout is already tight. Measure what the
  larger evidence costs before assuming the budget can simply double.
- The assignment can be large — a task file is not a status line. Bound it
  deliberately rather than letting one long input port crowd out every verdict.
- The backdrop helper must not dim the modal itself. The unfocused-window dim
  styles the whole frame buffer after the widget draws; a backdrop needs the
  region *behind* one, which is a different operation with the same modifier.
- Clearing must not cancel an in-flight question, or must say that it does.
- 0080, 0082 and 0083 all edit these panes; the modal sits over them.

## Done when

Asking "what is this run about?" gets an answer drawn from the trait and the
run's assignment; verdicts and blockers survive alongside it rather than being
evicted; the modal opens at roughly 3:2 and about 1.5× its current size,
capped by the terminal; the content behind it is dimmed and still readable,
through one helper the other two dim sites also use; the chat can be cleared;
and the hints read like the main view's.
