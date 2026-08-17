import * as cdk from "@ctx-traits/cdk";

export const taskBoard = cdk.resource({
  id: "task-board",
  path: ".internal/tasks",
  root: "repo",
  hint: "Repo-root directory for the task board.",
  trigger: "on-demand",
});
