# 0030 — Narrate through the provider API directly, not a harness

**Status:** closed against 0079 (`69ca2ef9`, "0079: narrator rides an API transport with seat-dispatch precedence") — the narrator seat is 0079's first consumer via `resolve_seat_dispatch` (`modules/cli/src/app/drive.rs:1558-1566`); this file stays as the statement of *why* for the narrator specifically. · **Raised:** 2026-07-29 · **Closed:** 2026-08-05

Narration currently spawns a whole coding-agent CLI per tick. That is enormous
overhead for a task that is: take a few lines of recent activity, return one
status line under 80 characters.

Call OpenAI/Anthropic directly for the narrator seat instead.

## Why

- **Latency.** An opencode narrator tick measured ~0.1s of model time against
  ~14s of spawn/server/snapshot overhead — it blew the 20s narrator timeout
  under load and the panel went silent. claude-code one-shots measure ~4-5s.
  A direct HTTPS call is one round trip.
- **Tokens.** Even the tuned `--pure` narrator carries a harness system prompt
  the task does not need. Direct calls send only what the status line requires.

## Watch

- This adds a credential path ctx does not have today. Narration must DEGRADE
  when no key is present — fall back to the harness narrator, or to quoting the
  agent's own output — never fail a run because a status line could not be
  generated.
- Narration is cosmetic. It must never block a frame, and its failures must
  never reach the ledger as run evidence.
- Keep the seat declared the same way (`[agent.role.narrator]`); this is a new
  transport, not a new concept.

## Done when

A narrator seat can be declared against a provider API directly; ticks are
sub-second; no key present degrades to today's behavior; narration failures
never affect a run.
