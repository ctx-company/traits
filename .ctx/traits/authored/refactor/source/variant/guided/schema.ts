import * as cdk from "@ctx-traits/cdk";

export const linePoint = cdk.schema.object(
  "line-point",
  {
    number: cdk.schema.field(cdk.schema.integer(), { description: "1-based line number." }),
    column: cdk.schema.field(cdk.schema.integer(), { description: "1-based column within the line." }),
  },
  { description: "One end of an exact text range." },
);

export const annotation = cdk.schema.object(
  "annotation",
  {
    file: cdk.schema.field(cdk.schema.text(), { description: "Repo-relative path of the annotated file." }),
    lines: cdk.schema.field(
      cdk.schema.union([cdk.schema.list(cdk.schema.integer()), cdk.schema.list(linePoint)]),
      {
        description:
          "Whole-line form: [start, end] line numbers, BOTH inclusive. Free-text form: exactly two line-point objects marking the start and end of the exact range.",
      },
    ),
    text: cdk.schema.field(cdk.schema.text(), { description: "The human's instruction for this annotated range." }),
  },
  { description: "One human annotation captured by ctx-annotate." },
);

export const annotations = cdk.schema.object(
  "annotations-doc",
  {
    annotations: cdk.schema.list(annotation),
  },
  { description: "The typed ctx-annotate output document." },
);
