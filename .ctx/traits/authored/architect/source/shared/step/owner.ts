// Owner plan-approval gate, served by the owner's ctx-annotate tool
// (owner delegation 2026-08-31; supersedes the plannotator transport of
// 2026-08-28). After the critic approves an iteration, the plan file is
// piped to `ctx-annotate --stdin`, which returns a JSON decision:
//   {"source":{"kind":"stdin"},"annotations":[]}      -> accepted: this
//     step emits the literal "approved" and the refinement loop exits;
//     the run commits/merges as usual.
//   {"source":...,"annotations":[{"lines":[a,b],"text":...},...]} ->
//     emitted verbatim as the owner's binding corrections for the next
//     rewrite iteration; line ranges refer to the plan file as piped.
// A gate-tool failure is surfaced as a non-approval with the captured
// output so the loop continues visibly rather than dying silently.
// Since the split authority (2026-09-01) the surface is every changed
// task file — parent and children — concatenated under ==> headers, so
// the owner annotates the whole family in one pass; a run that touched
// only the parent pipes just the parent, byte-identical to before.
// The frame budget is the outer bound on how long the owner has per
// iteration. 0253.4's ask machinery may later replace the transport;
// the loop semantics stay.
//
// The `owner-gate` port selects the transport per dispatch: the default
// 'annotate' pipes to the owner's ctx-annotate; 'off' emits the
// approval immediately so an unattended batch exits the loop on the
// critic's verdict alone. The loop shape is identical either way —
// only who answers changes.
import * as cdk from "@ctx-traits/cdk";

import { ownerAnswer, ownerGate, targetFile } from "../data.ts";

const GATE_SCRIPT = [
  'if [ "$2" = "off" ]; then',
  "  printf approved",
  "  exit 0",
  "fi",
  'files=$(git status --porcelain .internal/tasks | sed "s/^...//" | grep -v "^$" || true)',
  '[ -n "$files" ] || files="$1"',
  'out=$(for f in $files; do printf "==> %s <==\\n" "$f"; cat "$f"; printf "\\n"; done | ctx-annotate --stdin)',
  'if printf \'%s\' "$out" | python3 -c \'import json,sys; sys.exit(0 if json.load(sys.stdin).get("annotations")==[] else 1)\' 2>/dev/null; then',
  "  printf approved",
  "else",
  '  if [ -n "$out" ]; then printf \'%s\' "$out"; else printf \'annotation gate returned no decision; retrying next iteration\'; fi',
  "fi",
].join("\n");

export function approvalGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-approval-gate",
    argv: ["sh", "-c", GATE_SCRIPT, "_", targetFile, ownerGate],
    output: ownerAnswer,
  });
}
