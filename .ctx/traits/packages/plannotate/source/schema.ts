import { schema as cdkSchema } from "@ctx-traits/cdk";

export const reviewVerdict = cdkSchema.object(
    "review-verdict",
    {
        status: cdkSchema.field(cdkSchema.decision(), {
            description:
                "approved when no blocking defect remains (advisory notes may still exist) AND every owner annotation from a prior round is addressed; revise when at least one blocking defect or unaddressed owner annotation remains. Set to approved if and only if blockers is empty.",
        }),
        blockers: cdkSchema.field(cdkSchema.text(), {
            required: false,
            description:
                "The blocking defects that must be fixed before this work can ship: a correctness bug, a failing check, work the plan did not ask for, or an owner annotation from ownerDecision that is not yet satisfied. Present when status is revise; empty or absent when approved.",
        }),
        advisory: cdkSchema.field(cdkSchema.text(), {
            required: false,
            description:
                "Non-blocking notes: naming, structure, taste, optional improvements. Never affects status and never forces another round.",
        }),
    },
    {
        description:
            "Typed review verdict for the current work state. The loop blocks only on blockers, so taste and cosmetics never cause churn.",
    },
);

// Declares only an optional marker field: the object-shape check rejects
// non-JSON/non-object stdout loudly, while the undeclared
// `hookSpecificOutput.decision.behavior`/`.message` path passes through
// untouched for `plan-review` to read from the interpolated JSON text.
export const planDecision = cdkSchema.object(
    "plan-decision",
    {
        "session-id": cdkSchema.field(cdkSchema.text(), {
            required: false,
            description:
                "plannotator's session identifier for the plan-mode hook exchange, when present in the payload.",
        }),
    },
    {
        description:
            "plannotator's plan-mode hook JSON. Only declares an optional marker field so the object-shape check rejects non-JSON/non-object stdout loudly; the undeclared `hookSpecificOutput.decision.behavior`/`.message` path is read directly from the raw JSON text by `plan-review`'s prompt.",
    },
);

export const ownerDecision = cdkSchema.object(
    "owner-decision",
    {
        decision: cdkSchema.field(cdkSchema.text(), {
            description:
                '"approved", "denied", or "dismissed" — plannotator\'s gate verdict on the round briefing. Only "approved" advances the build loop; denied and dismissed both leave it unsatisfied and the loop continues.',
        }),
    },
    { description: "plannotator's `annotate --gate --json` verdict on one round's briefing." },
);

export const brief = cdkSchema.object(
    "brief",
    {
        slug: cdkSchema.field(cdkSchema.text(), {
            description: "A short kebab-case filename stem for the tracked brief, e.g. the assignment's own slug.",
        }),
        markdown: cdkSchema.field(cdkSchema.text(), {
            description:
                "The brief itself: what was built, why, how it was validated, and what the review found — for the tracked record.",
        }),
        "commit-message": cdkSchema.field(cdkSchema.text(), {
            description:
                "Commit message for the approved work: a short subject line naming the change, then a one-paragraph summary of what was implemented and how it was validated.",
        }),
    },
    {
        description:
            "The run's final record: a tracked markdown brief, the filename stem to save it under, and the commit message.",
    },
);
