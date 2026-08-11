import * as cdk from "@ctx-traits/cdk";

import { directAnnotations } from "../schema.ts";

export const annotations = cdk.slot({
    id: "annotations",
    schema: directAnnotations,
    description: "Typed ctx-annotate output, validated against this schema before use.",
});

export const annotateStage = cdk.stage({
    input: cdk.input.command`ctx-annotate`,
    timeoutMs: 3_600_000,
    output: annotations,
});
