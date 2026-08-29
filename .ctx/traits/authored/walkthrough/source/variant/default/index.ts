// walkthrough-default (0.2, exhaustive): the lowest level ends at code, and
// completeness is ARITHMETIC, not judgment. A survey proposes the topic's
// file closure and the upper tree; symbols.py deterministically enumerates
// every symbol in that closure (ground truth the model cannot wiggle out
// of); one for-each frame per file describes EVERY enumerated symbol,
// appending its batch; the covering loop cannot exit while the computed
// uncovered set is non-empty; a reviewer then samples quality; revisions are
// append-only, last id wins. The render tail is deterministic. No commit
// tail — the artifact lands gitignored under .hidden/walkthroughs, so runs
// want --no-worktree (the artifact strands in an isolation worktree).
import { reviewerRole, workerRole } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, intent, operation, signal, step, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Walkthrough (Default)",
    summary:
      "Exhaustive codebase walkthrough: survey the topic's file closure, deterministically enumerate every symbol in it, describe every single one in per-file frames under a computed coverage gate, review a sample for quality, then render a self-contained interactive treemap HTML.",
    metadata: { tag: shared.metadata.defaultTag },
    description:
      "Explain one codebase topic top-to-bottom, ending at code: a survey fixes the dependency-closure file set and the upper tree; a deterministic script enumerates every fn/struct/enum/trait/mod in those files; per-file frames describe each enumerated symbol (typed nodes, verbatim coverage keys, verified line spans); a coverage loop grinds until the computed uncovered set is empty; a reviewer samples grounding and register; a command step renders the zoomable treemap walkthrough.",
  });
  useBehavior(shared.behavior.family);
  useIntent({
    require: [intent.Correctness, intent.Leanness, intent.ReviewBeforeFinal],
    avoid: [intent.ScopeCreep, intent.RubberStampReview],
  });

  const investigator = workerRole(
    "investigator",
    "Explores the repository with its own tools, fixes the closure, and describes every enumerated symbol.",
  );
  const reviewer = reviewerRole(
    "smart-1",
    "Sole reviewer: samples refs against the actual files and judges register and horizon honesty — never re-litigates computed coverage.",
    "Review role.",
  );

  shared.step.derive.deriveTopicSlugStep();
  shared.step.derive.deriveHtmlPathStep();

  flow.loop("Surveying", (loop) => {
    loop.maxIterations(3, { onExhausted: signal.Abort });

    investigator.prompt("Survey the topic", {
      input: input.prompt`
                Survey ${shared.data.topic} in THIS repository with your own tools (search, read files). Follow ${shared.resource.walkthroughStandards} exactly. Deliver three things.
                1. The FILE CLOSURE: every source file the topic's code lives in, plus every same-workspace file it directly or transitively depends on (follow imports/use declarations), stopping only at the horizon. Each entry: the repo-relative path (verify it exists) and one sentence why it belongs. This list is the coverage contract — a deterministic script will enumerate EVERY symbol in these files and the run cannot finish until every one is described. The run refuses closures beyond roughly a thousand symbols: prefer the topic's owning files plus first-degree collaborators, and push deep transitive dependencies to the horizon — a wide neighbor earns a horizon sentence, not a closure entry.
                2. The HORIZON: plain prose naming where traversal deliberately stopped and why (std/third-party, unrelated subsystems, generated code). Honesty here is part of the deliverable.
                3. The SKELETON NODES: the upper tree only — one root (id "root", kind "overview", no parent) narrating architecture and intent; area nodes for the module groupings; ONE file node per closure entry with kind "file", parent set to its area (or the root), and id "f-" plus the path slugified (lowercase, every character outside a-z0-9 becomes "-", runs collapsed, ends trimmed — e.g. modules/io/src/mcp.rs becomes f-modules-io-src-mcp-rs); optional flow nodes for cross-cutting paths. NO type or function nodes here — those are produced per-file later. Every node carries verified refs.
                On a later round your previous closure and skeleton are attached — extend and correct them rather than starting over.
                READ-ONLY discipline: never create, edit, or delete any file in this repository — investigation means reading. Your ONLY deliverable is this step's structured output; submit it directly, never write it to a file, never wrap it in prose or explanation.`,
      output: [shared.data.fileClosure, shared.data.horizon, shared.data.skeletonNodes],
      include: [shared.data.fileClosure.optional(), shared.data.horizon.optional(), shared.data.skeletonNodes.optional()],
    });

    loop.until(
      condition.all([condition.count(shared.data.fileClosure).atLeast(1), condition.count(shared.data.skeletonNodes).atLeast(2)]),
    );
  });

  shared.step.render.enumerateSymbolsStep();

  // Seeds the guaranteed empty batch list the per-file loop appends into:
  // production inside a for-each body is only "possible" to the validator,
  // so the guaranteed producer every later reader needs sits before it.
  step.project("Seed the batches", {
    id: "seed-batches",
    projections: [{ source: operation.literal([]), destination: shared.data.nodeBatches }],
  });

  shared.data.symbolChunks.forEach("Describe each chunk", (chunk, loop) => {
    loop.limit(96);
    investigator.prompt("Describe the chunk", {
      input: input.prompt`
                Describe exactly one symbol chunk — ${chunk} — for the walkthrough of ${shared.data.topic}. The chunk names its file and lists the enumerated symbols you owe; other chunks run in their own frames.
                OPEN the chunk's file and emit EXACTLY ONE node per listed symbol — the batch's nodes length must equal the chunk's symbols length; no symbol may be skipped, merged, or invented. Each entry's key is path:line:name — read the symbol's name and 1-indexed definition line out of it. For each: id "s-" plus the symbol name plus "-" plus its line (kebab-case); parent = the file's node id ("f-" plus the slugified path: lowercase, non-alphanumerics to "-", runs collapsed, ends trimmed); kind copied from the entry (function or type); symbol = the entry's key copied VERBATIM — coverage joins on this exact string; refs = one span from the definition line to its real end, read from the file; summary one glanceable sentence; explanation the precise mechanics at leaf register per ${shared.resource.walkthroughStandards} — one tight paragraph for small items, up to three for load-bearing ones.
                Never emit null for any field; omit optional fields entirely when unused. Return only this chunk's nodes.
                READ-ONLY discipline: never create, edit, or delete any file in this repository — investigation means reading. Your ONLY deliverable is this step's structured output; submit it directly, never write it to a file, never wrap it in prose or explanation.`,
      output: shared.data.nodeBatches.with(operation.Append),
    });
  });

  flow.loop("Covering", (loop) => {
    loop.maxIterations(8, { onExhausted: signal.Abort });

    shared.step.render.coverageReportStep("loop");

    investigator.prompt("Cover the gaps", {
      input: input.prompt`
                The deterministic coverage report for the walkthrough of ${shared.data.topic} is ${shared.data.coverageReport}.
                If it shows zero uncovered entries, zero unknown symbol keys, and zero orphan parents with exactly one root: return an empty batch (nodes: []) and nothing else.
                Otherwise return one batch making progress on what it names, AT MOST 40 nodes this round — the loop runs again for the rest. Priority order: first corrected re-emissions for nodes with unknown symbol keys or orphaned parents (re-emitting an id replaces that node — keep its content, fix the broken field, never drop a valid symbol key); then a node per uncovered entry, same rules as chunk description — verbatim symbol key, verified span, leaf register per ${shared.resource.walkthroughStandards}. Never emit null for any field.
                READ-ONLY discipline: never create, edit, or delete any file in this repository — investigation means reading. Your ONLY deliverable is this step's structured output; submit it directly, never write it to a file, never wrap it in prose or explanation.`,
      output: shared.data.nodeBatches.with(operation.Append),
    });

    shared.step.render.coverageStatusStep("loop");

    loop.until(condition.equals(shared.data.coverageStatus, "complete"));
  });

  flow.loop("Reviewing", (loop) => {
    loop.maxIterations(3, { onExhausted: signal.Abort });

    reviewer.prompt("Review the walkthrough", {
      input: input.prompt`
                Review the walkthrough of ${shared.data.topic}: skeleton ${shared.data.skeletonNodes}, horizon ${shared.data.horizon}, and the described batches in ${shared.data.nodeBatches} (flatten in order; a later node with a seen id replaces the earlier one). Coverage is COMPUTED and already complete — do not re-litigate it.
                Verify with your OWN tools on a meaningful sample (at least a dozen symbol nodes across different files, plus every skeleton node): open the ref, confirm the span covers the symbol it claims, confirm the explanation matches the actual code and sits at its layer's register per ${shared.resource.walkthroughStandards}, and confirm the horizon note honestly matches where the closure stops.
                A BLOCKER is: a span that does not cover its symbol, an explanation contradicting the code or at the wrong register, a skeleton node ungrounded, or a horizon claim the closure contradicts. Everything else is advisory. Name every blocker with the node id.
                Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                A blocker must be FIXABLE BY THIS LOOP: by re-emitting nodes, or by rewriting the horizon note. Never raise a blocker that demands changes to the repository, to the closure itself, or exact command-output inventories over source files. When the same blocker survives two fix rounds textually unchanged, stop repeating it: downgrade it to advisory, set escalation to needs-owner with the reason, and judge the remaining state on its merits.
                Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict1,
      include: [shared.data.verdict1.optional()],
    });

    flow.when("Apply review fixes", condition.fieldEquals(shared.data.verdict1, "status", "revise"), () => {
      investigator.prompt("Fix reviewed nodes", {
        input: input.prompt`
                The reviewer's verdict for the walkthrough of ${shared.data.topic} is ${shared.data.verdict1}. For every blocker it names, re-emit the corrected node in one batch: same id (re-emission replaces), same VERBATIM symbol key where the node had one (coverage must not regress), the named defect actually fixed against the real file. Nodes without blockers are not re-emitted.
                Also return the horizon note: REWRITTEN when a blocker names the horizon (the current note is ${shared.data.horizon}), byte-identical otherwise — the horizon renders into the walkthrough, and this step is the only place review fixes can reach it.
                Follow ${shared.resource.walkthroughStandards}.
                READ-ONLY discipline: never create, edit, or delete any file in this repository — investigation means reading. Your ONLY deliverable is this step's structured output; submit it directly, never write it to a file, never wrap it in prose or explanation.`,
        output: [shared.data.nodeBatches.with(operation.Append), shared.data.horizon],
      });
    });

    loop.until(condition.fieldEquals(shared.data.verdict1, "status", "approved"));
  });

  shared.step.render.coverageStatusStep("final");

  investigator.prompt("Summarize the walkthrough", {
    input: input.prompt`
                Summarize the delivered walkthrough of ${shared.data.topic} in a few plain sentences: the closure size and horizon, how coverage was driven to complete, how refs were verified, and open concerns.`,
    output: shared.data.workSummary,
    include: [shared.data.horizon.optional(), shared.data.workSummary.optional()],
  });

  shared.step.render.renderWalkthroughStep();

  return {
    walkthroughPathPort: shared.data.walkthroughPathPort,
    walkthroughSummaryPort: shared.data.walkthroughSummaryPort,
    coveragePort: shared.data.coveragePort,
    renderReportPort: shared.data.renderReportPort,
  };
}
