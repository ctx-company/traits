import * as shared from "#trait/shared/index.ts";

import { clerk } from "../agent.ts";
import { taskBoard } from "../resource.ts";

export function extract(): void {
  shared.stage.frame.extractTaskContract(clerk, taskBoard);
}
