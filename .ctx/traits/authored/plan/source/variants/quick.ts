import { agent, input, intent, method, procedure, sequence, tone, variant, verbosity } from "@ctx-traits/cdk";
import { result, taskFiles, taskInput, writtenFiles } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

const smart1 = agent.reviewer("smart-1", {
  description: "Distills the task directly into house-format task files.",
  summary: "Distill role.",
});
const scribe = agent.planner("scribe", {
  description: "Writes the task files to .internal/tasks/.",
  summary: "Write role.",
});

const distillTasks = sequence.prompt("distill", {
  title: "Distill into task files (smart-1)",
  agent: smart1,
  text: input.prompt(
    `Turn the task below directly into one or more house-format task files for .internal/tasks/ — skip deriving separate grounding notes.
            Task as described: {task}
            Read the relevant parts of the repository with your tools to ground each task file in this codebase's concrete files, modules, and validation gates.
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
  name: "Plan (Quick)",
  summary:
    "Distill a described task directly into house-format task files and add them to .internal/tasks/ — no separate grounding pass, no review pass.",
  metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
  behavior: { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief },
  intent: {
    focus: [intent.focus.Specific, intent.focus.Correctness],
    avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep],
  },
  port: writtenFiles,
  procedure: procedure({
    description:
      "Distill the described task, grounded in the codebase, straight into house-format task files, then add them to .internal/tasks/.",
    sequence: [distillTasks, writeTasks],
  }),
});
