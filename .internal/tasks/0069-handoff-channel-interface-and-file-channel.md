# 0069 — The handoff channel interface, with the file channel as its first implementation

**Status:** ready to implement · **Depends on:** 0068 · **Raised:** 2026-08-03 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Second slice of the handoff arc, and the whole design. Everything after it is an adapter.

A run currently ends and nothing leaves the machine. This gives a finished run *effects* — a file, a
PR body, a Slack card — through one interface, without any of them being special-cased.

## Decisions

- **Emission is not state, and the two interfaces stay apart.** The task provider (0060) reads a
  backend that is the source of truth and can correct itself with `sync`. A channel writes to a
  destination we can never read back; no amount of syncing un-sends a message. Same house pattern
  (small verb set, config-declared backends, composition above, `resolve` at `doctor` time),
  deliberately different contract.
- **Verbs: `resolve · capabilities · deliver`.** No `list`, no `history`, no `read`. Read-back means
  an inbox, an inbox is state, and state already has a provider. A reply path (approving from Slack)
  is ingress — untrusted input with an authorization question — and is out of this interface by name.
- **The core declares only what the core consumes.** A channel declares the shape it accepts
  (`Spec { fidelity, wire, budget_bytes, attachments }`) because the renderer reads it: the router
  asks `capabilities()`, renders to that spec, and delivers. No channel renders; no renderer knows
  about channels.
- **Repeat behaviour is NOT core.** The router's only stake in a second delivery is whether a prior
  reference exists for this key and whether the channel wants it handed back. What the channel then
  does — Slack's `mode = "send" | "update" | "thread"`, a file's `append`, a PR body's fenced
  rewrite — is the channel's own vocabulary. There is no `Repeat` enum: guessing a shared repeat
  language for transports nobody has written yet constrains authors for no gain.
- **Each channel publishes an options schema and `doctor` validates config against it.** That is what
  keeps per-channel freedom from decaying into unchecked free-form config — a typo'd
  `mode = "thred"` fails at `doctor` because the Slack channel declared the enum. An options schema
  is data, so a WASM guest can export one later (0074) exactly as it exports `Spec`.
- **Delivery is a host act, never an in-run step.** The runs most worth hearing about are the ones
  that died hard — killed at a bound, crashed, parked at round 1 — and an in-run step cannot fire on
  any of them. It also keeps secrets out of the worktree and the network away from the sandbox, and
  it follows from 0060's capability split: runs hold read-only there and hold nothing here.
- **The delivery log is STORED and append-only**, keyed `(session, channel)`, carrying the external
  `reference` (file path, PR number, Slack `ts`). This is the one place in the system where a stored
  fact beats a derived one, because the other copy is outside the system and cannot be recomputed.
  0050 is the standing lesson: a fact recomputed on every transition re-fires on every resume.
- **A failed delivery is never a failed run.** Recorded with its reason, surfaced, and the run's
  outcome and exit code are untouched. Channels are independent: one failing does not stop the rest.
- **Channels fire in declared order and each receives the prior receipts.** One field on the
  envelope, and it is the difference between three unrelated notifications and one coordinated
  handoff — it is what lets the Slack card cite the PR the PR channel just created.
- **Routing lives in the repo, not the trait.** `[handoff]` in `.ctx/traits/runtime.toml`, secrets by
  env-var reference resolved in the host process. Same argument 0058 already won for gate bounds:
  which Slack channel to post to is not a property of a portable trait. The trait declares what it
  produces (ports); the repo declares where it goes; the binary matches them.
- **`--dry-run` is the posture, not a convenience.** Sending publishes irreversibly to people, so the
  exact bytes and destination must be printable without sending, and a channel must be declared in
  committed config before anything reaches it.
- **The `file` channel ships first**, writing the rendered brief into the run store. Zero config,
  zero secrets, zero network — and it exercises the entire pipeline, which is what makes every
  renderer snapshot-testable. Same move as 0060's files backend.

## Scope

The `Channel` trait and its three verbs; `Spec` / `Rendered` / `Envelope` / `Receipt` and the
per-channel options schema;
the append-only delivery log in the run store; the `[handoff.channel]` and `[handoff.route]` config
tables with `doctor --config` provenance and `resolve()` validation; the `file` channel;
`ctx traits handoff <session> [--channel …] [--dry-run]` running the same code path the host runs.

## Watch

- **Updating in place requires the channel to read its own destination** (does a PR already exist for
  this branch). That is a channel reading the world, not the interface exposing read-back to callers —
  `deliver` stays the only verb. Do not let this leak upward into a fourth verb.
- Secrets: env-var *reference* in config, resolved in the host, never printed. `doctor` reports
  whether a token resolves without echoing it, and `--dry-run` output must be safe to paste.
- A brief renders run content — paths, diffs, prompt fragments. Default to links and paths over
  content dumps, and never auto-attach a raw transcript. This is an outward-facing surface.
- Manual `ctx traits handoff` and automatic host delivery must be ONE code path. Two paths means the
  thing you can debug is not the thing that runs at 3am.
- Keep the delivery log out of the session ledger. The ledger is contract-validated on every
  transition (0050); adding an append-only side log to it invites exactly that class of failure.
- The log's key widens to `(session, peer, site)` when 0087 lands, and 0070 adds sequence numbers to
  the same rows. Two migrations of one table is avoidable if the row shape is chosen with both in
  view now.

## Done when

A `Channel` implementation is reachable through three verbs and nothing else; `[handoff]` config
resolves and validates at `doctor` time with a typo'd target failing there rather than after a run;
a terminal session fans out to its routed channels in declared order with each receiving the prior
receipts; `--dry-run` prints the exact bytes and destination and sends nothing; the file channel
writes a brief with no network; a failed channel is recorded without affecting the run; and the
delivery log records an external reference per `(session, channel)`.
