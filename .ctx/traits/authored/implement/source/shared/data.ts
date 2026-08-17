// Cross-variant ports and slots. Deliberately small (owner ruling
// 2026-08-17): no repository-gate machinery, no park report, no leftovers
// adjudication — the review loop IS the mechanism, and an exhausted loop
// aborts with its stop reason. The board task file is the contract; runs
// read it and never write it.
import { reviewVerdictSchema } from "@ctx-traits/agents";
import { port, slot } from "@ctx-traits/cdk";

// P450 S3: the board is the owner's status surface — a run never writes it.
export const TASK_WRITE_SCOPE_RIDER = `Never edit files under .internal/tasks/ — the task board is owner-maintained; a run records its progress in the work summary and the commit message, never in a task file.`;

export const task = port.input.text({
  id: "task",
  description:
    'Task to implement, named by its file in .internal/tasks/ — the key ("0044" or "0044.2"), the full name, or the filename.',
});

export const workSummary = slot.text({
  id: "work-summary",
  description:
    "Worker's cumulative account of the implemented state, extended each round — the worker's cross-round memory.",
  hint: "Cumulative across rounds: your own summary from the previous round arrives as input — extend it, never restart it. Per round: what changed (files), how it was validated, open concerns, and per-blocker progress against the attached verdict.",
});
export const changedFiles = slot.text({
  id: "changed-files",
  description:
    "Names and change-kinds of every file changed this round — an index of where the work landed, never the work itself. The reviewer opens whatever it needs with its own tools.",
  hint: "git diff --name-status output, excluding runtime state; it omits untracked files.",
});

export const verdict1 = slot({
  id: "review-verdict-1",
  schema: reviewVerdictSchema,
  description: "First reviewer's verdict for the current work state.",
});
export const verdict2 = slot({
  id: "review-verdict-2",
  schema: reviewVerdictSchema,
  description: "Second, independent reviewer's verdict for the current work state.",
});

export const commitOutput = slot.text({
  id: "commit-output",
  description: "Output evidence from the git commit command step: committed hash and subject.",
});
export const commitReport = port.output.text({
  id: "commit-report",
  description:
    "Final commit evidence from the git commit command step. Absent when the clean-tree gate (P397) skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
  optional: true,
  value: commitOutput,
});
