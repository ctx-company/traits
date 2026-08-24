import { TASK_CHECK_DOCTRINE } from "@ctx-traits/agents";

// The board task format the plan family writes. This is the TaskDocument
// TOML shape the core board machinery parses — `[tasks]` dispatch,
// `port:task` binding, and dependency preflights resolve ONLY `.toml`
// documents of this shape; a markdown task file is skipped by all three.
export const TASK_FORMAT_DOCTRINE = `Write each task as one TaskDocument TOML file in .internal/tasks/, named <key>-<kebab-slug-of-title>.toml, in exactly this shape:

    schema-version = "0.2"
    key = "<KEY1 or KEY1.2 — symbolic, see the key rule below>"
    title = "<short imperative title>"
    status = "draft"
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

    [[checks]]
    name = "<what this verifies>"
    command = "<repo-relative shell command>"
    # optional: timeout-ms = <n>, expect = "<regex the combined output must match>"

Rules:
- SHAPE FOLLOWS THE WORK — three equal shapes, none preferred: a single bare task; several independent bare tasks with no parent; a charter with children. A charter is legal only when a genuine umbrella exists — a shared stop condition no single child owns. Never introduce a charter as packaging: grouping alone is a defect, not an umbrella.
- Keys are SYMBOLIC everywhere — file names, key fields, parent, depends-on, and prose alike: parents (or standalone tasks) are "KEY1", "KEY2", ... in plan order; children are "KEY1.1", "KEY1.2", .... Never write a numeric board key anywhere; a final mechanical step assigns real numbers from the live board and rewrites every occurrence, so symbolic files can never collide with existing tasks.
- A parent charter task uses a bare "KEY<n>" key and holds the goal, the work items it covers, and the stop condition; its Done when is "every child task is closed and the stop condition holds". A charter has no [relations].parent (it may still declare depends-on toward earlier charters or bare tasks).
- Child tasks are keyed "KEY<n>.1", "KEY<n>.2", ... in dependency order; every child sets [relations].parent to its charter's key, and names any prerequisite task explicitly in depends-on — a child may depend only on tasks with earlier keys. Ordinals are plain integers with no upper bound — ".10", ".11", ... — never zero-padded; "earlier keys" means numeric ordinal order (.10 is later than .9, KEY1 is earlier than KEY2).
- Size SYMMETRICALLY to the run's requested duration target (default: roughly 10-15 minutes of focused agent work): split anything materially larger into more tasks, and merge anything materially smaller into a sibling. Never write a task whose body is only running gates, tests, or checks — gates belong in the [[checks]] of the task that changed the code, never as sibling tasks.
- Judge the implied total — task count times the duration target — against the described work before writing: a plan whose total is disproportionate to what was asked is wrong even when every task is well-formed. Cover the work with the FEWEST tasks that stay within the duration target.
- Fold the scope-level grounding each task needs — affected modules or areas, applicable invariants and constraints, and existing validation gates with exact commands — into that task's own content/scope/validation, so every file stands alone. Defer execution-level decisions to architect: never prescribe exact symbols, signatures, edits, or edit sequencing.
- Omit [relations] entirely when a task has neither parent nor dependencies; never emit empty tables or empty arrays.
${TASK_CHECK_DOCTRINE}
- Never modify an existing task file; symbolic keys exist so new files cannot collide with the board, and the renumber step never touches a file without a KEY-prefixed name.`;
