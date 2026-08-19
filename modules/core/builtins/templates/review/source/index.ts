// Template: review
//
// Scaffolded by `ctx traits create <name> --from review`. One file, no
// variants — the whole trait is below. Edit it freely; nothing here is
// special because it came from a template.
//
// The shape: a git range in, a written review out. Deterministic work
// (resolving the range, listing what changed) happens in command steps, so it
// cannot be hallucinated. The reviewer gets an INDEX of the change — file
// names and commit subjects — and opens whatever it needs with its own tools,
// rather than being handed patch bodies it must trust.
import * as cdk from "@ctx-traits/cdk";

const reviewer = cdk.agent.reviewer("reviewer", {
  description: "Reviews the change in a git range and reports what would block a merge.",
});

const range = cdk.port.input.text({
  id: "range",
  description: 'A git ref or range to review, e.g. "main...feature-x" or a branch name.',
});

// `git rev-parse` fails loudly on a bad ref, so a resolved range is real by
// the time the reviewer sees it.
const resolved = cdk.slot.text({
  id: "resolved-range",
  description: "The range after git resolved it; empty when the ref does not exist.",
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
  description: "The review. Absent when the range did not resolve or held no changes.",
  value: verdict,
  optional: true,
});

export default function () {
  cdk.defineTrait("Review", {
    version: "0.1.0",
    description: "Reviews a git ref or range and returns a written review, without touching the tree.",
    metadata: { tag: ["template", "review"] },
  });

  cdk.step.command("Resolve the range", {
    input: cdk.input.command`git rev-parse --abbrev-ref ${range}`,
    output: resolved,
  });

  // An empty slot parks the run by closing over nothing further — cheaper than
  // an error path, and it leaves the output port simply absent.
  cdk.flow.when("Range Resolved", cdk.condition.notEmpty(resolved), () => {
    cdk.step.command("Capture the changed files", {
      input: cdk.input.command`git diff --name-status ${range}`,
      output: changedFiles,
    });

    cdk.step.command("Capture the commit log", {
      input: cdk.input.command`git log --oneline ${range}`,
      output: commitLog,
    });

    reviewer.prompt("Review the range", {
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
  });

  return { verdictReport };
}
