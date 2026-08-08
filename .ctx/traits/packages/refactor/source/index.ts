import { trait, variant } from "@ctx-traits/cdk";

export default trait("refactor", {
    version: "0.9.0",
    variants: {
        direct: variant.import("./variants/direct/index.ts"),
        default: variant.import("./variants/default.ts").default(),
        quick: variant.import("./variants/quick.ts"),
        smart: variant.import("./variants/smart.ts"),
        strict: variant.import("./variants/strict.ts"),
    },
});
