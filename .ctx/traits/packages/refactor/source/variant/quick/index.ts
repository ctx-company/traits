import { condition, defineVariant, flow, useIntent, useResource } from "@ctx-traits/cdk";

import * as shared from "../../shared/index.ts";

import * as stage from "./stage/index.ts";

export default function () {
    defineVariant("Quick", {
        summary:
            "Quick refactoring procedure: turn the target into an actionable checklist, implement it, one reviewer pass, apply that pass once if needed, and commit.",
        metadata: { tag: shared.tag },
        procedure:
            "Refactor one module or entity quickly: an actionable checklist, implementation, exactly one reviewer pass, one round of fixes if needed, commit.",
    });

    useIntent(shared.intent);
    useResource(shared.resources);

    shared.stage.checklist.compose("Checklist the target");

    flow.loop("Reviewed refinement", (loop) => {
        loop.maxIterations(2, { onExhausted: "continue" });
        shared.stage.implement.apply("Implement the checklist");
        shared.stage.review.judge("Review the implementation");
        flow.until(condition.fieldEquals(shared.stage.review.judge.output, "status", "approved"));
    });

    stage.git.status("Check working tree status");
    flow.when("Maybe Commit", condition.not(condition.empty(stage.git.status.output)), () => {
        stage.git.commitMessage("Write the commit message");
        stage.git.commitStage("Stage all changes");
        stage.git.commitSubmit("Commit the refactor");
    });

    return { commitReport: shared.data.commitReport };
}
