// 0253.7, implemented directly by owner delegation (2026-08-28): the
// plan-approval gate, served by the owner's own plannotator UI. After
// the critic approves an iteration, the run opens the plan in
// plannotator's annotation view (`--gate --json --require-approval`) and
// blocks until the owner decides there:
//   approve  -> exit 0 -> this step emits the literal "approved" and the
//               refinement loop exits; the run commits/merges as usual.
//   annotate -> exit 1 with the decision JSON (annotations included) on
//               stdout -> emitted verbatim as the owner's binding
//               corrections for the next rewrite iteration.
// The plan file is copied to a .md sibling for the annotation view
// (plannotator opens md/txt/html); the copy is removed after the
// decision. The frame budget is the outer bound on how long the owner
// has per iteration. 0253.4's ask machinery may later replace the
// transport; the loop semantics stay.
//
// The `owner-gate` port selects the transport per dispatch: the default
// 'plannotator' parks on the owner's UI; 'off' emits the approval
// immediately so an unattended batch exits the loop on the critic's
// verdict alone. The loop shape is identical either way — only who
// answers changes.
import * as cdk from "@ctx-traits/cdk";

import { ownerAnswer, ownerGate, targetFile } from "../data.ts";

const GATE_SCRIPT = [
  'if [ "$2" = "off" ]; then',
  "  printf approved",
  "  exit 0",
  "fi",
  'd=$(pwd); c="$d/.plan-review.md"',
  'cp "$1" "$c"',
  'if out=$(plannotator annotate "$c" --gate --json --require-approval); then',
  "  printf approved",
  "else",
  '  printf "%s" "$out"',
  "fi",
  'rm -f "$c"',
].join("\n");

export function approvalGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-approval-gate",
    argv: ["sh", "-c", GATE_SCRIPT, "_", targetFile, ownerGate],
    output: ownerAnswer,
  });
}
