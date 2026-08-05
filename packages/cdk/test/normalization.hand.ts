// Normalization property test (0104): two trait sources differing only in
// non-semantic authoring order (declaration order, variable-declaration
// order) must emit byte-identical draft JSON — the ordering/id rules
// `NORMALIZATION.md` documents. Kept out of the gate's vitest glob per the
// standing validation ruling (2026-07-24, no behavior-freezing tests while
// surfaces churn): this asserts the emitter's own ordering contract, not a
// frozen behavior snapshot, but it is deliberately excluded from `pnpm test`
// (the default `vitest run` include glob only matches `*.test.ts`/
// `*.spec.ts`) so it stays hand-runnable instead of gated.
//
// Run by hand: `pnpm exec vitest run --config vitest.hand.config.ts`

import { agent, input, port, procedure, sequence, slot, toDraftJson, trait } from "@ctx-traits/cdk";
import { describe, expect, it } from "vitest";

describe("normalized emission (0104)", () => {
  it("is unaffected by reordering two module-level slot declarations", () => {
    const request = port.input.text({ id: "req" });

    function buildOrderA() {
      const first = slot.text("alpha-slot");
      const second = slot.text("beta-slot");
      return draftFor(request, first, second);
    }

    function buildOrderB() {
      const second = slot.text("beta-slot");
      const first = slot.text("alpha-slot");
      return draftFor(request, first, second);
    }

    expect(JSON.stringify(buildOrderA())).toBe(JSON.stringify(buildOrderB()));
  });

  it("is unaffected by reordering two agent declarations", () => {
    function buildOrderA() {
      const worker = agent("worker", { description: "Does the work." });
      const reviewer = agent("reviewer", { description: "Reviews the work." });
      return draftForAgents(worker, reviewer);
    }

    function buildOrderB() {
      const reviewer = agent("reviewer", { description: "Reviews the work." });
      const worker = agent("worker", { description: "Does the work." });
      return draftForAgents(worker, reviewer);
    }

    expect(JSON.stringify(buildOrderA())).toBe(JSON.stringify(buildOrderB()));
  });

  it("changes the canonical when two steps inside one procedure scope are reordered (correct — positional per 0102)", () => {
    function buildOrder(first: string, second: string) {
      return JSON.stringify(
        toDraftJson(
          trait("step-order", {
            name: "Step Order",
            procedure: procedure({
              description: "Two steps, order matters.",
              sequence: [
                sequence.prompt(first, { text: input.prompt`Step one.` }),
                sequence.prompt(second, { text: input.prompt`Step two.` }),
              ],
            }),
          }),
        ),
      );
    }

    expect(buildOrder("a-step", "b-step")).not.toBe(buildOrder("b-step", "a-step"));
  });
});

function draftFor(request: ReturnType<typeof port.input.text>, first: unknown, second: unknown) {
  return toDraftJson(
    trait("normalization-fixture", {
      name: "Normalization Fixture",
      port: request,
      slot: [first, second] as never,
      procedure: procedure({ description: "No steps.", sequence: [] }),
    }),
  );
}

function draftForAgents(worker: ReturnType<typeof agent>, reviewer: ReturnType<typeof agent>) {
  return toDraftJson(
    trait("normalization-agents-fixture", {
      name: "Normalization Agents Fixture",
      agents: [worker, reviewer],
      procedure: procedure({ description: "No steps.", sequence: [] }),
    }),
  );
}
