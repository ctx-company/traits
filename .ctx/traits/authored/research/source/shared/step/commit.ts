import { familyCommitTail } from "@ctx-traits/agents";
import type { AgentHandle, PromptTemplate } from "@ctx-traits/cdk";

import { commitOutput } from "../data.ts";

/** Thin wrapper over familyCommitTail (0153 recipe) fixing the shipping-tail id and receipt shared by every research variant. */
export function shippingCommitTail(scribe: { readonly agent: AgentHandle; readonly text: PromptTemplate }): void {
  familyCommitTail({ id: "shipping", scribe, receipt: commitOutput });
}
