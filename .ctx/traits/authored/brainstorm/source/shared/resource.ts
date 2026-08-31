import * as cdk from "@ctx-traits/cdk";

export const brainstorms = cdk.resource.directory("brainstorms", {
  path: ".internal/brainstorms",
  root: "repo",
  hint: "Repo-root directory brainstorm proposals and their diagrams are written under.",
  trigger: "on-demand",
});
