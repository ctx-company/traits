// Template: review
//
// Scaffolded by `ctx traits create <name> --from review`. One file, no
// variants — the whole trait is below. Edit it freely; nothing here is
// special because it came from a template.
//
// The shape: say what to review, get a written review back. The target is
// your own words — a path, a branch, a range, a subsystem — and the reviewer
// finds and reads it with its own tools.
//
// To make the evidence deterministic instead, put a command step ahead of the
// prompt and hand its slot to the reviewer:
//
//   const changed = cdk.slot.text({ id: "changed-files", description: "..." });
//   cdk.step.command("Capture the changed files", {
//     input: cdk.input.command`git diff --name-status HEAD`,
//     output: changed,
//   });
import * as cdk from "@ctx-traits/cdk";

const reviewer = cdk.agent.reviewer("reviewer", {
  description: "Reviews what it is pointed at and reports what would block a merge.",
});

const target = cdk.port.input.text({
  id: "target",
  description: 'What to review, in your own words — e.g. "the auth module", "src/parser.rs", "main...HEAD".',
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
    description: "Reviews whatever it is pointed at and returns a written review, without touching the tree.",
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

  reviewer.prompt("Review the target", {
    input: cdk.input.prompt`
      Review ${target}.
      Find it and read it with your own tools first — never review something from its name alone.
      Report what would block a merge, then what is advisory, then what you checked and found sound.
      Say plainly when it is fine; do not invent findings to fill the report.
    `,
    output: verdict,
  });

  return { verdictReport };
}
