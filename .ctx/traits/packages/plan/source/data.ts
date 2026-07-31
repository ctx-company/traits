import { port, slot } from "@ctx-traits/cdk";

export function planData(includeGrounding: boolean) {
    const taskInput = port.input.text({ id: "task", description: "The task you describe, in your own words — rough is fine." });
    const grounding = includeGrounding ? slot.text({
        id: "grounding",
        description: "Codebase grounding for the described work: the concrete files and modules it touches, the repo's validation gates (exact commands), and the invariants, rules, and constraints the task files must honor.",
        hint: "Grounded in the codebase: what the project is, its build/test/lint gates (exact invocations), architectural invariants, dependency rules, and constraints an implementer and reviewer must honor. Context the tasks carry, not the step-by-step plan.",
    }) : undefined;
    const taskFiles = slot.text({
        id: "task-files",
        description: "The house-format task file(s) to add to .internal/tasks/ — one or more standalone, dependency-ordered markdown tasks.",
        hint: "Each task: '# NNNN — <title>' placeholder heading, a Status/Raised line, a grounded body, optional '## Watch' hazards, and a verifiable '## Done when'.",
    });
    const result = slot.text({ id: "result", description: "Confirmation of the task files written (paths)." });
    const writtenFiles = port.output.text({
        id: "written-files",
        description: "Paths of the task files written under .internal/tasks/.",
        value: result,
    });
    return { taskInput, grounding, taskFiles, result, writtenFiles };
}
