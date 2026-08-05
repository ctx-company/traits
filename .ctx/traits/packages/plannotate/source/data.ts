import { port as cdkPort, slot as cdkSlot } from "@ctx-traits/cdk";

import * as schema from "./schema.ts";

const assignment = cdkPort.input.text({
    id: "assignment",
    description: "What to build, in the caller's own words. The scope contract plan-draft drafts from.",
});

const preflightOutput = cdkSlot.text({ id: "preflight-output", description: "Output evidence that plannotator is on PATH, from the preflight command step." });
const draft = cdkSlot.text({ id: "draft", description: "The implementation draft for the assignment, before the owner has annotated it.", hint: "Scope, files to touch, approach, validation plan, risks. A plan, not an implementation." });
const planDecision = cdkSlot({ id: "plan-decision", schema: schema.planDecision, description: "plannotator's plan-mode verdict on the draft: approve, or deny with annotation text." });
const plan = cdkSlot.text({ id: "plan", description: "The refined plan: the draft with every plan-mode annotation visibly addressed. The contract the build loop implements.", hint: "Every deny/annotation from plan-decision must be provably reflected here, not just acknowledged." });
const workSummary = cdkSlot.text({ id: "work-summary", description: "Worker's report of the implemented state; replaced in full each round.", hint: "What changed (files), how it was validated, and open concerns." });
const verdict = cdkSlot({ id: "review-verdict", schema: schema.reviewVerdict, description: "The reviewer's verdict for the current round." });
const ownerBriefing = cdkSlot.text({ id: "owner-briefing", description: "The round briefing for the owner, written after a reviewer approval from the work summary, the verdict, and the working tree; fed to the plannotator gate step." });
const ownerDecision = cdkSlot({ id: "owner-decision", schema: schema.ownerDecision, description: "plannotator's gate verdict on the round briefing, only once a review has approved and the owner has been summoned. Absent (Missing) every round the owner is not summoned, and on round 1." });
const brief = cdkSlot({ id: "brief", schema: schema.brief, description: "The final record: slug, brief markdown, and commit message, before the file-writing and commit command steps run." });
const briefSlug = cdkSlot.text({ id: "brief-slug", description: "brief.slug, lifted flat for the file-writing and display command steps." });
const briefMarkdown = cdkSlot.text({ id: "brief-markdown", description: "brief.markdown, lifted flat for the file-writing and display command steps." });
const briefWriteOutput = cdkSlot.text({ id: "brief-write-output", description: "Output evidence from the command step that writes the brief file." });
const briefDisplayOutput = cdkSlot.text({ id: "brief-display-output", description: "Output evidence from the command step that displays the brief to the owner (no --gate; no decision attached)." });
const commitMessage = cdkSlot.text({ id: "commit-message", description: "brief.commit-message, lifted flat for the git commit command step's argv." });
const stageOutput = cdkSlot.text({ id: "stage-output", description: "Output evidence from the git staging command step." });
const commitOutput = cdkSlot.text({ id: "commit-output", description: "Output evidence from the git commit command step: committed hash and subject." });

const briefReport = cdkPort.output.text({ id: "brief-report", title: "Brief", description: "The tracked brief written to .internal/briefs/<slug>.md.", format: ["text"], value: briefMarkdown });
const commitReport = cdkPort.output.text({ id: "commit-report", title: "Commit Report", description: "Final commit evidence from the git commit command step: the committed hash and subject.", format: ["text"], value: commitOutput });

export const port = { assignment, briefReport, commitReport };
export const slot = {
    preflightOutput,
    draft,
    planDecision,
    plan,
    workSummary,
    verdict,
    ownerBriefing,
    ownerDecision,
    brief,
    briefSlug,
    briefMarkdown,
    briefWriteOutput,
    briefDisplayOutput,
    commitMessage,
    stageOutput,
    commitOutput,
};
