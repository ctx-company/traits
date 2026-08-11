import { condition, defineVariant, flow, intent, useIntent } from "@ctx-traits/cdk";

import { commitReport } from "../../shared/data.ts";
import { implementStage } from "../../shared/stage/implement.ts";
import { reviewStage } from "../../shared/stage/review.ts";

import { annotateStage } from "./stage/annotate.ts";
import { checklistStage } from "./stage/checklist.ts";
import * as git from "./stage/git.ts";

export default function () {
    defineVariant("Guided", {
        summary: "Turns human annotations into implemented, reviewed changes.",
        metadata: { tag: ["first-party", "refactoring", "annotations"] },
        procedure:
            "Collect human annotations interactively, plan them as a checklist, implement every item in a reviewed loop, and commit.",
    });
    useIntent({
        require: [
            intent.require.AnnotationFidelity,
            intent.require.PreserveScope,
        ],
        avoid: [intent.avoid.ScopeCreep, intent.avoid.OverEngineering],
    });

    annotateStage("Collect annotations (ctx-annotate)");
    checklistStage("Build the checklist");

    flow.loop("Reviewed refinement", (loop) => {
        loop.maxIterations(2, { onExhausted: "continue" });
        implementStage("Implement the checklist");
        reviewStage("Review the implementation");
        flow.until(condition.fieldEquals(reviewStage.output, "status", "approved"));
    });

    git.status("Check working tree status");
    flow.when("Maybe Commit", condition.not(condition.empty(git.status.output)), () => {
        git.commitMessage("Write the commit message");
        git.commitStage("Stage all changes");
        git.commitSubmit("Commit the refactor");
    });

    return { commitReport };
}
