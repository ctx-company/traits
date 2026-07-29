import { reviewerRole, workerRole } from "@ctx-traits/agents";
import type { AgentHandle } from "@ctx-traits/cdk";

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
