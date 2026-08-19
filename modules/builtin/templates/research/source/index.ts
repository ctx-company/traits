// Scope a topic into questions, research them into a report, then check the
// report answers them.
//
// The report path is a port with a default rather than something the model
// picks, so the caller knows where the deliverable lands before the run
// starts.
import * as cdk from "@ctx-traits/cdk";

const scout = cdk.agent.planner("scout", {
  description: "Narrows the topic into the questions the research has to answer.",
});
const researcher = cdk.agent.worker("researcher", {
  description: "Investigates the questions and writes the report.",
});
const reviewer = cdk.agent.reviewer("reviewer", {
  description: "Checks the report against the questions it was meant to answer.",
});

const topic = cdk.port.input.text({
  id: "topic",
  description: "The topic or question to research.",
});
const reportPath = cdk.port.input.text({
  id: "report-path",
  description: "Repo-relative path to write the report to.",
  optional: true,
  default: { value: "research/report.md" },
});

const questions = cdk.slot.text({
  id: "questions",
  description: "The core question, the sub-questions worth answering, and what is deliberately out of scope.",
});
const workSummary = cdk.slot.text({
  id: "work-summary",
  description: "What was found, what was written, and which questions remain open.",
});
const verdict = cdk.slot.text({
  id: "verdict",
  description: "Whether the report answers its questions, and what is missing if it does not.",
});

const findingsReport = cdk.port.output.text({
  id: "findings-report",
  title: "Findings",
  description: "What the research found.",
  value: workSummary,
  optional: true,
});

export default function () {
  cdk.defineTrait("Research", {
    version: "0.1.0",
    description: "Researches a topic into a written report at a known path, then reviews it for coverage.",
    metadata: { tag: ["template", "research"] },
  });

  // Rendered into every step. Selecting an item is the instruction.
  cdk.useBehavior({
    tone: [cdk.behavior.tone.Plain],
    method: [cdk.behavior.method.EvidenceFirst, cdk.behavior.method.ImplicationsFirst],
  });
  cdk.useIntent({ avoid: [cdk.intent.SpeculativeClaim] });

  scout.prompt("Scope the topic", {
    input: cdk.input.prompt`
      Narrow ${topic} into the questions the research must answer.
      Give the core question, the sub-questions worth answering separately, and what is deliberately out of scope.
    `,
    output: questions,
  });

  researcher.prompt("Research the topic", {
    input: cdk.input.prompt`
      Research ${topic}, answering the questions in ${questions}.
      Write the report to ${reportPath}, creating the directory if it does not exist.
      Cite what each finding rests on, and say when the evidence is thin rather than filling the gap.
      Return what you found, what you wrote, and which questions remain open.
    `,
    output: workSummary,
  });

  reviewer.prompt("Review the report", {
    input: cdk.input.prompt`
      Read the report at ${reportPath} and judge it against the questions in ${questions}.
      The researcher's account: ${workSummary}
      Report which questions are answered, which are not, and any claim the report does not support.
    `,
    output: verdict,
  });

  return { findingsReport };
}
