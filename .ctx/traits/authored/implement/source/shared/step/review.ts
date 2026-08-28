import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { slot } from "../data.ts";

export const primary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the implemented state against the draft ${slot.draft} as source of truth.
    Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
    Verdicts from previous rounds (if available): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Don't rush the approval, take as many rounds as it needs, while staying pragmatic and task-focused.
    There is no reachable owner during this run — never set escalation to needs-owner. Any question you would have escalated is yours to decide: make the best-effort judgement call, ground it in evidence you verified yourself, and record the decision and its reason in the verdict. A decided question is never a blocker.
  `,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
    Verdict should come with proposed solutions.
    Put solutions in most efficient order.
  `,
});

export const secondary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the implemented state against the draft ${slot.draft} as source of truth.
    Worker's summary: ${slot.workSummary}, Changed files ${slot.changedFiles}.
    Verdicts from previous rounds (if available): ${slot.verdict1.optional()} & ${slot.verdict2.optional()}
    Don't rush the approval, take as many rounds as it needs, while staying pragmatic and task-focused.
    There is no reachable owner during this run — never set escalation to needs-owner. Any question you would have escalated is yours to decide: make the best-effort judgement call, ground it in evidence you verified yourself, and record the decision and its reason in the verdict. A decided question is never a blocker.
  `,
  output: cdk.output.prompt`
    Return the typed review verdict — status is revise while any blocker remains, approved when none do (${slot.verdict2})
  `,
});
