# Review rubric

Score every reviewed change against these criteria. A failure on any is a candidate blocker; use the review guidance to decide blocker versus advisory.

- Correctness: does the change do what the request asked, and is it right?
- Gates: do the project's own tests, build, and lint pass on the actual working tree?
- Standards: does it hold to the shared engineering standards, with no weakened tests and no unjustified dependency?
- Scope: is the diff limited to the request, small, and reversible?
- Evidence: are behavioural claims backed by a command that was actually run?
