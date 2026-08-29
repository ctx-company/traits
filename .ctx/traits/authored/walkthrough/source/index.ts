import { defineTrait, useResource, useVariant } from "@ctx-traits/cdk";
import * as shared from "./shared/index.ts";
import { default as variants } from "./variant/index.ts";

export default function () {
  defineTrait("walkthrough", { version: "0.3.4" });

  useResource([shared.resource.walkthroughStandards, shared.resource.renderScript, shared.resource.symbolsScript]);

  useVariant(variants.default).default();
}
