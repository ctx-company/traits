import { port as cdkPort, slot as cdkSlot } from "@ctx-traits/cdk";

import * as schema from "./schema.ts";

const annotations = cdkSlot({
    id: "annotations",
    schema: schema.annotations,
    description: "Typed ctx-annotate output, validated against this schema before use.",
});

const checklist = cdkSlot.text({
    id: "checklist",
    description: "Ordered checklist: one actionable item per annotation with file, location, and change.",
});

const workReport = cdkPort.output.text({
    id: "work-report",
    description: "Final report of the implemented annotation checklist.",
});

export const slot = { annotations, checklist };
export const port = { workReport };
