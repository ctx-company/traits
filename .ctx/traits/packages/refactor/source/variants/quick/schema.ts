import { blockerSchema } from "@ctx-traits/agents";
import { schema, slot } from "@ctx-traits/cdk";

export const reviewVerdict = schema.object(
    "review-verdict",
    {
        status: schema.field(schema.enum(["approved", "revise"] as const), {
            description:
                "approved when the checklist is correct, behavior-preserving, and introduces no new smell; revise only when a blocking defect remains. Set to approved if and only if blockers is empty.",
        }),
        blockers: schema.field(schema.list(blockerSchema), {
            required: false,
            description:
                "The blocking defects that must be fixed before merge: a behavior break, a new smell, or an interface widened to make a caller compile. Checklist-item fidelity is judged solely under the authority rule below — a left-undone step is forgivable there with a recorded reason and is never listed here as a categorical blocker on its own. Present when status is revise; empty or absent when approved.",
        }),
        advisory: schema.field(schema.text(), {
            required: false,
            description: "Non-blocking notes: taste, follow-up, deferred cleanup. Never affects status.",
        }),
        "forgiveness-reason": schema.field(schema.text(), {
            required: false,
            description:
                "Present ONLY when this verdict forgives a plan-fidelity gap — a missing or altered checklist step that changed nothing an observer could break. One line naming the gap and why it was reasonable to diverge. Never present for a genuine defect, which is always a blocker regardless of reason.",
        }),
    },
    {
        description:
            "Typed single-pass review verdict. The loop blocks only on blockers; forgivable plan-fidelity gaps are recorded, not silently dropped.",
    },
);

export const verdict = slot({
    id: "review-verdict",
    schema: reviewVerdict,
    description: "smart-1's single review verdict for the implemented checklist.",
});
