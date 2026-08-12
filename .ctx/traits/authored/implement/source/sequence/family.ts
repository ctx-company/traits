// Shared implementation concepts used by the native implement family leaves.
import {
  deriveParkReportStep,
  familyCommitTail,
  feasibilityGate,
  feasibilityVerdictSchema,
  ownerItemSchema,
} from "@ctx-traits/agents";
import { gateAndDiffEvidence } from "@ctx-traits/toolkit";
import type { AgentHandle, ResourceHandle, SequenceHandle, SlotHandle } from "@ctx-traits/cdk";
import { port, signal, slot } from "@ctx-traits/cdk";

import { task, taskBrief } from "../shared/data.ts";

// Redistributed into source/shared/data.ts (0183 steps 1-2); re-exported
// here so the still-legacy variants keep compiling unchanged until they
// migrate too.
export {
  commitOutput,
  commitReport,
  draft,
  gateTimedOut,
  gateTimedOutAbortIf,
  leftovers,
  leftoversPort,
  ONE_TURN_DISCIPLINE,
  parkReport,
  parkReportPort,
  repoGatesPassed,
  reviewDiff,
  task,
  taskBrief,
  TASK_WRITE_SCOPE_RIDER,
  workSummary,
} from "../shared/data.ts";
// Redistributed into source/shared/agent.ts and source/shared/resource.ts
// (0183 step 2); re-exported here so the still-legacy variants keep
// compiling unchanged until they migrate too.
export { clerk, scribe, smart1Role, smart2Role, worker } from "../shared/agent.ts";
export { declareTaskBoard } from "../shared/resource.ts";
// Redistributed into source/shared/stage/frame.ts (0183 step 2); re-exported
// as taskExtractionStep so the still-legacy variants keep compiling
// unchanged until they migrate too.
export { extractTaskContract as taskExtractionStep } from "../shared/stage/frame.ts";

/**
 * 0047 mechanism 1's agent layer: a shared pre-build feasibility triage —
 * possible, blocked, oversized, ambiguous — audited BEFORE any planning or
 * build round spends anything. Complementary to the deterministic
 * blocked-status-marker dispatch preflight (which catches DECLARED
 * blockage for free): this catches UNDECLARED blockage (a referenced
 * artifact that is simply absent) no header ever recorded.
 */
export const feasibility = slot({
  id: "feasibility",
  schema: feasibilityVerdictSchema,
  description:
    "Pre-build feasibility triage verdict (0047): audited once before any planning or build round spends anything. feasible lets the run continue; any other verdict is the park evidence for why it stopped here.",
});
export const taskNotFeasible = signal({
  id: "task-not-feasible",
  description:
    "The pre-build feasibility gate found the task blocked, oversized, or ambiguous before any build work began; the run parks on the typed verdict instead of grinding toward a doomed park.",
});
export const feasibilityPort = port.output.of(feasibilityVerdictSchema, {
  id: "feasibility",
  title: "Feasibility Verdict",
  description:
    "Typed pre-build feasibility triage verdict (0047), for discoverability in the run's structured final outputs.",
  optional: true,
  value: feasibility,
  format: ["structured", "table"],
});
/**
 * Build the feasibility-gate step for one variant's own drafting/reviewer
 * agent — a single shared builder so every adopter gets byte-identical
 * doctrine and guard wiring; only the agent and the contract ref vary.
 */
export function feasibilityStep(
  agentHandle: AgentHandle,
  contract: ResourceHandle | SlotHandle = taskBrief,
): SequenceHandle {
  return feasibilityGate({
    id: "feasibility",
    agent: agentHandle,
    task,
    contract,
    output: feasibility,
    // Owner ruling 2026-07-31 (first live firing, run-bba65cb5): the
    // audit is WARNING-ONLY for now — the typed verdict is recorded for
    // the reviewer and the owner, but never stops the run.
    mode: "warn",
  });
}

export { deriveParkReportStep, familyCommitTail, gateAndDiffEvidence, ownerItemSchema };
