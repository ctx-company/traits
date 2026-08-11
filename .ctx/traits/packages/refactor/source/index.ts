import { defineTrait, useVariant } from "@ctx-traits/cdk";
import { default as variants } from "./variants/index.ts";

export default function () {
    defineTrait("refactor", { version: "0.11.0" });

    useVariant(variants.base).default();
    useVariant(variants.direct);
    useVariant(variants.quick);
    useVariant(variants.smart);
    useVariant(variants.strict);
}
