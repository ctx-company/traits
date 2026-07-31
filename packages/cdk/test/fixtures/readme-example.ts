import {
  agent,
  method,
  port,
  procedure,
  prompt,
  sequence,
  slot,
  toDraftJson,
  tone,
  trait,
  verbosity,
} from "@ctx-traits/cdk";

const codeDiff = port.input.text({ id: "code-diff" });
const changeSummary = slot.text("change-summary");
const riskNotes = slot.text("risk-notes");
const reviewComment = slot.text("review-comment");
const reviewOutput = port.output.text({ id: "review-comment", value: reviewComment });
const worker = agent("worker", { description: "Writes the draft review." });
const reviewer = agent("reviewer", { description: "Checks the draft review." });

const prRiskTriage = trait({
  id: "pr-risk-triage",
  name: "PR Risk Triage",
  description: "Turns a code diff into one concise PR review comment.",
  behavior: {
    tone: [tone.direct, tone.technical],
    method: method.evidenceFirst,
    verbosity: verbosity.brief,
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

console.log(JSON.stringify(toDraftJson(prRiskTriage), null, 2));
