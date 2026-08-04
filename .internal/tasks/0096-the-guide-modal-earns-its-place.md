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
- **The context is the state at the moment the question is SENT, and the
  answer is attributable to it.** A run moves while the guide thinks; an
  answer about a step that finished thirty seconds ago is wrong even though it
  was right when asked. Two thirds of this already works and the remaining
  third is the real gap:
  - Already right: `apply_ask_key` (`run_view.rs:1387`) recomputes
    `guide::evidence` on EVERY key the ask pane handles — including the Enter
    that sends — and the worker captures `chat.context.clone()` at dispatch.
    So the evidence is not from when the modal opened. Do not "fix" this.
  - Still wrong: that evidence is built from `state.view`, which is rebuilt in
    `tick_locked` when the panel paints. Between paints the view lags the
    ledger, so "fresh" can mean "as of the last repaint". Rebuild, or read
    through to current state, before composing evidence for a send.
  - Still wrong: nothing tells the reader WHICH state an answer describes. An
    answer should carry the step it was composed against, so an answer that
    has been overtaken is visibly an answer about the past rather than a wrong
    answer about the present.
- **Each question is currently answered with no memory of the previous one.**
  `guide_prompt` (`guide.rs:158`) is instructions + question + evidence;
  `dispatch(question, context)` never receives the transcript. The modal
  renders a conversation the model is not having. Decide deliberately: either
  send prior exchanges so follow-ups like "why?" work, or make it plain that
  each question stands alone. Silently looking like a chat while behaving like
  single-shot lookups is the one option to reject — and it interacts with the
  budget above, since a transcript competes with evidence for the same room.
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

### Three smaller ones

- **It is the guide everywhere, and stops being "ask".** The seat, the modal
  and this task already say guide; only the labels, hints and the
  `AskPane`/`AskPhase` types still say ask (0054). Rename them. The word is
  wanted for the opposite direction — 0087 makes `ask` a trait-authoring verb
  for a run asking outward, and two opposite meanings of one word across the
  ledger and the UI is a collision worth spending an afternoon to avoid.
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
evicted; a question sent after the run advances is answered against the state
at send time rather than the last repaint, and its answer says which state that
was; a follow-up question either sees the previous exchange or is plainly not a
follow-up; the modal opens at roughly 3:2 and about 1.5× its current size,
capped by the terminal; the content behind it is dimmed and still readable,
through one helper the other two dim sites also use; the chat can be cleared;
and the hints read like the main view's.
