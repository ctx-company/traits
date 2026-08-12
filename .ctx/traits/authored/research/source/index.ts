import { defineTrait, useResource, useVariant } from "@ctx-traits/cdk";
import * as shared from "./shared/index.ts";
import { default as variants } from "./variant/index.ts";

export default function () {
  defineTrait("research", { version: "0.1.0" });

  useResource([shared.resource.researchStandards, shared.resource.sourceQualityGuide, shared.resource.citationStyle]);

  useVariant(variants.quick);
  useVariant(variants.default).default();
  useVariant(variants.deep);
}
