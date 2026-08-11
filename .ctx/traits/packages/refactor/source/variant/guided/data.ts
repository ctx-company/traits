import * as cdk from "@ctx-traits/cdk";

import * as schema from "./schema.ts";

export const slot = {
    annotations: cdk.slot({
        id: "annotations",
        schema: schema.annotations,
        description: "Typed ctx-annotate output, validated against this schema before use.",
    }),
};
