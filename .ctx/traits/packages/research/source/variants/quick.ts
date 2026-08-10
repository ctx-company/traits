// research-quick: an inline scope note, one bounded worker evidence pass,
// then a single-reviewer grind loop until approved — then commit. Lean, like
// implement-quick: no leftovers adjudication, no dual review. The reviewer
// loop is bounded and parks honestly (typed park report) on exhaustion.
import { deriveParkReportStep, familyCommitTail, reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";
import { condition, flow, input, procedure, slot, variant } from "@ctx-traits/cdk";

import { FAMILY_BEHAVIOR, QUICK_INTENT } from "../core.ts";
import {
    citationStyle,
    commitOutput,
    commitReport,
    deriveReportPathStep,
    deriveTopicSlugStep,
    outputDir,
    parkReport,
    parkReportPort,
    reportPath,
    reportPathPort,
    researchReportPort,
    researchStandards,
    sourceQualityGuide,
    topic,
    topicSlug,
    verdict1,
    workSummary,
} from "../data.ts";

const smart = reviewerRole("smart-1", "Scopes the topic and is the sole reviewer of the delivered research.", "Scoping and reviewer role.");
const worker = workerRole("worker", "Researches the topic and applies reviewer fixes.");
const scribe = scribeRole("scribe", "Writes the commit message for the completed research from the scope note and verdict");

const scopeNote = slot.text({
    id: "scope-note",
    description: "Short scope note for the topic: the core question, two to four sub-questions, and explicit exclusions.",
});

export default variant({
    name: "Research (Quick)",
    summary: "Quick dogfood research procedure: scope the topic, research it in one bounded worker pass, grind a single reviewer loop until approved, then commit.",
    metadata: { tag: ["dogfood", "research", "review", "lean"] },
    behavior: FAMILY_BEHAVIOR,
    intent: QUICK_INTENT,
    resource: [researchStandards, sourceQualityGuide, citationStyle],
    port: [topic, outputDir, commitReport, parkReportPort, researchReportPort, reportPathPort],
    procedure: procedure.from(
        {
            description:
                "Research one topic quickly: an inline scope note, one bounded worker research pass, a single bounded reviewer loop, then commit brief.md and sources.md.",
        },
        () => {
            deriveTopicSlugStep();
            deriveReportPathStep("brief.md");

            smart.prompt("Scope the topic", {
                input: input.prompt`
                    Write a short scope note for researching ${topic}: the core question, two to four sub-questions a worker should cover, and any explicit exclusions.
                    Reference ${researchStandards} for source-quality expectations. Do not research anything yet.`,
                output: scopeNote,
            });

            flow.loop("Researching", (loop) => {
                loop.maxIterations(3, { onExhausted: "abort" });

                worker.prompt("Researching Produce", {
                    input: input.prompt`
                        Research ${topic} against the scope note ${scopeNote} and write the deliverable under ${outputDir}/${topicSlug}/ (a flat directory — no numbered subfolders, no manifests): brief.md (findings with inline citations) and sources.md (a bibliography, canonical A-E source ratings per ${sourceQualityGuide}, formatted per ${citationStyle}).
                        No reviewer verdict attached means this is round 1: cover every sub-question from the scope note. On every later round a verdict IS attached — fix every blocker it names.
                        Apply ${researchStandards} throughout: cite every factual claim, prefer A/B-rated sources, record contradictions and gaps explicitly.
                        Return a work summary: what was written, source counts and ratings, open concerns.`,
                    output: workSummary,
                    include: [verdict1.optional(), workSummary.optional()],
                });

                smart.prompt("Researching Review", {
                    input: input.prompt`
                        Review the research delivered for ${topic} against the scope note ${scopeNote}. Work summary: ${workSummary}.
                        A BLOCKER is an uncited factual claim, a sub-question left uncovered, a source below C-rating used as sole support for a critical claim, or a file written outside the flat ${outputDir}/${topicSlug}/ layout. Everything else is advisory.
                        Your own verdict from last round is attached when one exists: carry every open blocker forward verbatim, verify with your own tools, and flip to done only on confirmed evidence.
                        Set status to revise while any blocker remains, approved when none do.`,
                    output: verdict1,
                    include: [verdict1.optional()],
                });

                deriveParkReportStep(verdict1, { parkReportSlot: parkReport });

                flow.until(condition.equals(verdict1.status, "approved"));
            });

            familyCommitTail({
                id: "shipping",
                scribe: {
                    agent: scribe,
                    text: input.prompt`
                        The review for the research on ${topic} has ended approved and the work is being committed.
                        Write a concise commit message from the work summary ${workSummary} and the verdict ${verdict1}: a short subject line naming the topic, then one paragraph on what was researched and delivered at ${reportPath}.
                        Return exactly that message. Do not run git commands and do not write files.`,
                },
                receipt: commitOutput,
            });
        },
    ),
});
