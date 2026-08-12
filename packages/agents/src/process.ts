import { condition, flow, input, slot, step } from "@ctx-traits/cdk";
import type { PromptTemplate, SequenceHandle, SlotHandle } from "@ctx-traits/cdk";

/** Functional-layer agent role: the `.prompt(...)` registrar `agent(...)` mints, not the bare `AgentHandle` object-layer callers pass to `sequence.prompt`. */
type FunctionalAgentRole = {
  readonly prompt: (
    title: string,
    opts: { readonly id?: string; readonly input: PromptTemplate; readonly output: SlotHandle },
  ) => SequenceHandle;
};

export { commitTail, deriveParkReportStep, guardedProduction, reviewerVerdict } from "@ctx-traits/toolkit";
export type {
  CommitTailOptions,
  GuardedProductionHandle,
  GuardedProductionOptions,
  GuardedProductionRole,
  ReviewerVerdictValue,
} from "@ctx-traits/toolkit";

/**
 * The clean-tree-guarded git tail, promoted out of implement's own
 * `source/sequence/family.ts` (0153) once a second family package
 * (`research`) needed the identical two-step staging recipe. Functional
 * layer (0109): builds directly into the current `procedure.from` scope via
 * `step.*`/`flow.when`, matching `guardedCommitTail`'s object-layer shape
 * with the family's own two-step staging fix baked in.
 */
export function familyCommitTail(opts: {
  id: string;
  scribe: { readonly agent: FunctionalAgentRole; readonly text: PromptTemplate };
  receipt: SlotHandle<string>;
  /** 0098: routes the commit through an external approval gate — e.g. `ctx-gate run --`. Absent, the commit command is untouched. */
  gate?: { prefix: string; title?: string; timeoutMs?: number };
}): void {
  const { id, scribe, receipt, gate } = opts;
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
  const stageOutput = slot.text({
    id: `${id}-stage-output`,
    description: "Verbatim output of the git-add staging command.",
  });
  const unstageOutput = slot.text({
    id: `${id}-unstage-output`,
    description:
      "Verbatim output of the runtime-state unstage command (git reset -- .agents/runs); empty when nothing was staged there.",
  });

  step.command("Check working tree status", {
    id: `${id}-status`,
    input: input.command`git status --porcelain`,
    output: status,
  });

  flow.when("Shipping Maybe Commit", condition.not(condition.equals(status, "")), () => {
    scribe.agent.prompt("Write the commit message (scribe)", {
      id: `${id}-message`,
      input: scribe.text,
      output: message,
    });
    // Two steps, not one `git add -A -- ':!.agents/runs'` (2026-08-10,
    // run-42bd7fb2): git exits 1 whenever an add PATHSPEC merely mentions a
    // gitignored path that exists — even as an exclusion — so the old
    // spelling failed the whole run the first time a build materialised
    // `.agents/` in a repo that gitignores it. Plain `add -A` skips ignored
    // paths natively; the reset then unstages runtime state in repos where
    // `.agents/runs` is tracked (the exclusion's original purpose), and is a
    // quiet no-op everywhere else.
    step.command("Stage all changes", {
      id: `${id}-stage`,
      input: input.command`git add -A`,
      output: stageOutput,
    });
    step.command("Unstage runtime state", {
      id: `${id}-unstage-runtime`,
      input: input.command`git reset -q -- .agents/runs`,
      output: unstageOutput,
    });
    const commitInput =
      gate === undefined
        ? input.command`git commit -m ${message}`
        : (() => {
            const strings = [`${gate.prefix} git commit -m `, ``];
            return input.command(Object.assign(strings, { raw: strings }) as unknown as TemplateStringsArray, message);
          })();
    step.command(gate?.title ?? "Commit the work", {
      id: `${id}-commit`,
      input: commitInput,
      output: receipt,
      ...(gate?.timeoutMs === undefined ? {} : { timeoutMs: gate.timeoutMs }),
    });
  });
}
