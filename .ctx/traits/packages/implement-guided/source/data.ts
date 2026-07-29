import { port as cdkPort, slot as cdkSlot } from "@ctx-traits/cdk";

import * as schema from "./schema.ts";

const annotations = cdkSlot({ id: "annotations", schema: schema.annotationSet, description: "The assignment, as typed annotations returned by the annotation tool. The scope contract every later step works from — this trait never asks a model to restate it." });
const draft = cdkSlot.text({ id: "draft", description: "The implementation draft for the annotated assignment — the contract the build loop implements.", hint: "Scope, files to touch, approach, validation plan, risks. A plan, not an implementation." });
const workSummary = cdkSlot.text({ id: "work-summary", description: "Worker's report of the implemented state; replaced in full each round.", hint: "What changed (files), how it was validated, and open concerns." });
const verdict = cdkSlot({ id: "review-verdict", schema: schema.reviewVerdict, description: "The reviewer's verdict for the current work state." });
const reviewedDiff = cdkSlot.text({ id: "reviewed-diff", description: "The approved change as unified-diff hunks, captured after the loop exits so it always describes the state the reviewer actually signed.", hint: "git diff HEAD output with @@ hunk headers; tracked files only — a brand-new untracked file does not appear until it is staged." });
const hunkNotes = cdkSlot.text({ id: "hunk-notes", description: "A markdown walkthrough of the approved change: one section per hunk, each with the hunk in a fenced code block followed by its intent and explanation.", hint: "Markdown source, written to be read by a human reviewing the change — not a data structure." });
const commitMessage = cdkSlot.text({ id: "commit-message", description: "Scribe's commit message for the approved work; also saved to .git/CTX_COMMITMSG for the commit command step." });
const stageOutput = cdkSlot.text({ id: "stage-output", description: "Output evidence from the git staging command step." });
const commitOutput = cdkSlot.text({ id: "commit-output", description: "Output evidence from the git commit command step: committed hash and subject." });
const pushOutput = cdkSlot.text({ id: "push-output", description: "Output evidence from the gated push via ctx-gate run -- git push, including the gate's own verdict." });

const changeWalkthrough = cdkPort.output.text({ id: "change-walkthrough", title: "Change Walkthrough", description: "Markdown walkthrough of the approved change, one fenced hunk per section with its intent and explanation.", format: ["text"], value: hunkNotes });
const commitReport = cdkPort.output.text({ id: "commit-report", title: "Commit Report", description: "Final commit evidence from the git commit command step: the committed hash and subject.", format: ["text"], value: commitOutput });
const pushReport = cdkPort.output.text({ id: "push-report", title: "Push Report", description: "Evidence from the gated push — the gate's own verdict plus the push result under it.", format: ["text"], value: pushOutput });

export const slot = { annotations, draft, workSummary, verdict, reviewedDiff, hunkNotes, commitMessage, stageOutput, commitOutput, pushOutput };
export const port = { changeWalkthrough, commitReport, pushReport };
