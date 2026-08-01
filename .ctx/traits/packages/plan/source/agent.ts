import { agent } from "@ctx-traits/cdk";

export const defaultSmart1 = agent.reviewer("smart-1", {
    description: "Strong model: refines the task, grounds it in the codebase, and splits it into task files.",
    summary: "Refinement + grounding role.",
});
export const defaultScribe = agent.planner("scribe", {
    description: "Writes the task files to .internal/tasks/.",
    summary: "Bootstrap-writer role.",
});
export const directSmart1 = agent.reviewer("smart-1", {
    description: "Formats the task text into one well-formed task file, near-verbatim.",
    summary: "Format role.",
});
export const directScribe = agent.planner("scribe", {
    description: "Writes the task file to .internal/tasks/.",
    summary: "Write role.",
});
export const quickSmart1 = agent.reviewer("smart-1", {
    description: "Distills the task directly into house-format task files.",
    summary: "Distill role.",
});
export const quickScribe = agent.planner("scribe", {
    description: "Writes the task files to .internal/tasks/.",
    summary: "Write role.",
});
export const complexSmart1 = agent.reviewer("smart-1", {
    description: "Strong model: refines the task, grounds it in the codebase, splits it into task files, and critiques both artifacts.",
    summary: "Refinement + grounding + critique role.",
});
export const complexScribe = agent.planner("scribe", {
    description: "Applies the critique to the task files, then writes them to .internal/tasks/.",
    summary: "Revise-and-write role.",
});
