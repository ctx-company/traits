// Functional authoring layer (0106): build-rule error paths, one per rule
// named in the task's Watch/Decisions, plus a couple of positive smoke
// checks that don't pin exact JSON shape (the byte-identity proof for that
// lives in `functional.hand.ts`, per the standing 2026-07-24 validation
// ruling — no behavior-freezing tests in the gated suite).
import {
  agent,
  behavior,
  condition,
  defineTrait,
  effect,
  evaluateTraitFunction,
  flow,
  input,
  intent,
  port,
  procedure,
  resource,
  schema,
  seats,
  signal,
  slot,
  step,
  toDraftJson,
  trait,
  useBehavior,
  useIntent,
  useResource,
  useVariant,
} from "@ctx-traits/cdk";
import { defineVariant, isTraitFamilyHandle, resolveTraitFamily, variant } from "@ctx-traits/cdk";
import type { JsonObject } from "@ctx-traits/cdk";
import { describe, expect, it } from "vitest";

describe("functional layer build rules (0106)", () => {
  it("a registrar called outside procedure.from throws, naming itself", () => {
    expect(() => step.command("Outside", { cmd: "echo hi" })).toThrow(
      /step\.command\("Outside"\) called outside procedure\.from/,
    );
  });

  it("agent.prompt called outside procedure.from throws", () => {
    const worker = agent.worker("outside-worker");
    expect(() => worker.prompt("Outside Prompt", { input: input.prompt`Do it.` })).toThrow(/outside procedure\.from/);
  });

  it("a flow.* block callback that returns a thenable is a build error", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Async Loop", async (loop) => {
          loop.maxIterations(1);
        });
      }),
    ).toThrow(/async callbacks are not supported/);
  });

  it("opening a second build while one is active throws", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("A", { cmd: "echo a" });
        procedure.from({ description: "d2" }, () => {
          step.command("B", { cmd: "echo b" });
        });
      }),
    ).toThrow(/a functional build is already in progress/);
  });

  it("loop.until/untilAll/untilAny wrap the flow registrars on the loop param", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Param Until", (loop) => {
          loop.maxIterations(2);
          step.command("A", { cmd: "echo a" });
          loop.untilAll([condition.empty(slot.text("param-until-a")), condition.empty(slot.text("param-until-b"))]);
        });
      }),
    ).not.toThrow();
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Param Until Twice", (loop) => {
          loop.maxIterations(2);
          step.command("A", { cmd: "echo a" });
          loop.until(condition.empty(slot.text("param-until-c")));
          loop.untilAny([condition.empty(slot.text("param-until-d"))]);
        });
      }),
    ).toThrow(/at most one loop\.until/);
  });

  it("a second loop.until in one loop scope throws", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Two Untils", (loop) => {
          loop.maxIterations(2);
          step.command("A", { cmd: "echo a" });
          loop.until(condition.empty(slot.text("first-cond-slot")));
          loop.until(condition.empty(slot.text("second-cond-slot")));
        });
      }),
    ).toThrow(/at most one loop\.until/);
  });

  it("a loop with neither a bound nor an exit guard is a build error", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("No Way Out", () => {
          step.command("A", { cmd: "echo a" });
        });
      }),
    ).toThrow(/a loop needs a way out/);
  });

  /// The canonical has always accepted an unbounded loop that declares an
  /// exit guard; the authoring layer used to be stricter than the contract it
  /// lowers to, forcing an arbitrary round ceiling on every review loop.
  it("an exit guard alone is a way out — no maxIterations needed", () => {
    const built = procedure.from({ description: "d" }, () => {
      flow.loop("Guarded Unbounded", (loop) => {
        step.command("A", { cmd: "echo a" });
        loop.until(condition.empty(slot.text("unbounded-guard-slot")));
      });
    });
    const loopItem = (built as { readonly sequence?: readonly Record<string, unknown>[] }).sequence?.[0];
    expect(loopItem?.["max-iterations"]).toBeUndefined();
    expect(loopItem?.until).toBeDefined();
  });

  it("a flow.when abort arm is a way out on its own", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Abort Armed", () => {
          step.command("A", { cmd: "echo a" });
          flow.when("Give up", condition.empty(slot.text("abort-arm-slot")), signal.Abort);
        });
      }),
    ).not.toThrow();
  });

  it("loop.maxIterations called twice throws", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Double Budget", (loop) => {
          loop.maxIterations(1);
          loop.maxIterations(2);
        });
      }),
    ).toThrow(/loop\.maxIterations.*called more than once/);
  });

  it("duplicate titles in one scope throw, naming both titles", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("Same Title", { cmd: "echo a" });
        step.command("Same Title", { cmd: "echo b" });
      }),
    ).toThrow(/steps titled "Same Title" and "Same Title"/);
  });

  it("a non-callback flow.match arm throws, naming the arm", () => {
    const subject = slot({
      id: "match-subject",
      schema: schema.object("match-subject-scaffold", { kind: schema.text() }),
    });
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.match("Bad Arm", subject.kind, {
          // oxlint-disable-next-line -- intentionally not a callback, exercising the build rule.
          foo: "not-a-function" as never,
        });
      }),
    ).toThrow(/arm "foo" must be a callback/);
  });

  it("par.concurrencyLimit is parked: it throws and emits nothing", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.parallel("Parked", (par) => {
          par.concurrencyLimit(2);
          step.command("A", { cmd: "echo a" });
        });
      }),
    ).toThrow(/par\.concurrencyLimit is parked/);
  });

  it("flow.when(..., signal.Abort) outside a loop throws", () => {
    const cap = slot.number("outside-loop-cap");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.when("Give up", condition.gte(cap, 3), signal.Abort);
      }),
    ).toThrow(/requires an enclosing flow\.loop/);
  });

  it("effect.onComplete/onAbort outside a loop throw", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        effect.onComplete("some-signal" as never);
      }),
    ).toThrow(/requires an enclosing flow\.loop/);
    expect(() =>
      procedure.from({ description: "d" }, () => {
        effect.onAbort("some-signal" as never);
      }),
    ).toThrow(/requires an enclosing flow\.loop/);
  });

  it("effect.onFailure outside a flow.parallel throws — a loop declares no failure of its own to route", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Has No Failure Route", (loop) => {
          loop.maxIterations(1);
          effect.onFailure(signal.Skip);
        });
      }),
    ).toThrow(/no target here/);
  });

  it("effect.onFailure(signal.Continue) is not a legal branch-failure verb", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.parallel("Fan Out", () => {
          step.command("A", { cmd: "echo a" });
          effect.onFailure(signal.Continue as never);
        });
      }),
    ).toThrow(/is not legal here/);
  });

  it("a second effect.onFailure decision verb in one flow.parallel throws", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.parallel("Fan Out Twice", () => {
          step.command("A", { cmd: "echo a" });
          effect.onFailure(signal.Skip);
          effect.onFailure(signal.Park);
        });
      }),
    ).toThrow(/decision verb already declared once/);
  });

  it("loop.maxIterations({ onExhausted: signal.Skip }) is not legal — only Abort/Continue govern exhaustion", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Bad Exhaustion", (loop) => {
          loop.maxIterations(2, { onExhausted: signal.Skip as never });
        });
      }),
    ).toThrow(/is not legal here/);
  });

  it("condition.signal(signal.Skip) is rejected — verb signals are raise-only", () => {
    expect(() => condition.signal(signal.Skip as never)).toThrow(/signal\.Skip.*raise-only/);
  });

  it("flow.match requires at least one value arm", () => {
    const subject = slot({
      id: "empty-match-subject",
      schema: schema.object("empty-match-scaffold", { kind: schema.text() }),
    });
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.match("Empty Match", subject.kind, {});
      }),
    ).toThrow(/requires at least one value arm/);
  });

  it("flow.match registered after loop.until in a loop throws, naming the block — silently unguarded emission is not acceptable", () => {
    const done = slot.boolean("match-after-until-done");
    const subject = slot({
      id: "match-after-until-subject",
      schema: schema.object("match-after-until-scaffold", { kind: schema.text() }),
    });
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Outer", (loop) => {
          loop.maxIterations(2);
          step.command("First", { cmd: "echo first" });
          loop.until(condition.equals(done, true));
          flow.match("Trailing Match", subject.kind, {
            a: () => {
              step.command("A", { cmd: "echo a" });
            },
          });
        });
      }),
    ).toThrow(/"Trailing Match".*registered after loop\.until.*cannot be guarded/);
  });

  it("flow.parallel registered after loop.until in a loop throws, naming the block — silently unguarded emission is not acceptable", () => {
    const done = slot.boolean("parallel-after-until-done");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Outer", (loop) => {
          loop.maxIterations(2);
          step.command("First", { cmd: "echo first" });
          loop.until(condition.equals(done, true));
          flow.parallel("Trailing Parallel", () => {
            step.command("A", { cmd: "echo a" });
          });
        });
      }),
    ).toThrow(/"Trailing Parallel".*registered after loop\.until.*cannot be guarded/);
  });

  it("a build that throws mid-body still frees the build slot for the next one", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("Same Title", { cmd: "echo a" });
        step.command("Same Title", { cmd: "echo b" });
      }),
    ).toThrow();

    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("Fine", { cmd: "echo fine" });
      }),
    ).not.toThrow();
  });

  it("a well-formed procedure.from build produces a real ProcedureHandle usable in a trait", () => {
    const worker = agent.worker("smoke-worker");
    const draft = slot.text("smoke-draft");
    const proc = procedure.from({ description: "Smoke test." }, () => {
      worker.prompt("Do The Thing", { input: input.prompt`Do it.`, output: draft });
    });
    const built = toDraftJson(trait("functional-smoke", { name: "Functional Smoke", summary: "s", procedure: proc }));
    expect(built).toMatchObject({ id: "functional-smoke" });
  });

  it("an `id:` override wins over idFromTitle(title) on every step.*/agent.prompt/flow.when/flow.loop registrar (0109 F2)", () => {
    const worker = agent.worker("id-override-worker");
    const out = slot.text("id-override-out");
    const flagSlot = slot.text("id-override-flag");
    const proc = procedure.from({ description: "d" }, () => {
      step.command("Capture the changed-file inventory", { id: "capture-diff", cmd: "echo hi", output: out });
      step.check("Run the repository gate chain", {
        id: "repo-gates",
        cmd: "echo ok",
        output: flagSlot as never,
      });
      worker.prompt("Draft the work (smart-1)", {
        id: "draft-writing",
        input: input.prompt`Draft.`,
        output: out,
      });
      flow.when(
        "Check working tree status",
        condition.not(condition.empty(out)),
        { id: "shipping-maybe-commit" },
        () => {
          step.command("Commit the work", { id: "shipping-commit", cmd: "echo commit", output: out });
        },
      );
      flow.loop("Refinement Loop", (loop) => {
        loop.maxIterations(1);
        loop.id("building");
        step.command("Round", { cmd: "echo round", output: out });
        loop.until(condition.not(condition.empty(out)));
      });
    });
    const built = toDraftJson(
      trait("id-override-smoke", { name: "Id Override Smoke", summary: "s", procedure: proc }),
    ) as { procedure: { sequence: readonly { id: string }[] } };
    const ids = built.procedure.sequence.map((item) => item.id);
    expect(ids).toEqual(["capture-diff", "repo-gates", "draft-writing", "shipping-maybe-commit", "building"]);
  });

  it("step.project (0109 F3) authors a deterministic project step with the same id-override escape", () => {
    const source = slot.text("project-source");
    const destination = slot.text("project-destination");
    const proc = procedure.from({ description: "d" }, () => {
      step.project("Park Report Clear", { id: "park-report-clear", projections: [{ source, destination }] });
    });
    const built = toDraftJson(
      trait("step-project-smoke", { name: "Step Project Smoke", summary: "s", procedure: proc }),
    ) as { procedure: { sequence: readonly { id: string; kind: string }[] } };
    expect(built.procedure.sequence).toMatchObject([{ id: "park-report-clear", kind: "project" }]);
  });

  it("flow.errorWhen lowers to when + an error terminal on the reserved flow-error port (0189)", () => {
    const verdict = slot.text("terminal-verdict");
    const proc = procedure.from({ description: "d" }, () => {
      step.command("Review", { cmd: "echo revise", output: verdict });
      flow.errorWhen("Version Bump Failed", condition.equals(verdict, "revise"), {
        message: "Version bump rejected by review",
        evidence: verdict,
      });
    });
    const built = toDraftJson(
      trait("terminal-error-smoke", { name: "Terminal Error Smoke", summary: "s", procedure: proc }),
    ) as {
      procedure: { sequence: readonly { id: string; kind: string; when?: unknown; sequence?: string }[] };
      sequence?: Readonly<
        Record<
          string,
          {
            sequence: readonly {
              id: string;
              kind: string;
              outcome?: string;
              message?: string;
              payload?: readonly { destination: string; source: string }[];
            }[];
          }
        >
      >;
    };
    const branch = built.procedure.sequence.find((item) => item.id === "version-bump-failed");
    expect(branch).toMatchObject({ kind: "branch", when: { slot: "slot:terminal-verdict", equals: "revise" } });
    const armId = (branch?.sequence ?? "").replace("sequence:", "");
    const arm = built.sequence?.[armId]?.sequence ?? [];
    expect(arm).toMatchObject([
      {
        id: "version-bump-failed-exit",
        kind: "terminal",
        outcome: "error",
        message: "Version bump rejected by review",
        payload: [{ destination: "flow-error", source: "slot:terminal-verdict" }],
      },
    ]);
  });

  it("flow.success binds declared output ports at the exit and defaults its message to the title (0189)", () => {
    const summary = slot.text("terminal-summary");
    const proc = procedure.from({ description: "d" }, () => {
      step.command("Do the work", { cmd: "echo done", output: summary });
      flow.success("Released", { bind: { "release-report": summary } });
    });
    const built = toDraftJson(
      trait("terminal-success-smoke", { name: "Terminal Success Smoke", summary: "s", procedure: proc }),
    ) as {
      procedure: {
        sequence: readonly {
          id: string;
          kind: string;
          outcome?: string;
          message?: string;
          payload?: readonly { destination: string; source: string }[];
        }[];
      };
    };
    expect(built.procedure.sequence[1]).toMatchObject({
      id: "released",
      kind: "terminal",
      outcome: "success",
      message: "Released",
      payload: [{ destination: "release-report", source: "slot:terminal-summary" }],
    });
  });

  it("step.project registered after loop.until in its own loop is a build error", () => {
    const gate = slot.text("project-until-gate");
    const target = slot.text("project-until-target");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Loop", (loop) => {
          loop.maxIterations(2);
          loop.until(condition.not(condition.empty(gate)));
          step.project("Late Project", { projections: [{ source: gate, destination: target }] });
        });
      }),
    ).toThrow(/step\.project registered after loop\.until/);
  });
});

