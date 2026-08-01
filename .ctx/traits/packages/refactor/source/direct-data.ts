import { port, slot } from "@ctx-traits/cdk";
import { directAnnotations } from "./direct-schema.ts";

export const directAnnotationsSlot = slot({
    id: "annotations",
    schema: directAnnotations,
    description: "Typed ctx-annotate output, validated against this schema before use.",
});
export const directChecklist = slot.text({
    id: "checklist",
    description: "Ordered checklist: one actionable item per annotation with file, location, and change.",
});
export const directWorkReport = port.output.text({
    id: "work-report",
    description: "Final report of the implemented annotation checklist.",
});
