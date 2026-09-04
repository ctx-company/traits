// Owner annotation gate on every review verdict (owner delegation
// 2026-09-01): after the verdict narration, the scribe's verbatim
// annotation surface is piped to ctx-annotate and the run blocks until
// the owner accepts (no annotations) or rules (any annotations). The
// decision JSON is appended to the run's owner-decisions record for the
// commit message; the reviewer then applies the annotations to its own
// verdict (0281.1) — the worker only ever sees the applied verdict.
// `--raw` makes every annotation carry the exact surface text it was made
// on, which is an exact substring of the verdict because the surface is a
// verbatim copy: that quote is the anchor, so no line map is needed.
// The gate script is the house gate shape (plan/brainstorm lineage):
// positional sh -c, runtime text as argv data, an off-mode for
// unattended runs, and no interpreter beyond sh's own case match.
import * as cdk from "@ctx-traits/cdk";

import { scribe } from "../agent.ts";
import {
  annotations,
  draft,
  gateAnswer,
  gateSurface,
  notifyDigest,
  ownerDecisions,
  ownerGate,
  planAnswer,
  planDigest,
  planGate,
  planSurface,
} from "../data.ts";

const GATE_SCRIPT = [
  'if [ "$2" = "off" ]; then',
  "  printf accepted",
  "  exit 0",
  "fi",
  'out=$(printf \'%s\' "$1" | ctx-annotate --stdin --raw)',
  'case "$out" in',
  '  *\'"annotations":[]\'*) printf accepted ;;',
  '  "") printf \'annotation gate returned no decision; not accepted\' ;;',
  '  *) printf \'%s\' "$out" ;;',
  "esac",
].join("\n");

// A human answers this step, not a process: the wall and idle ceilings
// are declared here because an undeclared command step inherits a
// 120-second default that kills the gate mid-wait ("required output not
// supplied"), which is how two runs died on 2026-09-04. Four hours is the
// machine fallback's command ceiling; the gate stays inside it.
const GATE_CEILING_MS = 4 * 60 * 60 * 1000;

/** Carry the digest's surface into a text slot the gate argv can address. */
export function carrySurface(title: string): void {
  cdk.step.project(title, {
    id: "gate-carry-surface",
    projections: [{ source: notifyDigest, field: "surface", destination: gateSurface }],
  });
}

export function verdictGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-verdict-gate",
    argv: ["sh", "-c", GATE_SCRIPT, "_", gateSurface, ownerGate],
    output: gateAnswer,
    timeoutMs: GATE_CEILING_MS,
    idleTimeoutMs: GATE_CEILING_MS,
  });
}

/** A ruling (any non-accepted gate answer) joins the run's durable owner
 * record, carried into the commit message. */
export function recordRuling(title: string): void {
  cdk.flow.when(
    title,
    cdk.condition.not(cdk.condition.equals(gateAnswer, "accepted")),
    () => {
      cdk.step.project(`${title}: append`, {
        id: "gate-append-ruling",
        projections: [{ source: gateAnswer, destination: ownerDecisions, operation: "append" }],
      });
    },
  );
}

// ---- The plan gate (0281.3): the same shape, pointed at the drafted plan.

/** The scribe copies the drafted plan into the owner's annotation surface. */
export function digestPlan(title: string): void {
  cdk.step.prompt(title, {
    id: "plan-digest",
    agent: scribe,
    input: cdk.input.prompt`
      Digest this drafted plan for the owner's annotation pass: ${draft}.
      Follow the output field's own description exactly.
    `,
    output: planDigest,
  });
}

/** Carry the plan digest's surface into a text slot the gate argv can address. */
export function carryPlanSurface(title: string): void {
  cdk.step.project(title, {
    id: "plan-carry-surface",
    projections: [{ source: planDigest, field: "surface", destination: planSurface }],
  });
}

export function planApprovalGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-plan-gate",
    argv: ["sh", "-c", GATE_SCRIPT, "_", planSurface, planGate],
    output: planAnswer,
    timeoutMs: GATE_CEILING_MS,
    idleTimeoutMs: GATE_CEILING_MS,
  });
}

// ---- The tree lane (implement:annotate): ctx-annotate opens on the run's
// working tree — no --stdin, no surface — and the owner's annotations are
// the task. Same house shape as the gates; no off mode, because in this
// lane the tree view is the whole point. `none` when the owner closes the
// tree without annotating, so the variant can skip the loop.

const TREE_SCRIPT = [
  "out=$(ctx-annotate --raw)",
  'case "$out" in',
  '  *\'"annotations":[]\'*) printf none ;;',
  '  "") printf none ;;',
  '  *) printf \'%s\' "$out" ;;',
  "esac",
].join("\n");

export function treeGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-tree-annotate",
    argv: ["sh", "-c", TREE_SCRIPT, "_"],
    output: annotations,
    timeoutMs: GATE_CEILING_MS,
    idleTimeoutMs: GATE_CEILING_MS,
  });
}

/** A plan correction (any non-accepted plan-gate answer) joins the owner
 * record too; the plan itself is redrafted with it as input. */
export function recordPlanRuling(title: string): void {
  cdk.flow.when(
    title,
    cdk.condition.not(cdk.condition.equals(planAnswer, "accepted")),
    () => {
      cdk.step.project(`${title}: append`, {
        id: "plan-append-ruling",
        projections: [{ source: planAnswer, destination: ownerDecisions, operation: "append" }],
      });
    },
  );
}
