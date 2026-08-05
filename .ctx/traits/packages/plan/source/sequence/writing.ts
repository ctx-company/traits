import type { AgentHandle, SlotHandle } from "@ctx-traits/cdk";
import { input, sequence } from "@ctx-traits/cdk";

export function writeTasksStep(agent: AgentHandle, taskFiles: SlotHandle, result: SlotHandle) {
    return sequence.prompt("write", {
        title: "Write the task files (scribe)", agent,
        text: input.prompt`
                    Write each task below to .internal/tasks/ at the repo root, one file per task.
                    Tasks: ${taskFiles}
                    Create the directory if missing. List the directory first and continue its numbering: name each new file NNNN-<kebab-slug>.md, where NNNN is the next free zero-padded number after the highest one already present (count archived/ too), and stamp that number into the task's own heading in place of its placeholder. Fill the Raised date from today's date.
                    Never overwrite or renumber an existing file. Keep each task's content faithful — do not add or drop material.
                    Return one line per file naming the path you wrote.`,
        output: result, input: [taskFiles],
    });
}
