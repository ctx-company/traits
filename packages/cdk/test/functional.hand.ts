// Functional-vs-object byte-identity property test (0106). Same shape as
// `normalization.hand.ts` (0104): two independent authorings of the same
// procedure — once through `procedure.from` + the functional registrars,
// once directly with `sequence.*` — must emit byte-identical draft JSON.
// This is 0106's "Done when" verbatim. Kept out of the gate's vitest glob
// per the standing validation ruling (2026-07-24, no behavior-freezing
// tests while surfaces churn): the gate stays drift/embed + builds + cargo
// test. This asserts the functional layer's own 1:1 lowering claim, not a
// frozen behavior snapshot.
//
// Run by hand: `pnpm exec vitest run --config vitest.hand.config.ts`

import {
  agent,
  condition,
  defineTrait,
  effect,
  evaluateTraitFunction,
  flow,
  input,
  intent,
  port,
  procedure,
  schema,
  sequence,
  signal,
  slot,
  step,
  toDraftJson,
  tone,
  trait,
  useBehavior,
  useIntent,
} from "@ctx-traits/cdk";
import { describe, expect, it } from "vitest";

function expectByteIdentical(a: unknown, b: unknown): void {
  expect(JSON.stringify(a)).toBe(JSON.stringify(b));
}

describe("functional layer emits byte-identical canonical to the object layer (0106)", () => {
  it("a straight-line prompt + command procedure", () => {
    function viaFunctional() {
      const worker = agent.worker("worker");
      const draft = slot.text("draft");
      const proc = procedure.from({ description: "d" }, () => {
        worker.prompt("Implement", { input: input.prompt`Do it.`, output: draft });
        step.command("Status", { input: input.command`git status --porcelain` });
      });
      return toDraftJson(trait("straight-line", { name: "Straight Line", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const worker = agent.worker("worker");
      const draft = slot.text("draft");
      const implement = sequence.prompt({
        id: "implement",
        title: "Implement",
        agent: worker,
        input: input.prompt`Do it.`,
        output: draft,
      });
      const status = sequence.command("status", { title: "Status", input: input.command`git status --porcelain` });
      const proc = procedure({ description: "d", sequence: [implement, status] });
      return toDraftJson(trait("straight-line", { name: "Straight Line", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("a bounded loop with flow.until (0102's worked example shape)", () => {
    function viaFunctional() {
      const worker = agent.worker("worker");
      const smart = agent("smart", { description: "Reviews." });
      const draft = slot.text("draft");
      const workSummary = slot.text("work-summary");
      const verdict = slot({
        id: "verdict",
        schema: schema.object("loop-verdict", { status: schema.enum(["approved", "needs-work"]) }),
      });
      const proc = procedure.from({ description: "d" }, () => {
        smart.prompt("Plan Draft", { input: input.prompt`Draft.`, output: draft });
        flow.loop("Refinement Loop", (loop) => {
          loop.maxIterations(4);
          worker.prompt("Implement", { input: input.prompt`Apply ${draft}.`, output: workSummary });
          smart.prompt("Review", { input: input.prompt`Review ${workSummary}.`, output: verdict });
          flow.until(condition.fieldEquals(verdict, "status", "approved"));
        });
      });
      return toDraftJson(trait("bounded-loop", { name: "Bounded Loop", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const worker = agent.worker("worker");
      const smart = agent("smart", { description: "Reviews." });
      const draft = slot.text("draft");
      const workSummary = slot.text("work-summary");
      const verdict = slot({
        id: "verdict",
        schema: schema.object("loop-verdict", { status: schema.enum(["approved", "needs-work"]) }),
      });
      const planDraft = sequence.prompt({
        id: "plan-draft",
        title: "Plan Draft",
        agent: smart,
        input: input.prompt`Draft.`,
        output: draft,
      });
      const implement = sequence.prompt({
        id: "implement",
        title: "Implement",
        agent: worker,
        input: input.prompt`Apply ${draft}.`,
        output: workSummary,
      });
      const review = sequence.prompt({
        id: "review",
        title: "Review",
        agent: smart,
        input: input.prompt`Review ${workSummary}.`,
        output: verdict,
      });
      const loopItem = sequence.loop("refinement-loop", {
        title: "Refinement Loop",
        body: [implement, review],
        maxIterations: 4,
        until: condition.fieldEquals(verdict, "status", "approved"),
      });
      const proc = procedure({ description: "d", sequence: [planDraft, loopItem] });
      return toDraftJson(trait("bounded-loop", { name: "Bounded Loop", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("a mid-loop flow.until checkpoint wraps trailing steps with when: not(cond)", () => {
    function viaFunctional() {
      const done = slot.boolean("done");
      const proc = procedure.from({ description: "d" }, () => {
        flow.loop("Checkpoint Loop", (loop) => {
          loop.maxIterations(3);
          step.command("First", { cmd: "echo first" });
          flow.until(condition.equals(done, true));
          step.command("Second", { cmd: "echo second" });
        });
      });
      return toDraftJson(trait("checkpoint", { name: "Checkpoint", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const done = slot.boolean("done");
      const first = sequence.command("first", { title: "First", cmd: "echo first" });
      const second = sequence.command("second", {
        title: "Second",
        cmd: "echo second",
        when: condition.not(condition.equals(done, true)),
      });
      const loopItem = sequence.loop("checkpoint-loop", {
        title: "Checkpoint Loop",
        body: [first, second],
        maxIterations: 3,
        until: condition.equals(done, true),
      });
      const proc = procedure({ description: "d", sequence: [loopItem] });
      return toDraftJson(trait("checkpoint", { name: "Checkpoint", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("flow.when(..., flow.Abort) inside a loop lowers to abortIf", () => {
    function viaFunctional() {
      const cap = slot.number("cap");
      const proc = procedure.from({ description: "d" }, () => {
        flow.loop("Bounded", (loop) => {
          loop.maxIterations(3);
          step.command("Work", { cmd: "echo work" });
          flow.when("Give up", condition.gte(cap, 3), flow.Abort);
        });
      });
      return toDraftJson(trait("abort-arm", { name: "Abort Arm", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const cap = slot.number("cap");
      const work = sequence.command("work", { title: "Work", cmd: "echo work" });
      const loopItem = sequence.loop("bounded", {
        title: "Bounded",
        body: [work],
        maxIterations: 3,
        abortIf: condition.gte(cap, 3),
      });
      const proc = procedure({ description: "d", sequence: [loopItem] });
      return toDraftJson(trait("abort-arm", { name: "Abort Arm", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("flow.when(title, cond, cb) block form lowers to a guarded nested sequence", () => {
    function viaFunctional() {
      const statusOut = slot.text("commit-status");
      const receipt = slot.text("commit-receipt");
      const proc = procedure.from({ description: "d" }, () => {
        step.command("Status", { input: input.command`git status --porcelain`, output: statusOut });
        flow.when("Ship only if dirty", condition.not(condition.empty(statusOut)), () => {
          step.command("Stage", { input: input.command`git add -A` });
          step.command("Commit", { input: input.command`git commit -m msg`, output: receipt });
        });
      });
      return toDraftJson(trait("guarded-ship", { name: "Guarded Ship", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const statusOut = slot.text("commit-status");
      const receipt = slot.text("commit-receipt");
      const status = sequence.command("status", {
        title: "Status",
        input: input.command`git status --porcelain`,
        output: statusOut,
      });
      const stage = sequence.command("stage", { title: "Stage", input: input.command`git add -A` });
      const commit = sequence.command("commit", {
        title: "Commit",
        input: input.command`git commit -m msg`,
        output: receipt,
      });
      const shipBranch = sequence.branch("ship-only-if-dirty", {
        title: "Ship only if dirty",
        check: condition.not(condition.empty(statusOut)),
        success: [stage, commit],
      });
      const proc = procedure({ description: "d", sequence: [status, shipBranch] });
      return toDraftJson(trait("guarded-ship", { name: "Guarded Ship", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("flow.match on a slot field lowers to nested sequence.branch chains", () => {
    function viaFunctional() {
      const status = slot({ id: "status", schema: schema.object("status-scaffold", { kind: schema.text() }) });
      const proc = procedure.from({ description: "d" }, () => {
        flow.match("Route", status.kind, {
          a: () => {
            step.command("A Step", { cmd: "echo a" });
          },
          b: () => {
            step.command("B Step", { cmd: "echo b" });
          },
          [flow.Otherwise]: () => {
            step.command("Other Step", { cmd: "echo other" });
          },
        });
      });
      return toDraftJson(trait("match-route", { name: "Match Route", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const status = slot({ id: "status", schema: schema.object("status-scaffold", { kind: schema.text() }) });
      const aArm = sequence.command("a-step", { title: "A Step", cmd: "echo a" });
      const bArm = sequence.command("b-step", { title: "B Step", cmd: "echo b" });
      const otherArm = sequence.command("other-step", { title: "Other Step", cmd: "echo other" });
      const innerBranch = sequence.branch("route-b", {
        title: "Route — b",
        check: condition.equals(status.kind, "b"),
        success: [bArm],
        failure: [otherArm],
      });
      const outerBranch = sequence.branch("route", {
        title: "Route",
        check: condition.equals(status.kind, "a"),
        success: [aArm],
        failure: [innerBranch],
      });
      const proc = procedure({ description: "d", sequence: [outerBranch] });
      return toDraftJson(trait("match-route", { name: "Match Route", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("flow.match on a guard subject lowers to one sequence.branch", () => {
    function viaFunctional() {
      const approved = signal({ id: "approved", description: "The change was approved." });
      const proc = procedure.from({ description: "d" }, () => {
        flow.match("Route Decision", condition.signal(approved), {
          [flow.True]: () => {
            step.command("Merge", { cmd: "echo merge" });
          },
          [flow.False]: () => {
            step.command("Revise", { cmd: "echo revise" });
          },
        });
      });
      return toDraftJson(trait("match-guard", { name: "Match Guard", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const approved = signal({ id: "approved", description: "The change was approved." });
      const merge = sequence.command("merge", { title: "Merge", cmd: "echo merge" });
      const revise = sequence.command("revise", { title: "Revise", cmd: "echo revise" });
      const routeBranch = sequence.branch("route-decision", {
        title: "Route Decision",
        check: condition.signal(approved),
        success: [merge],
        failure: [revise],
      });
      const proc = procedure({ description: "d", sequence: [routeBranch] });
      return toDraftJson(trait("match-guard", { name: "Match Guard", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("flow.parallel: registration order becomes independent named branches", () => {
    function viaFunctional() {
      const proc = procedure.from({ description: "d" }, () => {
        flow.parallel("Fan Out", () => {
          step.command("Review Branch", { cmd: "echo review" });
          step.command("Apply Fixes Branch", { cmd: "echo fix" });
        });
      });
      return toDraftJson(trait("fan-out", { name: "Fan Out", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const reviewStep = sequence.command("review-branch", { title: "Review Branch", cmd: "echo review" });
      const fixStep = sequence.command("apply-fixes-branch", { title: "Apply Fixes Branch", cmd: "echo fix" });
      const reviewLinear = sequence.linear("fan-out-branch-review-branch", [reviewStep]);
      const fixLinear = sequence.linear("fan-out-branch-apply-fixes-branch", [fixStep]);
      const parallelItem = sequence.parallel("fan-out", [reviewLinear, fixLinear], {});
      const proc = procedure({ description: "d", sequence: [parallelItem] });
      return toDraftJson(trait("fan-out", { name: "Fan Out", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("items.forEach lowers to sequence.forEach with an auto-minted item slot", () => {
    function viaFunctional() {
      const files = slot.texts("files");
      const proc = procedure.from({ description: "d" }, () => {
        files.forEach("Review Each", {}, (currentFile) => {
          step.command("Review File", { input: input.command`review ${currentFile}` });
        });
      });
      return toDraftJson(trait("review-each", { name: "Review Each", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const files = slot.texts("files");
      const forEachItem = sequence.forEach("review-each", files, (currentFile) => ({
        title: "Review Each",
        body: [sequence.command("review-file", { title: "Review File", input: input.command`review ${currentFile}` })],
      }));
      const proc = procedure({ description: "d", sequence: [forEachItem] });
      return toDraftJson(trait("review-each", { name: "Review Each", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("a trailing nested flow.loop after flow.until composes when: not(cond)", () => {
    function viaFunctional() {
      const done = slot.boolean("outer-done");
      const proc = procedure.from({ description: "d" }, () => {
        flow.loop("Outer", (outerLoop) => {
          outerLoop.maxIterations(3);
          step.command("First", { cmd: "echo first" });
          flow.until(condition.equals(done, true));
          flow.loop("Inner", (innerLoop) => {
            innerLoop.maxIterations(2);
            step.command("Inner Step", { cmd: "echo inner" });
          });
        });
      });
      return toDraftJson(trait("trailing-loop", { name: "Trailing Loop", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const done = slot.boolean("outer-done");
      const first = sequence.command("first", { title: "First", cmd: "echo first" });
      const innerStep = sequence.command("inner-step", { title: "Inner Step", cmd: "echo inner" });
      const innerLoopItem = sequence.loop("inner", {
        title: "Inner",
        body: [innerStep],
        maxIterations: 2,
        when: condition.not(condition.equals(done, true)),
      });
      const outerLoopItem = sequence.loop("outer", {
        title: "Outer",
        body: [first, innerLoopItem],
        maxIterations: 3,
        until: condition.equals(done, true),
      });
      const proc = procedure({ description: "d", sequence: [outerLoopItem] });
      return toDraftJson(trait("trailing-loop", { name: "Trailing Loop", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("a trailing items.forEach after flow.until composes when: not(cond)", () => {
    function viaFunctional() {
      const done = slot.boolean("for-each-done");
      const files = slot.texts("trailing-files");
      const proc = procedure.from({ description: "d" }, () => {
        flow.loop("Outer", (loop) => {
          loop.maxIterations(3);
          step.command("First", { cmd: "echo first" });
          flow.until(condition.equals(done, true));
          files.forEach("Review Each", {}, (currentFile) => {
            step.command("Review File", { input: input.command`review ${currentFile}` });
          });
        });
      });
      return toDraftJson(trait("trailing-foreach", { name: "Trailing ForEach", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const done = slot.boolean("for-each-done");
      const files = slot.texts("trailing-files");
      const first = sequence.command("first", { title: "First", cmd: "echo first" });
      const forEachItem = sequence.forEach("review-each", files, (currentFile) => ({
        title: "Review Each",
        body: [sequence.command("review-file", { title: "Review File", input: input.command`review ${currentFile}` })],
        when: condition.not(condition.equals(done, true)),
      }));
      const loopItem = sequence.loop("outer", {
        title: "Outer",
        body: [first, forEachItem],
        maxIterations: 3,
        until: condition.equals(done, true),
      });
      const proc = procedure({ description: "d", sequence: [loopItem] });
      return toDraftJson(trait("trailing-foreach", { name: "Trailing ForEach", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("a trailing flow.when block after flow.until ANDs the guard into check", () => {
    function viaFunctional() {
      const done = slot.boolean("when-block-done");
      const dirty = slot.boolean("when-block-dirty");
      const proc = procedure.from({ description: "d" }, () => {
        flow.loop("Outer", (loop) => {
          loop.maxIterations(3);
          step.command("First", { cmd: "echo first" });
          flow.until(condition.equals(done, true));
          flow.when("Trailing Block", condition.equals(dirty, true), () => {
            step.command("Trailing Step", { cmd: "echo trailing" });
          });
        });
      });
      return toDraftJson(trait("trailing-when", { name: "Trailing When", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const done = slot.boolean("when-block-done");
      const dirty = slot.boolean("when-block-dirty");
      const first = sequence.command("first", { title: "First", cmd: "echo first" });
      const trailingStep = sequence.command("trailing-step", { title: "Trailing Step", cmd: "echo trailing" });
      const trailingBranch = sequence.branch("trailing-block", {
        title: "Trailing Block",
        check: condition.all([condition.equals(dirty, true), condition.not(condition.equals(done, true))]),
        success: [trailingStep],
      });
      const loopItem = sequence.loop("outer", {
        title: "Outer",
        body: [first, trailingBranch],
        maxIterations: 3,
        until: condition.equals(done, true),
      });
      const proc = procedure({ description: "d", sequence: [loopItem] });
      return toDraftJson(trait("trailing-when", { name: "Trailing When", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("effect.onComplete/onAbort inside a loop attach to the loop's own fields", () => {
    function viaFunctional() {
      const complete = signal({ id: "loop-complete", description: "The loop finished a round." });
      const gaveUp = signal({ id: "loop-gave-up", description: "The loop aborted." });
      const cap = slot.number("cap");
      const proc = procedure.from({ description: "d" }, () => {
        flow.loop("Bounded", (loop) => {
          loop.maxIterations(3);
          effect.onComplete(complete);
          effect.onAbort(gaveUp);
          step.command("Work", { cmd: "echo work" });
          flow.when("Give up", condition.gte(cap, 3), flow.Abort);
        });
      });
      return toDraftJson(trait("effect-loop", { name: "Effect Loop", summary: "s", procedure: proc }));
    }

    function viaObject() {
      const complete = signal({ id: "loop-complete", description: "The loop finished a round." });
      const gaveUp = signal({ id: "loop-gave-up", description: "The loop aborted." });
      const cap = slot.number("cap");
      const work = sequence.command("work", { title: "Work", cmd: "echo work" });
      const loopItem = sequence.loop("bounded", {
        title: "Bounded",
        body: [work],
        maxIterations: 3,
        abortIf: condition.gte(cap, 3),
        onComplete: [complete],
        onAbort: [gaveUp],
      });
      const proc = procedure({ description: "d", sequence: [loopItem] });
      return toDraftJson(trait("effect-loop", { name: "Effect Loop", summary: "s", procedure: proc }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });
});

describe("evaluateTraitFunction emits byte-identical canonical to the object layer (0107)", () => {
  it("a procedural functional trait matches its object-layer equivalent", () => {
    function viaFunctional() {
      const envelope = evaluateTraitFunction((ctx) => {
        defineTrait("diff-review", {
          name: "Diff Review",
          summary: "Reviews a diff.",
          procedure: "Review a diff for the stated focus.",
        });
        port.input.text({ id: "diff" });
        useBehavior({ tone: tone.Direct });
        const review = slot.text("review");
        step.command("Review", { output: review, input: input.command`echo ${ctx.input.diff as never}` });
        return { review };
      });
      return envelope.draft;
    }

    function viaObject() {
      const diff = port.input.text({ id: "diff" });
      const review = slot.text("review");
      const reviewStep = sequence.command("review", {
        title: "Review",
        output: review,
        input: input.command`echo ${diff}`,
      });
      const output = port.output.of({ id: "review", schema: "schema:text", value: review });
      const proc = procedure({ description: "Review a diff for the stated focus.", sequence: [reviewStep] });
      return toDraftJson(trait("diff-review", {
        name: "Diff Review",
        summary: "Reviews a diff.",
        behavior: { tone: tone.Direct },
        port: output,
        procedure: proc,
      }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });

  it("a behavioral functional trait (no steps) matches its object-layer equivalent", () => {
    function viaFunctional() {
      const envelope = evaluateTraitFunction(() => {
        defineTrait("guidance-only", { name: "Guidance Only", summary: "Behavioral guidance." });
        useBehavior({ tone: tone.Direct });
        useIntent({ require: [intent("cite-evidence")] });
      });
      return envelope.draft;
    }

    function viaObject() {
      return toDraftJson(trait("guidance-only", {
        name: "Guidance Only",
        summary: "Behavioral guidance.",
        behavior: { tone: tone.Direct },
        intent: { require: [intent("cite-evidence")] },
      }));
    }

    expectByteIdentical(viaFunctional(), viaObject());
  });
});
