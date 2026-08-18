import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

export const draft = cdk.slot.text({
  id: "draft",
  description: "smart-1's implementation draft for the task — the plan the worker implements.",
  hint: "Scope (restated from the task file), files to touch, approach, reuse opportunities, validation plan, risks. A plan, not an implementation.",
});

export const workSummary = cdk.slot.text({
  id: "work-summary",
  description: "Worker's cumulative account of the implemented state, extended each round — the worker's cross-round memory.",
  hint: "Cumulative across rounds: your own summary from the previous round arrives as input — extend it, never restart it. Per round: what changed (files), how it was validated, open concerns, and per-blocker progress against the attached verdict.",
});

export const changedFiles = cdk.slot.text({
  id: "changed-files",
  description:
    "Names and change-kinds of every file changed since the session base — an index of where the work landed, never the work itself. The reviewer opens whatever it needs with its own tools.",
  hint: "git diff --name-status against slot:diff-base; covers committed and uncommitted changes, omits untracked files.",
});

export const diffBase = cdk.slot.text({
  id: "diff-base",
  description: "Commit the session started from — the fixed base every changed-files capture diffs against.",
  hint: "git rev-parse HEAD captured before any work; worker commits during the loop never move it.",
});

export const verdict1 = cdk.slot({
  id: "review-verdict-1",
  schema: agents.reviewVerdictSchema,
  description: "First reviewer's verdict for the current work state.",
});

export const verdict2 = cdk.slot({
  id: "review-verdict-2",
  schema: agents.reviewVerdictSchema,
  description: "Second, independent reviewer's verdict for the current work state.",
});

export const commitMessage = cdk.slot.text({
  id: "commit-message",
  description: "Commit message for the completed task; injected directly into the git commit command step.",
});

export const gitStatus = cdk.slot.text({
  id: "git-status",
  description: "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
});

export const stageOutput = cdk.slot.text({
  id: "stage-output",
  description: "Output evidence from the git add command step.",
});

export const task = cdk.port.input.text({
  id: "task",
  description: 'Task to implement, named by its file in .internal/tasks/ — the key ("0044" or "0044.2"), the full name, or the filename.',
});

export const commitReport = cdk.port.output.text({
  id: "commit-report",
  description:
    "Final commit evidence from the git commit command step. Absent when the clean-tree gate (P397) skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
  optional: true,
});

export const port = { task, commitReport };

export const slot = {
  draft,
  workSummary,
  changedFiles,
  diffBase,
  verdict1,
  verdict2,
  commitMessage,
  gitStatus,
  stageOutput,
};
