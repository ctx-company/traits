// Plan the work, do it, then judge the result.
//
// The command step between the work and the review is deliberate: the
// reviewer sees what actually changed, not only what the worker reports.
import * as cdk from "@ctx-traits/cdk";

const planner = cdk.agent.planner("planner", {
  description: "Turns the requested work into a short plan the worker can follow.",
});
const worker = cdk.agent.worker("worker", {
  description: "Carries out the plan and reports what changed.",
});
const reviewer = cdk.agent.reviewer("reviewer", {
  description: "Judges the finished work against the plan and the request.",
});

const goal = cdk.port.input.text({
  id: "goal",
  description: "The work to carry out, in your own words.",
});

const plan = cdk.slot.text({
  id: "plan",
  description: "Scope, the files to touch, the approach, and how the work will be checked.",
});
const workSummary = cdk.slot.text({
  id: "work-summary",
  description: "What the worker changed, how it was validated, and anything left open.",
});
const changedFiles = cdk.slot.text({
  id: "changed-files",
  description: "Names and change kinds of every file touched — an index of the work, never its content.",
});
const verdict = cdk.slot.text({
  id: "verdict",
  description: "The reviewer's assessment: what still blocks the work, and what was verified.",
});

const workReport = cdk.port.output.text({
  id: "work-report",
  title: "Work Summary",
  description: "What was changed and how it was validated.",
  value: workSummary,
  optional: true,
});
const reviewReport = cdk.port.output.text({
  id: "review-report",
  title: "Review",
  description: "The reviewer's assessment of the finished work.",
  value: verdict,
  optional: true,
});

export default function () {
  cdk.defineTrait("Implement", {
    version: "0.1.0",
    description: "Plans a stated piece of work, implements it, and reviews the result against the plan.",
    metadata: { tag: ["template", "implementation"] },
  });

  // Rendered into every step. Selecting an item is the instruction.
  cdk.useBehavior({
    tone: [cdk.behavior.tone.Plain],
    method: [cdk.behavior.method.RestateToConfirm],
  });
  cdk.useIntent({
    require: [cdk.intent.VerifiableGoal, cdk.intent.MatchSurroundingStyle],
    avoid: [cdk.intent.ScopeCreep],
  });

  planner.prompt("Draft the plan", {
    input: cdk.input.prompt`
      Plan the work for ${goal}.
      Name the scope, the files you expect to touch, the approach, and how the result will be checked.
      Keep it short enough to act on; this is a plan, not the implementation.
    `,
    output: plan,
  });

  worker.prompt("Do the work", {
    input: cdk.input.prompt`
      Carry out ${goal}, following the plan in ${plan}.
      Run the checks the plan names before reporting.
      Return what you changed, how you validated it, and anything still open.
    `,
    output: workSummary,
  });

  cdk.step.command("Capture the changed files", {
    input: cdk.input.command`git diff --name-status HEAD`,
    output: changedFiles,
  });

  reviewer.prompt("Review the work", {
    input: cdk.input.prompt`
      Judge the finished work for ${goal} against the plan in ${plan}.
      The worker's account: ${workSummary}
      The files that changed: ${changedFiles}
      Read the changed files yourself; the summary is a claim, not evidence.
      Report what still blocks the work, then what you verified and found sound.
    `,
    output: verdict,
  });

  return { workReport, reviewReport };
}
