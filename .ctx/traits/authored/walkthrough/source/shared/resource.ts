import { resource } from "@ctx-traits/cdk";

export const walkthroughStandards = resource.file("walkthrough-standards", {
  path: "resources/walkthrough-standards.md",
  hint: "Node-quality doctrine: layer registers, ref discipline, tree shape rules, kind taxonomy, sizing honesty.",
  trigger: "on-demand",
});
export const renderScript = resource.file("render-script", {
  path: "resources/render.py",
  hint: "Deterministic treemap renderer the render command step executes; never read by a model seat.",
  trigger: "on-demand",
});
