import type { CheckResultValue, SlotHandle } from "@ctx-traits/cdk";
import { step } from "@ctx-traits/cdk";

export type GateAndDiffEvidenceOptions = {
  /** Output slot the repository gate chain's verdict record (P565: `ok` plus the deciding `argv`) lands in. */
  readonly gatePassed: SlotHandle<CheckResultValue>;
  /** Output slot the scoped changed-file inventory lands in. */
  readonly diff: SlotHandle<string>;
  /** Overrides the default `["just", "test"]` gate invocation. */
  readonly gateArgv?: readonly string[];
  /** Overrides the default `git diff --stat -- . :(exclude).agents/runs` diff invocation. */
  readonly diffArgv?: readonly string[];
};

/**
 * The gate-and-diff evidence pair every implement-family variant runs once
 * per refinement round, before review: the repository gate chain's
 * pass/fail check, then a scoped changed-file inventory excluding runtime
 * state. Functional-layer (0109): calls `step.check`/`step.command`
 * directly into the current build scope, matching `deriveParkReportStep`'s
 * shape — a pre-built `SequenceHandle[]` has no way to be lifted into a
 * functional loop body. Default argv reproduce every existing variant's
 * literal invocation exactly; overriding either is available for a future
 * non-implement consumer, never exercised by implement itself today.
 * @example `gateAndDiffEvidence({ gatePassed: repoGatesPassed, diff: reviewDiff })`
 */
export function gateAndDiffEvidence(options: GateAndDiffEvidenceOptions): void {
  const {
    gatePassed,
    diff,
    gateArgv = ["just", "test"],
    diffArgv = ["git", "diff", "--stat", "--", ".", ":(exclude).agents/runs"],
  } = options;
  step.check("Run the repository gate chain", { id: "repo-gates", argv: gateArgv, output: gatePassed });
  step.command("Capture the changed-file inventory", { id: "capture-diff", argv: diffArgv, output: diff });
}
