// Owner acceptance gate for the written board (owner delegation
// 2026-08-31, same transport as architect): the run's written/modified
// task files are concatenated with '==> path <==' headers and piped to
// `ctx-annotate --stdin`. An empty annotations array accepts the board
// and the loop exits; a non-empty decision is emitted verbatim and the
// correction pass applies each annotation to the named file before the
// next gate iteration. The 'off' port value auto-approves for
// unattended batches.
import * as cdk from "@ctx-traits/cdk";
import { input } from "@ctx-traits/cdk";
import type { AgentHandle } from "@ctx-traits/cdk";

import { ownerAnswer, ownerGate, revisionNote } from "../data.ts";

const GATE_SCRIPT = [
  'if [ "$1" = "off" ]; then',
  "  printf approved",
  "  exit 0",
  "fi",
  "files=$(git status --porcelain .internal/tasks | awk '{print $NF}' | sort)",
  'if [ -z "$files" ]; then',
  "  printf 'annotation gate found no written task files; retrying next iteration'",
  "  exit 0",
  "fi",
  'out=$({ for f in $files; do printf \'==> %s <==\\n\' "$f"; cat "$f"; done; } | ctx-annotate --stdin)',
  'if printf \'%s\' "$out" | python3 -c \'import json,sys; sys.exit(0 if json.load(sys.stdin).get("annotations")==[] else 1)\' 2>/dev/null; then',
  "  printf approved",
  "else",
  '  if [ -n "$out" ]; then printf \'%s\' "$out"; else printf \'annotation gate returned no decision; retrying next iteration\'; fi',
  "fi",
].join("\n");

export function acceptanceGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-acceptance-gate",
    argv: ["sh", "-c", GATE_SCRIPT, "_", ownerGate],
    output: ownerAnswer,
  });
}

export function applyCorrections(agent: AgentHandle, title: string): void {
  agent.prompt(title, {
    id: "apply-owner-corrections",
    input: input.prompt`The owner reviewed the concatenated written task files (each introduced by an '==> path <==' header, in sorted path order) and returned this ctx-annotate decision; its annotations reference line numbers of that concatenated stream and are binding corrections: ${ownerAnswer}

Apply every annotation to the correct file and lines, editing the task files in place. Change nothing an annotation does not ask for; keys, relations and file names stay unless an annotation demands otherwise. Report each annotation and the exact edit made for it.`,
    output: revisionNote,
  });
}
