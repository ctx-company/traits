// research-deep: ingest the brief's own contract, plan 4-6 streams with a
// mandatory counterevidence stream (deterministic gate) covering every brief
// track, research each stream in its own for-each frame — full reading-notes
// file per stream, typed dossier indexing it — triangulate, then a
// Chain-of-Verification produce round reviewed by two INDEPENDENT reviewers
// each round (dual-approval, mirroring implement:default) — then commit.
// Both verdicts must approve before the commit tail is reachable.
//
// The evidence artery is files + dossiers, not slot prose — see the
// identical note in variant/default/index.ts. This replaces the 0153-era
// single-turn research step and its one campaign-wide findings slot.
import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, operation, step, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Deep", {
    name: "Research (Deep)",
    summary:
      "Deep dogfood research procedure: ingest the brief's contract, plan 4-6 track-covering streams including a mandatory counterevidence stream, research each in its own frame with a notes file per stream, triangulate, apply Chain-of-Verification, then a dual-reviewer refinement loop — then commit.",
    metadata: { tag: shared.metadata.fullTag },
    description:
      "Research one topic exhaustively: extract the brief's tracks and demanded report structure, plan 4-6 streams (counterevidence mandatory) covering every track, research each stream in its own frame (full reading notes on disk, typed dossier in the ledger), triangulate, apply Chain-of-Verification, then a bounded dual-reviewer refinement loop, then commit report.md, bibliography.md, evidence.csv, and verification.md following the brief's own structure.",
  });
  useBehavior(shared.behavior.family);
  useIntent(shared.intent.full);

  const smart1 = reviewerRole(
    "smart-1",
    "Ingests the brief, plans the research streams, and independently reviews the delivered research each refinement pass.",
    "Planning and first-reviewer role.",
  );
  const smart2 = reviewerRole(
    "smart-2",
    "Independently reviews the delivered research each refinement pass, separately from smart-1.",
    "Second-reviewer role.",
  );
  const worker = workerRole(
    "researcher",
    "Researches each stream in its own frame, triangulates, drafts and verifies the report, and applies reviewer fixes.",
  );
  const scribe = scribeRole("scribe", "Writes the commit message for the completed research from the plan and verdicts");

  const streamCountValid = condition.all([
    condition.any([
      condition.count(shared.data.streamPlan).equals(4),
      condition.count(shared.data.streamPlan).equals(5),
      condition.count(shared.data.streamPlan).equals(6),
    ]),
    condition.count(shared.data.streamPlan).where("kind", "counterevidence").atLeast(1),
  ]);

  shared.stage.derive.deriveTopicSlugStep();
  shared.stage.derive.deriveReportPathStep("report.md");

  flow.loop("Ingest", (loop) => {
    loop.maxIterations(3, { onExhausted: "abort" });

    smart1.prompt("Ingest the brief", {
      input: input.prompt`
                Read the research brief for ${shared.data.topic}. When the topic names a brief file, read that file from the repository; otherwise the topic text itself is the brief.
                Extract three things, faithful to the brief's own words — never your preferred framing:
                1. Its research tracks: every enumerated track or area the brief wants investigated, with the brief's own question for each. When the brief does not enumerate tracks, distill them from its prose.
                2. The deliverable structure it demands for the final report: one entry per required top-level section, in the brief's order. When the brief specifies no structure, propose one that fits its content — but when it does specify one, reproduce it exactly.
                3. Its explicit definition-of-done items, empty when it states none.
                Write nothing to disk in this step — later steps own every file this run produces.`,
      output: [shared.data.briefTracks, shared.data.deliverableSections, shared.data.doneCriteria],
      include: [
        shared.data.briefTracks.optional(),
        shared.data.deliverableSections.optional(),
        shared.data.doneCriteria.optional(),
      ],
    });

    flow.until(
      condition.all([
        condition.count(shared.data.briefTracks).atLeast(1),
        condition.count(shared.data.deliverableSections).atLeast(1),
      ]),
    );
  });

  flow.loop("Planning", (loop) => {
    loop.maxIterations(3, { onExhausted: "abort" });

    smart1.prompt("Plan the streams", {
      input: input.prompt`
                Plan four to six non-overlapping research streams for ${shared.data.topic}, from the brief's tracks ${shared.data.briefTracks}. Each stream gets a stable id, a complete non-overlapping focus question, a kind ("primary" or "counterevidence"), and covers: the brief-track ids it is responsible for. Every track MUST appear in at least one stream's covers; a stream may own several related tracks.
                At least one stream MUST be kind "counterevidence": its focus is deliberately searching for disconfirming evidence and competing conclusions against the other streams' emerging findings — not merely a skeptical framing of a primary question — and its covers lists the tracks whose conclusions it challenges.
                This is the run's one and only executable stream list, consumed directly by per-stream research.`,
      output: shared.data.streamPlan,
      include: [shared.data.streamPlan.optional()],
    });

    flow.until(streamCountValid);
  });

  // Seeds the guaranteed empty dossier list the per-stream loop appends
  // into — see the identical note in variant/default/index.ts.
  step.project("Seed the dossiers", {
    id: "seed-dossiers",
    projections: [{ source: operation.literal([]), destination: shared.data.dossiers }],
  });

  shared.data.streamPlan.forEach("Research each stream", { maxItems: 6 }, (stream) => {
    worker.prompt("Research the stream", {
      input: input.prompt`
                Research exactly one stream — ${stream} — for the topic ${shared.data.topic}. Do not research any other stream's focus; other streams run in their own frames. For a "counterevidence" stream, actively search for disconfirming evidence and competing conclusions against the tracks it covers rather than corroborating them.
                Apply ${shared.resource.researchStandards}: cite every factual claim, rate sources per the canonical A-E scale (${shared.resource.sourceQualityGuide}), format citations per ${shared.resource.citationStyle}, and note any counterevidence encountered.
                As you read, accumulate full reading notes in ${shared.data.outputDir}/${shared.data.topicSlug}/notes/<stream-id>.md (create directories as needed): quotes with attribution, per-source assessments, dead ends, and everything a later writer needs to reconstruct your reasoning. Write the notes file incrementally as you research — it is the stream's evidence corpus, never an after-the-fact summary.
                Return the stream's dossier: its stream-id, the notes file's repo-relative path, a cited summary, key claims (each with rating and citation inline), one sources entry per consulted source ("<A-E> | <citation or url> | <what it supports>"), and open questions.`,
      output: operation.over(shared.data.dossiers, operation.Append),
    });
  });

  worker.prompt("Triangulate the findings", {
    input: input.prompt`
                Triangulate the researched streams for ${shared.data.topic}: read every dossier in ${shared.data.dossiers} and every notes file they point at, then report where independent streams agree, where they conflict — including what the counterevidence stream surfaced — source-quality asymmetries (claims resting on materially weaker evidence than their neighbors), and remaining gaps.
                Return the triangulation.`,
    output: shared.data.triangulation,
  });

  flow.loop("Building", (loop) => {
    loop.maxIterations(8, { onExhausted: "abort" });

    worker.prompt("Building Produce", {
      input: input.prompt`
                    Produce the research delivery for ${shared.data.topic} from the dossiers ${shared.data.dossiers} and triangulation ${shared.data.triangulation} — reading each dossier's notes file for full depth. The dossiers are the index; the notes files are the evidence. Never write a section from dossier summaries alone.
                    Write under ${shared.data.outputDir}/${shared.data.topicSlug}/ (a flat directory apart from notes/ — no numbered subfolders, no manifests): report.md, bibliography.md, evidence.csv (claim, source, rating, url), and verification.md. report.md's top-level sections MUST be exactly ${shared.data.deliverableSections}, in that order — the structure is the brief's to dictate. Satisfy every entry of ${shared.data.doneCriteria}.
                    Apply the Chain-of-Verification method from ${shared.resource.researchStandards} to every critical claim before writing verification.md: generate the claim, write a verification question for it, independently answer that question against the cited sources, and revise the claim (or drop it) based on that answer. verification.md records each critical claim, its verification question, the independent answer, and the disposition.
                    No reviewer verdict attached means this is round 1: implement the full delivery. On every later round two independent verdicts ARE attached — fix every blocker either names.
                    Return a work summary: what was written, how citations and critical claims were verified, open concerns.`,
      output: shared.data.workSummary,
      include: [shared.data.verdict1.optional(), shared.data.verdict2.optional(), shared.data.workSummary.optional()],
    });

    smart1.prompt("Building Review 1", {
      input: input.prompt`
                    Review the research delivered for ${shared.data.topic} against the brief's contract and the plan ${shared.data.streamPlan}. Work summary: ${shared.data.workSummary}.
                    A BLOCKER is: a report.md whose top-level sections do not match ${shared.data.deliverableSections} one-to-one in order; a brief track in ${shared.data.briefTracks} the delivery leaves uncovered; an unmet entry of ${shared.data.doneCriteria}; a plan stream with no dossier or no readable notes file; a section written at dossier-summary depth when its notes file carries materially more evidence; an uncited factual claim; a critical claim without a recorded Chain-of-Verification pass in verification.md; an unaddressed cross-stream contradiction; a source below C-rating used as sole support for a critical claim; or a file written outside ${shared.data.outputDir}/${shared.data.topicSlug}/ (notes/ inside it is part of the layout). Everything else is advisory.
                    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                    Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict1,
      include: [shared.data.verdict1.optional()],
    });
    smart2.prompt("Building Review 2", {
      input: input.prompt`
                    Independently review the research delivered for ${shared.data.topic} against the brief's contract and the plan ${shared.data.streamPlan}. Work summary: ${shared.data.workSummary}. Do not trust the work summary or smart-1's verdict — inspect the written deliverables, the notes files, and cited sources yourself.
                    A BLOCKER is: a report.md whose top-level sections do not match ${shared.data.deliverableSections} one-to-one in order; a brief track in ${shared.data.briefTracks} the delivery leaves uncovered; an unmet entry of ${shared.data.doneCriteria}; a plan stream with no dossier or no readable notes file; a section written at dossier-summary depth when its notes file carries materially more evidence; an uncited factual claim; a critical claim without a recorded Chain-of-Verification pass in verification.md; an unaddressed cross-stream contradiction; a source below C-rating used as sole support for a critical claim; or a file written outside ${shared.data.outputDir}/${shared.data.topicSlug}/ (notes/ inside it is part of the layout). Everything else is advisory.
                    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                    Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict2,
      include: [shared.data.verdict2.optional()],
    });


    flow.until(
      condition.all([
        condition.equals(shared.data.verdict1.status, "approved"),
        condition.equals(shared.data.verdict2.status, "approved"),
      ]),
    );
  });

  shared.stage.commit.shippingCommitTail({
    agent: scribe,
    text: input.prompt`
                The dual review for the research on ${shared.data.topic} has ended in agreement and the work is being committed. The final verdicts are ${shared.data.verdict1} and ${shared.data.verdict2} (unreachable while either status is still revise — an exhausted loop aborts with its stop reason and never reaches here).
                Write a concise commit message from the work summary ${shared.data.workSummary} and those verdicts: a short subject line naming the topic, then one paragraph on what was researched, verified, and delivered at ${shared.data.reportPath}.
                Return exactly that message. Do not run git commands and do not write files.`,
  });

  return {
    commitReport: shared.data.commitReport,
    researchReportPort: shared.data.researchReportPort,
    reportPathPort: shared.data.reportPathPort,
  };
}