describe("defineTrait/use*/derived manifest build rules (0107)", () => {
  it("defineTrait never called is a build error", () => {
    expect(() => evaluateTraitFunction(() => undefined)).toThrow(/defineTrait\(\.\.\.\) was never called/);
  });

  it("defineTrait called twice is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("first-call");
        defineTrait("second-call");
      }),
    ).toThrow(/defineTrait: called more than once/);
  });

  it("a declared step is callable: invoking it registers its prompt or command step", () => {
    const target = slot.text("declared-target");
    const notes = slot.text("declared-notes");
    const probe = slot.text("declared-probe");
    const surveyStep = step({
      agent: agent.worker("declared-worker"),
      input: input.prompt`Survey ${target}.`,
      output: notes,
    });
    const statusStep = step({
      input: input.command`git status --porcelain`,
      output: probe,
    });
    expect(surveyStep.output).toBe(notes);
    const envelope = evaluateTraitFunction(() => {
      defineTrait("declared-step-shape", { description: "One-line declared steps." });
      surveyStep("Survey the target");
      statusStep("Check working tree status");
    });
    const draft = envelope.draft as {
      readonly procedure?: {
        readonly sequence?: readonly { readonly id?: string; readonly output?: unknown }[];
      };
    };
    expect(draft.procedure?.sequence?.[0]?.id).toBe("survey-the-target");
    expect(draft.procedure?.sequence?.[0]?.output).toEqual(["slot:declared-notes"]);
    expect(draft.procedure?.sequence?.[1]?.id).toBe("check-working-tree-status");
    expect(draft.procedure?.sequence?.[1]?.output).toEqual(["slot:declared-probe"]);
  });

  it("the declaration form and the inline registrars are the same `step` identifier", () => {
    const probe = slot.text("one-noun-probe");
    const declared = step({ input: input.command`git status --porcelain`, output: probe });
    const envelope = evaluateTraitFunction(() => {
      defineTrait("one-noun", { description: "Declared and inline steps side by side." });
      declared("Check working tree status");
      step.command("Show the current ref", { input: input.command`git rev-parse HEAD`, output: probe });
    });
    const draft = envelope.draft as {
      readonly procedure?: { readonly sequence?: readonly { readonly id?: string; readonly kind?: string }[] };
    };
    expect(draft.procedure?.sequence?.map((item) => item.id)).toEqual([
      "check-working-tree-status",
      "show-the-current-ref",
    ]);
  });

  it("defineTrait derives the canonical id from a display name and keeps the name", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("Display Name", { description: "s" });
    });
    expect(envelope.draft).toMatchObject({ id: "display-name", name: "Display Name" });
  });

  // `name` is REQUIRED on the canonical, so a draft without one cannot
  // decode. This used to inject nothing when the given name already kebab-
  // cased to itself, which meant every lowercase name produced an
  // unbuildable trait: `ctx traits create work` failed with "invalid
  // manifest at root: missing field `name`" while `create Work` succeeded.
  it("defineTrait with a bare slug still carries it as the name", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("bare-slug", { description: "s" });
    });
    expect(envelope.draft).toMatchObject({ id: "bare-slug", name: "bare-slug" });
  });

  it("defineTrait with a computed (non-literal) field is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        // oxlint-disable-next-line -- intentionally not JSON-safe, exercising the build rule.
        defineTrait("computed-field", { summary: (() => "nope") as never });
      }),
    ).toThrow(/must be plain literal data/);
  });

  it("defineTrait with an unknown field is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        // oxlint-disable-next-line -- intentionally an unknown field, exercising the build rule.
        defineTrait("unknown-field", { title: "nope" } as never);
      }),
    ).toThrow(/unknown field\(s\) title/);
  });

  it("two useBehavior calls setting the same facet throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("behavior-overlap");
        useBehavior({ tone: behavior.tone.Direct });
        useBehavior({ tone: behavior.tone.Warm });
      }),
    ).toThrow(/"tone" was already set/);
  });

  it("useBehavior with an unknown key throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("behavior-unknown-key");
        // oxlint-disable-next-line -- intentionally an unknown field, exercising the build rule.
        useBehavior({ mood: "chipper" } as never);
      }),
    ).toThrow(/unknown field\(s\) mood/);
  });

  it("useBehavior with an undefined array entry (an enum typo) throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("behavior-undefined-entry");
        // oxlint-disable-next-line -- intentionally undefined, exercising the enum-typo catch.
        useBehavior({ format: [behavior.tone.Direct, undefined] as never });
      }),
    ).toThrow(/format\[1\] is undefined/);
  });

  it("useIntent require/avoid contradiction on the same slug throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("intent-contradiction");
        useIntent({ require: [intent("cite-evidence")] });
        useIntent({ avoid: [intent("cite-evidence")] });
      }),
    ).toThrow(/"cite-evidence".*declared in both require and avoid/);
  });

  it("two useIntent calls setting the same facet throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("intent-overlap");
        useIntent({ require: [intent("a")] });
        useIntent({ require: [intent("b")] });
      }),
    ).toThrow(/"require" was already set/);
  });

  it("useResource with a non-resource value throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("resource-not-a-handle");
        // oxlint-disable-next-line -- intentionally not a resource handle, exercising the build rule.
        useResource(slot.text("not-a-resource") as never);
      }),
    ).toThrow(/is not a resource handle/);
  });

  it("an unknown ctx.input access throws, listing the declared input port ids", () => {
    expect(() =>
      evaluateTraitFunction((ctx) => {
        defineTrait("unknown-input", { description: "p" });
        port.input.text({ id: "diff" });
        step.command("Read Focus", { input: input.command`echo ${ctx.input.focus as never}` });
      }),
    ).toThrow(/ctx\.input: unknown input port\(s\) focus.*declared input ports are: diff/);
  });

  it("a declared-but-never-referenced resource is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("orphan-resource", { description: "s" });
        resource.inline("orphan", "Never used.");
      }),
    ).toThrow(/declared but never referenced.*resource "orphan"/);
  });

  it("a non-slot return value is a build error naming the key", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("bad-return");
        return { commitReport: "not-a-slot" };
      }),
    ).toThrow(/return value "commitReport" must be a slot handle/);
  });

  it("a behavioral trait (no steps) builds a valid draft with no procedure", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("engineering-standards-shape", { summary: "Behavioral guidance only." });
      useBehavior({ tone: behavior.tone.Direct });
      useIntent({ require: [intent("cite-evidence")] });
    });
    expect(envelope.draft).toMatchObject({ id: "engineering-standards-shape" });
    expect((envelope.draft as { procedure?: unknown }).procedure).toBeUndefined();
  });

  it("a procedural trait with declared input and returned output builds a valid draft", () => {
    const envelope = evaluateTraitFunction((ctx) => {
      defineTrait("procedural-shape", { description: "Review a diff." });
      port.input.text({ id: "diff" });
      const review = slot.text("review");
      step.command("Review", { output: review, input: input.command`echo ${ctx.input.diff as never}` });
      return { review };
    });
    expect(envelope.draft).toMatchObject({ id: "procedural-shape" });
    const draft = envelope.draft as { port?: readonly { readonly id: string; readonly direction: string }[] };
    const portIds = (draft.port ?? []).map((p) => `${p.id}:${p.direction}`).sort();
    expect(portIds).toEqual(["diff:input", "review:output"]);
  });

  it("a seats(...)-minted agent's .prompt registrar compiles to canonical output identical to hand-numbering (0162)", () => {
    const buildWith = (smart1: ReturnType<typeof agent.reviewer>, smart2: ReturnType<typeof agent.reviewer>) =>
      evaluateTraitFunction((ctx) => {
        defineTrait("seat-sugar-shape", { description: "Review a diff." });
        port.input.text({ id: "diff" });
        const review1 = slot.text("review-1");
        const review2 = slot.text("review-2");
        smart1.prompt("Review (smart-1)", {
          input: input.prompt`Review ${ctx.input.diff as never}.`,
          output: review1,
        });
        smart2.prompt("Review (smart-2)", {
          input: input.prompt`Review ${ctx.input.diff as never}.`,
          output: review2,
        });
        return { review1, review2 };
      });

    // Destructuring a `readonly H[]` under noUncheckedIndexedAccess types
    // each element `H | undefined`; the length is proven by construction.
    const [seatSmart1, seatSmart2] = seats(agent.reviewer, "smart", 2);
    if (seatSmart1 === undefined || seatSmart2 === undefined) {
      throw new Error("seats(2) must mint two handles");
    }
    const seatDraft = buildWith(seatSmart1, seatSmart2).draft;

    const handSmart1 = agent.reviewer("smart-1");
    const handSmart2 = agent.reviewer("smart-2");
    const handDraft = buildWith(handSmart1, handSmart2).draft;

    expect(JSON.stringify(seatDraft)).toBe(JSON.stringify(handDraft));
  });
});

