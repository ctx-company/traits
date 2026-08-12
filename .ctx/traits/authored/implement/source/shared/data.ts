// Cross-variant primitives every implement variant's build loop shares
// unchanged. Redistributed from source/sequence/family.ts (0183); that
// module re-exports these so unported variants keep compiling until they
// migrate too.
import { condition, schema, signal, slot } from "@ctx-traits/cdk";

export const reviewDiff = slot.text({
  id: "review-diff",
  description:
    "Inventory of every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. The reviewer opens whatever it needs with its own tools.",
  hint: "git diff --stat output, excluding runtime state. No patch bodies: a deleted generated artifact would otherwise contribute its entire content.",
});

// P565: a check reports the verdict AND the argv that produced it. A bare
// boolean cost three runs (P535, P552 twice): handed only `false`, the worker
// re-validated with whatever command the surrounding prose named — and when
// the docs and the declared check disagree, every round proves the wrong
// thing. The argv makes the gate self-describing, so there is no second
// source of truth for "what proves this done".
export const repoGatesPassed = slot({
  id: "repo-gates-passed",
  description:
    "This round's repository gate result: whether the repository gate chain passed, and the exact command that decided it.",
  schema: schema.object("repo-gates-result", {
    ok: schema.field(schema.boolean(), {
      description: "True when the gate command exited successfully.",
    }),
    argv: schema.field(schema.list(schema.text()), {
      description:
        "The exact argv the gate ran. This is the command that decides done-ness — re-run THIS, not a command named anywhere else.",
    }),
    "exit-code": schema.field(schema.number(), {
      required: false,
      description:
        "The gate command's exit status. Absent when the command never produced one, which itself means it could not be executed.",
    }),
    "timed-out": schema.field(schema.boolean(), {
      required: false,
      description:
        "Present and true only when the gate was killed by its own timeout rather than exiting. A timed-out gate proves nothing about the work.",
    }),
    tail: schema.field(schema.text(), {
      required: false,
      description:
        "The end of the failed gate's output — stderr when it said anything, otherwise stdout. Present ONLY when ok is false. This is the reason the gate failed: read it before attributing the failure to anything else, and never infer a cause the tail does not state.",
    }),
  }),
});

/**
 * 0047 mechanism 4: a gate the exit guard forced to `ok=false` purely by
 * timing out is a REPO CONDITION no worker round can fix — the ceiling is
 * fixed in the trait, not a defect in the work. Every family variant's
 * build loop stops on this arm the round a gate times out rather than
 * grinding toward a doomed park (measured: run-f60c3ef5's undeclared
 * check-step timeout). `gateTimedOutAbortIf` is provably mutually exclusive
 * with every `guardedProduction` `until` — each conjoins `repoGatesPassed.ok
 * == true`, which a timed-out gate (`command_execution_succeeded` forces
 * `ok=false` whenever `timed_out` is true) always falsifies — so no
 * `guard-conflict` diagnostic is reachable.
 */
export const gateTimedOut = signal({
  id: "gate-timed-out",
  description: "The repository gate exceeded its declared ceiling — a repo condition no worker round can fix.",
});
export const gateTimedOutAbortIf = condition.fieldEquals(repoGatesPassed, "timed-out", true);
