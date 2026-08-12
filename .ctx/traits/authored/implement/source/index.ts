import { defineTrait, useVariant } from "@ctx-traits/cdk";

import { default as variants } from "./variant/index.ts";
import { default as gated } from "./variants/gated.ts";
import { default as guarded } from "./variants/guarded.ts";
import { default as phase } from "./variants/phase.ts";
import { default as smart } from "./variants/smart.ts";
import { default as strict } from "./variants/strict.ts";

export default function () {
  defineTrait("Implement", { version: "0.24.0" });

  useVariant(variants.quick);
  useVariant(variants.default).default();
  useVariant(smart, "smart");
  useVariant(strict, "strict");
  useVariant(phase, "phase");
  useVariant(gated, "gated");
  useVariant(guarded, "guarded");
}
