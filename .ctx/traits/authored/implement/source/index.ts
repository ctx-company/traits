import * as cdk from "@ctx-traits/cdk";

import * as shared from "./shared/index.ts";
import * as variant from "./variant/index.ts";

export default function () {
  cdk.defineTrait("Implement", { version: "0.25.0" });

  cdk.useIntent(shared.INTENT);
  cdk.useBehavior(shared.BEHAVIOR);
  cdk.useResource(shared.resource.taskBoard);

  cdk.useVariant(variant.basic).default();
  cdk.useVariant(variant.quick);
  cdk.useVariant(variant.complex);
}
