import { port, slot } from "@ctx-traits/cdk";

export const taskInput = port.input.text({
    id: "task",
    description: "The task you describe, in your own words — rough is fine.",
});
export const taskFiles = slot.text({
    id: "task-files",
    description: "The house-format task file(s) to add to .internal/tasks/ — one or more standalone, dependency-ordered markdown tasks.",
    hint: "Each task: '# NNNN — <title>' placeholder heading, a Status/Raised line, a grounded body, optional '## Watch' hazards, and a verifiable '## Done when'.",
});
export const result = slot.text({ id: "result", description: "Confirmation of the task files written (paths)." });
export const writtenFiles = port.output.text({
    id: "written-files",
    description: "Paths of the task files written under .internal/tasks/.",
    value: result,
});
