import type { AgentHandle } from "@ctx-traits/cdk";
import { condition, flow, input } from "@ctx-traits/cdk";

import { doneCriteria, receipts, revisionLog, slicePlan, taskInput, verdict, workItems } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

/**
 * The complex variant's work -> review -> improve loop: an independent
 * reviewer issues a typed verdict against the written board, the composing
 * seat applies every blocker, and the loop runs until approved; an
 * exhausted loop aborts with its stop reason.
 */
export function loop(reviewer: AgentHandle, composer: AgentHandle) {
  flow.loop("Reviewing", (reviewLoop) => {
    reviewLoop.maxIterations(4, { onExhausted: "abort" });

    reviewer.prompt("Review the board", {
      input: input.prompt(
        `Independently review the task files written for {task} against the plan {slice-plan} and the receipts {receipts} — read every written file yourself with your tools; do not trust the receipts alone.
            The format contract every file must satisfy: ${TASK_FORMAT_DOCTRINE}
            A BLOCKER is: a file that fails the format contract (not parseable TaskDocument TOML, wrong key/name shape, missing status/raised); a planned task with no file, or a file outside the plan; a work item in {work-items} no written task covers; an unmet or unrepresented entry of {done-criteria}; a child task materially larger than 10-15 minutes of focused work, or with a vague, unverifiable Done when; a dependency error (depends-on naming a later or missing key, a child without its parent relation); or grounding a task needs that its own body omits or contradicts. Everything else is advisory.
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
          `Apply the review verdict {verdict} to the written task files, in one bounded pass — fix every blocker it names, change nothing it does not name, and keep every key stable (fixing a dependency edge or splitting an oversized child appends new ".N" keys, never renumbers existing ones).
            Format contract: ${TASK_FORMAT_DOCTRINE}
            Do not implement anything. Return one line per file you changed or added, naming its path.`,
          { verdict },
        ),
        output: revisionLog,
      });
    });

    flow.until(condition.equals(verdict.status, "approved"));
  });
}
