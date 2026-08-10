import type { JsonValue, SlotHandle } from "@ctx-traits/cdk";
import { condition, flow, operation, step } from "@ctx-traits/cdk";

/**
 * Deterministic, runtime-only derivation of this round's typed park-report
 * entries (P414) from each given verdict's own accepted value — never a
 * second, model-authored classification, so a mismatched
 * blocker/wall-id/escalation can never be accepted (park-report simply is
 * not independent model output any more). A `project` step can only copy a
 * whole source value (`replace`/`append`) — the CDK rejects
 * `set-field`/`merge` for `project` (those exist only for a prompt/command
 * step's own `output:` sink) — so this never builds park-report
 * field-by-field; it copies each verdict's own accepted value onto
 * `parkReportSlot` UNCHANGED, in full. Every round: `park-report-clear`
 * replaces `parkReportSlot` with `[]` unconditionally; one
 * `park-report-record` step per given verdict then runs only when that
 * verdict's own status is `revise`, appending its whole value onto the
 * (now-empty, or already-appended-to) list — so runtime approval being a
 * conjunction of every given verdict is reflected in the park report as a
 * conjunction too: a single still-revise verdict among several always
 * survives into the park report, even when a later verdict in the same
 * round is approved. This is why `parkReportSlot`'s declared schema must be
 * `schema.list(<verdict's own schema>)` — the SAME schema as each verdict,
 * not a separately hand-declared shape, so the whole-value append always
 * validates. Callers pass one verdict (a single-review procedure) or every
 * verdict reviewed this round (a dual-review procedure) through the same
 * helper shape.
 *
 * Functional-layer (0109): calls `step.project`/`flow.when` directly into
 * the current build scope instead of returning handles — a pre-built
 * `SequenceHandle[]` has no way to be lifted into a functional loop body.
 * Every id/title below is auto-derived from its own title text
 * (`idFromTitle`), so no `id:` override is needed here.
 * @example `deriveParkReportStep([verdict1, verdict2], { parkReportSlot: parkReport })`
 */
export function deriveParkReportStep(
  verdicts: SlotHandle | SlotHandle[],
  opts: { parkReportSlot: SlotHandle<JsonValue[]>; },
): void {
  const targetParkReport = opts.parkReportSlot;
  const verdictList = Array.isArray(verdicts) ? verdicts : [verdicts];
  step.project("Park Report Clear", {
    projections: [{ source: operation.literal([]), destination: targetParkReport }],
  });
  verdictList.forEach((verdict, index) => {
    const titleSuffix = verdictList.length > 1 ? ` ${index + 1}` : "";
    flow.when(`Park Report Record${titleSuffix}`, condition.fieldEquals(verdict, "status", "revise"), () => {
      step.project(`Park Report Append${titleSuffix}`, {
        projections: [{ source: verdict, destination: operation.over(targetParkReport, operation.Append) }],
      });
    });
  });
}
