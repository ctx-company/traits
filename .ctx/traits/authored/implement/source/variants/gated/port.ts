import { port, schema } from "@ctx-traits/cdk";
import { reviewVerdict } from "./schema.ts";
import { briefMarkdown, commitOutput, parkReport } from "./slot.ts";

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

export const parkReportPort = port.output.of(schema.list(reviewVerdict), {
    id: "park-report",
    title: "Park Report",
    description:
        "Typed park record for an unapproved run (P414): the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the refinement loop exhausted without approval — the run parks and no commit is created. A run that exhausts on owner denials alone (reviewer approved every round) parks with this empty; slot:owner-decision is the evidence in that case.",
    optional: true,
    value: parkReport,
    format: ["structured", "table"],
});

export const briefReport = port.output.text({
    id: "brief-report",
    title: "Brief",
    description: "The tracked brief written to .internal/briefs/<slug>.md. Absent when the clean-tree gate skipped the commit tail.",
    optional: true,
    value: briefMarkdown,
    format: ["text"],
});
