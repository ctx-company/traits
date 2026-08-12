// implement-guarded (0098): the quick loop, unchanged, except the final
// commit routes through `ctx-gate run --` and waits for the owner's
// approval before it lands. Not to be confused with `gated` (the existing
// plannotator-based variant) — "guarded" here means an approval gate stands
// between the work and the commit, and this variant adds the owner as one
// more approver on top of the two model reviewers.
import { variant } from "@ctx-traits/cdk";

import { FAMILY_BEHAVIOR, QUICK_INTENT } from "../core.ts";
import { gateTimedOut } from "../sequence/family.ts";
import * as port from "./quick/port.ts";
import * as resource from "./quick/resource.ts";
import { quickProcedure } from "./quick.ts";

export default variant({
  name: "Implement (Guarded)",
  summary:
    "Quick dogfood implementation procedure, with the final commit held for the owner's approval via ctx-gate before it lands.",
  metadata: { tag: ["dogfood", "implementation", "review", "lean", "gated"] },
  behavior: FAMILY_BEHAVIOR,
  intent: QUICK_INTENT,
  resource: [resource.taskBoard],
  signal: [gateTimedOut],
  port: [port.commitReport, port.parkReportPort],
  procedure: quickProcedure({
    prefix: "ctx-gate run --",
    title: "Commit the work (awaiting ctx-gate approval)",
    timeoutMs: 28_800_000,
  }),
});
