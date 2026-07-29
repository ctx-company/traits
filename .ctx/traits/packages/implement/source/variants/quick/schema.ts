import { blockerSchema } from "@ctx-traits/agents";
import { schema } from "@ctx-traits/cdk";

export const reviewVerdict = schema.object(
    "review-verdict",
    {
        status: schema.field(schema.enum(["approved", "revise"] as const), {
            description:
                "approved when the draft is correct and introduces no new smell — even if a forgivable plan-fidelity gap remains; revise only when a blocking defect remains. Set to approved if and only if blockers is empty.",
        }),
        blockers: schema.field(schema.list(blockerSchema), {
            description:
                "The blocking defects that must be fixed before merge: correctness bugs, house-rule violations (core purity, a new dependency, a new #[allow], required byte-stability broken), failing validation gates, clear over-build (accretion, defensive validation for states that cannot occur, scope creep beyond the phase), OR un-abstracted duplication. Draft-fidelity is judged solely under the quick authority rule — a left-undone step is forgivable there with a recorded reason and is never listed here as a categorical blocker on its own. Non-empty when status is revise; an empty list (never omitted — always return the key) when approved. Always present so the runtime can deterministically copy it into a park report without a missing-field failure.",
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
                "Stable wall id copied VERBATIM from an explicit \"**Wall:** <id>\" label in the phase's execution-plan section, non-empty only when status is revise and that label exists — never inferred from prose similarity or blocker content. Enables cross-run standing-wall refusal (P414); an empty string here never blocks a sibling run. Always present (never omitted) so the runtime can deterministically copy it into a park report without a missing-field failure.",
        }),
    },
    {
        description:
            "Typed single-pass review verdict. The loop blocks only on blockers; forgivable plan-fidelity gaps are recorded, not silently dropped.",
    },
);
