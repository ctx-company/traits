# 0136 — Trust v2: append-only approval history, start-time pinning, one canonical home for shared traits

**Status:** ready to implement (verify partials first — the session summary output already shows `trust-approval { seq, approved-at }` evidence, so part of the model may have landed) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P534 + P535, owner-settled semantics 2026-07-26)

Incident basis: two repos carried hand-copied, independently-drifting copies of one
trait id; the name-keyed one-lineage trust rule silently revoked the sibling's
approval on every approve — approval ping-pong while `trust list` said "verified"
and the run refusal said "no verified record".

**Mechanism (was P534), the owner-settled four-part model:**
(1) APPEND-ONLY HISTORY: every approval is a permanent record
`(trait-id, digest, approved-at, seq)` — seq monotonic under the store's flock;
the FACT of an approval is never deleted. (2) DERIVED CURRENCY: newest approval
per trait id is CURRENT; older approved digests are SUPERSEDED — starting a run on
one refuses with "approved <t1>, superseded by <digest> at <t2> — re-approve to
run this version" (re-approving IS the deliberate, journaled downgrade).
(3) PIN AT START, verbatim doctrine: the trust gate evaluates EXACTLY ONCE, at
session start, recording digest + approval seq into run provenance — a started
session never re-checks trust (not at resume, not at call/set/next, not after a
days-long pause). `preflight_lifecycle_trust` moves from every mutating verb to
session creation only. (4) `trust block` gates NEW STARTS only — it reports live
sessions running that digest, stops none of them; stopping work is always a
separate explicit human act.

Guards: (a) approve REFUSES when the package's lock canonical ≠ the computed
canonical of the bytes being approved; (b) approve states what it supersedes, by
digest and age; (c) approve warns on uncommitted trait content; (d) the preflight
refusal distinguishes no-record / superseded / blocked, each with digests;
(e) `trust approve --all-current` for deliberate store-wide rebuilds.

**Practice (was P535):** one canonical home for shared dogfood traits — the shared
family gets ONE home (this repo) and sibling repos consume it through the product
(lifting the `path:` install spec from its typed NotYetSupported refusal is the
natural fit), never `cp`. Propagation stays explicit (no-silent-propagation law):
a rebuild here reaches a sibling only via its own `update`. Sequencing: verify the
fold state (0134) first, then share.

## Watch

- Store schema evolution must migrate existing records cleanly.
- The cross-repo consumption step in the sibling repo is an owner-run command this
  task documents — config edits scope to the cwd repo (standing rule).
- Digest identity across repos becomes an assertable check, not a hope.

## Done when

The owner's three scenarios pass as written (update-unapproved: in-flight run
finishes, new runs blocked; update-approved: in-flight finishes on its pin;
downgrade: refuses as superseded until the one journaled re-approve); a session
paused for days resumes with zero trust consultation; blocking reports live
sessions and stops none; approve on lock-drifted or uncommitted bytes says so; the
receipt of any completed run proves its start-time approval from the ledger alone;
the sibling repo consumes the shared family with no hand-copied files.

Full original contract: `archived/board/execution-plan.md` (Group 130, P534/P535).
