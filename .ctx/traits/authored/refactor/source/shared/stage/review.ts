import * as agents from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { smart, smart } from "../agent.ts";
import { plan, target, verdict1, verdict2, workSummary } from "../data.ts";
import { architectureDialect } from "../resource.ts";

const PROMPT =
  `Review the refactored state of {target} against the agreed design {plan}, independently of the other reviewer.
    Current work summary: {workSummary}.
    Cite the smell id or the deep-module violation for every blocker. ${agents.INTEGRITY_DOCTRINE}`;

export const primary = cdk.stage({
  agent: smart,
  input: cdk.input.prompt(PROMPT, {
    target,
    plan,
    workSummary,
    phaseBrief: plan,
    productBrief: architectureDialect,
  }),
  output: verdict1,
});

export const secondary = cdk.stage({
  agent: smart,
  input: cdk.input.prompt(PROMPT, {
    target,
    plan,
    workSummary,
    phaseBrief: plan,
    productBrief: architectureDialect,
  }),
  output: verdict2,
});

const JUDGE_PROMPT =
  `Review the implemented state of the working tree against the plan {plan}.
    Current work summary: {workSummary}.
    Your own verdict from the previous round, if any: {previousVerdict}.
    Cite the smell id or the deep-module violation for every blocker. ${agents.INTEGRITY_DOCTRINE}`;

// Single-reviewer stage shared by the single-review variants (quick,
// guarded, guided): target-free at the prompt level, so it does not widen
// the annotation-driven guided variant's procedure interface with `target`
// (0200 round-2 review blocker). Quick and guarded already require `target`
// through their checklist stage, so omitting it here costs them nothing.
export const judge = cdk.stage({
  agent: smart,
  input: cdk.input.prompt(JUDGE_PROMPT, {
    plan,
    workSummary,
    previousVerdict: verdict1.optional(),
    phaseBrief: plan,
    productBrief: architectureDialect,
  }),
  output: verdict1,
});
