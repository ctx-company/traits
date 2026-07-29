import { port } from "@ctx-traits/cdk";
import { commitOutput } from "./slot.ts";

export const phase = port.input.text({
    id: "phase",
    description:
        'Phase or group to implement, exactly as named in .plans/EXECUTION_PLAN.md (e.g. "P196-fix" or "Group 43").',
});

export const commitReport = port.output.text({
    id: "commit-report",
    description:
        "Final commit evidence from the git commit command step. Absent when the clean-tree gate skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
    optional: true,
    value: commitOutput,
});
