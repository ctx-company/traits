import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";

// The family's seats: one declaration per seat, one description — the
// emitter refuses two declarations of one id that differ, so the wording
// lives here and nowhere else. Stages borrow these handles; a variant
// with a seat of its own declares it in its local agent.ts namespace.
export const smart1 = reviewerRole(
    "smart-1",
    "Surveys, frames, designs, and reviews.",
);
export const smart2 = reviewerRole("smart-2", "Independent reviewer.");

export const worker = workerRole(
    "worker",
    "Implements the agreed refactor design and applies reviewer fixes.",
);

export const scribe = scribeRole(
    "scribe",
    "Writes the refactor commit message from the agreed design and survey record",
);
