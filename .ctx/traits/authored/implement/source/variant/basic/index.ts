import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";
import * as shared from "#trait/shared/index.ts";

// The step-walk (0283). The reviewer owns a typed list of open steps; the
// runtime walks it with for-each, working each step in an inner loop until the
// worker returns done or blocked (not-done keeps the same step). When the pass
// exhausts, the reviewer validates the tree with its own tools and rewrites the
// list — dropping done steps, resolving blocked ones with a typed decision,
// appending new findings, emptying it when the task is done. No proof, no
// stage-claim, no brief projection: the worker sees the one bound step.
export default function () {
  cdk.defineVariant("Basic", {
    description: "Reviewed implementation: plan, then walk the steps — work each until done or blocked, review, repeat until the list is empty.",
    metadata: { tag: shared.metadata.tag },
  });

  shared.step.notify.begin("Open the owner notification");
  shared.step.notify.update("Notify: session base", "Capture the session base");
  shared.step.diff.baseline("Capture the session base");

  // The plan gate: the owner sees the plan before any work; it is redrafted
  // with corrections until accepted (plan-gate=off accepts on the first pass).
  shared.step.notify.update("Notify: plan drafted", "Draft the implementation plan");
  cdk.flow.loop("Plan approval", (loop) => {
    shared.step.plan.compose("Draft the implementation plan");
    shared.step.annotate.digestPlan("Digest the plan for annotation");
    shared.step.annotate.carryPlanSurface("Carry the plan surface");
    shared.step.annotate.planApprovalGate("Annotate the plan");
    shared.step.annotate.recordPlanRuling("Record the owner's plan correction");
    loop.untilAll([cdk.condition.equals(shared.data.planAnswer, "accepted")]);
  });

  // The baseline: the reviewer opens the step list before any work, so the
  // first dispatch has a real step and every step is validated against text
  // that preceded it.
  shared.step.notify.update("Notify: baseline review", "Open the step list");
  shared.step.review.baseline("Open the step list");

  // The review loop runs only if there is work: each pass walks the open steps,
  // then the reviewer validates and rewrites the list. An empty list — the
  // reviewer's confirmation that the task is done — is the only exit.
  cdk.flow.when("Work remains", cdk.condition.notEmpty(shared.data.steps), () => {
    cdk.flow.loop("Review loop", (round) => {

      // The step-walk: one for-each pass over the reviewer's open steps. Each
      // step is worked in an inner loop until the worker returns done or
      // blocked; not-done continues the SAME step. blocked advances like done,
      // so the pass always finishes to exhaustion before the reviewer runs.
      shared.step.notify.update("Notify: working the steps", "Working the steps");
      shared.data.steps.forEach("Work the steps", (step, walk) => {
        cdk.flow.loop("Work the step", (work) => {
          shared.step.work.implement("Implement the step", step);
          work.untilAll([
            cdk.condition.not(cdk.condition.fieldEquals(shared.data.report, "status", "not-done")),
          ]);
        });
      });

      // The reviewer is the only validator: it checks the tree with its own
      // tools and rewrites the open-step list for the next pass.
      shared.step.review.validate("Review the stage");
      shared.step.notify.reviewUpdate("Notify: review verdict");
      shared.step.notify.update("Notify: awaiting annotations", "awaiting owner annotations");
      shared.step.annotate.carrySurface("Carry the annotation surface");
      shared.step.annotate.verdictGate("Annotate the verdict");
      shared.step.annotate.recordRuling("Record the owner ruling");
      shared.step.notify.gateResult("Notify: owner ruling");

      // An annotated list is applied by the reviewer before the next pass, so
      // the worker only ever walks the applied list.
      cdk.flow.when(
        "Owner annotated",
        cdk.condition.not(cdk.condition.equals(shared.data.gateAnswer, "accepted")),
        () => {
          shared.step.review.apply("Apply the owner's annotations");
        },
      );

      // A step only the owner can settle parks the run (owner-ruling on); the
      // answer becomes the step's resolution and the loop continues.
      cdk.flow.when("Owner ruling", cdk.condition.signal(agents.needsOwnerSignal), () => {
        cdk.flow.when(
          "Owner reachable",
          cdk.condition.not(cdk.condition.equals(shared.data.ownerRuling, "off")),
          () => {
            const ruling = shared.step.summon.ask("Summon the owner", agents.needsOwnerSignal);
            shared.step.summon.record("Record the owner's ruling", agents.needsOwnerSignal, ruling.result);
          },
        );
      });

      // Commit progress whenever the tree is dirty (0283 v1 — per-stage-semantic
      // commit lands with the commit effect in 0283.1).
      shared.step.git.status("Check working tree status");
      cdk.flow.when("Commit progress", cdk.condition.notEmpty(shared.step.git.status.output), () => {
        shared.step.notify.update("Notify: committed", "Commit the progress");
        shared.step.git.commitMessage("Write the commit message");
        shared.step.git.commitStage("Stage all changes");
        shared.step.git.commitSubmit("Commit the progress");
        shared.step.git.taskBranch("Move the task branch");
      });

      // The exit reads the applied list: empty means the reviewer confirmed the
      // task done; anything open means another pass.
      round.untilAll([cdk.condition.not(cdk.condition.notEmpty(shared.data.steps))]);
    });
  });

  shared.step.notify.finish("Close the owner notification");
}
