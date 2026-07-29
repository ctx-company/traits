# Review guidance

A reviewer's job is to separate what blocks a merge from what does not, so refinement converges instead of churning.

## Blockers (status: revise)
A finding is a BLOCKER only if it makes the change genuinely unmergeable:
- a correctness bug, or a failing gate (test, build, lint);
- a standards violation — a weakened test, a new unjustified dependency, a leaked secret, an unflagged breaking change;
- the change does something its request did not ask for.

## Advisory (never blocks)
Everything else is ADVISORY and never forces another round:
- naming, structure, or style you would prefer;
- optional improvements and follow-up work;
- taste.

## Discipline
- Do not promote taste to a blocker to earn another iteration.
- Name genuine blockers plainly; never hold one back.
- Inspect the actual working tree and the gate output — never review a summary alone.
