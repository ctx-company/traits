import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

import { smart } from "../agent.ts";
import { slot } from "../data.ts";

// The reviewer opens and revises the step list (0283) and is the ONLY
// validator: it checks the working tree with its own tools, never a proof the
// worker ran. It rewrites the open-step list only here, at the handoff — never
// while the worker walks it — so stability is structural, not a frozen field.
// A step whose done-when it verified is DROPPED (a re-added acceptance is a new
// step, never a silent reopen); a blocked step gets a typed resolution or is
// escalated to the owner; the list is emptied only when the task is done.

export const baseline = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Before any work: read the plan ${slot.plan}, and with your own tools the task file it covers and the current working tree.
    Open the step list: an ordered list of concrete, actionable steps that, done, satisfy the plan's stages in order. Each step names its stage, ONE operation, and a falsifiable done-when. Front-load the first stage; do not enumerate steps that depend on work not yet done — you extend the list as the work reveals it. A step whose done-when already holds in the tree is not listed. If the plan is not actionable as written — a stage cannot be done as specified — make the earliest step the operation that surfaces or resolves that, so the first pass hits it rather than grinding.
  `,
  output: cdk.output.prompt`Return the open step list: (${slot.steps}).`,
});

export const validate = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    A for-each pass just walked the open steps (attached as input). Validate the working tree YOURSELF with your own tools against the plan ${slot.plan} — there is no proof, and the worker's word is not evidence. The worker's last return: ${slot.report.optional()}. Owner rulings already made this run: ${slot.ownerDecisions.optional()} — settled, never reopened.
    Rewrite the open step list for the next pass:
    - DROP every step whose done-when you verified holds in the tree. Carry none forward; a re-added acceptance is a NEW step.
    - KEEP a step whose done-when does not yet hold.
    - For a step the worker could not get past, attach a resolution — the blocker, your decision, and the concrete approach the next dispatch takes — never a bare "proceed". Only when the blocker is something ONLY the owner can settle, leave the step open and raise it to the owner instead of resolving it yourself.
    - APPEND genuinely new steps the work revealed, in order, prerequisites first; ADVANCE to the next stage's steps once the current stage's goal holds.
    Leave the list EMPTY only when every stage's goal holds in the tree — an empty list is your confirmation that the task is done.
  `,
  include: [slot.steps.optional()],
  output: cdk.output.prompt`
    Return the revised open step list: (${slot.steps}).
  `.extend`Separately: for a blocker only the owner can settle, emit ${agents.needsOwnerSignal}.`,
});

export const apply = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    The owner annotated the step list at the gate: ${slot.gateAnswer}. Each annotation carries the exact surface text it was made on (raw) and the owner's note (text); the surface was a verbatim copy of the step list, so raw locates the step.
    Apply every annotation to the open step list attached as input — binding edits, never arguments: reorder, rewrite, add, or drop steps as the notes direct; where you disagree, apply it anyway.
  `,
  include: [slot.steps.optional()],
  output: cdk.output.prompt`Return the edited open step list: (${slot.steps}).`,
});

// The unattended lanes (complex, annotate): no owner is reachable, so the
// reviewer decides every blocker itself — attaching a resolution — and never
// leaves a step open for an owner who will not answer.
export const validateSolo = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    A for-each pass just walked the open steps (attached as input). Validate the working tree YOURSELF with your own tools against the plan ${slot.plan} — there is no proof, and the worker's word is not evidence. The worker's last return: ${slot.report.optional()}.
    Rewrite the open step list for the next pass: DROP every step whose done-when you verified holds (carry none forward); KEEP a step whose done-when does not yet hold; for a step the worker could not get past, attach a resolution — the blocker, your ruling, and the concrete approach — never a bare "proceed". There is no reachable owner: decide every blocker yourself, ground the call in evidence you verified, and record it in the resolution — never leave a step for the owner. APPEND new steps the work revealed, prerequisites first; ADVANCE to the next stage's steps once the current stage's goal holds.
    Leave the list EMPTY only when every stage's goal holds in the tree.
  `,
  include: [slot.steps.optional()],
  output: cdk.output.prompt`Return the revised open step list: (${slot.steps}).`,
});

// The complex lane's second, independent validator: it re-checks the tree
// itself and has the final say on the list for the next pass — never
// deferring to the first reviewer's word.
export const crossValidate = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    A second, independent review of the same state. The first reviewer just rewrote the open step list (attached as input). Validate the working tree YOURSELF with your own tools against the plan ${slot.plan} — do not defer to the first reviewer's word. The worker's last return: ${slot.report.optional()}.
    Return the step list you stand behind for the next pass: DROP steps whose done-when you verified holds; KEEP or resolve the rest with a typed resolution; APPEND anything the first reviewer missed. Leave it EMPTY only when every stage's goal holds in the tree.
  `,
  include: [slot.steps.optional()],
  output: cdk.output.prompt`Return the open step list: (${slot.steps}).`,
});
