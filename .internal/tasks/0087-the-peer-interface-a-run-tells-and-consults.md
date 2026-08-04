# 0087 — The peer interface: a run tells and consults, and the host performs the effect

**Status:** ready to implement · **Depends on:** 0069 (Spec, renderer, delivery log), 0084 (the wait bound) · **Raised:** 2026-08-04 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

First slice of the peer arc, and the whole design. Everything after it is a transport.

0069 gives the *host* a way to report about a run it may have outlived. This gives the *run* a way to
speak while it is alive — and to ask something and use the answer. 0086 already does exactly this,
by hand, with one hardcoded binary; this is what makes it a property of the system rather than a
property of one trait.

## Decisions

- **Two verbs, one interface: `tell` and `consult`.** Direction is a declared property of a peer, not
  a second abstraction. A peer that can only be told things and a peer that can only answer are the
  same trait with different capability flags.
- **`consult`, not `ask`.** `ask` is taken, by the opposite thing: 0009 and 0054 own it for the owner
  questioning a live run through the guide pane — read-only, ephemeral, structurally forbidden from
  touching the ledger. Reusing the word for a typed, recorded, outward call would collide on exactly
  the property that matters.
- **The run never performs the effect.** A step emits a typed request; the host observes it in the
  drive path, performs it with its own credentials outside the sandbox, and writes the result back as
  an accepted slot value. No network in the worktree, no token in the sandbox, one delivery path
  shared with 0069. Every other property in this file follows from this one move.
- **The request record exists and is inspectable before the effect fires.** That ordering is what
  makes mediation possible at all — a policy layer decides on a typed record rather than intercepting
  a socket, and a denial is a recorded first-class outcome rather than a network error nobody can
  attribute. It costs nothing to guarantee now and cannot be retrofitted cheaply.
- **A `consult` is a step, with everything that implies.** Round 2 of a loop consults again and gets
  its own entry and its own digest; it never reuses round 1's answer. Caching is keyed by execution,
  never by declaration site. Replaying a *recorded* run replays recorded answers — that is the same
  rule, not an exception to it.
- **A `tell` collapses across iterations; a `consult` never does.** A tell upserts on its site key, so
  twelve rounds edit one message instead of posting twelve. Repeating yourself is spam; reusing an
  answer is a bug.
- **The trait declares a role, the repo declares the binding, `doctor` matches them.** Trait source
  names `"docs"` and a reply shape; `[[peer]]` says what answers. Same split 0069 set for ports and
  routing, and the third instance of it. A trait consulting a peer that cannot answer, or whose
  schema does not fit, fails at `doctor` — before a run, before spend.
- **A `consult` declares both shapes, request and reply.** Checking one direction catches half the
  mismatches, and the half it misses is the one that fails mid-run.
- **`evidence` is references, never prose.** A consult points at slots and ports the run already
  holds; it does not author a description of them. A question surface is therefore 0068's brief
  narrowed to a subset — one renderer, and the question cannot drift from the run's actual state.
- **Two plain fields, no taxonomy: `wait-ms` and `deferred`.** How long to hold, and what an
  unanswered consult *means*: a fault (park with a blocker, like any failed command step) or an open
  question (park with something still answerable). Latency was never the real property. `wait-ms`
  reuses 0084's per-step budget rather than inventing a third notion of a bound.
- **Unbound roles: required fails, optional degrades.** A required role with no `[[peer]]` fails at
  `doctor`. An optional one falls back to the `local` peer, visibly, in `doctor` and in the run
  record — so a fresh clone with no token still shows you the messages instead of swallowing them.
- **It reuses 0069 wholesale.** Same `Spec`, same `render(&State, &Spec)`, same backend
  implementations, same append-only delivery log — widened from `(session, channel)` to
  `(session, peer, site)`. A `tell` is an irreversible outward effect with the same replay hazard as
  a handoff delivery, so it takes the same idempotency key rather than a second log.
- **The `local` peer ships first.** Messages and questions appear in the live view and the dashboard:
  no network, no secrets, no config, and it exercises request → host → effect → ledger row → replay
  end to end before any transport exists. Same move as 0069's file channel and 0060's files backend.
- **Policy stays in the trait.** 0086's escalation ladder — the owner is summoned only on a round the
  machine reviewer already approved, so the run *earns* the right to ask — is a `when` guard in a
  procedure. The interface supplies the mechanism and takes no position on when it is used.

## Scope

The `Peer` trait and its verbs; `tell` / `consult` as CDK authoring surfaces with typed request and
reply schemas and reference-only `evidence`; the request record and the host's perform-and-write-back
path in the drive loop; `wait-ms` and `deferred` per peer; `[[peer]]` config with `resolve()` at
`doctor` time and role-to-binding matching against declared capabilities; the delivery log widened to
`(session, peer, site)`; the `local` peer; `--dry-run` covering both verbs.

## Watch

- **The perform-and-write-back path runs while a frame is open.** It is the first thing in the drive
  loop that blocks on something outside the machine, so it must be bounded, cancellable and visible
  in the live view — a silent hold is indistinguishable from a hang (0084's whole subject).
- Do not let `tell` grow a template language. 0073 already refused this and the reasoning is
  unchanged: config selects shape, it never authors prose.
- Widening the delivery log's key touches 0069's rows. Do it in one migration with the sequence
  numbers 0070 adds, not in two passes.
- Keep the request record out of the session ledger's contract-validated surface. 0050 is the
  standing lesson, and this adds a third producer to exactly the shape that failed.
- A denied or failed `tell` is not a failed run (0069). A failed `consult` *is* a failed step, because
  the run's next frame depends on it. The asymmetry is deliberate; do not flatten it.

## Done when

A trait can `tell` and `consult` a named role with typed shapes; the run emits requests and never
performs an effect itself; the host performs, records, and writes replies back as accepted slot
values; a request record is inspectable before its effect fires; a consult inside a loop asks again
each round while a tell edits one message; `wait-ms` bounds the hold and `deferred` decides whether
an unanswered consult parks as a fault or as an open question; a required unbound role fails at
`doctor` and an optional one degrades to `local` visibly; and the `local` peer proves the whole path
with no network.
