import { defineTrait, useVariant } from "@ctx-traits/cdk";

import { default as variants } from "./variant/index.ts";

export default function () {
  defineTrait("release", { version: "0.1.0" });
  useVariant(variants.default).default();
}
