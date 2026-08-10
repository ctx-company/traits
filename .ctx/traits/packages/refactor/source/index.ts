import { defineTrait, useVariant } from "@ctx-traits/cdk";

import defaultVariant from "./variants/default.ts";
import direct from "./variants/direct/index.ts";
import quick from "./variants/quick.ts";
import smart from "./variants/smart.ts";
import strict from "./variants/strict.ts";

export default function () {
    defineTrait("refactor", { version: "0.10.0" });
    useVariant(direct);
    useVariant(defaultVariant).default();
    useVariant(quick);
    useVariant(smart);
    useVariant(strict);
}
