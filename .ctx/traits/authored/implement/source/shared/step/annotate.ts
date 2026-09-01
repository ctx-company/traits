// Owner annotation gate on every review verdict (owner delegation
// 2026-09-01): after the verdict narration, the scribe's verbatim
// annotation surface is piped to ctx-annotate and the run blocks until
// the owner accepts (no annotations) or rules (any annotations). The
// decision JSON is appended to the run's owner-decisions record, which
// the reviewer already treats as settled law and the worker now reads.
// The gate script is the house gate shape (plan/brainstorm lineage):
// positional sh -c, runtime text as argv data, an off-mode for
// unattended runs, and no interpreter beyond sh's own case match.
import * as cdk from "@ctx-traits/cdk";

import { gateAnswer, gateSurface, notifyDigest, ownerDecisions, ownerGate } from "../data.ts";

const GATE_SCRIPT = [
  'if [ "$2" = "off" ]; then',
  "  printf accepted",
  "  exit 0",
  "fi",
  'out=$(printf \'%s\' "$1" | ctx-annotate --stdin)',
  'case "$out" in',
  '  *\'"annotations":[]\'*) printf accepted ;;',
  '  *) printf \'%s\' "$out" ;;',
  "esac",
].join("\n");

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
  });
}

/** A ruling (any non-accepted gate answer) joins the run's durable owner
 * record, carried into every later review and the commit message. */
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
