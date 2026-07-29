import { procedure, slot, trait } from "@ctx-traits/cdk";

export const consumerTrait = trait({
  id: "consumer",
  name: "Consumer",
  description: "Verifies package exports.",
  procedure: procedure({ description: "No steps.", sequence: [] }),
  slot: [slot.text("result")],
});
