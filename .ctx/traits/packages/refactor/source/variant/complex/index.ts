import { condition, defineVariant, flow, useIntent, useResource } from "@ctx-traits/cdk";

import * as shared from "../../shared/index.ts";

import * as stage from "./stage/index.ts";

export default function () {
    defineVariant("Complex", {
        metadata: { tag: shared.tag },
        summary: "Refactor procedure: survey, frame, design, implement, refine, and commit.",
        procedure: "Refactor procedure: survey, frame, design, implement, refine, and commit.",
    });

    useIntent(shared.intent);
    useResource(shared.resources);

    shared.stage.survey.gather("Survey the target");
    shared.stage.frame.select("Frame the problem");
    shared.stage.design.draft("Design the boundary");

    flow.loop("Doubly-reviewed verbatim refinement", (loop) => {
        loop.maxIterations(6, { onExhausted: "continue" });

        shared.stage.implement.apply("Implement the design specification");
        stage.review.first("Review refactor");
        stage.review.second("Cross-review refactor");

        flow.untilAll([
            condition.fieldEquals(stage.review.first.output, "status", "approved"),
            condition.fieldEquals(stage.review.second.output, "status", "approved"),
        ]);
    });

    stage.git.status("Check working tree status");
    flow.when("Maybe Commit", condition.not(condition.empty(stage.git.status.output)), () => {
        stage.git.commitMessage("Write the commit message");
        stage.git.commitStage("Stage all changes");
        stage.git.commitSubmit("Commit the refactor");
    });

    return { commitReport: shared.data.commitReport };
}
