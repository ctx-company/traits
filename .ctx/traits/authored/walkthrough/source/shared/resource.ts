import { resource } from "@ctx-traits/cdk";

export const walkthroughStandards = resource.file("walkthrough-standards", {
  path: "resources/walkthrough-standards.md",
  hint: "Exhaustive-walkthrough doctrine: closure and horizon rules, id/parent conventions, coverage-key discipline, layer registers, last-wins revisions.",
  trigger: "on-demand",
});
export const renderScript = resource.file("render-script", {
  path: "resources/render.py",
  hint: "Deterministic treemap renderer the render command step executes; never read by a model seat.",
  trigger: "on-demand",
});
export const symbolsScript = resource.file("symbols-script", {
  path: "resources/symbols.py",
  hint: "Deterministic symbol enumerator and coverage checker (enumerate | coverage | status); ground truth for the exhaustiveness gate; never read by a model seat.",
  trigger: "on-demand",
});
