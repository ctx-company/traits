// Template: research
//
// Scaffolded by `ctx traits create <name> --from research`. One file, no
// variants — the whole trait is below. Edit it freely; nothing here is
// special because it came from a template.
//
// The shape: a topic in, a written report at a predictable path out. The
// path is derived by command steps rather than chosen by the model, so the
// caller knows where the deliverable landed before the run starts and can
// assert on it afterwards.
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
const outputDir = cdk.port.input.text({
  id: "output-dir",
  description: "Repo-relative directory the report is written under.",
  optional: true,
  default: { value: "research" },
});

// Derived by command steps, never by the model: a predictable path is what
// makes the deliverable assertable.
const slug = cdk.slot.text({
  id: "topic-slug",
  description: "Kebab-case slug derived from the topic, used as the report's directory name.",
});
const reportPath = cdk.slot.text({
  id: "report-path",
  description: "Repo-relative path the report is written to.",
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

const reportPathReport = cdk.port.output.text({
  id: "report-path-report",
  title: "Report Path",
  description: "Where the report was written.",
  value: reportPath,
  optional: true,
});
const findingsReport = cdk.port.output.text({
  id: "findings-report",
  title: "Findings",
  description: "What the research found.",
  value: workSummary,
  optional: true,
});

export default cdk.trait("research", {
  version: "0.1.0",
  name: "Research",
  description: "Researches a topic into a written report at a predictable path, then reviews it for coverage.",
  metadata: { tag: ["template", "research"] },
  // Ports are declared here, not on the procedure: an object-shell trait has
  // no variant return for them to be derived from.
  port: [topic, outputDir, reportPathReport, findingsReport],
  procedure: cdk.procedure.from(
    {
      description: "Scope the topic, research it into a report, and check the report answers it.",
    },
    () => {
      cdk.step.command("Derive the topic slug", {
        input: cdk.input.command`sh -c "printf %s \\"\\$1\\" | tr \\"A-Z\\" \\"a-z\\" | tr -cs \\"a-z0-9\\" \\"-\\" | sed \\"s/^-*//;s/-*\\$//\\"" _ ${topic}`,
        output: slug,
      });

      cdk.step.command("Derive the report path", {
        input: cdk.input.command`sh -c "printf %s/%s/report.md \\"\\$1\\" \\"\\$2\\"" _ ${outputDir} ${slug}`,
        output: reportPath,
      });

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
    },
  ),
});
