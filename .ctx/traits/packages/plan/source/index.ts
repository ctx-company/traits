import { trait, variant } from "@ctx-traits/cdk";

export default trait("plan", {
    version: "0.3.0",
    variants: {
        default: variant.import("./variants/default.ts").default(),
        direct: variant.import("./variants/direct.ts"),
        quick: variant.import("./variants/quick.ts"),
        complex: variant.import("./variants/complex.ts"),
    },
});
