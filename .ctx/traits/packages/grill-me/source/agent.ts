import { agent as cdkAgent } from "@ctx-traits/cdk";

const interrogator = cdkAgent("smart-1", {
    description:
        "Strong questioning model: walks the plan's decision tree in dependency order and asks the single most load-bearing unresolved question each round, always with its own recommended answer attached.",
    summary: "Interrogator role.",
});
const scout = cdkAgent("worker", {
    description:
        "Tool-equipped scout: settles fact questions by exploring the repository and environment, sharpens decision questions into options for the owner without ever deciding them, and keeps the interview ledger.",
    summary: "Scout and ledger-keeper role.",
});
const synthesist = cdkAgent("smart-2", {
    description:
        "Strong synthesis model from a second family: reads the finished interview cold and writes the grill report — the shared understanding, the owner decision sheet, and an honest account of anything the interview never reached.",
    summary: "Report role.",
});

export const agent = { interrogator, scout, synthesist };
