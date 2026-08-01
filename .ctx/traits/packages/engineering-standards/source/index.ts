import { trait } from "@ctx-traits/cdk";
import { codingStandards, reviewGuidance } from "./resource.ts";

export default trait("engineering-standards", {
    version: "0.1.0",
    name: "Engineering Standards",
    summary:
        "Shared, versioned engineering standards and review guidance that reviewed-change traits depend on instead of pasting rules into prompts.",
    metadata: {
        tag: ["first-party", "knowledge", "standards"],
    },
    resource: [codingStandards, reviewGuidance],
});
