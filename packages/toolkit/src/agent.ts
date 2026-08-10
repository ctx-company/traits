import type { AgentHandle } from "@ctx-traits/cdk";
import { agent } from "@ctx-traits/cdk";

/**
 * Declares the shared worker role: implements a draft/design and applies reviewer fixes.
 * @param id Agent identifier.
 * @param description Trait-specific object of the work (e.g. "the draft", "the agreed refactor design").
 * @example `workerRole("worker", "Implements the draft and applies reviewer fixes.")`
 */
export function workerRole(id: string, description: string): AgentHandle {
  return agent.worker(id, { description, summary: "Implementation role." });
}

/**
 * Declares the shared scribe role: writes the commit message for a completed run.
 * @param id Agent identifier.
 * @param taskDescription What the scribe writes the message from, without the trailing git-tail clause.
 * @example `scribeRole("scribe", "Writes the commit message for the completed task from the task contract")`
 */
export function scribeRole(id: string, taskDescription: string): AgentHandle {
  return agent.planner(id, {
    description: `${taskDescription}; git staging and committing run as command steps.`,
    summary: "Commit-message role.",
  });
}

/**
 * Declares the shared reviewer role: drafts and/or reviews the work in a refinement loop.
 * @param id Agent identifier.
 * @param description Trait-specific scope of what this reviewer drafts and/or judges.
 * @param summary Trait-specific model-visible role label; defaults to the generic "Review role."
 * @example `reviewerRole("smart-1", "Strong model: drafts the implementation plan, and reviews the work in the refinement loop.", "Drafting and first-reviewer role.")`
 */
export function reviewerRole(id: string, description: string, summary?: string): AgentHandle {
  return agent.reviewer(id, { description, summary: summary ?? "Review role." });
}

/**
 * Declares the shared clerk role: extracts and distills context so later steps never re-read source files.
 * @param id Agent identifier.
 * @param description Trait-specific description of what this clerk extracts.
 * @example `clerkRole("clerk", "Fast extraction model: copies the task file out of the task board verbatim.")`
 */
export function clerkRole(id: string, description: string): AgentHandle {
  return agent.searcher(id, { description, summary: "Context-extraction role." });
}

/**
 * Declares a Rust-specialized worker role on top of the shared `workerRole` doctrine: implements
 * a draft or agreed design and applies reviewer fixes, holding the change to this repo's Rust
 * engineering-standards and gate-conventions (`cargo fmt --check`, `cargo check`,
 * `cargo clippy --workspace --all-targets --all-features -- -D warnings`).
 * @param id Agent identifier.
 * @param description Trait-specific object of the work (e.g. "the draft", "the agreed refactor design").
 * @example `rustWorkerRole("worker", "Implements the draft and applies reviewer fixes.")`
 */
export function rustWorkerRole(id: string, description: string): AgentHandle {
  return workerRole(
    id,
    `${description} Holds every change to this repo's Rust engineering-standards and gate-conventions.`,
  );
}

/**
 * Declares a Rust-specialized reviewer role on top of the shared `reviewerRole` doctrine: drafts
 * and/or reviews the work in a refinement loop, judging it against Rust engineering-standards and
 * gate-conventions in addition to the generic review-verdict doctrine.
 * @param id Agent identifier.
 * @param description Trait-specific scope of what this reviewer drafts and/or judges.
 * @param summary Trait-specific model-visible role label; defaults to "Rust review role."
 * @example `rustReviewerRole("reviewer", "Reviews the implemented Rust change against the design.")`
 */
export function rustReviewerRole(id: string, description: string, summary?: string): AgentHandle {
  return reviewerRole(
    id,
    `${description} Judges the work against this repo's Rust engineering-standards and gate-conventions.`,
    summary ?? "Rust review role.",
  );
}
