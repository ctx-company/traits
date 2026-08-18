import * as agents from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

export const reviewVerdictSchema = cdk.schema.object(
  "review-verdict",
  {
    status: cdk.schema.field(cdk.schema.enum(["approved", "revise"] as const), {
      description:
        "approved when the framed change is correct, behavior-preserving, and introduces no new smell — even if the broader ideal is not fully reached; revise only when a blocking defect remains. Set to approved if and only if blockers is empty.",
    }),
    blockers: cdk.schema.field(cdk.schema.list(agents.blockerSchema), {
      required: false,
      description:
        "The blocking defects that must be fixed before merge: a behavior or byte-stability break, a NEW S1-S10 smell introduced by this change (including S9 — code this change supersedes surviving beside its replacement), or an interface widened to make a caller compile. Fidelity to the agreed design is judged solely under this variant's authority rule, not as a separate categorical item here. Present when status is revise; empty or absent when approved.",
    }),
    advisory: cdk.schema.field(cdk.schema.text(), {
      required: false,
      description:
        "Non-blocking notes: residual pre-existing smells outside the framed change, code the agreed design did not cover, further refactoring the survey deferred, taste. Never affects status; belongs on the deferred-candidates list, not this run.",
    }),
  },
  {
    description:
      "Typed review verdict. The loop blocks only on blockers; an incomplete migration of the broader ideal is advisory-deferred, not a blocker, so a correct framed refactor converges.",
  },
);
