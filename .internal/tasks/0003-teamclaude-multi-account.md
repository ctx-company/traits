# 0003 — TeamClaude: multi-account Claude routing under ctx runs

**Status:** ready to implement · **Researched:** 2026-07-28 (upstream README, `KarpelesLab/teamclaude`, master, 181★, updated 2026-07-28) · **Owner ask:** "research how we can integrate teamclaude into the mix, so people with multi accounts like me can run it"

## What it is

A **self-hosted local proxy** that holds several Claude accounts and rotates between them on quota. Not a fork of Claude Code, not a wrapper library — a process that sits between the `claude` binary and Anthropic.

- Install: `npm install -g @karpeleslab/teamclaude`
- Config: `~/.config/teamclaude.json` (accounts with OAuth tokens, `switchThreshold` default `0.98`, proxy port)
- Reads `anthropic-ratelimit-unified-*` response headers, switches accounts round-robin at threshold
- **Model-aware**: an account exhausted for Fable is skipped for Fable only, still serves other models
- Distinguishes 429-quota (switch account) from 429-throttle (pause that account), with storm control when moving onto a fresh account

## VERIFIED 2026-08-04 — headless `claude -p` works through the proxy, live on this machine

The question this round of research answered: does teamclaude support the way
ctx actually spawns claude — `claude -p` one-shots, no TUI, no `teamclaude run`
wrapper. **Yes, proven end to end** against the running server (v1.1.11, all
three accounts logged in), with each claim tested rather than read off a README:

- **`claude -p` honors `ANTHROPIC_BASE_URL` for `/v1/messages`.** Proven two
  ways: a local capture sink received claude's `POST /v1/messages?beta=true`
  (claude 2.1.220, headless), and a dead-port base URL hangs the call rather
  than falling through to api.anthropic.com. The `--output-format stream-json
  --verbose` shape ctx uses for worker seats works identically, and the
  stream still carries `rate_limit_event` — the P556 signal survives proxying
  (it now describes the INJECTED account, which is the operationally useful
  one; the "do not develop P556 against the proxy" warning below stands).
- **The proxy REPLACES the client's auth unconditionally on `/v1/messages`.**
  The machine's logged-in claude identity is irrelevant: a request with a
  bogus bearer succeeds, one with no auth succeeds, and a real headless call
  was served by `oskar-1@ctx.company` (its `usage.totalRequests` moved and
  `lastUsed` matched) while this machine's claude is logged in as a different
  account. Confirmed in source: `/v1/messages` is not in
  `CLIENT_CREDENTIAL_PATHS` (only `/v1/code/*` and OAuth file endpoints pass
  through) and `forwardRequest` has no client-auth branch — selection then
  `headers['authorization'] = Bearer <account.credential>` always.
- **`TC_ACCT` is NOT read from claude's environment — do not put it in
  `[worktree.env]` and expect pinning.** Tested: a spawn with `TC_ACCT` set
  rotated normally. The variable is consumed by `teamclaude env`/`run`, which
  encode the pin INTO the emitted environment: `--no-mitm` yields
  `ANTHROPIC_BASE_URL=http://localhost:3456/tc-acct/<name-or-uuid>`, MITM
  yields proxy basic-auth credentials. So per-seat pinning for ctx is simply a
  per-seat `ANTHROPIC_BASE_URL` VALUE — which collapses P557's claude-code
  path to "one env var per seat, different path per account".
- **An unknown pin is refused, not rotated**: 404 `Unknown account pin`,
  verified live. Note the path-prefix form is marked deprecated in source in
  favour of `TC_ACCT` — but kept explicitly "for the warmer and for direct API
  callers", which is precisely what ctx is. It is single-segment, so the
  fully-qualified `accountUuid/orgUuid` form is not expressible; use the
  accountUuid, which is unambiguous for these three accounts.
- **Headless server operation is real**: the server here has run for 25h+
  from a plain background start; `--headless` exists, `status --json` is
  script-friendly, and `--activity-log FILE` is documented to work headless —
  the right attribution source for step 3's rotation test, because…
- **…do not build verification on `status --json` counters.** They are
  in-memory and reset when the server re-syncs accounts from config, which
  token refreshes trigger; a counter reading zero does not mean no traffic
  (this cost an hour of confusion during this research). Use the activity log.
- Two operational notes from the live check: the background quota probe was
  429-throttled on all three accounts at once (probe-endpoint throttling, not
  quota; serving was unaffected) — treat probe `error` states as advisory.
  And `distributeSessions: false` means rotation serves everything from one
  account until `switchThreshold`; flipping it to `true` is the zero-code way
  to get concurrent multi-account spread BEFORE P557's per-seat pinning
  exists, and should be part of the Level-1 trial.

Still unverified, deliberately: a rotation crossing mid-run without a failed
frame (needs a real quota approach, not worth forcing), and loopback egress
from inside ctx's sandbox-exec confinement — the tests above ran unconfined,
so step 2 below should confirm a confined frame reaches `localhost:3456`.

## The integration point, and it is smaller than expected

