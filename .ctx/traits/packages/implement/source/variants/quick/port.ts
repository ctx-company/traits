import { port } from "@ctx-traits/cdk";
import { commitOutput } from "./slot.ts";

export const task = port.input.text({
    id: "task",
    description:
        'Task to implement, named by its file in .internal/tasks/ — the number ("0044"), the full name ("0044-live-view-pane-polish"), or the filename.',
});

export const commitReport = port.output.text({
    id: "commit-report",
    description:
        "Final commit evidence from the git commit command step. Absent when the clean-tree gate skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
    optional: true,
    value: commitOutput,
});
