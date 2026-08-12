import { schema } from "@ctx-traits/cdk";

export const linePoint = schema.object(
  "line-point",
  {
    number: schema.field(schema.integer(), { description: "1-based line number." }),
    column: schema.field(schema.integer(), { description: "1-based column within the line." }),
  },
  { description: "One end of an exact text range." },
);

export const annotation = schema.object(
  "annotation",
  {
    file: schema.field(schema.text(), { description: "Repo-relative path of the annotated file." }),
    lines: schema.field(schema.union([schema.list(schema.integer()), schema.list(linePoint)]), {
      description:
        "Whole-line form: [start, end] line numbers, BOTH inclusive. Free-text form: exactly two line-point objects marking the start and end of the exact range.",
    }),
    text: schema.field(schema.text(), { description: "The human's instruction for this annotated range." }),
  },
  { description: "One human annotation captured by ctx-annotate." },
);

export const annotations = schema.object(
  "annotations-doc",
  {
    annotations: schema.list(annotation),
  },
  { description: "The typed ctx-annotate output document." },
);
