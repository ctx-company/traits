# Engineering standards

The standards every reviewed change is held to. They are intentionally generic: a team adapts them by editing this one versioned resource, not by pasting rules into each prompt.

## Correctness
- Correctness beats cleverness. Prefer the change that is simpler to verify over the one that is shorter.
- Every behavioural change is covered by a test that fails before the change and passes after it.
- Never weaken, skip, or delete a test to make a suite pass. If a test is wrong, fix the test and state why.

## Scope
- A change does exactly what its request asks — no drive-by refactors, no unrelated renames.
- Prefer small, reversible diffs. If a change must be large, split it into reviewable steps.

## Safety
- No secrets, tokens, or credentials in code, tests, or fixtures.
- No new external dependency without a stated reason; prefer what the project already has.
- Public interfaces are additive by default; call out any breaking change explicitly.

## Evidence
- Claims about behaviour are backed by a command that was actually run — a test, a build, a linter — not by assertion.
- Report what changed (files), how it was validated, and any open concern.
