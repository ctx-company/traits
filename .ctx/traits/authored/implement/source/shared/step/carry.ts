// Deterministic carries (0281.7): the values a worker or a command needs
// are copied out of the reviewer's verdict by the runtime's project step —
// no model in between, so the brief the worker reads is byte-for-byte the
// reviewer's, and the proof command the runtime runs is byte-for-byte the
// plan's. Argv takes whole slot refs only, hence the text slot for the
// proof command; the brief keeps its own object schema so the worker's
// frame renders its field descriptions beside the values.
import * as cdk from "@ctx-traits/cdk";

import { brief, proofCommand, verdict1 } from "../data.ts";

export function carryBrief(title: string): void {
  cdk.step.project(title, {
    id: "carry-brief",
    projections: [
      { source: verdict1, field: "brief", destination: brief },
      { source: verdict1, field: "brief.proof", destination: proofCommand },
    ],
  });
}
