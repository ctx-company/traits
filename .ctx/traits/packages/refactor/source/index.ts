import { defineTrait, useIntent, useResource, useVariant } from "@ctx-traits/cdk";
import * as shared from "./shared/index.ts";
import { default as variants } from "./variant/index.ts";

export default function () {
    defineTrait("Refactor", { version: "0.11.0" });

    useIntent(shared.intent);
    useResource(shared.resources);

    useVariant(variants.quick).default();
    useVariant(variants.guided);
    useVariant(variants.complex);
}
