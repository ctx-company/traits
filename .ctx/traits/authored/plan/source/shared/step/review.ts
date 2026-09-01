import type { AgentHandle } from "@ctx-traits/cdk";
import { condition, flow, input, signal } from "@ctx-traits/cdk";

import { doneCriteria, receipts, revisionLog, slicePlan, taskInput, verdict, workItems } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

/**
 * One review round: the reviewer issues a typed verdict against the written
 * board, and a revise verdict gets one bounded composer pass. Shared by the
 * complex variant's grind-until-approved loop and default's simple bounded
 * pass — one blocker vocabulary, so the two variants never drift on what a
 * defect is.
 */
function round(reviewer: AgentHandle, composer: AgentHandle) {
  reviewer.prompt("Review the board", {
    input: input.prompt(
      `Independently review the task files written for {task} against the plan {slice-plan} and the receipts {receipts} — read every written file yourself with your tools; do not trust the receipts alone.
            The format contract every file must satisfy: ${TASK_FORMAT_DOCTRINE}
            A BLOCKER is: a file that fails the format contract (not parseable TaskDocument TOML, wrong key/name shape, missing status/raised); a planned task with no file, or a file outside the plan; a work item in {work-items} no written task covers; an unmet or unrepresented entry of {done-criteria}; a slice keyed to a stack layer instead of a work item (more slices than work items is the tell); a define-first, architecture-only, or audit-only task or slice; a task with a vague, unverifiable Done when; two tasks with no reason to be separate; a task whose body is only running gates, tests, or checks (gates belong in the [[checks]] of the task that changed the code); a charter with no genuine umbrella — children sharing no stop condition beyond all being done; a dependency error (depends-on naming a later or missing key, a child without its parent relation — symbolic order: KEY1 before KEY2, .9 before .10); grounding a task needs that its own body omits or contradicts; a mechanically verifiable Done when with no corresponding [[checks]] entry; or a declared check whose command does not exist in the repository or does not actually verify its Done when. Everything else is advisory.
            Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
            Set status to revise while any blocker remains, approved when none do.`,
      {
        task: taskInput,
        "slice-plan": slicePlan,
        receipts,
        "work-items": workItems,
        "done-criteria": doneCriteria,
      },
    ),
    output: verdict,
    include: [verdict.optional()],
  });

  flow.when("Apply the review", condition.equals(verdict.status, "revise"), () => {
    composer.prompt("Revise the board", {
      input: input.prompt(
        `Apply the review verdict {verdict} to the written task files, in one bounded pass — fix every blocker it names, change nothing it does not name, and keep every symbolic key stable: splitting an oversized task appends new ".N" keys; merging under-sized tasks folds the content into the surviving file and deletes the other (its key simply goes unused); never renumber a surviving key.
            Format contract: ${TASK_FORMAT_DOCTRINE}
            Do not implement anything. Return one line per file you changed or added, naming its path.`,
        { verdict },
      ),
      output: revisionLog,
    });
  });
}

/**
 * The complex variant's work -> review -> improve loop: grinds until the
 * independent reviewer approves; an exhausted loop aborts with its stop
 * reason.
 */
export function loop(reviewer: AgentHandle, composer: AgentHandle) {
  flow.loop("Reviewing", (reviewLoop) => {
    reviewLoop.maxIterations(4, { onExhausted: signal.Abort });
    round(reviewer, composer);
    reviewLoop.until(condition.equals(verdict.status, "approved"));
  });
}

/**
 * Default's simple review: the same blocker vocabulary, bounded at two
 * rounds, and NEVER fatal — exhaustion continues to the renumber/commit
 * tail with whatever the board holds, because default's contract is "one
 * honest pass", not "grind until approved".
 */
export function simple(reviewer: AgentHandle, composer: AgentHandle) {
  flow.loop("Simple review", (reviewLoop) => {
    reviewLoop.maxIterations(2, { onExhausted: signal.Continue });
    round(reviewer, composer);
    reviewLoop.until(condition.equals(verdict.status, "approved"));
  });
}
