import { condition, defineVariant, flow, intent, useIntent, useResource } from "@ctx-traits/cdk";

import { commitReport } from "../../shared/data.ts";
import { architectureDialect } from "../../shared/resource.ts";
import { checklistStage } from "../../shared/stage/checklist.ts";
import { implementStage } from "../../shared/stage/implement.ts";
import { reviewStage } from "../../shared/stage/review.ts";

import * as git from "./stage/git.ts";

export default function () {
    defineVariant("Quick", {
        summary: "Quick refactoring procedure.",
        metadata: { tag: ["first-party", "refactoring", "review", "multi-agent"] },
        procedure: "Refactor quickly: checklist, implementation, reviewer, commit.",
    });
    useIntent({
        require: [intent.require.ReviewBeforeFinal, intent.require.BehaviorPreservingDefault],
        avoid: [intent.avoid.RubberStampReview, intent.avoid.InterfaceWidening],
    });
    useResource(architectureDialect);

    checklistStage("Checklist the target");

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
