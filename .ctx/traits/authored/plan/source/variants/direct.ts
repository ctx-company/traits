import { agent, input, intent, method, procedure, sequence, tone, variant, verbosity } from "@ctx-traits/cdk";
import { result, taskFiles, taskInput, writtenFiles } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

const smart1 = agent.reviewer("smart-1", {
  description: "Formats the task text into one well-formed task file, near-verbatim.",
  summary: "Format role.",
});
const scribe = agent.planner("scribe", {
  description: "Writes the task file to .internal/tasks/.",
  summary: "Write role.",
});

const formatTask = sequence.prompt("format", {
  title: "Format into a task file (smart-1)",
  agent: smart1,
  text: input.prompt(
    `Format the task below into exactly one well-formed task file for .internal/tasks/.
            Task as described: {task}
            Keep the task's own wording near-verbatim. Do NOT refine, invent, or add requirements the task does not state; do not read the codebase or ground it in anything beyond the task text itself.
            ${TASK_FORMAT_DOCTRINE}
            Do not implement anything.`,
    { task: taskInput },
  ),
  output: taskFiles,
  input: [taskInput],
});

const writeTasks = sequence.prompt("write", {
  title: "Write the task files (scribe)",
  agent: scribe,
  text: input.prompt`
                    Write each task below to .internal/tasks/ at the repo root, one file per task.
                    Tasks: ${taskFiles}
                    Create the directory if missing. List the directory first and continue its numbering: name each new file NNNN-<kebab-slug>.md, where NNNN is the next free zero-padded number after the highest one already present (count archived/ too), and stamp that number into the task's own heading in place of its placeholder. Fill the Raised date from today's date.
                    Never overwrite or renumber an existing file. Keep each task's content faithful — do not add or drop material.
                    Return one line per file naming the path you wrote.`,
  output: result,
  input: [taskFiles],
});

export default variant({
  name: "Plan (Direct)",
  summary:
    "Format a described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/ — no refinement, no invention, the fastest path to just implement.",
  metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
  behavior: { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief },
  intent: {
    focus: [intent.focus.Specific, intent.focus.Correctness],
    avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep],
  },
  port: writtenFiles,
  procedure: procedure({
    description:
      "Format the described task into one well-formed house-format task file with wording kept near-verbatim, then add it to .internal/tasks/.",
    sequence: [formatTask, writeTasks],
  }),
});
