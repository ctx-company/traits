import { resource } from "@ctx-traits/cdk";

export const taskBoard = resource({
  id: "task-board",
  path: ".internal/tasks",
  root: "repo",
  hint: "Repo-root directory for the task board: one markdown file per task, named NNNN-kebab-slug.md; agents read task files with their own tools and never inline them.",
  trigger: "on-demand",
});
