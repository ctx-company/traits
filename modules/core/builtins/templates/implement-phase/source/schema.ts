import { schema as cdkSchema } from "@ctx-traits/cdk";

export const implementationVerdict = cdkSchema.object(
  "implementation-verdict",
  {
    status: cdkSchema.decision(),
    notes: cdkSchema.text(),
  },
  { description: "The reviewer's approved/revise verdict for the implemented work." },
);
