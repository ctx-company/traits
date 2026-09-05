import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

import { smart } from "../agent.ts";
import { slot } from "../data.ts";

// The reviewer grades claims (0281.7): it runs once before any work (the
// baseline, which opens the ledger and writes the first brief) and then
// once per claim the stage's proof let through — or once per stage whose
// time ran out or whose worker reported itself blocked. Its previous
// verdict is the ledger it carries; the runtime keeps every carried step's
// done-when from moving. The conventions (what a claim is graded against,
// what the brief holds, when to summon the owner) live in the verdict
// schema's own field descriptions, not here.
const reviewBody = cdk.input.prompt`
  Review the working tree against the plan ${slot.draft} as source of truth; the task file is what the plan must cover, read it with your tools.
  The worker's report on its latest dispatch: ${slot.report}. What the stage's proof said about its claim, when it made one: ${slot.proofResult.optional()}. Changed files: ${slot.changedFiles}.
  Your previous verdict, the ledger you carry forward: ${slot.verdict1.optional()}. The second reviewer's verdict, if any: ${slot.verdict2.optional()}.
`;

const NO_OWNER = `There is no reachable owner during this run — never set escalation to needs-owner. Any question you would have escalated is yours to decide: make the best-effort judgement call, ground it in evidence you verified yourself, and record the decision and its reason in the verdict. A decided question is never a blocker.`;

// The baseline: before any work, the reviewer opens the ledger — every
// stage of the plan, open unless its goal already holds in the tree — and
// writes the first brief, so the first dispatch already has one unit of
// work and the first claim is graded against text that existed before it.
const baselineBody = cdk.input.prompt`
  Before any work: review the working tree against the plan ${slot.draft} as source of truth; the task file is what the plan must cover, read it with your tools.
  Produce the opening verdict: the stage ledger with every stage open unless its goal already holds in the tree, the blockers and steps that stand between the tree and the first open stage's goal (each step with its frozen done-when), and the brief.
`;

export const baseline = cdk.defineStep.prompt({
  agent: smart,
  input: baselineBody.extend`Owner rulings already made in this run (if any): ${slot.ownerDecisions.optional()} — they are settled; never reopen one.`,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
  `,
});

// The tree lane's baseline: the owner's annotations are what the plan must
// cover, and no owner is reachable during the run.
export const baselineAnnotated = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Before any work: review the working tree against the plan ${slot.draft} as source of truth; the owner's annotations ${slot.annotations} are what the plan must cover.
    Produce the opening verdict: the stage ledger with every stage open unless its goal already holds in the tree, the blockers and steps that stand between the tree and the first open stage's goal (each step with its frozen done-when), and the brief.
    ${NO_OWNER}
  `,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
  `,
});

export const primary = cdk.defineStep.prompt({
  agent: smart,
  input: reviewBody.extend`${NO_OWNER}`,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
  `,
});

// The tree lane: no task file exists; the owner's annotations are what the
// plan must cover. No owner is reachable during the run.
export const primaryAnnotated = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the working tree against the plan ${slot.draft} as source of truth; the owner's annotations ${slot.annotations} are what the plan must cover.
    The worker's report on its latest dispatch: ${slot.report}. What the stage's proof said about its claim, when it made one: ${slot.proofResult.optional()}. Changed files: ${slot.changedFiles}.
    Your previous verdict, the ledger you carry forward: ${slot.verdict1.optional()}.
    ${NO_OWNER}
  `,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
  `,
});

export const primarySummoning = cdk.defineStep.prompt({
  agent: smart,
  input: reviewBody
    .extend`Owner rulings already made in this run (if any): ${slot.ownerDecisions.optional()} — they are settled; never reopen one. Decide what you can decide yourself and record the decision and its reason in the verdict; a decided question is never a blocker.`,
  output: cdk.output.prompt`
    Return the typed review verdict: (${slot.verdict1}).
  `.extend`Separately: for any contradiction or question only the owner can answer, emit ${agents.needsOwnerSignal}.`,
});

// The apply pass (0281.1): one interpreter, one artifact. Runs right after
// an annotated gate and before the next work loop, so the worker only ever
// sees the applied verdict. The verdict is this step's own output, so it
// rides in as an optional include and is referenced in prose (the
// sanctioned self-input form); the vocabulary of what an annotation can do
// lives in the verdict schema's dispositions field, not here.
export const apply = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    The owner annotated your verdict at the gate: ${slot.gateAnswer}.
    Each annotation carries the exact surface text it was made on (raw) and the owner's note (text); the surface was a verbatim copy of the verdict, so raw locates the item.
    Apply every annotation to your current verdict (attached as input) against the plan ${slot.draft}. Annotations are binding edits, never arguments: where you disagree, apply it and say so in the advisory. The brief follows the applied ledger.
  `,
  include: [slot.verdict1.optional()],
  output: cdk.output.prompt`
    Return the same verdict, edited, with one disposition per annotation: (${slot.verdict1}).
  `,
});

export const secondary = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt`
    Review the working tree against the plan ${slot.draft} as source of truth; the task file is what the plan must cover, read it with your tools.
    The worker's report on its latest dispatch: ${slot.report}. What the stage's proof said about its claim, when it made one: ${slot.proofResult.optional()}. Changed files: ${slot.changedFiles}.
    The first reviewer's verdict on this same state: ${slot.verdict1.optional()}. Your previous verdict, the ledger you carry forward: ${slot.verdict2.optional()}.
    ${NO_OWNER}
  `,
  output: cdk.output.prompt`
    Return the typed review verdict — status is revise while any blocker remains, approved when none do (${slot.verdict2})
  `,
});
