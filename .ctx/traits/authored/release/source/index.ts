import { defineTrait, useVariant } from "@ctx-traits/cdk";

import defaultVariant from "./variants/default.ts";

export default function () {
  defineTrait("release", { version: "0.1.0" });
  useVariant(defaultVariant).default();
}
