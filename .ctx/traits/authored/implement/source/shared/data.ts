import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

export const draft = cdk.slot.text({
  id: "draft",
  description: "smart-1's implementation draft for the task — the plan the worker implements.",
  hint: "Scope (restated from the task file), files to touch, approach, reuse opportunities, validation plan, risks. A plan, not an implementation.",
});

export const workSummary = cdk.slot.text({
  id: "work-summary",
  description: "Worker's account of what this round changed.",
  hint: "What changed, how it was validated, and open concerns.",
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

export const ownerDecisions = cdk.slot.texts({
  id: "owner-decisions",
  description:
    "Every owner ruling made during this run, one formatted entry per summons — the run's durable decision record, carried into later review rounds and the commit message.",
  hint: "One entry per ruling: the question the run could not settle, and the owner's answer, in one or two sentences.",
});

export const gitStatus = cdk.slot.text({
  id: "git-status",
  description: "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
});

export const notifyId = cdk.slot.text({
  id: "notify-id",
  description:
    "The owner-notification activity id, exactly as `begin` printed it — every later notification step addresses this.",
});

export const notifyDigest = cdk.slot({
  id: "notify-digest",
  schema: cdk.schema.object(
    "notify-digest",
    {
      badge: cdk.schema.field(cdk.schema.text(), {
        description:
          'Short status badge for the activity, 64 characters at most, always "review: <verdict status>".',
      }),
      summary: cdk.schema.field(cdk.schema.text(), {
        description:
          "Multi-line prose block for the owner's phone, 4096 characters at most: the verdict in two to four plain sentences — how many blocker steps are open, what closed this round, and where the run is heading. Empty string when the verdict is approved, which clears the previous summary.",
      }),
      journal: cdk.schema.field(cdk.schema.text(), {
        description:
          'One compact journal line: every still-open blocker step\'s text trimmed to 200 characters, joined with " | ". The literal "approved" when none are open.',
      }),
    },
    { description: "The reviewer verdict digested for the owner notification thread." },
  ),
  description: "Scribe's per-round digest of the review verdict, fed directly into the notification commands.",
});

export const notifyBadge = cdk.slot.text({
  id: "notify-badge",
  description: "The digest's badge field, carried as text for command argv.",
});

export const notifySummary = cdk.slot.text({
  id: "notify-summary",
  description: "The digest's summary field, carried as text for command argv.",
});

export const notifyJournal = cdk.slot.text({
  id: "notify-journal",
  description: "The digest's journal field, carried as text for command argv.",
});

export const notifyLog = cdk.slot.text({
  id: "notify-log",
  description: "The notifier's own JSON receipt for the most recent notification command.",
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
  ownerDecisions,
  gitStatus,
  stageOutput,
  notifyId,
  notifyDigest,
  notifyBadge,
  notifySummary,
  notifyJournal,
  notifyLog,
};
