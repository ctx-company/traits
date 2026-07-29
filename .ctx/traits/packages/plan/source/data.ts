import { port, slot } from "@ctx-traits/cdk";

export function planData(includeProductContract: boolean) {
    const taskInput = port.input.text({ id: "task", description: "The task you describe, in your own words — rough is fine." });
    const productContext = includeProductContract ? slot.text({
        id: "product-context",
        description: "The product contract for .docs/PRODUCT.md — project overview, conventions, validation gates, invariants, and the house rules this work must respect.",
        hint: "Grounded in the codebase: what the project is, its build/test/lint gates (exact commands), architectural invariants, dependency/unsafe-code/stability rules, and constraints an implementer and reviewer must honor.",
    }) : undefined;
    const executionPlan = slot.text({
        id: "execution-plan",
        description: "The phased plan for .plans/EXECUTION_PLAN.md — a group of small dependency-ordered phases.",
        hint: "One '## Group ...' with phases as '- [ ] **P1** — title' bullets; each phase ~10-15 min of agent work, with a Markdown checklist and a Definition of Done.",
    });
    const result = slot.text({ id: "result", description: includeProductContract ? "Confirmation of the files written (paths)." : "Confirmation of the file written (path)." });
    const writtenFiles = port.output.text({
        id: includeProductContract ? "written-files" : "written-file",
        description: includeProductContract ? "Paths of the files written (.docs/PRODUCT.md and .plans/EXECUTION_PLAN.md)." : "Path of the file written (.plans/EXECUTION_PLAN.md).",
        value: result,
    });
    return { taskInput, productContext, executionPlan, result, writtenFiles };
}
