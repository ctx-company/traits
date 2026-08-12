import { feasibilityVerdictSchema } from "@ctx-traits/agents";
import { port, schema, slot } from "@ctx-traits/cdk";

import { reviewVerdict } from "./schema.ts";

export const task = port.input.text({
  id: "task",
  description:
    'Task to implement, named by its file in .internal/tasks/ — the number ("0044"), the full name ("0044-live-view-pane-polish"), or the filename.',
});

export const draft = slot.text({
  id: "draft",
  description: "smart-1's implementation draft for the task — the contract the worker implements.",
  hint: "Scope (restated from the task file), files to touch, approach, reuse/abstraction opportunities, validation plan, risks. A plan, not an implementation.",
});
export const workSummary = slot.text({
  id: "work-summary",
  description: "Worker's report of the implemented state; revised in place after each refinement pass.",
  hint: "What changed (files), how it was validated, and open concerns. Revised after every apply-fixes pass.",
});
export const commitOutput = slot.text({
  id: "commit-output",
  description: "Output evidence from the git commit command step: committed hash and subject.",
});

export const verdict = slot({
  id: "review-verdict",
  schema: reviewVerdict,
  description: "smart-1's refinement verdict for the current work state.",
});

export const parkReport = slot({
  id: "park-report",
  schema: schema.list(reviewVerdict),
  description:
    "This round's typed park record (P414): empty when the round's verdict is approved, exactly one entry — the round's own verdict, copied unchanged — when it is revise. Written each round by a deterministic project step (deriveParkReportStep), never model-authored, so it can never disagree with the verdict it comes from.",
});

export const feasibility = slot({
  id: "feasibility",
  schema: feasibilityVerdictSchema,
  description:
    "Pre-build feasibility triage verdict (0047): audited once before the draft step spends anything. feasible lets the run continue; any other verdict is the park evidence for why it stopped here.",
});

export const commitReport = port.output.text({
  id: "commit-report",
  description:
    "Final commit evidence from the git commit command step. Absent when the clean-tree gate skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
  optional: true,
  value: commitOutput,
});

export const parkReportPort = port.output.of(schema.list(reviewVerdict), {
  id: "park-report",
  title: "Park Report",
  description:
    "Typed park record for an unapproved run (P414): the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the refinement loop exhausted without approval — the run parks and no commit is created.",
  optional: true,
  value: parkReport,
  format: ["structured", "table"],
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
