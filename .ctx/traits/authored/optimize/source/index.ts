import { defineTrait, useVariant } from "@ctx-traits/cdk";
import { default as variants } from "./variant/index.ts";

export default function () {
  defineTrait("Optimize", { version: "0.1.0" });

  useVariant(variants.experiment).default();
  useVariant(variants.benchmark);
}
