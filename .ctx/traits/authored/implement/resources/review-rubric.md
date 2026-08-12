# Review rubric

Score the implemented work against these three dimensions every round. Each dimension carries a mandatory first finding plus any further findings — a dimension with nothing wrong still gets that first finding recording what was checked and confirmed. Every finding cites a concrete repo-relative file and line — a claim without both is not evidence and must not be reported.

- Scope: does the diff implement exactly what the task contract asks, nothing more and nothing less?
- Correctness: is the implemented behavior actually right — no bugs, no broken invariants, no untested claim?
- Gates ran: were the gates this task's own Done-when names actually run against the current tree, with evidence of their result?

These dimensions are review evidence, not a second blocker channel: status and blockers remain the sole scalars the loop guards read. A finding that reveals a genuine failure on any dimension is also recorded as a typed blocker in the same verdict — the dimension findings document what was checked and why, the blocker list is what must be fixed. status is approved if and only if blockers is empty, exactly as before; the richer dimensions never change that rule on their own.
