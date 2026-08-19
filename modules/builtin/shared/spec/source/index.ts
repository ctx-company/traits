import { resource, trait } from "@ctx-traits/cdk";

const designRubric = resource({
  id: "design-rubric",
  path: "resources/design-rubric.md",
  hint: "How to classify a trait, model its dataflow, and name its identifiers.",
  trigger: "on-activation",
});

export default trait("spec", {
  version: "0.1.0",
  name: "Trait Design Spec",
  description: "The design rubric trait authors are judged against: derived kind, speech acts, dataflow, and naming.",
  metadata: {
    tag: ["first-party", "knowledge", "meta-trait", "protocol"],
  },
  resource: [designRubric],
});
