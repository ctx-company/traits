import type { AgentHandle } from "@ctx-traits/cdk";
import { input } from "@ctx-traits/cdk";

import { doneCriteria, taskInput, workItems } from "../data.ts";

/**
 * Extract the source's own contract: its units of demanded work and its
 * explicit done criteria. Coverage — every work item reaching at least one
 * slice — is judged against these, not against the plan's own decomposition.
 */
export function contract(agent: AgentHandle) {
  return agent.prompt("Ingest the work source", {
    id: "ingest",
    input: input.prompt`
            Read the source of the described work: ${taskInput}. When it references a document (a research report, an MVP plan, a spec) by path, read that document from the repository; otherwise the description itself is the source.
            Extract two things, faithful to the source's own words — never your preferred framing:
            1. Its work items: every unit of work the source demands (an MVP slice, an enumerated deliverable, a named milestone), each with the source's own requirement for it. When the source does not enumerate, distill 2-8 items from its prose.
            2. Its explicit definition-of-done items (a stop condition, acceptance criteria), empty when it states none.
            Do not implement anything and do not plan yet — this step only records what the source demands.`,
    output: [workItems, doneCriteria],
  });
}
