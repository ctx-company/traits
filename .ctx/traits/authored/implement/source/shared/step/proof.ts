// The stage's mechanical proof (0281.7): when the worker claims the stage
// complete, the runtime runs the stage's proof command — copied verbatim
// from the plan through the verdict's brief — and the loop only reaches
// the reviewer on a claim that passed. A failing proof goes back to the
// worker with the command's exit code and output tail; no model reads it
// first, no review is spent on it. `none` (a stage with nothing mechanical
// to check) passes by definition, so its claims go straight to review.
//
// House gate shape: positional sh -c, runtime text as argv data, sh's own
// case match as the only interpreter.
import * as cdk from "@ctx-traits/cdk";

import { proofCommand, proofResult } from "../data.ts";

const PROOF_SCRIPT = [
  'case "$1" in',
  '  none|"") printf pass; exit 0 ;;',
  "esac",
  'out=$(sh -c "$1" 2>&1)',
  "code=$?",
  'if [ "$code" -eq 0 ]; then',
  "  printf pass",
  "else",
  "  printf 'fail (exit %s)\\n%s' \"$code\" \"$(printf '%s' \"$out\" | tail -c 6000)\"",
  "fi",
].join("\n");

// A proof may be a test suite: the ceilings are those of a build, not of a
// gate. An undeclared command step inherits a 120-second default that
// would kill any real suite mid-run.
const PROOF_CEILING_MS = 45 * 60 * 1000;

export function run(title: string): void {
  cdk.step.command(title, {
    id: "prove-the-claim",
    argv: ["sh", "-c", PROOF_SCRIPT, "_", proofCommand],
    output: proofResult,
    timeoutMs: PROOF_CEILING_MS,
    idleTimeoutMs: PROOF_CEILING_MS,
  });
}
