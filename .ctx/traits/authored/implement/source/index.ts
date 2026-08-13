import { defineTrait, useVariant } from "@ctx-traits/cdk";

import { default as variants } from "./variant/index.ts";

export default function () {
  defineTrait("Implement", { version: "0.24.0" });

  useVariant(variants.quick);
  useVariant(variants.default).default();
  useVariant(variants.smart);
  useVariant(variants.strict);
  useVariant(variants.phase);
  useVariant(variants.gated);
  useVariant(variants.guarded);
}
