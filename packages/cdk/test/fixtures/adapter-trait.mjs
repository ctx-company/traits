import { agent, input, port, procedure, schema, sequence, slot, trait } from "@ctx-traits/cdk";
import { z } from "zod";
import { zodToJsonSchema } from "zod-to-json-schema";

const result = schema.zod("result", z.object({
  name: z.string(),
  enabled: z.boolean(),
  count: z.number().optional(),
}), {
  // `zod-to-json-schema` emits a top-level `$schema` key by default, which
  // is outside the documented/enforced `schema.zod` subset (type,
  // properties, required, items, enum, description,
  // additionalProperties === false only); a real adapter callback strips it.
  toJsonSchema: (value) => {
    const { $schema: _schema, ...rest } = zodToJsonSchema(value);
    return rest;
  },
});
const worker = agent("worker", { description: "Produces the adapter result." });
const output = slot.of({ id: "result", schema: result });
const resultPort = port.output.of({ id: "result", schema: result, value: output });

export const draft = trait({
  id: "adapter-smoke",
  name: "Adapter Smoke",
  description: "Validates a Zod-backed CDK schema through the CLI boundary.",
  procedure: procedure({
    description: "Builds an adapter-backed result.",
    output: resultPort,
    sequence: sequence.prompt({
      id: "build-result",
      agent: worker,
      prompt: input.prompt`Return one result.`,
      output,
    }),
  }),
  schema: [result],
});
