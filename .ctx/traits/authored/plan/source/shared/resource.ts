// The board task format the plan family writes. This is the TaskDocument
// TOML shape the core board machinery parses — `[tasks]` dispatch,
// `port:task` binding, and dependency preflights resolve ONLY `.toml`
// documents of this shape; a markdown task file is skipped by all three.
export const TASK_FORMAT_DOCTRINE = `Write each task as one TaskDocument TOML file in .internal/tasks/, named <key>-<kebab-slug-of-title>.toml, in exactly this shape:

    schema-version = "0.2"
    key = "<NNNN or NNNN.X>"
    title = "<short imperative title>"
    status = "ready"
    raised = "<YYYY-MM-DD>"
    content = """
    What this task is and why it exists — grounded, concrete, standalone.
    """
    scope = """
    ## Decisions
    The rulings made; what is in scope and what is explicitly out.
    """
    validation = """
    ## Watch
    Hazards and constraints an implementer must not trip.
    ## Done when
    The observable, verifiable condition proving the task is complete.
    """
    [relations]
    parent = "<NNNN>"
    depends-on = ["<key>", "<key>"]

Rules:
- A parent charter task uses a bare "NNNN" key and holds the slice's goal, the work items it covers, and the slice's stop condition; its Done when is "every child task is closed and the stop condition holds". A charter has no [relations].parent (it may still declare depends-on toward earlier slices' charters).
- Child tasks are keyed "NNNN.1", "NNNN.2", ... in dependency order; every child sets [relations].parent to its charter's key, and names any prerequisite task explicitly in depends-on — a child may depend only on tasks with earlier keys.
- Size every child task to roughly 10-15 minutes of focused agent work — small enough to finish in one short pass; split anything larger into more children.
- Fold the grounding each task needs — its concrete files, validation gates with exact commands, invariants, constraints — into that task's own content/scope/validation, so every file stands alone.
- Omit [relations] entirely when a task has neither parent nor dependencies; never emit empty tables or empty arrays.
- Never overwrite or renumber an existing file; keys are assigned once from the derived next-free key and are final.`;
