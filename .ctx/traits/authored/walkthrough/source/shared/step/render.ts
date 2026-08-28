import { input, step } from "@ctx-traits/cdk";

import { htmlPath, renderLog, walkthroughNodes } from "../data.ts";
import { renderScript } from "../resource.ts";

/**
 * The deterministic render tail: python3 runs the package's render.py resource
 * over the typed node list and the derived output path. Data travels as one
 * argv element (JSON), the script as a `{resource:render-script}` token IO
 * resolves to a real path — no shell, no string assembly, nothing for the
 * hidden-content audit to flag, and no model seat anywhere near the HTML.
 */
export function renderWalkthroughStep(): void {
  step.command("Render walkthrough", {
    id: "render-walkthrough",
    input: input.command`python3 ${renderScript} ${walkthroughNodes} ${htmlPath}`,
    output: renderLog,
  });
}
