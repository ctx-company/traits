import { agent, Method, port, procedure, prompt, sequence, slot, Tone, trait, Verbosity } from "@ctx-traits/cdk";

const codeDiff = port.input.text({ id: "code-diff" });
const changeSummary = slot.text("change-summary");
const riskNotes = slot.text("risk-notes");
const reviewComment = slot.text("review-comment");
const reviewOutput = port.output.text({ id: "review-comment", value: reviewComment });
const worker = agent("worker", { description: "Writes the draft review." });
const reviewer = agent("reviewer", { description: "Checks the draft review." });

export const prRiskTriage = trait({
  id: "pr-risk-triage",
  name: "PR Risk Triage",
  description: "Turns a code diff into one concise PR review comment.",
  behavior: {
    tone: [Tone.direct, Tone.technical],
    method: Method.evidenceFirst,
    verbosity: Verbosity.brief,
  },
  agents: [worker, reviewer],
  procedure: procedure({
    description: "Review a PR diff through summary, risk notes, and final comment.",
    input: codeDiff,
    output: reviewOutput,
    sequence: [
      sequence.prompt({
        id: "summarize-code-diff",
        agent: worker,
        input: codeDiff,
        prompt: prompt.text`Summarize what changed in ${codeDiff}.`,
        output: changeSummary,
      }),
      sequence.prompt({
        id: "find-risks-in-code-diff",
        agent: reviewer,
        input: [codeDiff, changeSummary],
        prompt: prompt.text`Using ${codeDiff} and ${changeSummary}, list concrete risks.`,
        output: riskNotes,
      }),
      sequence.prompt({
        id: "write-pr-comment",
        agent: worker,
        input: [changeSummary, riskNotes],
        prompt: prompt.text`Write one concise PR review comment using ${changeSummary} and ${riskNotes}.`,
        output: reviewComment,
      }),
    ],
  }),
});
