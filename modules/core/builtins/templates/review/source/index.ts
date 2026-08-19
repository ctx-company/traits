// Template: review
//
// Scaffolded by `ctx traits create <name> --from review`. One file, no
// variants — the whole trait is below. Edit it freely; nothing here is
// special because it came from a template.
//
// The shape: a git range in, a written review out. The two listing steps are
// commands rather than prompts, so what changed cannot be hallucinated. The
// reviewer gets an INDEX of the change — file names and commit subjects — and
// opens whatever it needs with its own tools, rather than being handed patch
// bodies it must trust.
import * as cdk from "@ctx-traits/cdk";

const reviewer = cdk.agent.reviewer("reviewer", {
  description: "Reviews the change in a git range and reports what would block a merge.",
});

// Handed to git as written. Git already knows what a range is and rejects one
// it cannot resolve, so there is no resolution logic here to drift out of step
// with it — a bad range fails the first command, carrying git's own message.
const range = cdk.port.input.text({
  id: "range",
  description: 'The change to review, as a git range — e.g. "main...HEAD" or "HEAD~3..HEAD".',
});

const changedFiles = cdk.slot.text({
  id: "changed-files",
  description: "Names and change kinds of every file in the range — an index of the change, never its content.",
});
const commitLog = cdk.slot.text({
  id: "commit-log",
  description: "One line per commit in the range.",
});
const verdict = cdk.slot.text({
  id: "verdict",
  description: "The reviewer's assessment: what blocks a merge, what is advisory, and what was checked.",
});

const verdictReport = cdk.port.output.text({
  id: "verdict-report",
  title: "Review",
  description: "The review.",
  value: verdict,
});

export default function () {
  cdk.defineTrait("Review", {
    version: "0.1.0",
    description: "Reviews a git range and returns a written review, without touching the tree.",
    metadata: { tag: ["template", "review"] },
  });

  // Vocabulary the runtime renders into every step of this trait. Selecting
  // an item is the instruction; the catalog says what each one means.
  cdk.useBehavior({
    tone: [cdk.behavior.tone.Plain, cdk.behavior.tone.Direct],
    method: [cdk.behavior.method.EvidenceFirst, cdk.behavior.method.Steelman, cdk.behavior.method.PushBack],
  });
  cdk.useIntent({
    focus: [cdk.intent.Correctness],
    avoid: [cdk.intent.RubberStampReview],
  });

  cdk.step.command("Capture the changed files", {
    input: cdk.input.command`git diff --name-status ${range}`,
    output: changedFiles,
  });

  cdk.step.command("Capture the commit log", {
    input: cdk.input.command`git log --oneline ${range}`,
    output: commitLog,
  });

  reviewer.prompt("Review the change", {
    input: cdk.input.prompt`
      Review the change in ${range}.
      The files it touches: ${changedFiles}
      The commits it contains: ${commitLog}
      Open the files themselves for anything you need to judge — the lists above are an index, not the change.
      Report what would block a merge, then what is advisory, then what you checked and found sound.
      Say plainly when the change is fine; do not invent findings to fill the report.
    `,
    output: verdict,
  });

  return { verdictReport };
}
