import * as cdk from "@ctx-traits/cdk";

import { task, taskBrief, TASK_WRITE_SCOPE_RIDER } from "#trait/shared/data.ts";

import { verdict1, verdict2 } from "../data.ts";

export const scribeText = cdk.input.prompt(
  `The build loop for {task} has ended in dual approval and the work is being committed. The final reviewer verdicts are {verdict1} and {verdict2} — the sole authority on what was implemented (P414: this step is unreachable while either status is still revise; that case parks instead, with a typed park report, and never reaches here).
            Write a concise commit message for the completed work based only on the task contract {taskBrief} and those verdicts: a short subject line naming the task, then a one-paragraph summary of what the verdicts certify implemented — never work belonging to other tasks.
            Return exactly that message as your output; the runtime injects it into the git commit step.
            Do not run any git commands and do not write any files for the message; staging and committing happen in later runtime steps.
            Do NOT record task status anywhere. ${TASK_WRITE_SCOPE_RIDER}
            Compose the tail of the commit message from the verdicts, including only the sections the verdicts actually support — never write a "none" placeholder for a section with no entries. If a verdict carries owner-items, add "Owner items:" quoting each entry (item, reason class, close-out command or decision). If a verdict carries remaining (cross-task seam citations only), add "Cross-task seams:" quoting it verbatim. The final adjudicated leftovers are attached as context: when non-empty, add a "Leftovers:" section reproducing every entry's typed fields (what, reason, needs, evidence, done-when) verbatim — never paraphrased, never invented; omit the section entirely when the list is empty. A non-empty leftovers list never affects approval or is written as if it were a blocker.`,
  { task, verdict1, verdict2, taskBrief },
);