**`teamclaude env` prints export lines to stdout, and nothing else** (summary/hints go to stderr, so `eval` is safe):

```
eval "$(teamclaude env)"            # MITM mode: HTTPS_PROXY + NODE_EXTRA_CA_CERTS
eval "$(teamclaude env --no-mitm)"  # base-URL mode: ANTHROPIC_BASE_URL only
```

No `ANTHROPIC_API_KEY` is emitted — loopback clients are trusted by the proxy. So **integration is environment variables on the spawn.** Any `claude` process launched with that environment routes through the proxy. There is nothing to wrap, no CLI to shell out to per frame, and no ctx-side account logic — the proxy owns rotation.

Two modes matter differently to us:
- **MITM (default since 1.1.0)** intercepts even hardcoded `api.anthropic.com` endpoints via a local CA. Needs `NODE_EXTRA_CA_CERTS` to be honored by the spawned process.
- **`--no-mitm`** sets only `ANTHROPIC_BASE_URL`. Simpler, no CA trust, but misses hardcoded endpoints. **Start here** — fewer moving parts, and ctx's frames only make ordinary API calls.

Run the server headless for automation: `teamclaude server --headless` (or `--no-tui`). It falls back to plain logs when stdout is not a TTY, so it backgrounds cleanly.

## Why this is worth doing — with today's evidence

At **06:40 on 2026-07-28**, six runs across ctx-trait *and* ctx-gate died within seconds of each other:

```json
{"status":"rejected","resetsAt":1785213600,"rateLimitType":"five_hour",
 "overageStatus":"rejected","overageDisabledReason":"out_of_credits"}
```

One exhausted five-hour window took out every run in both repos, because **every seat authenticates as the same ambient account**. The owner has three Claude Max accounts. TeamClaude is exactly the missing piece — and its `switchThreshold: 0.98` acts on the same signal `P556` charters ctx to read from the stream.

## What ctx is missing, precisely

**There is no per-seat env.** `harness_config.rs:199` has `pub env: BTreeMap<String,String>` but it belongs to `[worktree.env]` — **run-wide**, applied to every spawn in the run. The spawn plumbing itself is fine (`command.rs:67` calls `command.env(key, value)` per pair), so the mechanism exists; only the per-seat binding is absent.

That is precisely **P557** ("One harness, several accounts: per-seat credential routing"). **TeamClaude is the reference implementation for P557's claude-code path** — and it is a strong argument for shaping P557 as *env-per-seat* rather than inventing a credential abstraction, because the ecosystem already speaks env vars.

## Two integration levels

**Level 1 — works today, zero code.** Put TeamClaude's exports in `[worktree.env]`, or `eval "$(teamclaude env --no-mitm)"` in the shell that launches ctx. Every claude-code seat in the run rotates across all three accounts. **This is probably enough for the owner's own use and should be tried first**, before any phase is dispatched.

**Level 2 — P557.** Per-seat env, so seat A pins `TC_ACCT=<uuid-or-name>` (pins a session to one account, bypassing rotation; **stripped from the environment before claude starts**) while seat B pins another. That buys *deliberate* concurrency — two seats genuinely running on two accounts at once — rather than sequential rotation.

## Risks to state honestly

- **A third-party proxy in the auth path.** It holds OAuth tokens in `~/.config/teamclaude.json` and MITMs Anthropic traffic with a local CA. Fine as an owner tool; **do not make it a product dependency or a documented default** without a security review. Recommending it to users is a different decision from using it yourself.
- **ToS.** Multi-account rotation against subscription quotas is the owner's call to make knowingly, not something ctx should quietly automate for others.
- **It hides the signal P556 wants.** If the proxy rotates at 0.98, ctx may never observe a `rejected` event — the failure mode simply stops appearing. That is good operationally and bad for testing: **do not develop or validate P556 with TeamClaude in the path**, or you will be testing an unreachable branch.
- **Credentials must never reach argv.** ctx writes spawn argv verbatim into `~/.config/ctx/debug/**`. Env-var indirection is safe; a token as an argument would be written to disk in cleartext. This is already recorded in P557's Watch.

## Steps

1. Install, `teamclaude login` for each of the three accounts, `teamclaude accounts` to confirm.
2. `teamclaude server --headless &`, then `eval "$(teamclaude env --no-mitm)"` in a shell and run one small ctx phase. Confirm from the debug trace that frames succeed and the proxy log shows the account in use.
3. Force a rotation: run until one account crosses threshold, confirm the switch happens without a failed frame.
4. Decide whether MITM mode is needed (only if something in the path hardcodes `api.anthropic.com`).
5. Record the outcome on **P557** — specifically whether env-per-seat is the right shape for that phase, which this research says it is.

## Done when

Three accounts serve one ctx run through the proxy; a quota rotation happens mid-run with no failed frame; and P557's design records env-per-seat with `TC_ACCT` pinning as its claude-code reference path.

## Explicitly not in scope

Implementing P557; bundling, vendoring or defaulting to TeamClaude in the product; automatic account rotation logic inside ctx (the proxy owns that, and duplicating it would be worse).
