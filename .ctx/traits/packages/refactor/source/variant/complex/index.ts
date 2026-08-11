import { condition, defineVariant, flow, useIntent, useResource } from "@ctx-traits/cdk";

import { designStage } from "../../shared/stage/design.ts";
import { frameStage } from "../../shared/stage/frame.ts";
import * as git from "./stage/git.ts";
import { implementStage } from "../../shared/stage/implement.ts";
import { reviewFirstStage, reviewSecondStage } from "./stage/review.ts";
import { surveyStage } from "../../shared/stage/survey.ts";

import { commitReport } from "../../shared/data.ts";
import * as resource from "../../shared/resource.ts";
import { intent } from "../../shared/intent.ts";

export default function () {
    defineVariant("Complex", {
        metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
        summary: "Refactor procedure: survey, frame, design, implement, refine, and commit.",
        procedure: "Refactor procedure: survey, frame, design, implement, refine, and commit.",
    });

    useIntent(intent);
    useResource([resource.architectureDialect, resource.smellCatalog]);

    surveyStage("Survey the target");
    frameStage("Frame the problem");
    designStage("Design the boundary");

    flow.loop("Doubly-reviewed verbatim refinement", (loop) => {
        loop.maxIterations(6, { onExhausted: "continue" });

        implementStage("Implement the design specification");
        reviewFirstStage("Review refactor");
        reviewSecondStage("Cross-review refactor");

        flow.untilAll([
            condition.fieldEquals(reviewFirstStage.output, "status", "approved"),
            condition.fieldEquals(reviewSecondStage.output, "status", "approved"),
        ]);
    });

    git.status("Check working tree status");
    flow.when("Maybe Commit", condition.not(condition.empty(git.status.output)), () => {
        git.commitMessage("Write the commit message");
        git.commitStage("Stage all changes");
        git.commitSubmit("Commit the refactor");
    });

    return { commitReport };
}
