import type { AgentHandle } from "@ctx-traits/cdk";
import { condition, flow, input, operation, step } from "@ctx-traits/cdk";

import { boardCheck, boardSnapshot, doneCriteria, grounding, raisedDate, receipts, slicePlan, workItems } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

/**
 * One frame per slice: the composing seat writes its own slice's task files
 * directly to the board — the deliverable goes straight to disk, there is no
 * transcription hop through another seat — and appends a typed receipt. The
 * receipt list is seeded empty first: production inside a for-each body is
 * only "possible" to the validator, so the guaranteed producer every later
 * reader needs must sit before the loop.
 */
export function tasks(agent: AgentHandle, maxSlices: number) {
  // The pre-write board guard: the prompts above forbid early writes, but a
  // prompt is advisory — this check re-lists the board and compares against
  // the run-start snapshot, and the authored error terminal fails the run
  // (typed, nonzero) rather than letting out-of-plan files ride the merge.
  // Detection at the one boundary that matters, not prevention: the harness
  // confinement layer deliberately allows writes inside the run worktree.
  step.check("Board untouched before write", {
    id: "board-untouched",
    argv: [
      "sh",
      "-c",
      'ls .internal/tasks .internal/tasks/archived 2>/dev/null | sort | cksum | grep -qxF -- "$1"',
      "_",
      boardSnapshot,
    ],
    output: boardCheck,
  });
  flow.errorWhen("Premature board write", condition.isFalse(boardCheck.ok), {
    message:
      "an early step wrote to .internal/tasks before the write phase; ingest/refine/split must leave the board untouched, so the run fails rather than adopting out-of-plan files",
  });

  step.project("Seed the receipts", {
    id: "seed-receipts",
    projections: [{ source: operation.literal([]), destination: receipts }],
  });

  slicePlan.forEach("Write each slice", (slice, loop) => {
    loop.limit(maxSlices);
    agent.prompt("Write the slice", {
      input: input.prompt(
        `Write exactly one slice of the plan to the board — {slice} — as TaskDocument TOML files in .internal/tasks/ (create the directory if missing). Do not write any other slice's files; other slices run in their own frames.
            ${TASK_FORMAT_DOCTRINE}
            Write the charter task first (the slice's key), then every child task from the slice's own plan, keeping each child's key, title, and depends-on exactly as planned. Stamp raised = {raised-date} in every file. Ground each task's content/scope/validation in the work items {work-items}, the done criteria {done-criteria}, and the grounding notes {grounding} — fold in what that task needs so it stands alone. Derive each child's [[checks]] from its done criteria per the doctrine, sourcing commands from the grounding notes' validation gates.
            Do not implement anything. Return the receipt: the slice's key and the repo-relative path of every file you wrote.`,
        {
          slice,
          "raised-date": raisedDate,
          "work-items": workItems,
          "done-criteria": doneCriteria,
          grounding,
        },
      ),
      output: receipts.with(operation.Append),
    });
  });
}
