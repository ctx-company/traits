// Cross-variant resource declaration every implement variant shares.
// Redistributed from source/sequence/family.ts (0183); that module
// re-exports this so unported variants keep compiling until they migrate
// too.
import type { ResourceHandle } from "@ctx-traits/cdk";
import { resource } from "@ctx-traits/cdk";

/**
 * Declares the task-board resource: the repo-root directory holding one
 * markdown file per task. Each package instantiates its own declaration via
 * this factory (never a trait `dependency` ref — a dependency-vendored
 * root="repo" resource loses the on-demand audit exemption a package's own
 * direct declaration gets), keeping the declaration itself single-sourced.
 */
export function declareTaskBoard(): ResourceHandle {
  return resource({
    id: "task-board",
    path: ".internal/tasks",
    root: "repo",
    hint: "Repo-root directory for the task board: one markdown file per task, named NNNN-kebab-slug.md; agents read task files with their own tools and never inline them.",
    trigger: "on-demand",
  });
}
