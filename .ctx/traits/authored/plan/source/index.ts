import { defineTrait, useIntent, useResource, useVariant } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";
import variants from "#trait/variant/index.ts";

export default function () {
  defineTrait("plan", { version: "0.5.0" });
  useIntent(shared.intent);
  useResource([]);

  useVariant(variants.default).default();
  useVariant(variants.direct);
  useVariant(variants.quick);
  useVariant(variants.complex);
}
