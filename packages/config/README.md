# ctx configuration semantics

`ctx` resolves the existing user-global `~/.config/ctx/config.toml`, repository
configuration, and optional `CTX_CONFIG` in one pass. `repos.toml` is an
operational checkout index, not configuration. No local override file or
migration is required.

## Field matrix

| Semantic | Fields |
| --- | --- |
| Default | Every `harness.*` leaf, including `cli.*` and `mcp.*`; `agent.model-tier.*`; every `agent.role.*` and `agent.variant.*.role.*` assignment leaf, including `budget.*`; every `host.*` leaf; `run.wait`; `run.story`; `merge.wait`; `merge.auto`; `merge.deep`; `git.long-seconds`; `registry.base` |
| Requirement | `schema-version` (document compatibility only); `worktree.setup`; `worktree.setup-seconds`; `worktree.setup-capture-bytes`; `worktree.confinement.enabled`; `worktree.confinement.sandbox`; `worktree.confinement.allow`; `worktree.tripwire.policy`; `worktree.retention.cheap`; `worktree.retention.expensive`; `worktree.retention.expensive-grace-days`; `run.worktree`; `run.max-frames`; `run.frame-seconds`; `run.total-seconds`; `run.max-retries`; `run.attach-wait-seconds`; `run.idle-seconds`; `run.max-in-flight`; `run.strict-loops`; `run.inline-prompt-bytes`; `merge.overlap`; `merge.branch`; `merge.gate`; `merge.gate-seconds`; `merge.generated` (including `paths` and `rebuild`); `merge.disk-floor-mb` |
| Additive | `worktree.seed`; `worktree.warm`; each `worktree.env.<name>`; `worktree.tripwire.sentinel`; each `run.build-cache.<name>`; `publish.exclude` |

Defaults use built-in, global, repo, matching personal override, then
`CTX_CONFIG` precedence. Requirements use the global fallback until a repo
declaration exists; personal and `CTX_CONFIG` declarations cannot weaken that
repository declaration. Additive lists are stable first-occurrence deduplicated;
maps combine distinct keys and keep repository keys on conflicts. Ordered argv,
seat, and other command lists are whole-value replacements, never additive.

## Personal repo override

Only the matching global block is active. It intentionally exposes default and
additive values only:

```toml
[repo."repo-key".agent.role.worker]
model = "personal-model"

[repo."repo-key".worktree]
seed = [".cache"]
```

When `CTX_CONFIG` tries to set a field already required by repository config,
loading still succeeds. The report contains a structured diagnostic with
`field`, `rejected-source`, and `repo-source`; command output and
`doctor --config` render it as a warning.
