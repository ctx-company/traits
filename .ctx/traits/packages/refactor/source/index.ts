import { defineTrait, useVariant } from "@ctx-traits/cdk";
import { default as variants } from "./variant/index.ts";

export default function () {
    defineTrait("Refactor", { version: "0.11.0" });

    useVariant(variants.quick).default();
    useVariant(variants.guided);
    useVariant(variants.complex);
}
