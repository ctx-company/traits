import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

import { smart } from "../agent.ts";
import { slot } from "../data.ts";

const reviewBody = cdk.input.prompt`
  Review the implemented state against the plan ${slot.draft} as source of truth; the task file is what the plan must cover, read it with your tools.
  Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
  Verdicts from previous rounds (if available): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
  Don't rush the approval, take as many rounds as it needs, while staying pragmatic and task-focused.
`;

export const primary = cdk.defineStep.prompt({
  agent: smart,
  input: reviewBody
    .extend`There is no reachable owner during this run — never set escalation to needs-owner. Any question you would have escalated is yours to decide: make the best-effort judgement call, ground it in evidence you verified yourself, and record the decision and its reason in the verdict. A decided question is never a blocker.`,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
    Verdict should come with proposed solutions.
  `,
});

export const primarySummoning = cdk.defineStep.prompt({
  agent: smart,
  input: reviewBody
    .extend`Owner rulings already made in this run (if any): ${slot.ownerDecisions.optional()} — they are settled; never reopen one. Decide what you can decide yourself and record the decision and its reason in the verdict; a decided question is never a blocker.`,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
    Verdict should come with proposed solutions.
  `.extend`Separately: for any contradiction or question only the owner can answer, emit ${agents.needsOwnerSignal}.`,
});

// The apply pass (0281.1): one interpreter, one artifact. Runs right after
// an annotated gate and before the next implement round, so the worker only
// ever sees the applied verdict. The verdict is this step's own output, so
// it rides in as an optional include and is referenced in prose (the
// sanctioned self-input form); the vocabulary of what an annotation can do
// lives in the verdict schema's dispositions field, not here.
export const apply = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    The owner annotated your verdict at the gate: ${slot.gateAnswer}.
    Each annotation carries the exact surface text it was made on (raw) and the owner's note (text); the surface was a verbatim copy of the verdict, so raw locates the item.
    Apply every annotation to your current verdict (attached as input) against the plan ${slot.draft}. Annotations are binding edits, never arguments: where you disagree, apply it and say so in the advisory.
  `,
  include: [slot.verdict1.optional()],
  output: cdk.output.prompt`
    Return the same verdict, edited, with one disposition per annotation: (${slot.verdict1}).
  `,
});

export const secondary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the implemented state against the plan ${slot.draft} as source of truth; the task file is what the plan must cover, read it with your tools.
    Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
    Verdicts from previous rounds (if available): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Don't rush the approval, take as many rounds as it needs, while staying pragmatic and task-focused.
    There is no reachable owner during this run — never set escalation to needs-owner. Any question you would have escalated is yours to decide: make the best-effort judgement call, ground it in evidence you verified yourself, and record the decision and its reason in the verdict. A decided question is never a blocker.
  `,
  output: cdk.output.prompt`
    Return the typed review verdict — status is revise while any blocker remains, approved when none do (${slot.verdict2})
  `,
});