describe("defineVariant/useVariant hook-style families", () => {
  const quickVariant = (ctx: unknown) => {
    void ctx;
    defineVariant("quick", {
      name: "Family (Quick)",
      description: "One step.",
    });
    useBehavior({ tone: behavior.tone.Direct });
    const out = slot.text("result");
    agent.worker("worker", { description: "Does the step." }).prompt("Do the step", {
      input: input.prompt`Do it.`,
      output: out,
    });
    return { result: out };
  };

  it("a family shell binds hook variants and resolves through the object family path", async () => {
    const family = evaluateTraitFunction(function () {
      defineTrait("hook-family", { version: "0.1.0" });
      useVariant(quickVariant).default();
    });
    if (!isTraitFamilyHandle(family)) {
      throw new Error("expected a trait family handle");
    }
    const resolved = await resolveTraitFamily(family);
    expect(resolved.id).toBe("hook-family");
    const paths = resolved.variants.map((entry) => entry.path);
    expect(paths).toContain("quick");
    const quick = resolved.variants.find((entry) => entry.path === "quick");
    const draft = quick?.draft as JsonObject | undefined;
    expect(draft?.["name"]).toBe("Family (Quick)");
    expect((draft?.["procedure"] as JsonObject | undefined)?.["description"]).toBe("One step.");
  });

  it("0206: a family variant's command step may write directly to an output port at the native-variant schema-version", async () => {
    const commitReport = port.output.text({
      id: "commit-report",
      description: "Direct command output port.",
    });
    const commitVariant = (ctx: unknown) => {
      void ctx;
      defineVariant("commit", { description: "One command step writing to a port." });
      step.command("Commit", {
        input: input.command`git commit -m msg`,
        output: commitReport,
      });
      return { commitReport };
    };
    const family = evaluateTraitFunction(function () {
      defineTrait("port-write-family", { version: "0.1.0" });
      useVariant(commitVariant).default();
    });
    if (!isTraitFamilyHandle(family)) {
      throw new Error("expected a trait family handle");
    }
    const resolved = await resolveTraitFamily(family);
    const commit = resolved.variants.find((entry) => entry.path === "commit");
    const draft = commit?.draft as JsonObject | undefined;
    expect(
      draft?.["schema-version"],
      "every native-variant leaf is stamped at the 0.5 floor a direct command-to-port write requires",
    ).toBe("0.5");
    const sequence = (draft?.["procedure"] as JsonObject | undefined)?.["sequence"] as
      | readonly JsonObject[]
      | undefined;
    expect(sequence?.[0]?.["output"]).toEqual(["port:commit-report"]);
  });

  it("an explicit id keeps the display-form first argument off the public variant key", async () => {
    const displayVariant = (ctx: unknown) => {
      void ctx;
      defineVariant("Family (Quick)", { id: "quick", description: "One step." });
      const out = slot.text("display-result");
      agent.worker("display-worker", { description: "Does the step." }).prompt("Do the step", {
        input: input.prompt`Do it.`,
        output: out,
      });
      return { result: out };
    };
    const family = evaluateTraitFunction(function () {
      defineTrait("display-family", { version: "0.1.0" });
      useVariant(displayVariant).default();
    });
    if (!isTraitFamilyHandle(family)) {
      throw new Error("expected a trait family handle");
    }
    const resolved = await resolveTraitFamily(family);
    const quick = resolved.variants.find((entry) => entry.path === "quick");
    expect(quick, "variant key must be the explicit id, never the slugged display form").toBeDefined();
    expect((quick?.draft as JsonObject | undefined)?.["name"]).toBe("Family (Quick)");
  });

  it("a non-slug explicit id is a build error", () => {
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("bad-id-family", { version: "0.1.0" });
        useVariant((ctx: unknown) => {
          void ctx;
          defineVariant("Whatever", { id: "Not A Slug" });
        }).default();
      }),
    ).toThrow(/id "Not A Slug" is not a kebab slug/);
  });

  it("defineVariant inside a trait frame is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineVariant("nope");
      }),
    ).toThrow(/defineVariant: called inside a trait function/);
  });

  it("defineTrait inside a variant function is a build error", () => {
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(() => {
          defineTrait("wrong-call");
        }).default();
      }),
    ).toThrow(/defineTrait: called inside a variant function/);
  });

  it("a variant function that never calls defineVariant is a build error", () => {
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(() => undefined).default();
      }),
    ).toThrow(/never called defineVariant/);
  });

  it("duplicate variant keys are a build error", () => {
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(quickVariant).default();
        useVariant(quickVariant);
      }),
    ).toThrow(/duplicate variant key "quick"/);
  });

  it("no default marked is a build error, and two defaults are too", () => {
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(quickVariant);
      }),
    ).toThrow(/no variant marked default/);
    const other = (ctx: unknown) => {
      void ctx;
      defineVariant("other", { description: "One step." });
      const out = slot.text("out");
      agent.worker("worker", { description: "Does the step." }).prompt("Do the step", {
        input: input.prompt`Do it.`,
        output: out,
      });
    };
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(quickVariant).default();
        useVariant(other).default();
      }),
    ).toThrow(/exactly one variant may be the family default/);
  });

  it("a family shell registering its own steps is a build error", () => {
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(quickVariant).default();
        step.command("Stray step", { argv: ["true"], output: slot.text("stray") });
      }),
    ).toThrow(/family shell must not register its own steps/);
  });

  it("an object-style handle bridges in with an explicit key; a function must not pass one", () => {
    const legacy = variant({
      summary: "Legacy object variant.",
      procedure: procedure({
        description: "No steps.",
        sequence: [],
      }),
    });
    const family = evaluateTraitFunction(function () {
      defineTrait("mixed-family", { version: "0.1.0" });
      useVariant(quickVariant).default();
      useVariant(legacy, "legacy");
    });
    expect(isTraitFamilyHandle(family)).toBe(true);
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(legacy);
      }),
    ).toThrow(/needs an explicit family key/);
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("family-shell", { version: "0.1.0" });
        useVariant(quickVariant, "quick");
      }),
    ).toThrow(/carries its own key via defineVariant/);
  });

  it("a shell's useIntent/useResource distribute into every bound variant, union-merged with the variant's own", async () => {
    const shared = resource.inline("shared-style", "Shared style guide.");
    const family = evaluateTraitFunction(function () {
      defineTrait("shell-facets", { version: "0.1.0" });
      useIntent({ require: [intent("cite-evidence")] });
      useResource(shared);
      useVariant((ctx: unknown) => {
        void ctx;
        defineVariant("quick", { description: "One step." });
        useResource(shared);
        useIntent({ require: [intent("be-concise")] });
        const out = slot.text("result");
        agent.worker("worker", { description: "Does the step." }).prompt("Do the step", {
          input: input.prompt`Do it.`,
          output: out,
        });
        return { result: out };
      }).default();
    });
    if (!isTraitFamilyHandle(family)) {
      throw new Error("expected a trait family handle");
    }
    const resolved = await resolveTraitFamily(family);
    const quick = resolved.variants.find((entry) => entry.path === "quick");
    const draft = quick?.draft as JsonObject | undefined;
    const requireSlugs = ((draft?.["intent"] as JsonObject | undefined)?.["require"] as JsonObject[] | undefined)?.map(
      (entry) => entry["id"],
    );
    expect(requireSlugs).toContain("cite-evidence");
    expect(requireSlugs).toContain("be-concise");
    const resourceRefs = (draft?.["resource"] as JsonObject[] | undefined)?.map((entry) => entry["id"]);
    expect(resourceRefs?.filter((id) => id === "shared-style")).toHaveLength(1);
  });

  it("a shell's useIntent/useResource cannot reach an object-style variant handle — build error", () => {
    const legacy = variant({
      summary: "Legacy object variant.",
      procedure: procedure({ description: "No steps.", sequence: [] }),
    });
    expect(() =>
      evaluateTraitFunction(function () {
        defineTrait("mixed-shell-facets", { version: "0.1.0" });
        useIntent({ require: [intent("cite-evidence")] });
        useVariant(quickVariant).default();
        useVariant(legacy, "legacy");
      }),
    ).toThrow(/object-style handles bypass frame evaluation/);
  });

  it("a shell-declared use* twin is byte-identical to the same facets authored per-variant", async () => {
    const buildVariant = (declareInShell: boolean) => (ctx: unknown) => {
      void ctx;
      defineVariant("quick", { description: "One step." });
      if (!declareInShell) {
        useResource(shared);
        useIntent({ require: [intent("cite-evidence")] });
      }
      const out = slot.text("result");
      agent.worker("worker", { description: "Does the step." }).prompt("Do the step", {
        input: input.prompt`Do it.`,
        output: out,
      });
      return { result: out };
    };
    const shared = resource.inline("shared-style", "Shared style guide.");
    const shellOnly = evaluateTraitFunction(function () {
      defineTrait("twin-shell-only", { version: "0.1.0" });
      useIntent({ require: [intent("cite-evidence")] });
      useResource(shared);
      useVariant(buildVariant(true)).default();
    });
    const perVariant = evaluateTraitFunction(function () {
      defineTrait("twin-per-variant", { version: "0.1.0" });
      useVariant(buildVariant(false)).default();
    });
    if (!isTraitFamilyHandle(shellOnly) || !isTraitFamilyHandle(perVariant)) {
      throw new Error("expected trait family handles");
    }
    const shellResolved = await resolveTraitFamily(shellOnly);
    const perVariantResolved = await resolveTraitFamily(perVariant);
    const shellDraft = shellResolved.variants.find((entry) => entry.path === "quick")?.draft as JsonObject | undefined;
    const perVariantDraft = perVariantResolved.variants.find((entry) => entry.path === "quick")?.draft as
      | JsonObject
      | undefined;
    const strip = (draft: JsonObject | undefined) => {
      const { id: _id, ...rest } = { ...draft };
      return rest;
    };
    expect(JSON.stringify(strip(shellDraft))).toBe(JSON.stringify(strip(perVariantDraft)));
  });

  it("a returned decorated output port passes through with its declaration intact", () => {
    const draft = evaluateTraitFunction(function () {
      defineTrait("decorated-output", { version: "0.1.0", description: "One step." });
      const out = slot.text("payload");
      agent.worker("worker", { description: "Does the step." }).prompt("Do the step", {
        input: input.prompt`Do it.`,
        output: out,
      });
      return {
        report: port.output.text({
          id: "report",
          title: "Report",
          description: "The decorated output.",
          optional: true,
          value: out,
        }),
      };
    }) as { draft: JsonObject };
    const ports = (draft.draft as { port?: JsonObject[] }).port ?? [];
    const report = ports.find((p) => p["id"] === "report");
    expect(report).toBeDefined();
    expect(report?.["title"]).toBe("Report");
    expect(report?.["optional"]).toBe(true);
  });
});

