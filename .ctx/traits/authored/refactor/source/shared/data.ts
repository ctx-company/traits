import * as agents from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { reviewVerdictSchema } from "./schema.ts";

export const target = cdk.port.input.text({
  id: "target",
  description:
    'Module, file path, or entity to refactor; append an optional emphasis after a dash (e.g. "modules/cli/src/app/run_view.rs", "crate::lockfile — typed errors only").',
});

export const survey = cdk.slot.text({
  id: "survey",
  description:
    "Numbered refactoring candidates for the target: cluster, why coupled, smell ids, test impact. No interface proposals.",
  hint: "The friction encountered while reading IS the signal. Deferred candidates stay listed here.",
});

export const refactorFrame = cdk.slot.text({
  id: "refactor-frame",
  description:
    "The selected candidate set plus problem framing: constraints any new boundary must satisfy, dependencies, an illustrative (non-binding) sketch.",
});

export const plan = cdk.slot.text({
  id: "plan",
  description:
    "The plan the implementation executes — the agreed design or the actionable checklist, complete enough to implement from.",
});

export const workSummary = cdk.slot.text({
  id: "work-summary",
  description: "Worker's report of the refactored state; revised in place after each refinement pass.",
  hint: "What changed (files), how behavior was preserved, gate results, open concerns.",
});

export const workDeviationReport = cdk.slot({
  id: "deviation-report",
  schema: cdk.schema.list(agents.deviationReportSchema),
  description:
    "Every proposed departure from the verbatim agreed design, including a rejected improvement the worker considered but did not make.",
});

export const commitMessage = cdk.slot.text({
  id: "commit-message",
  description: "Refactor commit message; injected directly into the git commit command step.",
});

export const stageOutput = cdk.slot.text({
  id: "stage-output",
  description: "Output evidence from the git staging command step.",
});

export const gitStatus = cdk.slot.text({
  id: "git-status",
  description:
    "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
  hint: "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and the scribe never runs.",
});

export const commitOutput = cdk.slot.text({
  id: "commit-output",
  description: "Output evidence from the git commit command step: committed hash and subject.",
});

export const commitReport = cdk.port.output.text({
  id: "commit-report",
  description:
    "Final commit evidence from the git commit command step. Absent when the clean-tree gate (P397) skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
  optional: true,
  value: commitOutput,
});

export const verdict1 = cdk.slot({
  id: "review-verdict-1",
  schema: reviewVerdictSchema,
  description: "smart-1's refinement verdict for the current refactored state.",
});

export const verdict2 = cdk.slot({
  id: "review-verdict-2",
  schema: reviewVerdictSchema,
  description: "smart-2's refinement verdict for the current refactored state.",
});

export const port = { target, commitReport };

export const slot = {
  survey,
  refactorFrame,
  plan,
  workSummary,
  workDeviationReport,
  commitMessage,
  gitStatus,
  stageOutput,
  commitOutput,
  verdict1,
  verdict2,
};
