import { DEFAULT_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { procedure, variant } from "@ctx-traits/cdk";
import { FAMILY_BEHAVIOR, FAMILY_INTENT } from "../core.ts";
import {
    clerk,
    commitReport,
    declareTaskBoard,
    familyProcedure,
    gateTimedOut,
    leftoversPort,
    parkReportPort,
    scribe,
    smart1Role,
    smart2Role,
    task,
    verdictSchemaFor,
    verdictSlot,
    worker,
} from "../sequence/family.ts";

const smart1 = smart1Role("Strong model: drafts the implementation plan, and reviews the work in the refinement loop.");
const smart2 = smart2Role("Independent strong review model: reviews the implemented work each refinement pass, separately from smart-1.");

const taskBoard = declareTaskBoard();

const verdictSchema = verdictSchemaFor("default");
const verdict1 = verdictSlot("review-verdict-1", "smart-1", verdictSchema);
const verdict2 = verdictSlot("review-verdict-2", "smart-2", verdictSchema);

export default variant({
    name: "Implement (Default)",
    summary:
        "Surveyed dogfood implementation procedure: extract the task contract from the task board, draft the approach, implement it, then a doubly-reviewed bounded refinement loop — then summarize and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "multi-agent"] },
    behavior: FAMILY_BEHAVIOR,
    intent: FAMILY_INTENT,
    resource: [taskBoard],
    signal: [gateTimedOut],
    port: [commitReport, leftoversPort, parkReportPort],
    procedure: procedure.from(
        {
            description:
                "Implement one task from the task board end to end: extract its contract, draft the approach, implement it, refine against two independent reviewers until both approve, then summarize and commit — favoring the minimal, reuse-first implementation.",
        },
        () => {
            familyProcedure({
                clerk,
                smart1,
                smart2,
                worker,
                scribe,
                taskBoard,
                variantDoctrine: DEFAULT_VARIANT_DOCTRINE,
                verdict1,
                verdict2,
            });
        },
    ),
});
