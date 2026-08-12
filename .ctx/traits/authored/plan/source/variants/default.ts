import { agent, input, intent, method, procedure, sequence, slot, tone, variant, verbosity } from "@ctx-traits/cdk";
import { result, taskFiles, taskInput, writtenFiles } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

const smart1 = agent.reviewer("smart-1", {
  description: "Strong model: refines the task, grounds it in the codebase, and splits it into task files.",
  summary: "Refinement + grounding role.",
});
const scribe = agent.planner("scribe", {
  description: "Writes the task files to .internal/tasks/.",
  summary: "Bootstrap-writer role.",
});

const grounding = slot.text({
  id: "grounding",
  description:
    "Codebase grounding for the described work: the concrete files and modules it touches, the repo's validation gates (exact commands), and the invariants, rules, and constraints the task files must honor.",
  hint: "Grounded in the codebase: what the project is, its build/test/lint gates (exact invocations), architectural invariants, dependency rules, and constraints an implementer and reviewer must honor. Context the tasks carry, not the step-by-step plan.",
});

const refineTask = sequence.prompt("refine-task", {
  title: "Ground & refine the task in the codebase",
  agent: smart1,
  text: input.prompt`
            Task is described as: ${taskInput}, ground it in the codebase & refine it.
            Read the relevant parts of the repository with your tools:
                - identify the concrete files, modules, and patterns the task touches
                - the build/test/lint commands (the repo's validation gates, with exact invocations)
                - architectural invariants and requirements
                - dependencies or rules that need to be respected
            Produce the grounding notes:
                - short project/context overview,
                - rules and invariants an implementer and reviewer must honor,
                - validation gates (exact commands)
                - constraints specific to this task.
            This is the context the task files carry, not the step-by-step plan.
            Do not implement anything.`,
  input: taskInput,
  output: grounding,
});

const splitTasks = sequence.prompt("split", {
  title: "Split the work into task files",
  agent: smart1,
  text: input.prompt(
    `Task format doctrine: ${TASK_FORMAT_DOCTRINE}
            The work, as described: {task}
            Grounding notes: {grounding}
            Break the refined work into small, ordered task files.
            Fold the grounding each task needs — its files, gates, invariants, constraints — into that task's own body and Watch section, so every file stands alone.
            Do not implement anything.`,
    { task: taskInput, grounding },
  ),
  input: [taskInput, grounding],
  output: taskFiles,
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
  name: "Plan",
  summary:
    "Turn a described task into codebase-grounded, house-format task files in .internal/tasks/ — the board the implement family runs from in any repo.",
  metadata: { tag: ["task", "plan", "bootstrap", "planning"] },
  behavior: { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief },
  intent: {
    focus: [intent.focus.Specific, intent.focus.Correctness],
    avoid: [intent.avoid.SpeculativeClaim, intent.avoid.ScopeCreep],
  },
  port: writtenFiles,
  procedure: procedure({
    description:
      "Refine the described task against the codebase, split it into small task files, and write them to .internal/tasks/ so the implement family can run.",
    sequence: [refineTask, splitTasks, writeTasks],
  }),
});