describe("effect.session.title (0110)", () => {
  it("a string input is a verbatim sink", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("sink-string", { description: "s" });
      effect.session.title("Fixed session title");
    });
    expect(envelope.draft).toMatchObject({
      sink: { "session-title": { mode: "verbatim", input: "Fixed session title" } },
    });
  });

  it("an input.prompt template (including one wrapping a slot) is a verbatim sink", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("sink-template", { description: "p" });
      const topic = slot.text("topic");
      step.command("Set Topic", { output: topic, input: input.command`echo hi` });
      effect.session.title(input.prompt`Working on ${topic}`);
      return { topic };
    });
    const draft = envelope.draft as { readonly sink?: { readonly "session-title"?: JsonObject } };
    expect(draft.sink?.["session-title"]).toEqual({ mode: "verbatim", input: "Working on {slot:topic}" });
  });

  it("a bare slot input is a generated sink", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("sink-slot", { description: "p" });
      const draftSlot = slot.text("draft");
      step.command("Draft", { output: draftSlot, input: input.command`echo hi` });
      effect.session.title(draftSlot);
      return { draftSlot };
    });
    const draft = envelope.draft as { readonly sink?: { readonly "session-title"?: JsonObject } };
    expect(draft.sink?.["session-title"]).toEqual({ mode: "generated", input: "slot:draft" });
  });

  it("an array of slots (with an optional part) is a generated sink", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("sink-array", { description: "p" });
      const first = slot.text("first");
      const second = slot.text("second");
      step.command("Fill", { output: [first, second], input: input.command`echo hi` });
      effect.session.title([first, input.optional(second)]);
      return { first, second };
    });
    const draft = envelope.draft as { readonly sink?: { readonly "session-title"?: JsonObject } };
    expect(draft.sink?.["session-title"]).toEqual({
      mode: "generated",
      input: ["slot:first", { slot: "slot:second", optional: true }],
    });
  });

  it("a second effect.session.title declaration in the same build throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("sink-duplicate");
        effect.session.title("First title");
        effect.session.title("Second title");
      }),
    ).toThrow(/effect\.session\.title: already declared once in this build/);
  });

  it("declaring the sink both via effect.session.title and the object-layer sink field is a build error", () => {
    expect(() =>
      toDraftJson(
        trait("sink-both", {
          summary: "Conflicting sink declarations.",
          sink: { sessionTitle: "Object-layer title" },
          procedure: procedure.from({ description: "p" }, () => {
            effect.session.title("Functional title");
          }),
        }),
      ),
    ).toThrow(/\[sink\.session-title\] declared twice/);
  });

  it("no declaration means no sink in the draft", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("sink-none", { description: "s" });
    });
    expect((envelope.draft as { readonly sink?: unknown }).sink).toBeUndefined();
  });
});

