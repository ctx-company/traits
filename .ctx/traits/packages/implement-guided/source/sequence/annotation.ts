import { sequence } from "@ctx-traits/cdk";

import { slot } from "../data.ts";

export default sequence.command("collect-annotations", {
    title: "Collect the assignment (ctx-annotate)",
    // Runs through `sh -c` for ONE reason: the tool reads the question on
    // stdin, and command steps have no stdin channel today — every spawn
    // is `Stdio::null()`. The shell supplies the pipe the step model
    // cannot. Drop the wrapper the moment the step model carries stdin.
    argv: ["sh", "-c", 'echo "what we doing today?" | ctx-annotate --stdin'],
    output: slot.annotations,
});
