// 0253.7, implemented directly by owner delegation (2026-08-28): the
// plan-approval gate. After the critic approves an iteration, the run
// parks on a plain filesystem handshake until the owner answers — the
// interim transport until 0253.4's ask machinery replaces it. The step's
// stdout is the owner's answer verbatim: the literal string "approved"
// exits the refinement loop; anything else is treated as binding
// corrections for the next rewrite iteration.
//
// Handshake contract (consumed by the justfile's plan-show /
// approve-plan / revise-plan recipes): the step writes `.plan-await` in
// the run's working directory containing the plan's path, then blocks
// until `.plan-answer` appears, emits its content as the answer, and
// removes both files. A heartbeat line goes to stderr each minute so the
// wait is visibly alive; the frame budget is the outer bound on how long
// the owner has per iteration.
import * as cdk from "@ctx-traits/cdk";

import { ownerAnswer, targetFile } from "../data.ts";

const GATE_SCRIPT = [
  'd=$(pwd); m="$d/.plan-await"; f="$d/.plan-answer"',
  'printf "%s\\n" "$1" > "$m"',
  "i=0",
  'while [ ! -f "$f" ]; do',
  "  i=$((i+1))",
  '  if [ $((i%60)) -eq 0 ]; then echo "plan awaiting owner answer ($((i/60))m): $1" >&2; fi',
  "  sleep 1",
  "done",
  'cat "$f"',
  'rm -f "$f" "$m"',
].join("\n");

export function approvalGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-approval-gate",
    argv: ["sh", "-c", GATE_SCRIPT, "_", targetFile],
    output: ownerAnswer,
  });
}
