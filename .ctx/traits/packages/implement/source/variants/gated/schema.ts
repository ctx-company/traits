import { blockerSchema } from "@ctx-traits/agents";
import { schema } from "@ctx-traits/cdk";

// Verbatim copy of quick's verdict schema: gated's machine-review loop is
// quick's, so the verdict contract is quick's too. An unsatisfied owner
// annotation enters as an ordinary typed blocker — no schema change.
export const reviewVerdict = schema.object(
    "review-verdict",
    {
        status: schema.field(schema.enum(["approved", "revise"] as const), {
            description:
                "approved when the draft is correct and introduces no new smell — even if a forgivable plan-fidelity gap remains; revise only when a blocking defect remains. Set to approved if and only if blockers is empty.",
        }),
        blockers: schema.field(schema.list(blockerSchema), {
            description:
                "The blocking defects that must be fixed before merge: correctness bugs, failing validation gates, clear over-build (accretion, defensive validation for states that cannot occur, scope creep beyond the task), un-abstracted duplication, OR an owner annotation from a denied round briefing that the work does not yet visibly address. Draft-fidelity is judged solely under the quick authority rule — a left-undone step is forgivable there with a recorded reason and is never listed here as a categorical blocker on its own. Non-empty when status is revise; an empty list (never omitted — always return the key) when approved. Always present so the runtime can deterministically copy it into a park report without a missing-field failure.",
        }),
        advisory: schema.field(schema.text(), {
            required: false,
            description:
                "Non-blocking notes: subjective style, naming, taste, optional improvements, follow-up work. Never affects status and never forces another refinement round.",
        }),
        "forgiveness-reason": schema.field(schema.text(), {
            required: false,
            description:
                "Present ONLY when this verdict forgives a plan-fidelity gap — a missing or altered draft step that changed nothing an observer could break. One line naming the gap and why it was reasonable to diverge. Never present for a genuine defect, which is always a blocker regardless of reason.",
        }),
        "wall-id": schema.field(schema.text(), {
            description:
                "Stable wall id copied VERBATIM from an explicit \"**Wall:** <id>\" label in the task file, non-empty only when status is revise and that label exists — never inferred from prose similarity or blocker content. Enables cross-run standing-wall refusal (P414); an empty string here never blocks a sibling run. Always present (never omitted) so the runtime can deterministically copy it into a park report without a missing-field failure.",
        }),
        escalation: schema.field(schema.enum(["none", "needs-owner"] as const), {
            description:
                "needs-owner if and only if the run as a whole cannot reach an approvable state — never merely because one blocker is outside this round's reach. none otherwise.",
        }),
        "escalation-reason": schema.field(schema.text(), {
            description:
                "One plain sentence: WHY the run cannot reach an approvable state and the one owner action that would clear it. Non-empty when escalation is needs-owner; an empty string (never omitted — always return the key) otherwise. Always present so the runtime can deterministically copy it into a park report without a missing-field failure.",
        }),
    },
    {
        description:
            "Typed single-pass review verdict. The loop blocks only on blockers; forgivable plan-fidelity gaps are recorded, not silently dropped.",
    },
);

// Declares only an optional marker field: the object-shape check rejects
// non-JSON/non-object stdout loudly, while the undeclared
// `hookSpecificOutput.decision.behavior`/`.message` path passes through
// untouched for `plan-refine` to read from the interpolated JSON text.
export const planDecision = schema.object(
    "plan-decision",
    {
        "session-id": schema.field(schema.text(), {
            required: false,
            description:
                "plannotator's session identifier for the plan-mode hook exchange, when present in the payload.",
        }),
    },
    {
        description:
            "plannotator's plan-mode hook JSON. Only declares an optional marker field so the object-shape check rejects non-JSON/non-object stdout loudly; the undeclared `hookSpecificOutput.decision.behavior`/`.message` path is read directly from the raw JSON text by `plan-refine`'s prompt.",
    },
);

export const ownerDecision = schema.object(
    "owner-decision",
    {
        decision: schema.field(schema.text(), {
            description:
                '"approved", "denied", or "dismissed" — plannotator\'s gate verdict on the round briefing. Only "approved" advances the build loop; denied and dismissed both leave it unsatisfied and the loop continues.',
        }),
    },
    { description: "plannotator's `annotate --gate --json` verdict on one round's briefing." },
);

export const brief = schema.object(
    "brief",
    {
        slug: schema.field(schema.text(), {
            description: "A short kebab-case filename stem for the tracked brief, e.g. the task's own slug.",
        }),
        markdown: schema.field(schema.text(), {
            description:
                "The brief itself: what was built, why, how it was validated, and what the review found — for the tracked record.",
        }),
        "commit-message": schema.field(schema.text(), {
            description:
                "Commit message for the approved work: a short subject line naming the change, then a one-paragraph summary of what was implemented and how it was validated.",
        }),
    },
    {
        description:
            "The run's final record: a tracked markdown brief, the filename stem to save it under, and the commit message.",
    },
);
