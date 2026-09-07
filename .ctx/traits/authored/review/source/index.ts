import { defineTrait, useVariant } from "@ctx-traits/cdk";

import { default as variants } from "./variant/index.ts";

export default function () {
  defineTrait("review", { version: "0.2.0" });
  useVariant(variants.default).default();
  useVariant(variants.pr);
}
