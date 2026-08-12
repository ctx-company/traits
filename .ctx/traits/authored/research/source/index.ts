import { trait, variant } from "@ctx-traits/cdk";

export default trait("research", {
  version: "0.1.0",
  variants: {
    quick: variant.import("./variants/quick.ts"),
    default: variant.import("./variants/default.ts").default(),
    deep: variant.import("./variants/deep.ts"),
  },
});
