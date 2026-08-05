import { input, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";

const summaryBrief = sequence.prompt("summary-brief", {
    title: "Write the brief and commit message (smart)",
    agent: agent.smart2,
    text: input.prompt`
                    The build loop for the plan ${slot.plan} has ended in a double approval (reviewer and owner), and the work is being recorded and committed. Work summary: ${slot.workSummary}. Final reviewer verdict: ${slot.verdict}.
                    Inspect the working tree with your tools (git diff, git status) so the record reflects the actual change, not just the summary's claims.
                    Write: a short kebab-case slug for this assignment; the brief in markdown — what was built, why, how it was validated, and what the review found; and the commit message — a short subject line naming the change, then a one-paragraph summary of what was implemented and how it was validated.
                    Return the slug, the markdown, and the commit message as the typed output. Do not write any file and do not run git commands; later runtime steps save and commit.`,
    output: slot.brief,
});

const liftBrief = sequence.project("lift-brief", {
    projections: [
        { source: slot.brief, field: "slug", destination: slot.briefSlug },
        { source: slot.brief, field: "markdown", destination: slot.briefMarkdown },
        { source: slot.brief, field: "commit-message", destination: slot.commitMessage },
    ],
});

const writeBrief = sequence.command("write-brief", {
    title: "Save the brief to .internal/briefs/",
    argv: ["sh", "-c", 'mkdir -p .internal/briefs && printf %s "$1" > ".internal/briefs/$2.md"', "sh", "{slot:brief-markdown}", "{slot:brief-slug}"],
    output: slot.briefWriteOutput,
});

// No `--gate`: the brief is a record, not a decision.
const displayBrief = sequence.command("display-brief", {
    title: "Show the brief to the owner (plannotator, display only)",
    argv: ["plannotator", "annotate", ".internal/briefs/{slot:brief-slug}.md"],
    include: [slot.briefWriteOutput],
    // The owner may read at length; the per-step bound must beat
    // runtime.toml's `command-idle-seconds`.
    idleTimeoutMs: 4 * 60 * 60 * 1000,
    output: slot.briefDisplayOutput,
});

const gitStage = sequence.command("git-stage", {
    title: "Stage all changes except runtime state",
    // The brief under .internal/briefs/ is inside this stage — it ships in
    // the same commit as the work it describes.
    argv: ["git", "add", "-A", "--", ":(exclude).agents/runs"],
    output: slot.stageOutput,
});

const gitCommit = sequence.command("git-commit", {
    title: "Commit the work",
    argv: ["git", "commit", "-m", "{slot:commit-message}"],
    input: [slot.commitMessage],
    output: slot.commitOutput,
});

export const brief = [summaryBrief, liftBrief, writeBrief, displayBrief];
export const commit = [gitStage, gitCommit];
export const outputPorts = [port.briefReport, port.commitReport];
