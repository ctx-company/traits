import { TASK_CHECK_DOCTRINE } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { smart } from "../agent.ts";
import { criticVerdict, grounding, ownerAnswer, receipt, target, taskSnapshot } from "../data.ts";

export const inPlace = cdk.defineStep.prompt({
  agent: smart,
  input: cdk.input.prompt(
    `Architect {target}: judge its scope first, then shape the board accordingly (owner ruling 2026-09-01 — architect owns decomposition; plan delivers one parent). Scope target: work one agent run can progress through swiftly. Two legitimate outcomes:
     SPLIT OF ONE (the parent is already that scope): rewrite only target.file in place from draft to a ready execution plan, exactly as before. Create no other file.
     SPLIT (the parent is materially larger): create child TaskDocument TOML files in .internal/tasks/, keyed positionally as <parent-key>.1, <parent-key>.2, ... in dependency order (ordinals continue .10, .11 — never zero-padded, never renumber existing children), each a complete ready execution plan at the swift-run scope with parent = "<parent-key>" in [relations] and depends-on listing only real prerequisite siblings; then rewrite target.file itself as the charter — the shared intent, the umbrella stop condition no single child owns, and the acceptance shape — carrying no execution plan of its own. Split only when a genuine umbrella exists; never split as packaging.
     In both outcomes: modify no file other than target.file and the child files you create. Preserve the parent's filename, key, and raised date; stamp each child's raised date with today's. Keep the parent's relations unchanged unless code reading proves a dependency is wrong; if changed, explain the proof in relations-correction. Change the parent's status from draft to ready and add no execution-plan header.
     Use grounding {grounding} and checksum context {task-snapshot}. The plan separates goals from approach, and must say so in its own text. GOALS are explicit and binding: each Done-when states an observable outcome with a runnable existing command that decides it, plus explicit exclusions. APPROACH is the suggested route, never a boundary: expected files, symbols and signatures, edit order, commands to run, and proof mechanics are the architect's best guidance — stated precisely where the grounding supports them — and the run may deviate from any of it whenever a goal requires it, reporting deviations in the work summary rather than treating them as violations or escalating them. Never pin internal proof mechanics (exact fixtures, matchers, or literal expected output) beyond what the grounding evidences as already occurring: state what must be proven and let the run choose how to observe it. Reuse points stay named. [[checks]] must follow this shared doctrine exactly:\n${TASK_CHECK_DOCTRINE}\nPrior critic verdict, when present: {critic-verdict}. Fix its blockers only.\nOwner corrections, when present, are the owner's binding instructions for this iteration and override every other consideration: {owner-answer}`,
    { target, grounding, "task-snapshot": taskSnapshot, "critic-verdict": criticVerdict.optional(), "owner-answer": ownerAnswer.optional() },
  ),
  output: cdk.output.prompt`Return the typed receipt for this rewrite, recording draft to ready, any permitted relations correction, and — when you split — every child's key and file in order: ${receipt}`,
});
