# 0010 — Config composition: decide what OVERRIDES and what DEFAULTS

**Status:** DESIGN — decide before implementing · **Raised:** 2026-07-28

## The problem, in the owner's words

> "config composition is more subtle than take what's closer to repo, some things are about overrides some things about default"

Today the layer chain is **last-writer-wins by proximity**: user-global → every ancestor's `.ctx/config.toml` → `$CTX_CONFIG` (`runtime_config_layers`, `harness_config.rs:2504-2540`). Whichever layer is nearest the repo replaces the value.

That is right for some fields and wrong for others, and the product has never said which is which. Two examples that pull in opposite directions:

- **`[agent.role.worker] model`** — a repo saying "this project needs a strong worker" is a DEFAULT the person should be able to override for their own machine, their own quota, their own preference. Repo-wins is backwards.
- **`[worktree] seed`** — a repo saying "these gitignored paths must reach a worktree" is a REQUIREMENT of that repo's runs. A person overriding it silently breaks their own runs. Repo-wins is right.

And a third the current model cannot express at all: **a person's per-repo overrides**. "In ctx-gate, use my second Claude account" is neither a global default nor a repo fact — it is one human's opinion about one project, and it has nowhere to live.

## What to decide (this is the task)

**1. Per-field semantics, written down.** Every config field is one of: `default` (nearer layers may replace), `requirement` (repo wins; a person cannot silently weaken it), or `additive` (layers merge rather than replace — deny lists and seed paths plausibly want this). The classification is the deliverable; the merge code follows from it.

**2. Where a person's per-repo override lives.** Candidates: a `[repo."<key>"]` section in the user-global config; a `~/.config/ctx/repos/<repo-key>.toml`; or a gitignored `.ctx/config.local.toml` in the repo. The first two keep the repo checkout clean and travel with the person; the third is discoverable but easy to commit by accident. **Note `~/.config/ctx/repos.toml` already exists** and is the natural home if its shape allows it.

**3. Precedence with the new layer.** Presumably: repo requirements > person's per-repo override > repo defaults > person's global defaults > built-ins. Confirm, because "person's per-repo beats repo default but not repo requirement" is the whole point and is not obvious.

**4. How a person SEES the result.** `doctor --config` already prints each resolved value with its winning layer — that provenance must survive whatever this becomes, and should also say WHY a layer won ("repo requirement", "your override") rather than only which file it came from.

## Watch

- Do not start by writing merge code. The classification is the decision; the code is mechanical after it and unfixable before it.
- A requirement a person cannot override is a real cost — it must be refused loudly at load time ("repo requires X; your override was ignored"), never silently discarded. Silent discard is how the current model would fail if requirements were bolted on without diagnostics.
- P418 already split "machine facts" from "project facts" and landed. Read that split first: this is either its natural completion or a contradiction of it, and it matters which.
- Keep the existing chain working throughout. Every repo on this machine resolves config today; a redesign that requires editing every config before anything runs is not shippable.

## Done when

Every config field carries a stated semantic; a person can override their own choices per repo without touching the repo; a repo can state a requirement that survives; `doctor --config` shows which layer won and why; and no existing config stops working.
