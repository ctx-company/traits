import { port as cdkPort, resource as cdkResource, slot as cdkSlot } from "@ctx-traits/cdk";

import * as schema from "./schema.ts";

const plan = cdkPort.input.text({
    id: "plan",
    description:
        "The plan, design, or decision to grill — inline text, or the repo-relative path of a file (a task file, a draft, a spec) the interview reads first.",
});

const doctrine = cdkResource({
    id: "grilling-doctrine",
    path: "resources/grilling-doctrine.md",
    hint: "The interview doctrine every step holds to; agents read this file with their own tools.",
    trigger: "on-activation",
});

const probe = cdkSlot({
    id: "probe",
    schema: schema.probe,
    description: "The current round's probe: the single question under interrogation, or the typed exhausted signal that ends the interview.",
});
const ledger = cdkSlot.text({
    id: "ledger",
    description:
        "The cumulative interview ledger: every settled round, appended one entry at a time by the scout and reproduced verbatim between rounds. The report's single source of truth.",
    hint: 'One "### Q<n> — fact|decision" block per round, in interview order: the question, the recommended answer, then the resolution — "Answered:" with evidence (paths, commands, observed values) for a fact, "Owner decision:" with options and tradeoffs for a judgment call. Prior entries are reproduced verbatim; the ledger only grows.',
});
const report = cdkSlot.text({
    id: "report",
    description: "The finished grill report: shared understanding, the owner decision sheet, and what was not settled.",
});

const grillReport = cdkPort.output.text({
    id: "grill-report",
    title: "Grill Report",
    description: "The interview's only product: the sharpened shared understanding, the owner decision sheet, and an honest account of anything not settled.",
    format: ["text"],
    value: report,
});

export const port = { plan, grillReport };
export const resource = { doctrine };
export const slot = { probe, ledger, report };
