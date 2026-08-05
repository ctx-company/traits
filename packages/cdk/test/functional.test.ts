// Functional authoring layer (0106): build-rule error paths, one per rule
// named in the task's Watch/Decisions, plus a couple of positive smoke
// checks that don't pin exact JSON shape (the byte-identity proof for that
// lives in `functional.hand.ts`, per the standing 2026-07-24 validation
// ruling — no behavior-freezing tests in the gated suite).
import {
  agent,
  condition,
  effect,
  flow,
  input,
  procedure,
  schema,
  slot,
  step,
  toDraftJson,
  trait,
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
});
