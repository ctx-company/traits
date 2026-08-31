// Owner acceptance gate: open the diagram in the browser, pipe the
// markdown proposal to ctx-annotate. Empty annotations accept; a
// decision with annotations is emitted verbatim as binding corrections.
// 'off' auto-approves for unattended runs.
import * as cdk from "@ctx-traits/cdk";

import { ownerAnswer, ownerGate, slug } from "../data.ts";

const GATE_SCRIPT = [
  'if [ "$2" = "off" ]; then',
  "  printf approved",
  "  exit 0",
  "fi",
  'md=".internal/brainstorms/$1.md"',
  'html=".internal/brainstorms/$1.html"',
  '[ -f "$html" ] && open "$html" >/dev/null 2>&1 || true',
  'if [ ! -f "$md" ]; then',
  "  printf 'annotation gate: %s does not exist; compose must write it' \"$md\"",
  "  exit 0",
  "fi",
  'out=$(ctx-annotate --stdin < "$md")',
  'if printf \'%s\' "$out" | python3 -c \'import json,sys; sys.exit(0 if json.load(sys.stdin).get("annotations")==[] else 1)\' 2>/dev/null; then',
  "  printf approved",
  "else",
  '  if [ -n "$out" ]; then printf \'%s\' "$out"; else printf \'annotation gate returned no decision; retrying next iteration\'; fi',
  "fi",
].join("\n");

export function acceptanceGate(title: string): void {
  cdk.step.command(title, {
    id: "owner-acceptance-gate",
    argv: ["sh", "-c", GATE_SCRIPT, "_", slug, ownerGate],
    output: ownerAnswer,
  });
}
