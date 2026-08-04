import { prompt, sequence } from "@ctx-traits/cdk";

import { agent } from "../agent.ts";
import { port, slot } from "../data.ts";

const summaryBrief = sequence.prompt("summary-brief", {
    title: "Write the brief (scribe)",
    agent: agent.scribe,
    text: prompt.text`
                    The build loop for the plan ${slot.plan} has ended in a double approval (reviewer and owner), and the work is being recorded. Current work summary: ${slot.workSummary}.
                    Write a short kebab-case slug for this assignment, and a brief in markdown: what was built, why, and how it was validated. Draw its substance from the work summary rather than re-deriving the change from the working tree.
                    Return the slug and the markdown as the typed brief output. Do not write any file yourself; a later runtime step saves it.`,
    output: slot.brief,
});

const liftBrief = sequence.project("lift-brief", {
    projections: [
        { source: slot.brief, field: "slug", destination: slot.briefSlug },
        { source: slot.brief, field: "markdown", destination: slot.briefMarkdown },
    ],
});

const writeBrief = sequence.command("write-brief", {
    title: "Save the brief to .internal/briefs/",
    argv: ["sh", "-c", 'mkdir -p .internal/briefs && printf %s "$1" > ".internal/briefs/$2.md"', "sh", "{slot:brief-markdown}", "{slot:brief-slug}"],
    output: slot.briefWriteOutput,
});

// No `--gate`: the brief is a record, not a decision, so nothing here
// attaches an `ownerDecision` and nothing blocks on it. Whether this window
// blocks the run until the owner closes it, or returns immediately (fire-
// and-forget display), was not possible to confirm from this environment —
// `plannotator annotate` without `--gate` could not be driven to completion
// headlessly (see plan.ts's note on the same constraint for plan mode). The
// per-step idle bound below (0084) is what makes either behavior safe: an
// immediate return never touches it, and a long wait no longer risks
// runtime.toml's `command-idle-seconds = 600` idle-killing the step (and the
// run) before `summary-commit` — confirm the actual behavior before
// recording and adjust this comment.
const displayBrief = sequence.command("display-brief", {
    title: "Show the brief to the owner (plannotator, display only)",
    argv: ["plannotator", "annotate", ".internal/briefs/{slot:brief-slug}.md"],
    include: [slot.briefWriteOutput],
    // Declared per-step (0084) so it wins over runtime.toml's
    // `command-idle-seconds = 600`.
    idleTimeoutMs: 4 * 60 * 60 * 1000,
    output: slot.briefDisplayOutput,
});

const summaryCommitMessage = sequence.prompt("summary-commit", {
    title: "Write the commit message (scribe)",
    agent: agent.scribe,
    text: prompt.text`
                    The build loop for the plan ${slot.plan} has ended in a double approval, and the work is being committed. The final verdict is ${slot.verdict}, and the tracked brief is ${slot.brief}.
                    Write a concise commit message: a short subject line naming the change, then a one-paragraph summary of what was implemented and how it was validated. Draw its substance from the brief's stated intents rather than re-deriving the change from the working tree.
                    Save exactly that message to the file .git/CTX_COMMITMSG at the repo root (create or overwrite it), and return the same message as your output.
                    Do not run any git commands; staging and committing happen in later runtime steps.`,
    output: slot.commitMessage,
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
    argv: ["git", "commit", "-F", ".git/CTX_COMMITMSG"],
    input: [slot.commitMessage],
    output: slot.commitOutput,
});

export const brief = [summaryBrief, liftBrief, writeBrief, displayBrief];
export const commit = [summaryCommitMessage, gitStage, gitCommit];
export const outputPorts = [port.briefReport, port.commitReport];
