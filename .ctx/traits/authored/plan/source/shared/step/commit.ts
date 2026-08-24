// The commit tail every variant ends with: without it a --worktree run
// strands its task files as untracked changes on a branch with no commits,
// and merge — which adopts the run branch's COMMITS — finds nothing (the
// exact failure of run-e78531f8, 2026-08-24). Deterministic, no model seat:
// plan's deliverable is the board files, so the message is mechanical — the
// key map IS the account of what this run added.
import { step } from "@ctx-traits/cdk";

import { commitLog, keyMap } from "../data.ts";

/**
 * Stage and commit the written task files in one command step. Pathspec'd
 * `git commit -- .internal/tasks` so a run in the user's own checkout can
 * never sweep unrelated staged work into the plan's commit; the key map
 * rides as the message body via "$1", so no string assembly and no command
 * substitution (hidden-content audit). `git add` covers the renumbered
 * files (renumber runs strictly before this).
 */
export function boardCommitStep(): void {
  step.command("Commit the board tasks", {
    id: "commit-tasks",
    argv: [
      "sh",
      "-c",
      'git add .internal/tasks && git commit -m "plan: board tasks" -m "$1" -- .internal/tasks',
      "_",
      keyMap,
    ],
    output: commitLog,
  });
}
