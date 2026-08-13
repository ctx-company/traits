import { port, schema, slot } from "@ctx-traits/cdk";

import {
  brief as briefSchema,
  ownerDecision as ownerDecisionSchema,
  planDecision as planDecisionSchema,
  reviewVerdict,
} from "./schema.ts";

export const task = port.input.text({
  id: "task",
  description:
    'Task to implement, named by its file in .internal/tasks/ — the number ("0044"), the full name ("0044-live-view-pane-polish"), or the filename.',
});

export const preflightOutput = slot.text({
  id: "preflight-output",
  description: "Output evidence that plannotator is on PATH, from the preflight command step.",
});

export const draft = slot.text({
  id: "draft",
  description: "smart-1's implementation draft for the task, before the owner has annotated it.",
  hint: "Scope (restated from the task file), files to touch, approach, reuse/abstraction opportunities, validation plan, risks. A plan, not an implementation.",
});
export const planDecision = slot({
  id: "plan-decision",
  schema: planDecisionSchema,
  description: "plannotator's plan-mode verdict on the draft: approve, or deny with annotation text.",
});
export const plan = slot.text({
  id: "plan",
  description:
    "The refined plan: the draft with every plan-mode annotation visibly addressed. The contract the build loop implements.",
  hint: "Every deny/annotation from plan-decision must be provably reflected here, not just acknowledged.",
});

export const workSummary = slot.text({
  id: "work-summary",
  description: "Worker's report of the implemented state; revised in place after each refinement pass.",
  hint: "What changed (files), how it was validated, and open concerns. Revised after every apply-fixes pass.",
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
export const parkReportPort = port.output.of(schema.list(reviewVerdict), {
  id: "park-report",
  title: "Park Report",
  description:
    "Typed park record for an unapproved run (P414): the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the refinement loop exhausted without approval — the run parks and no commit is created. A run that exhausts on owner denials alone (reviewer approved every round) parks with this empty; slot:owner-decision is the evidence in that case.",
  optional: true,
  value: parkReport,
  format: ["structured", "table"],
});

export const ownerBriefing = slot.text({
  id: "owner-briefing",
  description:
    "The round briefing for the owner, written after a gate-green reviewer approval from the work summary, the verdict, and the working tree; fed to the plannotator gate step.",
});
export const ownerDecision = slot({
  id: "owner-decision",
  schema: ownerDecisionSchema,
  description:
    "plannotator's gate verdict on the round briefing, only once a review has approved with the repository gate green and the owner has been summoned. Absent (Missing) every round the owner is not summoned, and on round 1.",
});

export const brief = slot({
  id: "brief",
  schema: briefSchema,
  description:
    "The final record: slug, brief markdown, and commit message, before the file-writing and commit command steps run.",
});
export const briefSlug = slot.text({
  id: "brief-slug",
  description: "brief.slug, lifted flat for the file-writing and display command steps.",
});
export const briefMarkdown = slot.text({
  id: "brief-markdown",
  description: "brief.markdown, lifted flat for the file-writing and display command steps.",
});
export const commitMessage = slot.text({
  id: "commit-message",
  description: "brief.commit-message, lifted flat for the git commit command step's argv.",
});
export const briefWriteOutput = slot.text({
  id: "brief-write-output",
  description: "Output evidence from the command step that writes the brief file.",
});
export const briefDisplayOutput = slot.text({
  id: "brief-display-output",
  description:
    "Output evidence from the command step that displays the brief to the owner (no --gate; no decision attached).",
});

export const shippingStatus = slot.text({
  id: "shipping-status",
  description:
    "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
  hint: "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and no brief is written.",
});
export const stageOutput = slot.text({
  id: "stage-output",
  description: "Output evidence from the git staging command step.",
});
export const unstageOutput = slot.text({
  id: "unstage-output",
  description:
    "Output evidence from the runtime-state unstage step (git reset -- .agents/runs); empty when nothing was staged there.",
});
export const commitOutput = slot.text({
  id: "commit-output",
  description: "Output evidence from the git commit command step: committed hash and subject.",
});
export const commitReport = port.output.text({
  id: "commit-report",
  description:
    "Final commit evidence from the git commit command step. Absent when the clean-tree gate skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
  optional: true,
  value: commitOutput,
});

export const briefReport = port.output.text({
  id: "brief-report",
  title: "Brief",
  description:
    "The tracked brief written to .internal/briefs/<slug>.md. Absent when the clean-tree gate skipped the commit tail.",
  optional: true,
  value: briefMarkdown,
  format: ["text"],
});
