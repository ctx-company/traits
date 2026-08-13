import type { Behavior } from "@ctx-traits/cdk";
import { method, tone, verbosity } from "@ctx-traits/cdk";

export const tag = ["task", "plan", "bootstrap", "planning"];
export const behavior: Behavior = { tone: [tone.Direct, tone.Technical], method: method.EvidenceFirst, verbosity: verbosity.Brief };
