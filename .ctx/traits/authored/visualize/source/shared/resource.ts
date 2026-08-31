import * as cdk from "@ctx-traits/cdk";

export const visualizations = cdk.resource.directory("visualizations", {
  path: ".internal/visualizations",
  root: "repo",
  hint: "Repo-root directory visualize walkthroughs and their diagrams are written under.",
  trigger: "on-demand",
});
