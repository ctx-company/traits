import { resource, trait } from "@ctx-traits/cdk";

const agentTraitsSchema = resource({
    id: "agent-traits-schema",
    path: "resources/agent-traits.schema.json",
    hint: "Generated JSON Schema for the canonical Agent Traits trait-root model.",
    trigger: "on-activation",
});

const designRubric = resource({
    id: "design-rubric",
    path: "resources/design-rubric.md",
    hint: "Design guidance for classifying and composing Agent Traits.",
    trigger: "on-activation",
});

export default trait("trait-spec", {
    version: "0.1.0",
    name: "Trait Specification",
    description: "Provides the Agent Traits schema and a grounded design rubric for trait authors.",
    metadata: {
        tag: ["first-party", "knowledge", "meta-trait", "protocol"],
    },
    resource: [agentTraitsSchema, designRubric],
});
