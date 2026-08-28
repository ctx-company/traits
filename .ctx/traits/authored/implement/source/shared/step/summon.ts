import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

import { scribe } from "../agent.ts";
import { slot } from "../data.ts";

export const ask = cdk.defineStep.ask((sig: typeof agents.needsOwnerSignal) => ({
  when: sig,
  input: cdk.input.prompt`
    This run cannot settle a question on its own and is parked until you answer.
    Why only you can settle it: ${sig.reason}
    The question: ${sig.question}
    Answer in one or two sentences — the run resumes with your answer as evidence.
  `,
  output: cdk.schema.text(),
}));

export const record = cdk.defineStep.prompt(
  (sig: typeof agents.needsOwnerSignal, ruling: cdk.SlotHandle<string>) => ({
    agent: scribe,
    input: cdk.input.prompt`
      The owner answered a summons raised during this run.
      The question put to the owner: ${sig.question}
      Why it needed the owner: ${sig.reason}
      The owner's answer: ${ruling}
      Write exactly ONE entry recording this ruling — the question and what the owner decided, in one or two sentences, no preamble.
    `,
    output: slot.ownerDecisions.with(cdk.operation.Append),
  }),
);
