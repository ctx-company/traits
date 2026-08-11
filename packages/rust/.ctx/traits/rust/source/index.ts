import { rustReviewerRole, rustWorkerRole } from "@ctx-traits/rust";
import { resource, trait } from "@ctx-traits/cdk";

const workerAgent = rustWorkerRole("worker", "Implements a draft or agreed Rust design and applies reviewer fixes.");
const reviewerAgent = rustReviewerRole(
    "reviewer",
    "Drafts and/or reviews Rust work in a bounded refinement loop.",
);

const engineeringStandards = resource({
    id: "engineering-standards",
    path: "resources/engineering-standards.md",
    hint:
        "Rust engineering doctrine: fmt/clippy baseline, module and API shape, error handling, testing, and dependency conventions; agents read this file with their own tools and never inline it.",
    trigger: "on-activation",
});

const gateConventions = resource({
    id: "gate-conventions",
    path: "resources/gate-conventions.md",
    hint:
        "The three Rust validation gates (fmt --check, check, clippy -D warnings), how to read a failure, and how to report gate results; agents read this file with their own tools.",
    trigger: "on-activation",
});

export default trait("rust", {
    version: "0.1.0",
    name: "Rust",
    description:
        "Shared Rust doctrine: worker/reviewer roles specialized for Rust changes, plus the engineering-standards and gate-conventions resources this repo's Rust core is held to, for dependents that declare it instead of pasting the roles or doctrine into their own trait source.",
    metadata: {
        tag: ["first-party", "knowledge", "rust"],
    },
    agent: [workerAgent, reviewerAgent],
    resource: [engineeringStandards, gateConventions],
});
