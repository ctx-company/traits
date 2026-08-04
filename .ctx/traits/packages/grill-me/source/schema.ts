import { schema as cdkSchema } from "@ctx-traits/cdk";

export const probe = cdkSchema.object(
    "interview-probe",
    {
        status: cdkSchema.field(cdkSchema.enum(["continue", "exhausted"]), {
            description:
                "continue when at least one unresolved point worth asking remains; exhausted only when every branch of the plan's decision tree is either settled in the ledger or already queued for the owner. Never exhausted merely because questioning feels finished — walk the tree.",
        }),
        question: cdkSchema.field(cdkSchema.text(), {
            required: false,
            description:
                "The single question this round: the fork it names, why it matters, and what choosing wrong would cost. Present when status is continue; absent when exhausted.",
        }),
        kind: cdkSchema.field(cdkSchema.enum(["fact", "decision"]), {
            required: false,
            description:
                "fact when the repository or environment can settle the question; decision when it is genuinely the owner's call — two or more defensible answers, and picking one is a judgment. Present when status is continue.",
        }),
        recommendation: cdkSchema.field(cdkSchema.text(), {
            required: false,
            description:
                "The interrogator's own recommended answer with a one-line rationale — every question arrives as a proposal to react to, never a blank prompt. Present when status is continue.",
        }),
    },
    {
        description:
            "One interview round: the single most load-bearing unresolved point in dependency order, or the typed signal that the tree is exhausted. The loop exits on status alone.",
    },
);
