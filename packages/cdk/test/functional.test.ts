// Functional authoring layer (0106): build-rule error paths, one per rule
// named in the task's Watch/Decisions, plus a couple of positive smoke
// checks that don't pin exact JSON shape (the byte-identity proof for that
// lives in `functional.hand.ts`, per the standing 2026-07-24 validation
// ruling — no behavior-freezing tests in the gated suite).
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
  resource,
  schema,
  slot,
  step,
  toDraftJson,
  tone,
  trait,
  useBehavior,
  useIntent,
  useResource,
} from "@ctx-traits/cdk";
import { describe, expect, it } from "vitest";

describe("functional layer build rules (0106)", () => {
  it("a registrar called outside procedure.from throws, naming itself", () => {
    expect(() => step.command("Outside", { cmd: "echo hi" })).toThrow(
      /step\.command\("Outside"\) called outside procedure\.from/,
    );
  });

  it("agent.prompt called outside procedure.from throws", () => {
    const worker = agent.worker("outside-worker");
    expect(() => worker.prompt("Outside Prompt", { input: input.prompt`Do it.` })).toThrow(
      /outside procedure\.from/,
    );
  });

  it("a flow.* block callback that returns a thenable is a build error", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Async Loop", async (loop) => {
          loop.maxIterations(1);
        });
      })
    ).toThrow(/async callbacks are not supported/);
  });

  it("opening a second build while one is active throws", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("A", { cmd: "echo a" });
        procedure.from({ description: "d2" }, () => {
          step.command("B", { cmd: "echo b" });
        });
      })
    ).toThrow(/a functional build is already in progress/);
  });

  it("a second flow.until in one loop scope throws", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Two Untils", (loop) => {
          loop.maxIterations(2);
          step.command("A", { cmd: "echo a" });
          flow.until(condition.empty(slot.text("first-cond-slot")));
          flow.until(condition.empty(slot.text("second-cond-slot")));
        });
      })
    ).toThrow(/at most one flow\.until/);
  });

  it("loop.maxIterations is required — a loop with no way out is a build error", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("No Budget", () => {
          step.command("A", { cmd: "echo a" });
        });
      })
    ).toThrow(/loop\.maxIterations.*is required/);
  });

  it("loop.maxIterations called twice throws", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Double Budget", (loop) => {
          loop.maxIterations(1);
          loop.maxIterations(2);
        });
      })
    ).toThrow(/loop\.maxIterations.*called more than once/);
  });

  it("duplicate titles in one scope throw, naming both titles", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("Same Title", { cmd: "echo a" });
        step.command("Same Title", { cmd: "echo b" });
      })
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
      })
    ).toThrow(/arm "foo" must be a callback/);
  });

  it("par.maxAtOnce is parked: it throws and emits nothing", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.parallel("Parked", (par) => {
          par.maxAtOnce(2);
          step.command("A", { cmd: "echo a" });
        });
      })
    ).toThrow(/par\.maxAtOnce is parked/);
  });

  it("flow.when(..., flow.Abort) outside a loop throws", () => {
    const cap = slot.number("outside-loop-cap");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.when("Give up", condition.gte(cap, 3), flow.Abort);
      })
    ).toThrow(/requires an enclosing flow\.loop/);
  });

  it("effect.onComplete/onAbort outside a loop throw", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        effect.onComplete("some-signal" as never);
      })
    ).toThrow(/requires an enclosing flow\.loop/);
    expect(() =>
      procedure.from({ description: "d" }, () => {
        effect.onAbort("some-signal" as never);
      })
    ).toThrow(/requires an enclosing flow\.loop/);
  });

  it("effect.onFailure always throws — a loop declares no failure of its own to route", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Has No Failure Route", (loop) => {
          loop.maxIterations(1);
          effect.onFailure();
        });
      })
    ).toThrow(/no target here/);
  });

  it("flow.match requires at least one value arm", () => {
    const subject = slot({
      id: "empty-match-subject",
      schema: schema.object("empty-match-scaffold", { kind: schema.text() }),
    });
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.match("Empty Match", subject.kind, {});
      })
    ).toThrow(/requires at least one value arm/);
  });

  it("flow.match registered after flow.until in a loop throws, naming the block — silently unguarded emission is not acceptable", () => {
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
          flow.until(condition.equals(done, true));
          flow.match("Trailing Match", subject.kind, {
            a: () => {
              step.command("A", { cmd: "echo a" });
            },
          });
        });
      })
    ).toThrow(/"Trailing Match".*registered after flow\.until.*cannot be guarded/);
  });

  it("flow.parallel registered after flow.until in a loop throws, naming the block — silently unguarded emission is not acceptable", () => {
    const done = slot.boolean("parallel-after-until-done");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Outer", (loop) => {
          loop.maxIterations(2);
          step.command("First", { cmd: "echo first" });
          flow.until(condition.equals(done, true));
          flow.parallel("Trailing Parallel", () => {
            step.command("A", { cmd: "echo a" });
          });
        });
      })
    ).toThrow(/"Trailing Parallel".*registered after flow\.until.*cannot be guarded/);
  });

  it("a build that throws mid-body still frees the build slot for the next one", () => {
    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("Same Title", { cmd: "echo a" });
        step.command("Same Title", { cmd: "echo b" });
      })
    ).toThrow();

    expect(() =>
      procedure.from({ description: "d" }, () => {
        step.command("Fine", { cmd: "echo fine" });
      })
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
      step.check("Run the repository gate chain", { id: "repo-gates", cmd: "echo ok", output: flagSlot as never });
      worker.prompt("Draft the work (smart-1)", { id: "draft-writing", input: input.prompt`Draft.`, output: out });
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
        flow.until(condition.not(condition.empty(out)));
      });
    });
    const built = toDraftJson(
      trait("id-override-smoke", { name: "Id Override Smoke", summary: "s", procedure: proc }),
    ) as { procedure: { sequence: readonly { id: string; }[]; }; };
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
    ) as { procedure: { sequence: readonly { id: string; kind: string; }[]; }; };
    expect(built.procedure.sequence).toMatchObject([{ id: "park-report-clear", kind: "project" }]);
  });

  it("step.project registered after flow.until in its own loop is a build error", () => {
    const gate = slot.text("project-until-gate");
    const target = slot.text("project-until-target");
    expect(() =>
      procedure.from({ description: "d" }, () => {
        flow.loop("Loop", (loop) => {
          loop.maxIterations(2);
          flow.until(condition.not(condition.empty(gate)));
          step.project("Late Project", { projections: [{ source: gate, destination: target }] });
        });
      })
    ).toThrow(/step\.project registered after flow\.until/);
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
      })
    ).toThrow(/defineTrait: called more than once/);
  });

  it("defineTrait with a bad slug is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("Not_A_Slug");
      })
    ).toThrow(/expected a lowercase slug/);
  });

  it("defineTrait with a computed (non-literal) field is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        // oxlint-disable-next-line -- intentionally not JSON-safe, exercising the build rule.
        defineTrait("computed-field", { summary: (() => "nope") as never });
      })
    ).toThrow(/must be plain literal data/);
  });

  it("defineTrait with an unknown field is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        // oxlint-disable-next-line -- intentionally an unknown field, exercising the build rule.
        defineTrait("unknown-field", { title: "nope" } as never);
      })
    ).toThrow(/unknown field\(s\) title/);
  });

  it("two useBehavior calls setting the same facet throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("behavior-overlap");
        useBehavior({ tone: tone.Direct });
        useBehavior({ tone: tone.Warm });
      })
    ).toThrow(/"tone" was already set/);
  });

  it("useBehavior with an unknown key throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("behavior-unknown-key");
        // oxlint-disable-next-line -- intentionally an unknown field, exercising the build rule.
        useBehavior({ mood: "chipper" } as never);
      })
    ).toThrow(/unknown field\(s\) mood/);
  });

  it("useBehavior with an undefined array entry (an enum typo) throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("behavior-undefined-entry");
        // oxlint-disable-next-line -- intentionally undefined, exercising the enum-typo catch.
        useBehavior({ format: [tone.Direct, undefined] as never });
      })
    ).toThrow(/format\[1\] is undefined/);
  });

  it("useIntent require/avoid contradiction on the same slug throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("intent-contradiction");
        useIntent({ require: [intent("cite-evidence")] });
        useIntent({ avoid: [intent("cite-evidence")] });
      })
    ).toThrow(/"cite-evidence".*declared in both require and avoid/);
  });

  it("two useIntent calls setting the same facet throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("intent-overlap");
        useIntent({ require: [intent("a")] });
        useIntent({ require: [intent("b")] });
      })
    ).toThrow(/"require" was already set/);
  });

  it("useResource with a non-resource value throws", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("resource-not-a-handle");
        // oxlint-disable-next-line -- intentionally not a resource handle, exercising the build rule.
        useResource(slot.text("not-a-resource") as never);
      })
    ).toThrow(/is not a resource handle/);
  });

  it("an unknown ctx.input access throws, listing the declared input port ids", () => {
    expect(() =>
      evaluateTraitFunction((ctx) => {
        defineTrait("unknown-input", { procedure: "p" });
        port.input.text({ id: "diff" });
        step.command("Read Focus", { input: input.command`echo ${ctx.input.focus as never}` });
      })
    ).toThrow(/ctx\.input: unknown input port\(s\) focus.*declared input ports are: diff/);
  });

  it("a declared-but-never-referenced resource is a build error", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("orphan-resource");
        resource.inline("orphan", "Never used.");
      })
    ).toThrow(/declared but never referenced.*resource "orphan"/);
  });

  it("a non-slot return value is a build error naming the key", () => {
    expect(() =>
      evaluateTraitFunction(() => {
        defineTrait("bad-return");
        return { commitReport: "not-a-slot" };
      })
    ).toThrow(/return value "commitReport" must be a slot handle/);
  });

  it("a behavioral trait (no steps) builds a valid draft with no procedure", () => {
    const envelope = evaluateTraitFunction(() => {
      defineTrait("engineering-standards-shape", { summary: "Behavioral guidance only." });
      useBehavior({ tone: tone.Direct });
      useIntent({ require: [intent("cite-evidence")] });
    });
    expect(envelope.draft).toMatchObject({ id: "engineering-standards-shape" });
    expect((envelope.draft as { procedure?: unknown; }).procedure).toBeUndefined();
  });

  it("a procedural trait with declared input and returned output builds a valid draft", () => {
    const envelope = evaluateTraitFunction((ctx) => {
      defineTrait("procedural-shape", { summary: "Reviews a diff.", procedure: "Review a diff." });
      port.input.text({ id: "diff" });
      const review = slot.text("review");
      step.command("Review", { output: review, input: input.command`echo ${ctx.input.diff as never}` });
      return { review };
    });
    expect(envelope.draft).toMatchObject({ id: "procedural-shape" });
    const draft = envelope.draft as { port?: readonly { readonly id: string; readonly direction: string; }[]; };
    const portIds = (draft.port ?? []).map((p) => `${p.id}:${p.direction}`).sort();
    expect(portIds).toEqual(["diff:input", "review:output"]);
  });
});
