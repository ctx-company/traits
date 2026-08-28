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
  output,
  port,
  procedure,
  ref,
  resource,
  schema,
  seats,
  signal,
  slot,
  defineStep,
  step,
  toDraftJson,
  trait,
  useBehavior,
  useIntent,
  useResource,
  useVariant,
} from "@ctx-traits/cdk";
import { defineVariant, isTraitFamilyHandle, resolveTraitFamily, variant } from "@ctx-traits/cdk";
import type { AskStepFields, FieldRef, JsonObject, ParameterizedStep, SlotHandle } from "@ctx-traits/cdk";
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

  it("agent.prompt, step.prompt, and defineStep.prompt reuse prompt intent lowering and infer schema 0.6", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("functional-prompt-intent", { description: "Prompt intent paths." });
      const worker = agent.worker("intent-worker");
      const first = slot.text("intent-first");
      const second = slot.text("intent-second");
      const third = slot.text("intent-third");
      const declared = defineStep.prompt({
        agent: worker,
        input: input.prompt`Declared.`,
        output: third,
        intent: { block: intent.OverEngineering },
      });
      worker.prompt("Agent Prompt", {
        input: input.prompt`Agent.`,
        output: first,
        intent: { require: intent.Correctness },
      });
      step.prompt("Inline Prompt", {
        agent: worker,
        input: input.prompt`Inline.`,
        output: second,
        intent: { focus: intent.Robustness },
      });
      declared("Declared Prompt");
      return { first, second, third };
    });
    const draft = envelope.draft as {
      readonly "schema-version": string;
      readonly procedure?: { readonly sequence?: readonly { readonly intent?: unknown }[] };
    };
    expect(draft["schema-version"]).toBe("0.6");
    expect(draft.procedure?.sequence?.map((item) => item.intent)).toEqual([
      { require: [{ id: "correctness" }] },
      { focus: [{ id: "robustness" }] },
      { block: [{ id: "over-engineering" }] },
    ]);
  });

  it("agent.prompt, step.prompt, and defineStep.prompt preserve prompt behavior through nested lowering and infer schema 0.6", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("functional-prompt-behavior", { description: "Prompt behavior paths." });
      const worker = agent.worker("behavior-worker");
      const first = slot.text("behavior-first");
      const second = slot.text("behavior-second");
      const third = slot.text("behavior-third");
      const declared = defineStep.prompt({
        agent: worker,
        input: input.prompt`Declared.`,
        output: third,
        behavior: { scopeControl: behavior.scopeControl.Strict },
      });
      worker.prompt("Agent Prompt", {
        input: input.prompt`Agent.`,
        output: first,
        behavior: { tone: behavior.tone.Direct },
      });
      step.prompt("Inline Prompt", {
        agent: worker,
        input: input.prompt`Inline.`,
        output: second,
        behavior: { method: behavior.method.EvidenceFirst },
      });
      declared("Declared Prompt");
      return { first, second, third };
    });
    const draft = envelope.draft as {
      readonly "schema-version": string;
      readonly procedure?: { readonly sequence?: readonly { readonly behavior?: unknown }[] };
    };
    expect(draft["schema-version"]).toBe("0.6");
    expect(draft.procedure?.sequence?.map((item) => item.behavior)).toEqual([
      { tone: [{ id: "direct" }] },
      { method: [{ id: "evidence-first" }] },
      { "scope-control": { id: "strict" } },
    ]);
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

