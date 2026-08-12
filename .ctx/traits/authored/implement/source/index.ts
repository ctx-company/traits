import { trait, variant } from "@ctx-traits/cdk";

export default trait("implement", {
  version: "0.24.0",
  variants: {
    quick: variant.import("./variants/quick.ts"),
    default: variant.import("./variants/default.ts").default(),
    smart: variant.import("./variants/smart.ts"),
    strict: variant.import("./variants/strict.ts"),
    phase: variant.import("./variants/phase.ts"),
    gated: variant.import("./variants/gated.ts"),
    guarded: variant.import("./variants/guarded.ts"),
  },
});
