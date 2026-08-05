# Future ideas — parked, not phases

Not in the execution plan and not on the board's queues. Items here are recorded so the
reasoning is not lost, and are only promoted back into EXECUTION_PLAN.md on an explicit
owner order. Nothing here is dispatchable.

## Frame persona envelope (was P495, moved out 2026-07-25)

**Idea.** Mode A driven frames carry NO behavioral layer today: a frame is title +
instructions + inputs + output contract, and the trait's Intent/Behavior material never
reaches the driven agent — only the per-role `[[agent]] system` string does. Group 119's
render v2 rule 8 (one shared section renderer, two envelopes) would make `<persona>` — the
same `<intent>`/`<behavior>` blocks the behavioral render emits — available to
`frame_prompt`, so the reviewer/worker frames would carry the trait's doctrine verbatim and
digest-tracked, exactly as a Mode B injection does.

**Shape if ever built.** `frame_prompt` composes `<persona>` from the P493 emitters ahead of
`<instructions>`/`<inputs>`/`<output-contract>`; `ctx traits preview` shows the same. Behind
a switch, off by default until measured.

**Why parked (owner, 2026-07-25).** "I don't care about frame envelope" — and the real
frame-rendering question is bigger than a persona block: driven frames will likely want
richer structural rendering (ASCII diagrams, call trees, structured evidence layouts) before
a persona layer matters. Revisit as part of that larger frame-rendering effort, not as a
rider on the behavioral render.

**Cost/risk when revisited.** Touches live drive prompts — a behavioral change to every
driven frame in every run; measure per-frame token cost before defaulting it on.

**Depends on.** P493 (the shared emitters).

## ctx-runtime — the owned frame executor (strategy locked 2026-07-26; not a project yet)

**Sequencing, owner-stated:** (1) get process/typed traits great first; (2) Flue integration
(`useTrait` + inverted `useProcedure` — see `.docs/research/COMPETITIVE_FLUE.md`); (3) build
ctx-runtime when ctx-gate forces it. Not before.

**What it is.** A frame executor, not a coding agent: provider-streaming model calls + a small
bounded tool set + `ctx-sandbox` (P480) confinement + a **turn-level durable event log inside
the frame-level ledger** (every model call, tool call, result appended — event sourcing, the
same pattern as Flue's Durable Streams). Pause-mid-frame = stop appending; resume = replay;
distributed = the log lives in shared storage and any worker picks it up. The loop for a
narrow typed frame is much simpler than a general agent's: bounded tools, bounded turns,
schema-validated exit.

**Why it becomes inevitable.** (a) ctx-gate needs a runtime that is DISTRIBUTED and PAUSABLE —
today a frame is an atomic harness subprocess; in-flight state (conversation, partial tool
loop, context window) lives in someone else's process, so pause means restart and compaction
happens TO us. (b) Every harness incident this window (narrator redaction, P464 session
identity, P427 resume eligibility, background-task escapes) was a harness-contract failure —
reverse-engineering four undocumented runtimes that change in point releases. (c) When you own
the loop, **what is in the context window on every call is a pure function of the ledger** —
compaction becomes a declared, receipted operation (or unnecessary for narrow frames), and
"prove the loop" extends to **"prove what the model saw."** The receipts thesis completing
itself. (d) Mid-frame `ask` (the model pausing partway through work to ask a human) becomes a
natural unlock — Group 124 cannot express it and correctly does not try.

**What it is NOT.** Not a coding agent (no TUI, no interactive UX, no human extension
ecosystem — Pi serves humans; we serve frames). Not a Pi competitor: Pi is deliberately very
thin and we are not building its category; evaluate Pi as a BASE vs building from zero when
the time comes. Not a TS framework ("our own Flue" is the wrong frame — Flue is the authoring
DX layer, and our authoring layer already exists: the trait + CDK). Thinner than Pi where Pi
serves humans; thicker where Pi is thin (durable turn log, distributed state, deterministic
context assembly, first-party activity events).

**Product split that resolves the economics.** ctx-trait rides BYO harnesses FOREVER as the
default — subscriptions and adoption live there. ctx-gate rides ctx-runtime — CI/hosted
contexts are API-key-native, nobody installs a subscription harness into a pipeline. Two
execution substrates, one procedure engine, one receipt model.

**Guardrail 1 — scope creep tell.** NOTHING pre-launch may depend on mid-frame pause.
Frame-boundary pause (P508/P509) suffices for everything chartered. Any phase whose contract
quietly requires owning the loop is rejected on sight until ctx-runtime is a real project.

**Guardrail 2 — P504 is the seam.** The normalized activity model is the contract both worlds
share: for the four harnesses it is an adapter over reverse-engineered streams; for
ctx-runtime it is first-party truth. Build everything against it and the runtime slots in
later as another (better) implementation of the same contract, with nothing above it changing.

**Boundary doctrine (owner, 2026-07-26).** WASM stays a thin authoring layer (pure core:
decode/validate/render/resolve at most — and audit the wasm-core ABI's existing
context-pack/reconcile exposure against this line); the runtime stays Rust. The layer stack is
being claimed — Flue the framework, Pi the harness, Cloudflare the platform. Nobody claims
process + governance (typed procedures, validated handoffs, gates, receipts). That layer is
ours, it appreciates as the layers below commoditize, and ctx-runtime exists to SERVE it —
if the runtime ever becomes the product, we have built a worse Pi and abandoned the only
layer nobody is fighting for.