describe("step.ask / defineStep.ask (0253.4)", () => {
  it("step.ask lowers to kind ask with a signal guard and mints a virtual slot for a bare-schema output", () => {
    const sig = signal({
      id: "ask-lower-signal",
      description: "d",
      schema: schema.object("ask-lower-payload", { reason: schema.text() }),
    });
    const proc = procedure.from({ description: "d" }, () => {
      const ask = step.ask("What now", { when: sig, input: input.prompt`What next?`, output: schema.text() });
      step.command("Echo answer", { cmd: "echo hi", input: [ask.result] });
    });
    const built = toDraftJson(trait("ask-lower-fixture", { name: "Ask Lower", summary: "s", procedure: proc })) as {
      procedure: { sequence: readonly { id: string; kind: string; when?: unknown }[] };
    };
    const item = built.procedure.sequence.find((entry) => entry.id === "what-now");
    expect(item).toMatchObject({ kind: "ask", when: "signal:ask-lower-signal" });
  });

  it("defineStep.ask({...}) declares a reusable, static ask step, reachable from the package root", () => {
    const sig = signal({
      id: "ask-static-signal",
      description: "d",
      schema: schema.object("ask-static-payload", { reason: schema.text() }),
    });
    const summon: AskStepFields = {
      when: sig,
      input: input.prompt`What now?`,
      output: schema.text(),
    };
    const ask = defineStep.ask(summon);
    const proc = procedure.from({ description: "d" }, () => {
      ask("Static Summon");
    });
    const built = toDraftJson(trait("ask-static-fixture", { name: "Ask Static", summary: "s", procedure: proc })) as {
      procedure: { sequence: readonly { id: string; kind: string }[] };
    };
    expect(built.procedure.sequence).toMatchObject([{ id: "static-summon", kind: "ask" }]);
  });

  it("defineStep.ask((sig) => ...) declares a reusable, parameterized ask step", () => {
    const sig = signal({
      id: "ask-factory-signal",
      description: "d",
      schema: schema.object("ask-factory-payload", { reason: schema.text() }),
    });
    const summon = defineStep.ask((s: typeof sig) => ({
      when: s,
      input: input.prompt`Why: ${s.reason}`,
      output: schema.text(),
    }));
    const proc = procedure.from({ description: "d" }, () => {
      summon("Summon Owner", sig);
    });
    const built = toDraftJson(trait("ask-factory-fixture", { name: "Ask Factory", summary: "s", procedure: proc })) as {
      procedure: { sequence: readonly { id: string; kind: string }[] };
    };
    expect(built.procedure.sequence).toMatchObject([{ id: "summon-owner", kind: "ask" }]);
  });

  it("a signal field read in an ask's own body is legal without an enclosing flow.when — the ask's own `when` is the guard", () => {
    const sig = signal({
      id: "ask-own-guard-signal",
      description: "d",
      schema: schema.object("ask-own-guard-payload", { reason: schema.text() }),
    });
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.ask("Self Guarded", { when: sig, input: input.prompt`Why: ${sig.reason}`, output: schema.text() });
      }),
    ).not.toThrow();
  });

  it("a signal field read from a DIFFERENT signal than an ask's own guard is still a build error", () => {
    const sig = signal({
      id: "ask-mismatched-guard-signal",
      description: "d",
      schema: schema.object("ask-mismatched-guard-payload", { reason: schema.text() }),
    });
    const other = signal({ id: "ask-mismatched-other-signal", description: "d" });
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.ask("Mismatched Ask", { when: other, input: input.prompt`Why: ${sig.reason}`, output: schema.text() });
      }),
    ).toThrow(/"Mismatched Ask".*"signal:ask-mismatched-guard-signal" field "reason" is readable only inside/);
  });

  it("step.ask registered after loop.until in its own loop is a build error", () => {
    const sig = signal({ id: "ask-until-signal", description: "d" });
    const gate = slot.text("ask-until-gate");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Loop", (loop) => {
          loop.maxIterations(2);
          loop.until(condition.not(condition.empty(gate)));
          step.ask("Late Ask", { when: sig, input: input.prompt`Why?`, output: schema.text() });
        });
      }),
    ).toThrow(/step\.ask registered after loop\.until/);
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
    const surveyStep = defineStep.prompt({
      agent: agent.worker("declared-worker"),
      input: input.prompt`Survey ${target}.`,
      output: notes,
    });
    const statusStep = defineStep.command({
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

  // Declaring and placing are two verbs now: `defineStep.*` declares,
  // `step.*` places. They used to share the noun `step`, which read as a
  // function at one call site and a namespace at the next.
  it("a declared step and an inline step sit side by side in one procedure", () => {
    const probe = slot.text("one-noun-probe");
    const declared = defineStep.command({ input: input.command`git status --porcelain`, output: probe });
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

  it("a parameterized defineStep.command factory binds a fresh ref per instantiation", () => {
    const first = slot.text("factory-command-first");
    const second = slot.text("factory-command-second");
    const status = defineStep.command((target: SlotHandle<string>) => ({
      input: input.command`git log ${target}`,
      output: target,
    }));
    const envelope = evaluateTraitFunction(() => {
      defineTrait("factory-command", { description: "parameterized command factory." });
      status("Log first", first);
      status("Log second", second);
    });
    const draft = envelope.draft as {
      readonly procedure?: { readonly sequence?: readonly { readonly id?: string; readonly output?: unknown }[] };
    };
    expect(draft.procedure?.sequence?.[0]?.output).toEqual(["slot:factory-command-first"]);
    expect(draft.procedure?.sequence?.[1]?.output).toEqual(["slot:factory-command-second"]);
  });

  it("a parameterized defineStep.prompt factory's unannotated param interpolates with no cast", () => {
    const target = slot.text("factory-prompt-target");
    const notes = slot.text("factory-prompt-notes");
    const surveyStep = defineStep.prompt((subject) => ({
      agent: agent.worker("factory-worker"),
      input: input.prompt`Survey ${subject}.`,
      output: notes,
    }));
    const envelope = evaluateTraitFunction(() => {
      defineTrait("factory-prompt", { description: "parameterized prompt factory." });
      surveyStep("Survey the target", target);
    });
    const draft = envelope.draft as {
      readonly prompt?: Record<string, { readonly text?: string; readonly input?: readonly string[] }>;
    };
    expect(draft.prompt?.["survey-the-target"]?.input).toEqual(["slot:factory-prompt-target"]);
    expect(draft.prompt?.["survey-the-target"]?.text).toBe("Survey {slot:factory-prompt-target}.");
  });

  it("a parameterized defineStep.check factory keeps .pass typing through gate.pass.ok", () => {
    const marker = slot.text("factory-check-marker");
    const verdict = slot({
      id: "factory-check-verdict",
      schema: schema.object("factory-check-result", {
        ok: schema.field(schema.boolean()),
        argv: schema.field(schema.list(schema.text())),
      }),
    });
    const gate = defineStep.check((cmdTarget: SlotHandle<string>) => ({
      input: input.command`test -f ${cmdTarget}`,
      output: verdict,
    }));
    const envelope = evaluateTraitFunction(() => {
      defineTrait("factory-check", { description: "parameterized check factory." });
      const placed = gate("Check marker", marker);
      expect(placed.pass.ok).toBeDefined();
    });
    expect(envelope.draft).toBeDefined();
  });

  it("a parameterized step factory's arity and ref kind are checked at the instantiation call site (typecheck only)", () => {
    // oxlint-disable-next-line no-constant-condition -- gated typecheck-only block, never executed.
    if (false) {
      const textTarget = slot.text("factory-typed-target");
      const numberTarget = slot.number("factory-typed-number");
      const status = defineStep.command((target: SlotHandle<string>) => ({
        input: input.command`git log ${target}`,
        output: target,
      }));
      // @ts-expect-error too few refs — the factory declares one positional ref.
      status("Missing ref");
      // @ts-expect-error too many refs — the factory declares exactly one positional ref.
      status("Extra ref", textTarget, textTarget);
      // @ts-expect-error wrong kind — the factory's ref is annotated SlotHandle<string>, not SlotHandle<number>.
      status("Wrong kind", numberTarget);
      status("Right shape", textTarget);
    }
  });

  it("a factory-declared prompt or command exposes .result with the full augmented handle (typecheck only)", () => {
    // oxlint-disable-next-line no-constant-condition -- gated typecheck-only block, never executed.
    if (false) {
      const commandStep = defineStep.command(() => ({
        input: input.command`git status --porcelain`,
        output: schema.text(),
      }));
      const placedCommand = commandStep("Status");
      placedCommand.result satisfies SlotHandle<string>;
      placedCommand.result.optional().optional satisfies true;
      void placedCommand.result.with; // present on the type — the augmented handle, not a bare SlotHandle
      // The virtual slot is consumable by a later step, exactly like a hand-declared one.
      step.command("Log", { cmd: "log", input: [placedCommand.result] });

      const promptStep = defineStep.prompt((subject: SlotHandle<string>) => ({
        agent: agent.worker("factory-result-worker"),
        input: input.prompt`Survey ${subject}.`,
        output: schema.text(),
      }));
      const placedPrompt = promptStep("Survey", slot.text("factory-result-subject"));
      placedPrompt.result satisfies SlotHandle<string>;

      // `ParameterizedStep` is importable from the package root, not just the internal functional module path —
      // `commandStep` (a real `defineStep.command((...refs) => ...)` factory) satisfies it directly, no cast.
      const rootImported: ParameterizedStep<readonly [], typeof placedCommand> = commandStep;
      void rootImported;

      // A factory-declared step with a HETEROGENEOUS schema tuple output routes
      // through the same VirtualSlotSurfaceOf as a static-fields step (0253.3) —
      // `.results` keeps each member's own inferred type, not `unknown`.
      const findingSchema = schema.object("factory-result-finding", { file: schema.text(), summary: schema.text() });
      const multiStep = defineStep.command(() => ({
        input: input.command`find-issue`,
        output: [schema.text(), findingSchema] as const,
      }));
      const placedMulti = multiStep("Find");
      placedMulti.results[0] satisfies SlotHandle<string>;
      placedMulti.results[1].file satisfies FieldRef<string>;
    }
  });

  it("a declared/factory step with no output: never types .result (typecheck only)", () => {
    // oxlint-disable-next-line no-constant-condition -- gated typecheck-only block, never executed.
    if (false) {
      // `.result` alone would still type-check as `unknown` through `CdkObject`'s
      // index signature — calling `.optional()` is the check that actually
      // needs a real virtual-slot handle to compile.
      const noOutputDeclared = defineStep.command({ input: input.command`git status --porcelain` });
      const placedNoOutput = noOutputDeclared("Status");
      // @ts-expect-error no output: at all — a plain SequenceHandle, no .result promised.
      placedNoOutput.result.optional();

      const noOutputFactory = defineStep.command(() => ({ input: input.command`git status --porcelain` }));
      const placedFactoryNoOutput = noOutputFactory("Status");
      // @ts-expect-error same guard through the factory overload.
      placedFactoryNoOutput.result.optional();

      const named = slot.text("factory-named-only");
      const namedOutputDeclared = defineStep.command({ input: input.command`git log`, output: named });
      const placedNamed = namedOutputDeclared("Log");
      // @ts-expect-error a hand-declared named slot output is not virtual — no .result promised.
      placedNamed.result.optional();
    }
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
  // The gap the split closes. `step(fields)` chose its kind by SHAPE — agent
  // present for a prompt, otherwise a command — so a reusable CHECK could not
  // be declared at all, and the reusable face of `step` was strictly less
  // capable than the inline one.
  it("declares a reusable check", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("Reusable Check", { version: "0.1.0", description: "d" });
      const verdict = slot({
        id: "gate-verdict",
        schema: schema.object("gate-result", {
          ok: schema.field(schema.boolean(), { description: "Whether the gate passed." }),
          argv: schema.field(schema.list(schema.text()), { description: "The argv that decided it." }),
        }),
      });
      const gate = defineStep.check({ input: input.command`just test`, output: verdict });
      gate("Run the test gate");
      return {};
    });
    const draft = envelope.draft as {
      readonly procedure: { readonly sequence: readonly { readonly kind?: string }[] };
    };
    expect(draft.procedure.sequence.map((item) => item.kind)).toContain("check");
  });

  // A declared step places more than once, which is the whole point of
  // declaring it, and each placement gets its own id from its own title.
  it("places a declared step more than once, with an id per title", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("Twice Placed", { version: "0.1.0", description: "d" });
      const status = defineStep.command({
        input: input.command`git status --porcelain`,
        output: slot.text("tree"),
      });
      status("Check the tree before");
      status("Check the tree after");
      return {};
    });
    const draft = envelope.draft as {
      readonly procedure: { readonly sequence: readonly { readonly id?: string }[] };
    };
    expect(draft.procedure.sequence.map((item) => item.id)).toEqual(["check-the-tree-before", "check-the-tree-after"]);
  });

  // Every entity takes a NAME and derives its id, the way defineTrait and
  // step.command always have. Before this, `slot`, `port`, `agent` and the
  // rest ran their first argument through `validateSlug` unchanged, so an
  // author had to remember which builders kebab-case for them and which
  // reject a capital letter.
  it("derives an id from a name on every declaration builder", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("Name First", { version: "0.1.0", description: "d" });
      const goal = port.input.text({ id: "Task Key", description: "x" });
      const result = slot.text("Work Summary");
      const hand = agent.worker("Lead Worker", { description: "w" });
      hand.prompt("Do it", { input: input.prompt`Do ${goal}.`, output: result });
      return { report: port.output.text({ id: "Final Report", value: result, description: "r" }) };
    });
    const draft = envelope.draft as {
      readonly port: readonly { readonly id: string }[];
      readonly slot: readonly { readonly id: string }[];
      readonly agent: readonly { readonly id: string }[];
    };
    expect(draft.port.map((entry) => entry.id).sort()).toEqual(["final-report", "task-key"]);
    expect(draft.slot.map((entry) => entry.id)).toEqual(["work-summary"]);
    expect(draft.agent.map((entry) => entry.id)).toEqual(["lead-worker"]);
  });

  // The reason this could ship without touching a single existing canonical:
  // a name that is already a slug derives to itself.
  it("leaves an id-shaped name byte-identical", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("Slug Named", { version: "0.1.0", description: "d" });
      const result = slot.text("work-summary");
      const hand = agent.worker("worker", { description: "w" });
      hand.prompt("Do it", { input: input.prompt`Do it.`, output: result });
      return { report: port.output.text({ id: "report", value: result, description: "r" }) };
    });
    const draft = envelope.draft as { readonly slot: readonly { readonly id: string }[] };
    expect(draft.slot.map((entry) => entry.id)).toEqual(["work-summary"]);
  });

  // The error names the string the author wrote. Being told "" is invalid
  // when you typed "!!!" names something that appears nowhere in the source.
  it("quotes the given name when no id can be derived from it", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("Underivable", { version: "0.1.0", description: "d" });
        slot.text("!!!");
      }),
    ).toThrow(/cannot derive an id from "!!!"/);
  });

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

  it("an intent-guided agent collected through .prompt is normalized and infers schema 0.6", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("intent-guided-prompt-agent", { description: "Review a diff." });
      const review = slot.text("review");
      agent.reviewer("reviewer", { intent: { avoid: intent.ScopeCreep } }).prompt("Review", {
        input: input.prompt`Review the diff.`,
        output: review,
      });
      return { review };
    });
    expect(envelope.draft).toMatchObject({
      "schema-version": "0.6",
      agent: [{ id: "reviewer", intent: { avoid: [{ id: "scope-creep" }] } }],
    });
  });

  it("a behavior-guided agent collected through .prompt is normalized and infers schema 0.6", () => {
    const build = (guided: boolean) =>
      evaluateTraitFunction(() => {
        defineTrait("behavior-guided-prompt-agent", { description: "Review a diff." });
        const review = slot.text("review");
        agent
          .reviewer("reviewer", guided ? { behavior: { tone: behavior.tone.Direct } } : undefined)
          .prompt("Review", { input: input.prompt`Review the diff.`, output: review });
        return { review };
      }).draft;
    const guided = build(true);
    const ordinary = build(false) as Record<string, unknown>;
    expect(guided).toMatchObject({
      "schema-version": "0.6",
      agent: [{ id: "reviewer", behavior: { tone: [{ id: "direct" }] } }],
    });
    expect(ordinary["schema-version"]).toBe("0.5");
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

  it("native variants infer 0.6 for guided transitively collected agents and retain 0.5 for ordinary leaves", async () => {
    const intentGuidedVariant = (ctx: unknown) => {
      void ctx;
      defineVariant("intent-guided", { description: "Intent-guided step." });
      const out = slot.text("intent-guided-result");
      agent.worker("intent-guided-worker", { intent: { avoid: "scope-creep" } }).prompt("Do intent-guided work", {
        input: input.prompt`Do it.`,
        output: out,
      });
      return { out };
    };
    const behaviorGuidedVariant = (ctx: unknown) => {
      void ctx;
      defineVariant("behavior-guided", { description: "Behavior-guided step." });
      const out = slot.text("behavior-guided-result");
      agent
        .worker("behavior-guided-worker", { behavior: { tone: behavior.tone.Direct } })
        .prompt("Do behavior-guided work", {
          input: input.prompt`Do it.`,
          output: out,
        });
      return { out };
    };
    const ordinaryVariant = (ctx: unknown) => {
      void ctx;
      defineVariant("ordinary", { description: "Ordinary step." });
      const out = slot.text("ordinary-result");
      agent.worker("ordinary-worker").prompt("Do ordinary work", { input: input.prompt`Do it.`, output: out });
      return { out };
    };
    const family = evaluateTraitFunction(function () {
      defineTrait("agent-version-family", { version: "0.1.0" });
      useVariant(intentGuidedVariant).default();
      useVariant(behaviorGuidedVariant);
      useVariant(ordinaryVariant);
    });
    if (!isTraitFamilyHandle(family)) {
      throw new Error("expected a trait family handle");
    }
    const resolved = await resolveTraitFamily(family);
    expect(resolved.variants.find((entry) => entry.path === "intent-guided")?.draft["schema-version"]).toBe("0.6");
    expect(resolved.variants.find((entry) => entry.path === "behavior-guided")?.draft["schema-version"]).toBe("0.6");
    const ordinary = resolved.variants.find((entry) => entry.path === "ordinary")?.draft;
    expect(JSON.stringify(ordinary)).toBe(
      '{"agent":[{"description":"Completes assigned work and produces the requested outputs.","id":"ordinary-worker","summary":"Execution role."}],"description":"Ordinary step.","id":"agent-version-family","port":[{"description":"output port out.","direction":"output","id":"out","schema":"schema:text","value":"slot:ordinary-result"}],"procedure":{"description":"Ordinary step.","output":["port:out"],"sequence":[{"agent":"agent:ordinary-worker","id":"do-ordinary-work","output":["slot:ordinary-result"],"prompt":"prompt:do-ordinary-work","title":"Do ordinary work"}]},"prompt":{"do-ordinary-work":{"output":["slot:ordinary-result"],"text":"Do it."}},"schema-version":"0.5","slot":[{"description":"Runtime slot ordinary-result.","id":"ordinary-result","schema":"schema:text"}],"variant":"ordinary","version":"0.1.0"}',
    );
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

describe("signal field scope rule (0253.2)", () => {
  it("a signal field read inside a flow.when guarded on condition.signal(sig) lowers to {signal:<id>.<field>}, out of the step's input", () => {
    const worker = agent.worker("signal-scope-worker");
    const sig = signal({
      id: "signal-scope-review",
      description: "d",
      schema: schema.object("signal-scope-payload", { reason: schema.text() }),
    });
    const proc = procedure.from({ description: "d" }, () => {
      flow.when("Handle Review", condition.signal(sig), () => {
        step.prompt("Consume", { agent: worker, input: input.prompt`Reason: ${sig.reason}` });
      });
    });
    const built = toDraftJson(
      trait("signal-scope-fixture", { name: "Signal Scope Fixture", summary: "s", procedure: proc }),
    ) as { readonly prompt?: Record<string, { readonly text?: string; readonly input?: unknown }> };

    expect(built.prompt?.consume?.text).toBe("Reason: {signal:signal-scope-review.reason}");
    expect(built.prompt?.consume?.input).toBeUndefined();
  });

  it("a signal field read outside any guard is a build error naming the signal, the field, and the step", () => {
    const worker = agent.worker("signal-scope-worker-2");
    const sig = signal({
      id: "signal-scope-review-2",
      description: "d",
      schema: schema.object("signal-scope-payload-2", { reason: schema.text() }),
    });

    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.prompt("Unguarded", { agent: worker, input: input.prompt`Reason: ${sig.reason}` });
      }),
    ).toThrow(
      /"Unguarded".*"signal:signal-scope-review-2" field "reason" is readable only inside a flow\.when block guarded on condition\.signal\(signal-scope-review-2\)/,
    );
  });

  it("a signal field read inside a flow.when guarded on a DIFFERENT signal is still a build error", () => {
    const worker = agent.worker("signal-scope-worker-3");
    const sig = signal({
      id: "signal-scope-review-3",
      description: "d",
      schema: schema.object("signal-scope-payload-3", { reason: schema.text() }),
    });
    const other = signal({ id: "signal-scope-other-3", description: "d" });

    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.when("Wrong Guard", condition.signal(other), () => {
          step.prompt("Mismatched", { agent: worker, input: input.prompt`Reason: ${sig.reason}` });
        });
      }),
    ).toThrow(/"Mismatched".*"signal:signal-scope-review-3" field "reason" is readable only inside a flow\.when/);
  });

  it("an unguarded signal field read inside an output.text/output.of instruction-output throws too, not just input/prompt/text", () => {
    const worker = agent.worker("signal-scope-worker-4");
    const sig = signal({
      id: "signal-scope-review-4",
      description: "d",
      schema: schema.object("signal-scope-payload-4", { reason: schema.text() }),
    });
    const proposal = slot.text("signal-scope-output-proposal-4");

    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.prompt("Unguarded Output", {
          agent: worker,
          input: input.prompt`Produce a proposal.`,
          output: [proposal, output.text`Reason: ${sig.reason}`],
        });
      }),
    ).toThrow(
      /"Unguarded Output".*"signal:signal-scope-review-4" field "reason" is readable only inside a flow\.when block guarded on condition\.signal\(signal-scope-review-4\)/,
    );
  });

  it("a guarded signal field read inside an output.text instruction-output is legal", () => {
    const worker = agent.worker("signal-scope-worker-5");
    const sig = signal({
      id: "signal-scope-review-5",
      description: "d",
      schema: schema.object("signal-scope-payload-5", { reason: schema.text() }),
    });
    const proposal = slot.text("signal-scope-output-proposal-5");
    const proc = procedure.from({ description: "d" }, () => {
      flow.when("Handle Review Output", condition.signal(sig), () => {
        step.prompt("Consume Output", {
          agent: worker,
          input: input.prompt`Produce a proposal.`,
          output: [proposal, output.text`Reason: ${sig.reason}`],
        });
      });
    });
    const built = toDraftJson(
      trait("signal-scope-output-fixture", { name: "Signal Scope Output Fixture", summary: "s", procedure: proc }),
    ) as { readonly prompt?: Record<string, { readonly text?: string }> };

    expect(built.prompt?.["consume-output"]?.text).toContain("Reason: {signal:signal-scope-review-5.reason}");
  });

  it("a signal field read inside a flow.when guarded through a LOCAL named condition.all(...) wrapping condition.signal(sig) is legal", () => {
    const worker = agent.worker("signal-scope-worker-6");
    const sig = signal({
      id: "signal-scope-review-6",
      description: "d",
      schema: schema.object("signal-scope-payload-6", { reason: schema.text() }),
    });
    const proc = procedure.from({ description: "d" }, () => {
      flow.when(
        "Handle Review Named Guard",
        condition.all("signal-scope-named-guard-6", [condition.signal(sig)]),
        () => {
          step.prompt("Consume Named Guard", { agent: worker, input: input.prompt`Reason: ${sig.reason}` });
        },
      );
    });
    const built = toDraftJson(
      trait("signal-scope-named-guard-fixture", {
        name: "Signal Scope Named Guard Fixture",
        summary: "s",
        procedure: proc,
      }),
    ) as { readonly prompt?: Record<string, { readonly text?: string }> };

    expect(built.prompt?.["consume-named-guard"]?.text).toBe("Reason: {signal:signal-scope-review-6.reason}");
  });

  it("a signal field read inside a flow.when guarded through a NESTED chain of local named conditions is legal", () => {
    const worker = agent.worker("signal-scope-worker-7");
    const sig = signal({
      id: "signal-scope-review-7",
      description: "d",
      schema: schema.object("signal-scope-payload-7", { reason: schema.text() }),
    });
    const inner = condition.all("signal-scope-inner-guard-7", [condition.signal(sig)]);
    const outer = condition.all("signal-scope-outer-guard-7", [inner]);
    const proc = procedure.from({ description: "d" }, () => {
      flow.when("Handle Review Nested Guard", outer, () => {
        step.prompt("Consume Nested Guard", { agent: worker, input: input.prompt`Reason: ${sig.reason}` });
      });
    });
    const built = toDraftJson(
      trait("signal-scope-nested-guard-fixture", {
        name: "Signal Scope Nested Guard Fixture",
        summary: "s",
        procedure: proc,
      }),
    ) as { readonly prompt?: Record<string, { readonly text?: string }> };

    expect(built.prompt?.["consume-nested-guard"]?.text).toBe("Reason: {signal:signal-scope-review-7.reason}");
  });

  it("a signal field read inside a flow.when guarded on condition.not(localNamedCondition) is legal", () => {
    const worker = agent.worker("signal-scope-worker-8");
    const sig = signal({
      id: "signal-scope-review-8",
      description: "d",
      schema: schema.object("signal-scope-payload-8", { reason: schema.text() }),
    });
    const named = condition.all("signal-scope-not-guard-8", [condition.signal(sig)]);
    const proc = procedure.from({ description: "d" }, () => {
      flow.when("Handle Review Not Guard", condition.not(named), () => {
        step.prompt("Consume Not Guard", { agent: worker, input: input.prompt`Reason: ${sig.reason}` });
      });
    });
    const built = toDraftJson(
      trait("signal-scope-not-guard-fixture", {
        name: "Signal Scope Not Guard Fixture",
        summary: "s",
        procedure: proc,
      }),
    ) as { readonly prompt?: Record<string, { readonly text?: string }> };

    expect(built.prompt?.["consume-not-guard"]?.text).toBe("Reason: {signal:signal-scope-review-8.reason}");
  });

  it("a signal field read guarded only by an OPAQUE external condition ref remains unauthorized", () => {
    const worker = agent.worker("signal-scope-worker-9");
    const sig = signal({
      id: "signal-scope-review-9",
      description: "d",
      schema: schema.object("signal-scope-payload-9", { reason: schema.text() }),
    });

    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.when("Opaque Guard", ref.condition("signal-scope-external-condition-9"), () => {
          step.prompt("Consume Opaque Guard", { agent: worker, input: input.prompt`Reason: ${sig.reason}` });
        });
      }),
    ).toThrow(
      /"Consume Opaque Guard".*"signal:signal-scope-review-9" field "reason" is readable only inside a flow\.when block guarded on condition\.signal\(signal-scope-review-9\)/,
    );
  });
});
