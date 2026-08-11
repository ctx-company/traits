import { condition, defineVariant, flow, useIntent, useResource } from "@ctx-traits/cdk";

import * as shared from "../../shared/index.ts";

import * as stage from "./stage/index.ts";

export default function () {
    defineVariant("Guided", {
        summary: "Turns human annotations into implemented, reviewed changes.",
        metadata: { tag: [...shared.tag, "annotations"] },
        procedure:
            "Collect human annotations interactively, plan them as a checklist, implement every item in a reviewed loop, and commit.",
    });

    useIntent(shared.intent);
    useResource(shared.resources);

    stage.annotate.collect("Collect annotations (ctx-annotate)");
    stage.checklist.compose("Build the checklist");

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
