import { schema as cdkSchema } from "@ctx-traits/cdk";

export const linePoint = cdkSchema.object(
    "line-point",
    {
        number: cdkSchema.field(cdkSchema.integer(), {
            description: "1-based line number.",
        }),
        column: cdkSchema.field(cdkSchema.integer(), {
            description: "1-based column within the line.",
        }),
    },
    { description: "One end of an exact text range." },
);

export const annotation = cdkSchema.object(
    "annotation",
    {
        file: cdkSchema.field(cdkSchema.text(), {
            description: "Repo-relative path of the annotated file.",
        }),
        lines: cdkSchema.field(
            cdkSchema.union([cdkSchema.list(cdkSchema.integer()), cdkSchema.list(linePoint)]),
            {
                description:
                    "Whole-line form: [start, end] line numbers, BOTH inclusive. Free-text form: exactly two line-point objects marking the start and end of the exact range.",
            },
        ),
        text: cdkSchema.field(cdkSchema.text(), {
            description: "The human's instruction for this annotated range.",
        }),
    },
    { description: "One human annotation captured by ctx-annotate." },
);

export const annotations = cdkSchema.object(
    "annotations-doc",
    {
        annotations: cdkSchema.list(annotation),
    },
    { description: "The typed ctx-annotate output document." },
);
