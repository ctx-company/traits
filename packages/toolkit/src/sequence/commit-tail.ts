import type { AgentHandle, SequenceHandle, SlotHandle, SlotWithFields } from "@ctx-traits/cdk";
import { condition, input, sequence, slot } from "@ctx-traits/cdk";
import type { ReviewerVerdictValue } from "../schema.ts";
import type { GuardedProductionRole } from "./guarded-production.ts";

export type CommitTailOptions = {
  readonly id: string;
  /** Writes the commit message; reads the verdict first (honest-exhaustion doctrine) and the caller's work summary. A bare agent gets the kit's own scribe prose (unchanged); `{ agent, text }` lets the caller own the scribe prompt text entirely — the kit synthesizes none of it in that case. */
  readonly scribe: AgentHandle | GuardedProductionRole;
  /** The caller's work summary, forwarded to the scribe alongside the verdict. */
  readonly summary: SlotHandle<string>;
  /** The verdict the scribe reads before writing the message — typically a `guardedProduction` round's own `.verdict`. */
  readonly verdict: SlotWithFields<ReviewerVerdictValue>;
  /** Caller-declared slot the commit step's output lands in. */
  readonly receipt: SlotHandle<string>;
};

/**
 * The clean-tree-guarded git tail: `git status --porcelain`, and when the
 * tree is non-empty, scribe writes the message (reading the verdict first)
 * → stage → commit. A clean tree skips the whole tail (preserves P397
 * behavior) — no scribe frame, no commit attempt.
 *
 * Returns the tail's ordered steps rather than one named `sequence.linear`
 * block: no existing trait uses a named linear as a top-level procedure
 * item, and `ProcedureFields.sequence` does not accept a
 * `SequenceLinearHandle` there — this is the documented fallback (today's
 * `guardedCommitTail` shape), costing the tail's "named block" property and
 * nothing else.
 * @example `sequence: [...planning, ...building, ...commitTail({ id: "shipping", scribe, summary: workSummary, verdict: building.verdict, receipt: commitOutput })]`
 */
export function commitTail(options: CommitTailOptions): readonly SequenceHandle[] {
  const { id, scribe, summary, verdict, receipt } = options;
  const status = slot.text({
    id: `${id}-status`,
    description:
      "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
    hint: "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and the scribe never runs.",
  });
  const message = slot.text({
    id: `${id}-message`,
    description: "The commit message the scribe writes for the completed work.",
  });
  // The runtime requires exactly one output slot on every command-backed
  // sequence item; nothing reads this evidence today, but the slot is
  // mandatory, not a `guardedCommitTail`-style optional extra.
  const stageOutput = slot.text({
    id: `${id}-stage-output`,
    description: "Verbatim output of the git-add staging command.",
  });
  const statusStep = sequence.command({
    id: `${id}-status`,
    title: "Check working tree status",
    input: input.command`git status --porcelain`,
    output: status,
  });
  const isOwnedScribe = (s: AgentHandle | GuardedProductionRole): s is GuardedProductionRole => "text" in s;
  const ownedScribe = isOwnedScribe(scribe) ? scribe : undefined;
  const scribeAgent: AgentHandle = ownedScribe ? ownedScribe.agent : (scribe as AgentHandle);
  const scribeStep = sequence.prompt(`${id}-message`, {
    title: "Write the commit message (scribe)",
    agent: scribeAgent,
    text: ownedScribe
      ? ownedScribe.text
      : input.prompt`
      Read the reviewer verdict ${verdict} first. If its status is revise, say so plainly and end with an "Unresolved reviewer blockers" section quoting each of its blockers verbatim.
      Write a concise commit message for the completed work based on the work summary ${summary} and the verdict.
      Return exactly that message as your output; the runtime injects it into the git commit step.`,
    output: message,
  });
  const stageStep = sequence.command({
    id: `${id}-stage`,
    title: "Stage all changes except runtime state",
    input: input.command`git add -A -- :!.agents/runs`,
    output: stageOutput,
  });
  const commitStep = sequence.command({
    id: `${id}-commit`,
    title: "Commit the work",
    input: input.command`git commit -m ${message}`,
    output: receipt,
  });
  return [
    statusStep,
    sequence.branch(`${id}-maybe-commit`, {
      check: condition.not(condition.equals(status, "")),
      success: [scribeStep, stageStep, commitStep],
    }),
  ];
}
