import type { AgentHandle } from "@ctx-traits/cdk";
import { input } from "@ctx-traits/cdk";

import { nextKey, raisedDate, receipts, taskInput } from "../data.ts";
import { TASK_FORMAT_DOCTRINE } from "../resource.ts";

/**
 * quick's single-frame path: distill the described work straight into board
 * task files — grounded by the frame's own repository reading, written
 * directly by the composing seat, no separate grounding/split/write hops.
 */
export function tasks(agent: AgentHandle) {
  return agent.prompt("Distill into task files", {
    id: "distill",
    input: input.prompt(
      `Turn the described work directly into TaskDocument TOML task files on the board — skip deriving separate grounding notes, but read the relevant parts of the repository with your tools so every task is grounded in this codebase's concrete files, modules, and validation gates.
            The work, as described: {task}
            ${TASK_FORMAT_DOCTRINE}
            Key the first file {next-key} and continue from there ("<key>.1", "<key>.2", ... under one charter when the work needs several tasks; a single bare-key task when it does not). Stamp raised = {raised-date} in every file. Derive each task's [[checks]] from its done criteria per the doctrine, confirming with your tools that any command you name actually exists in the repo before declaring it.
            Do not implement anything. Return the receipts: one entry per charter (or standalone task) naming every file written.`,
      { task: taskInput, "next-key": nextKey, "raised-date": raisedDate },
    ),
    output: receipts,
  });
}

/**
 * direct's single-frame path: format the description near-verbatim into
 * exactly one well-formed board task file — no refinement, no invention,
 * no repository reading.
 */
export function verbatim(agent: AgentHandle) {
  return agent.prompt("Format into a task file", {
    id: "format",
    input: input.prompt(
      `Format the described work into exactly one well-formed TaskDocument TOML task file on the board.
            The work, as described: {task}
            Keep the work's own wording near-verbatim. Do NOT refine, invent, or add requirements the description does not state; do not read the codebase or ground it in anything beyond the description itself.
            ${TASK_FORMAT_DOCTRINE}
            Key the file {next-key} and stamp raised = {raised-date}.
            Do not implement anything. Return the receipt: one entry naming the file written.`,
      { task: taskInput, "next-key": nextKey, "raised-date": raisedDate },
    ),
    output: receipts,
  });
}
