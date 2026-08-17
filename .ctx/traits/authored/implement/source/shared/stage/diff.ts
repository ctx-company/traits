import { input, step } from "@ctx-traits/cdk";

import { changedFiles } from "../data.ts";

/**
 * The one deterministic evidence step the reviewed variants keep (owner
 * ruling 2026-08-17): a short changed-file inventory — names and change
 * kinds, never patch bodies — captured by command, excluding runtime state.
 */
export function changedFilesStep(): void {
  step.command("Capture the changed files", {
    id: "changed-files",
    input: input.command`git diff --name-status HEAD -- . ":(exclude).agents/runs"`,
    output: changedFiles,
  });
}
