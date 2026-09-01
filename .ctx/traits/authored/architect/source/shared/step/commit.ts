// The commit tail, same reason as plan's: without it a --worktree run
// strands its rewritten task file as an uncommitted change on a branch
// with no commits, and merge — which adopts the run branch's COMMITS —
// finds nothing. Deterministic, no model seat: architect's deliverable is
// one file, so the message body is that file's path.
import * as cdk from "@ctx-traits/cdk";

import { commitLog, targetFile } from "../data.ts";

/**
 * Board-scoped commit: the split authority means the deliverable may be
 * the parent plus its children, so the pathspec is the board directory. `git commit -- <path>`
 * commits the working-tree state of that one path — no staging step, and
 * unrelated staged work in a non-worktree run is never swept in. A target
 * that ends the run byte-identical (re-architecting an already-ready
 * task) is reported, not failed: the run's outcome is the receipt, and
 * "nothing changed" is a legitimate receipt. No command substitution
 * (hidden-content audit); the path rides as "$1".
 */
export function commitStep(title: string): void {
  cdk.step.command(title, {
    id: "commit-task",
    input: cdk.input.command`sh -c 'git add .internal/tasks && if git diff --quiet --cached -- .internal/tasks; then echo "no changes to commit for $1"; else git commit -m "architect: execution plan" -m "$1" -- .internal/tasks; fi' _ ${targetFile}`,
    output: commitLog,
  });
}
