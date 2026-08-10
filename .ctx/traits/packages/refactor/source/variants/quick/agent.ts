import { reviewerRole, scribeRole, workerRole } from "@ctx-traits/agents";

export const smart1 = reviewerRole(
    "smart-1",
    "Turns the target into an actionable checklist and reviews the implemented work once.",
    "Checklist and reviewer.",
);
export const worker = workerRole("worker", "Implements the checklist and applies the reviewer's single pass.");
export const scribe = scribeRole("scribe", "Writes the refactor commit message from the checklist and review record");
