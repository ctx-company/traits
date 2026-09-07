import { defineTrait, useVariant } from "@ctx-traits/cdk";

import { default as variants } from "./variant/index.ts";

export default function () {
  defineTrait("review", { version: "0.3.1" });
  useVariant(variants.default).default();
  useVariant(variants.pr);
}
