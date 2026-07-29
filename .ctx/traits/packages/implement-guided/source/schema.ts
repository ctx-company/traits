import { schema as cdkSchema } from "@ctx-traits/cdk";

export const annotationSet = cdkSchema.object(
    "annotation-set",
    {
        source: cdkSchema.field(
            cdkSchema.object("annotation-source", {
                kind: cdkSchema.field(cdkSchema.text(), { description: 'Where the annotated text came from, e.g. "stdin".' }),
            }),
            { required: false, description: "Provenance of the annotated text. Recorded, not acted on." },
        ),
        annotations: cdkSchema.field(
            cdkSchema.list(
                cdkSchema.object("annotation", {
                    lines: cdkSchema.field(cdkSchema.list(cdkSchema.integer()), {
                        description: "The inclusive line range this annotation covers in the annotated text, as [start, end].",
                    }),
                    text: cdkSchema.field(cdkSchema.text(), {
                        description: "The annotation itself — what the author wants done, in their own words.",
                    }),
                }),
            ),
            { description: "Every annotation the tool returned. Together these are the assignment; none of them is optional." },
        ),
    },
    { description: "One annotation run: the annotated source plus the annotations the author left on it." },
);

export const reviewVerdict = cdkSchema.object(
    "review-verdict",
    {
        status: cdkSchema.field(cdkSchema.decision(), {
            description:
                "approved when no blocking defect remains (advisory notes may still exist); revise when at least one blocking defect remains. Set to approved if and only if blockers is empty.",
        }),
        blockers: cdkSchema.field(cdkSchema.text(), {
            required: false,
            description:
                "The blocking defects that must be fixed before this work can ship: a correctness bug, a failing check, or work the annotations did not ask for. Present when status is revise; empty or absent when approved.",
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
