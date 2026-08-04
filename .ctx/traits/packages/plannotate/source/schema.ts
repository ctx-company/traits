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
        briefing: cdkSchema.field(cdkSchema.text(), {
            description:
                "A short markdown briefing of this round for the owner: what changed, what this verdict found, and (when status is approved) what the owner is being asked to confirm. Written to be read cold — the owner has not seen the working tree.",
        }),
    },
    {
        description:
            "Typed review verdict for the current work state, plus the round's owner-facing briefing. The loop blocks only on blockers, so taste and cosmetics never cause churn.",
    },
);

// `schema:any` rides the Envelope IO route (modules/io/src/run.rs:4203-4211),
// which never JSON-parses command stdout — a garbage line would land in the
// slot unrejected, the exact "quietly wrong and the loop spins" failure the
// task's Watch forbids. A declared `schema.object` rides the Typed route
// instead and rejects non-JSON/non-object stdout loudly. It doesn't need to
// declare `hookSpecificOutput` (camelCase, so it couldn't be a CDK field id
// anyway — every field id must be a lowercase kebab slug per
// `packages/cdk/src/normalize.ts`'s `validateSlug`): inline-field validation
// only checks declared fields and ignores undeclared keys
// (`modules/core/src/procedure/runtime/schema_validation.rs:640-676`), so the
// real envelope's `hookSpecificOutput` passes through untouched while a
// non-object/non-JSON line is rejected. `plan-review`'s prompt reads
// `hookSpecificOutput.decision.behavior`/`.message` straight out of the
// interpolated JSON text, the same nested path the core fixture at
// `modules/core/src/procedure/session.rs:4374` mirrors.
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
            description: "The brief itself: what was built, why, and how it was validated — for the tracked record.",
        }),
    },
    { description: "The run's final brief: a tracked markdown record plus the filename stem to save it under." },
);
