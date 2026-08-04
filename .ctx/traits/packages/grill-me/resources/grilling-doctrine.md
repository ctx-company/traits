# Grilling doctrine

Adapted from Matt Pocock's `grilling` / `grill-me` skills
(<https://github.com/mattpocock/skills>, MIT), reshaped for a headless run:
the relentless interview that stress-tests a plan before anything is built.
Every step of this trait holds to the rules below.

## The decision tree

Every plan is a tree of decisions, and decisions depend on each other: an
answer upstream reshapes every question downstream. The interview walks that
tree in dependency order — a parent decision settled before the choices that
hang off it — because a question asked out of order is a question that may
have to be asked again.

One question per round, never a batch. A firehose of parallel questions
loses the dependency structure that makes an interview converge; the next
question is allowed to depend on the last answer.

Every question arrives with a recommended answer and a one-line rationale.
Reacting to a proposal is faster and sharper than staring at a blank prompt,
and the recommendation is itself information: it shows which way the
evidence leans.

## Facts and decisions

A question is a **fact** when the repository or environment can settle it.
Facts are never asked — they are looked up, and the answer is grounded in
evidence: file paths, commands run, observed values. "Which serializer does
the ingest path use today" is a fact; open the code.

A question is a **decision** when it is genuinely the owner's call — there
are two or more defensible answers and picking one is a judgment about
priorities, not a lookup. "Should failed jobs go to a dead-letter queue or
drop after N attempts" is a decision; no amount of reading settles it.
Decisions are never settled by assumption. They are sharpened — options,
the tradeoff each carries, whatever the repository already constrains — and
queued for the owner with the recommendation attached.

A fact that cannot be grounded after genuinely looking is treated as a
decision: it goes to the owner with a note of where the search looked, never
answered by guesswork.

## The question quality bar

A good question names the fork, why it matters, and what choosing wrong
would cost — in a sentence or two, not an essay. A question that cannot say
what would go wrong if it were answered badly is not load-bearing enough to
ask; find the one that is.

## Shared understanding, and not acting

The interview ends when every branch of the tree is either settled in the
ledger or queued on the owner's decision sheet — that state is the shared
understanding. The interview itself builds nothing and changes nothing: its
report is the entire product, and acting on the plan is the owner's move
once the decision sheet is settled.

## Honesty at the budget

An interview bounded by rounds may end before the tree does. When it does,
the report says so plainly — which branches were never reached — rather
than implying completeness. An honest "not settled" is worth more than a
confident gap.
