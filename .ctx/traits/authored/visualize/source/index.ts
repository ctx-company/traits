import * as cdk from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import * as variant from "#trait/variant/index.ts";

export default function () {
  cdk.defineTrait("Visualize", { version: "0.1.0" });
  cdk.useIntent(shared.INTENT);
  cdk.useBehavior(shared.BEHAVIOR);
  cdk.useResource(shared.resource.visualizations);
  cdk.useVariant(variant.default).default();
}
