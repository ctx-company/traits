import { sequence } from "@ctx-traits/cdk";

import { slot } from "../data.ts";

export default sequence.command("annotate", {
    title: "Collect annotations (ctx-annotate)",
    cmd: "ctx-annotate",
    timeoutMs: 3_600_000,
    output: slot.annotations,
});