describe("items.forEach typed item slots (0153) + loop scope param (0211)", () => {
  const draftSlots = (built: unknown): readonly { id: string; schema: string }[] =>
    (built as { slot?: readonly { id: string; schema: string }[] }).slot ?? [];

  it("the item slot inherits the iterated list's element schema", () => {
    const reviewItem = schema.object("fe-review-item", { note: schema.text() });
    const items = slot.list(reviewItem, "fe-items");
    const out = slot.text("fe-out");
    const proc = procedure.from({ description: "d" }, () => {
      items.forEach("Handle each item", (item) => {
        step.command("Echo", { cmd: "echo hi", output: out });
        void item;
      });
    });
    const built = toDraftJson(trait("fe-inherit", { name: "FE Inherit", summary: "s", procedure: proc }));
    const itemSlot = draftSlots(built).find((declared) => declared.id === "handle-each-item-item");
    expect(itemSlot?.schema).toBe("schema:fe-review-item");
  });

  it("an explicit loop.itemSchema override wins over inheritance", () => {
    const items = slot.texts("fe-override-items");
    const out = slot.text("fe-override-out");
    const proc = procedure.from({ description: "d" }, () => {
      items.forEach("Handle overridden", (item, loop) => {
        loop.itemSchema(schema.text());
        step.command("Echo", { cmd: "echo hi", output: out });
        void item;
      });
    });
    const built = toDraftJson(trait("fe-override", { name: "FE Override", summary: "s", procedure: proc }));
    const itemSlot = draftSlots(built).find((declared) => declared.id === "handle-overridden-item");
    expect(itemSlot?.schema).toBe("schema:text");
  });

  it("a non-list over slot falls back to the untyped pre-0153 item mint", () => {
    const raw = slot.any("fe-raw");
    const out = slot.text("fe-fallback-out");
    const proc = procedure.from({ description: "d" }, () => {
      raw.forEach("Handle raw", (item) => {
        step.command("Echo", { cmd: "echo hi", output: out });
        void item;
      });
    });
    const built = toDraftJson(trait("fe-fallback", { name: "FE Fallback", summary: "s", procedure: proc }));
    const itemSlot = draftSlots(built).find((declared) => declared.id === "handle-raw-item");
    expect(itemSlot?.schema).toBe("schema:any");
  });

  it("loop.limit/concurrent land in the emitted for-each fields", () => {
    const items = slot.texts("fe-knobs-items");
    const out = slot.text("fe-knobs-out");
    const proc = procedure.from({ description: "d" }, () => {
      items.forEach("Handle knobs", (item, loop) => {
        loop.limit(3);
        loop.concurrent();
        step.command("Echo", { cmd: "echo hi", output: out });
        void item;
      });
    });
    const built = toDraftJson(trait("fe-knobs", { name: "FE Knobs", summary: "s", procedure: proc })) as {
      procedure: { sequence: readonly { id: string; "max-items"?: number; concurrent?: boolean }[] };
    };
    const forEachItem = built.procedure.sequence.find((entry) => entry.id === "handle-knobs");
    expect(forEachItem).toMatchObject({ "max-items": 3, concurrent: true });
  });

  it("loop.limit called twice throws", () => {
    const items = slot.texts("fe-limit-twice-items");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        items.forEach("Handle limit twice", (item, loop) => {
          loop.limit(1);
          loop.limit(2);
          step.command("Echo", { cmd: "echo hi" });
          void item;
        });
      }),
    ).toThrow(/loop\.limit\(\.\.\.\) called more than once/);
  });

  it("loop.itemSchema called twice throws", () => {
    const items = slot.texts("fe-schema-twice-items");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        items.forEach("Handle schema twice", (item, loop) => {
          loop.itemSchema(schema.text());
          loop.itemSchema(schema.text());
          step.command("Echo", { cmd: "echo hi" });
          void item;
        });
      }),
    ).toThrow(/loop\.itemSchema\(\.\.\.\) called more than once/);
  });

  it("calling the outer each from inside a nested items.forEach body throws — no aliasing across scopes", () => {
    const outerItems = slot.texts("fe-nested-outer-items");
    const innerItems = slot.texts("fe-nested-inner-items");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        outerItems.forEach("Outer", (outerItem, outerLoop) => {
          void outerItem;
          innerItems.forEach("Inner", (innerItem) => {
            step.command("Echo", { cmd: "echo hi" });
            void innerItem;
            outerLoop.limit(1);
          });
        });
      }),
    ).toThrow(/loop\.limit\(\.\.\.\) called outside its own items\.forEach body/);
  });

  it("an item field accessed before loop.itemSchema(...) declares the schema is a loud build error", () => {
    const items = slot.list(schema.object("fe-lazy-item", { note: schema.text() }), "fe-lazy-items");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        items.forEach("Handle lazy", (item) => {
          const fieldItem = item as unknown as Record<string, unknown>;
          step.command("Echo", { cmd: "echo hi", input: input.command`echo ${fieldItem["note"] as never}` });
        });
      }),
    ).toThrow(/item field "note" accessed before loop\.itemSchema\(\.\.\.\) declared the item schema/);
  });

  it("a field accessed after loop.itemSchema(...) declares an object schema reaches the built draft", () => {
    const items = slot.texts("fe-lazy-declared-items");
    expect(() => {
      const proc = procedure.from({ description: "d" }, () => {
        items.forEach("Handle lazy declared", (item, loop) => {
          loop.itemSchema(schema.object("fe-lazy-declared-schema", { note: schema.text() }));
          const fieldItem = item as unknown as { readonly note: never };
          flow.when("Note Is Set", condition.equals(fieldItem.note, "set"), () => {
            step.command("Echo", { cmd: "echo hi" });
          });
        });
      });
      return toDraftJson(trait("fe-lazy-declared", { name: "FE Lazy Declared", summary: "s", procedure: proc }));
    }).not.toThrow();
  });

  it("effect.onComplete inside items.forEach attaches to the for-each's own onComplete", () => {
    const items = slot.texts("fe-oncomplete-items");
    const complete = signal({ id: "fe-oncomplete-signal", description: "The for-each finished a round." });
    const proc = procedure.from({ description: "d" }, () => {
      items.forEach("Handle complete", (item) => {
        effect.onComplete(complete);
        step.command("Echo", { cmd: "echo hi" });
        void item;
      });
    });
    const built = toDraftJson(trait("fe-oncomplete", { name: "FE OnComplete", summary: "s", procedure: proc })) as {
      procedure: { sequence: readonly { id: string; "on-complete"?: readonly string[] }[] };
    };
    const forEachItem = built.procedure.sequence.find((entry) => entry.id === "handle-complete");
    expect(forEachItem?.["on-complete"]).toEqual(["signal:fe-oncomplete-signal"]);
  });

  it("effect.onComplete outside any flow.loop/items.forEach still throws, naming both container kinds", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        effect.onComplete("some-signal" as never);
      }),
    ).toThrow(/requires an enclosing flow\.loop or items\.forEach/);
  });

  it("flow no longer exposes until/untilAll/untilAny — loop.until is the only spelling", () => {
    expect((flow as { readonly until?: unknown }).until).toBeUndefined();
    expect((flow as { readonly untilAll?: unknown }).untilAll).toBeUndefined();
    expect((flow as { readonly untilAny?: unknown }).untilAny).toBeUndefined();
  });
});
