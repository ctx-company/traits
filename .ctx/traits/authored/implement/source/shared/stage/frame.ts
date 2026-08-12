import type { AgentHandle, ResourceHandle } from "@ctx-traits/cdk";
import { input } from "@ctx-traits/cdk";

import { task, taskBrief } from "../data.ts";

export function extractTaskContract(agentHandle: AgentHandle, taskBoard: ResourceHandle): void {
  agentHandle.prompt("Copy the task contract", {
    input: input.prompt`
            Copy the task file for ${task} EXACTLY as written.
            Open the task-board directory named in ${taskBoard} with your tools. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — list the directory and match it (a bare number matches its NNNN- prefix). Files under archived/ are not live tasks; match one only when the request names it explicitly.
            Return the file's entire contents, byte-for-byte — no paraphrasing, no summaries, no commentary, no added headers. Every later step works from this copy instead of the board, so anything you drop is lost.`,
    output: taskBrief,
  });
}
