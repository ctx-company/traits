// research-default: ingest the brief's own contract (tracks, demanded report
// structure, done criteria), plan a typed, cardinality-gated stream list
// that covers every track, research each stream in its own for-each frame —
// writing a full reading-notes file per stream and returning a typed dossier
// that indexes it — triangulate, then grind a single-reviewer loop until
// approved — then commit. The plan's cardinality (2-5 streams) is a
// deterministic gate on the typed slot, never an agent judgment.
//
// The evidence artery is files + dossiers, not slot prose: each stream's
// notes file carries the corpus (written incrementally during the research
// turn, unbounded by what one model answer can return), and its dossier slot
// carries the typed index. The produce step reads the notes files for depth;
// the dossiers alone are never treated as the evidence. This replaces the
// 0153-era single-turn research step, whose one findings slot compressed an
// entire campaign into one model answer (~18KB observed) before the report
// was written.
import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, operation, signal, step, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("Default", {
    name: "Research (Default)",
    summary:
      "Surveyed dogfood research procedure: ingest the brief's contract, plan a track-covering set of streams, research each in its own frame with a notes file per stream, triangulate, then a bounded reviewer loop — then commit.",
    metadata: { tag: shared.metadata.fullTag },
    description:
      "Research one topic end to end: extract the brief's tracks and demanded report structure, plan 2-5 non-overlapping streams covering every track, research each stream in its own frame (full reading notes on disk, typed dossier in the ledger), triangulate, then a bounded single-reviewer refinement loop, then commit report.md, bibliography.md, and evidence.csv following the brief's own structure.",
  });
  useBehavior(shared.behavior.family);
  useIntent(shared.intent.full);

  const smart = reviewerRole(
    "smart-1",
    "Ingests the brief, plans the research streams, and is the sole reviewer of the delivered research.",
    "Planning and reviewer role.",
  );
  const worker = workerRole(
    "researcher",
    "Researches each stream in its own frame, triangulates, drafts the report, and applies reviewer fixes.",
  );
  const scribe = scribeRole("scribe", "Writes the commit message for the completed research from the plan and verdict");

  const streamCountValid = condition.any([
    condition.count(shared.data.streamPlan).equals(2),
    condition.count(shared.data.streamPlan).equals(3),
    condition.count(shared.data.streamPlan).equals(4),
    condition.count(shared.data.streamPlan).equals(5),
  ]);

  shared.stage.derive.deriveTopicSlugStep();
  shared.stage.derive.deriveReportPathStep("report.md");

  flow.loop("Ingest", (loop) => {
    loop.maxIterations(3, { onExhausted: signal.Abort });

    smart.prompt("Ingest the brief", {
      input: input.prompt`
                Read the research brief for ${shared.data.topic}. When the topic names a brief file, read that file from the repository; otherwise the topic text itself is the brief.
                Extract three things, faithful to the brief's own words — never your preferred framing:
                1. Its research tracks: every enumerated track or area the brief wants investigated, with the brief's own question for each. When the brief does not enumerate tracks, distill them from its prose.
                2. The deliverable structure it demands for the final report: one entry per required top-level section, in the brief's order. When the brief specifies no structure, propose one that fits its content — but when it does specify one, reproduce it exactly.
                3. Its explicit definition-of-done items, empty when it states none.
                Write nothing to disk in this step — later steps own every file this run produces.`,
      output: [shared.data.briefTracks, shared.data.deliverableSections, shared.data.doneCriteria],
      include: [shared.data.briefTracks.optional(), shared.data.deliverableSections.optional(), shared.data.doneCriteria.optional()],
    });

    flow.until(
      condition.all([condition.count(shared.data.briefTracks).atLeast(1), condition.count(shared.data.deliverableSections).atLeast(1)]),
    );
  });

  flow.loop("Planning", (loop) => {
    loop.maxIterations(3, { onExhausted: signal.Abort });

    smart.prompt("Plan the streams", {
      input: input.prompt`
                Plan two to five non-overlapping research streams for ${shared.data.topic}, from the brief's tracks ${shared.data.briefTracks}. Each stream gets a stable id, a complete non-overlapping focus question, kind "primary", and covers: the brief-track ids it is responsible for. Every track MUST appear in at least one stream's covers; a stream may own several related tracks.
                This is the run's one and only executable stream list, consumed directly by per-stream research — do not propose more than five.`,
      output: shared.data.streamPlan,
      include: [shared.data.streamPlan.optional()],
    });

    flow.until(streamCountValid);
  });

  // Seeds the guaranteed empty dossier list the per-stream loop appends
  // into: production inside a for-each body is only "possible" to the
  // validator (an empty list never runs it), so the guaranteed producer
  // every later reader needs must sit before the loop.
  step.project("Seed the dossiers", {
    id: "seed-dossiers",
    projections: [{ source: operation.literal([]), destination: shared.data.dossiers }],
  });

  shared.data.streamPlan.forEach("Research each stream", { maxItems: 5 }, (stream) => {
    worker.prompt("Research the stream", {
      input: input.prompt`
                Research exactly one stream — ${stream} — for the topic ${shared.data.topic}. Do not research any other stream's focus; other streams run in their own frames.
                Apply ${shared.resource.researchStandards}: cite every factual claim, rate sources per the canonical A-E scale (${shared.resource.sourceQualityGuide}), format citations per ${shared.resource.citationStyle}, and note any counterevidence encountered.
                As you read, accumulate full reading notes in ${shared.data.outputDir}/${shared.data.topicSlug}/notes/<stream-id>.md (create directories as needed): quotes with attribution, per-source assessments, dead ends, and everything a later writer needs to reconstruct your reasoning. Write the notes file incrementally as you research — it is the stream's evidence corpus, never an after-the-fact summary.
                Return the stream's dossier: its stream-id, the notes file's repo-relative path, a cited summary, key claims (each with rating and citation inline), one sources entry per consulted source ("<A-E> | <citation or url> | <what it supports>"), and open questions.`,
      output: shared.data.dossiers.with(operation.Append),
    });
  });

  worker.prompt("Triangulate the findings", {
    input: input.prompt`
                Triangulate the researched streams for ${shared.data.topic}: read every dossier in ${shared.data.dossiers} and every notes file they point at, then report where independent streams agree, where they conflict, source-quality asymmetries (claims resting on materially weaker evidence than their neighbors), and remaining gaps.
                Return the triangulation.`,
    output: shared.data.triangulation,
  });

  flow.loop("Building", (loop) => {
    loop.maxIterations(6, { onExhausted: signal.Abort });

    worker.prompt("Building Produce", {
      input: input.prompt`
                    Produce the research delivery for ${shared.data.topic} from the dossiers ${shared.data.dossiers} and triangulation ${shared.data.triangulation} — reading each dossier's notes file for full depth. The dossiers are the index; the notes files are the evidence. Never write a section from dossier summaries alone.
                    Write under ${shared.data.outputDir}/${shared.data.topicSlug}/ (a flat directory apart from notes/ — no numbered subfolders, no manifests): report.md, bibliography.md, and evidence.csv (claim, source, rating, url). report.md's top-level sections MUST be exactly ${shared.data.deliverableSections}, in that order — the structure is the brief's to dictate. Satisfy every entry of ${shared.data.doneCriteria}.
                    No reviewer verdict attached means this is round 1: implement the full delivery. On every later round a verdict IS attached — fix every blocker it names.
                    Return a work summary: what was written, how citations were verified, open concerns.`,
      output: shared.data.workSummary,
      include: [shared.data.verdict1.optional(), shared.data.workSummary.optional()],
    });

    smart.prompt("Building Review", {
      input: input.prompt`
                    Review the research delivered for ${shared.data.topic} against the brief's contract and the plan ${shared.data.streamPlan}. Work summary: ${shared.data.workSummary}.
                    A BLOCKER is: a report.md whose top-level sections do not match ${shared.data.deliverableSections} one-to-one in order; a brief track in ${shared.data.briefTracks} the delivery leaves uncovered; an unmet entry of ${shared.data.doneCriteria}; a plan stream with no dossier or no readable notes file; a section written at dossier-summary depth when its notes file carries materially more evidence; an uncited factual claim; an unaddressed cross-stream contradiction; a source below C-rating used as sole support for a critical claim; or a file written outside ${shared.data.outputDir}/${shared.data.topicSlug}/ (notes/ inside it is part of the layout). Everything else is advisory.
                    Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                    Set status to revise while any blocker remains, approved when none do.`,
      output: shared.data.verdict1,
      include: [shared.data.verdict1.optional()],
    });


    flow.until(condition.equals(shared.data.verdict1.status, "approved"));
  });

  shared.stage.commit.shippingCommitTail({
    agent: scribe,
    text: input.prompt`
                The review for the research on ${shared.data.topic} has ended approved and the work is being committed.
                Write a concise commit message from the work summary ${shared.data.workSummary} and the verdict ${shared.data.verdict1}: a short subject line naming the topic, then one paragraph on what was researched and delivered at ${shared.data.reportPath}.
                Return exactly that message. Do not run git commands and do not write files.`,
  });

  return {
    commitReport: shared.data.commitReport,
    researchReportPort: shared.data.researchReportPort,
    reportPathPort: shared.data.reportPathPort,
  };
}
