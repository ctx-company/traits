import * as cdk from "@ctx-traits/cdk";

import * as shared from "./shared/index.ts";
import * as variant from "./variant/index.ts";

export default function () {
  cdk.defineTrait("Refactor", { version: "0.13.0" });

  cdk.useIntent(shared.INTENT);
  cdk.useBehavior(shared.BEHAVIOR);
  cdk.useResource([shared.resource.architectureDialect, shared.resource.smellCatalog]);

  cdk.useVariant(variant.quick).default();
  cdk.useVariant(variant.guarded);
  cdk.useVariant(variant.guided);
  cdk.useVariant(variant.complex);
}
