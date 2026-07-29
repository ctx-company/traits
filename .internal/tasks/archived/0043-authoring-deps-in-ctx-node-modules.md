# 0043 — Authoring deps must install into `.ctx/`, not the user's repo root

**Status:** DONE 2026-07-29 · **Raised:** 2026-07-29

**Landed:** the CDK build anchors Node's cwd to `.ctx` when
`.ctx/node_modules` exists (falling back to the repo root, so this repo and any
existing user setup resolve unchanged); `ctx traits init` scaffolds
`.ctx/package.json` (private, pinned `@ctx-traits/cdk`) and `.ctx/.gitignore`
with `node_modules/` in the canonical entries; the unresolved-package message
names `ctx traits init` instead of this repo's pnpm setup. Proven in a repo
with no root `package.json` and no root `node_modules`: `init` → `new` →
`build` succeeds resolving purely from `.ctx/node_modules`.

**Two deliberate residuals, recorded not hidden:**
- `init` writes the manifest but does not run the install — offline-safe, and
  untestable against the registry until 0031's first publish actually lands.
  The build's failure message carries the remedy. Revisit after first publish.
- `AUTHORING_PACKAGE_VERSION` in `modules/io/src/init.rs` is a pin nothing
  gates: it must move with every published release of the authoring packages,
  and a stale value is discovered by an author whose first build fails. This
  belongs on the release checklist until a gate exists.

## What breaks

`ctx traits build` runs the authored source through Node:

```
node --input-type=module --eval <script> <source_path>
```

and anchors bare-specifier resolution by setting the child's cwd
(`modules/io/src/cdk_build.rs:113-127`):

> "Anchors bare-specifier resolution (e.g. `@ctx-traits/cdk`,
> `@ctx-traits/config`) to the repo's `node_modules` regardless of the invoking
> process's actual cwd — Node resolves `--eval` bare imports by walking up from
> the child's cwd."

So `@ctx-traits/cdk` resolves **only** from `<repo_root>/node_modules` and its
ancestors. It works here because this repository is a pnpm workspace with the
packages linked into its own root. In a clean repo there is no `node_modules`,
nothing has been published, and `ctx traits init` writes no `package.json` and
installs nothing.

The diagnostic the product prints in that situation
(`cdk_build.rs:166-169`) is:

```
cannot resolve @ctx-traits/cdk — run pnpm install (or pnpm add -D @ctx-traits/cdk)
```

That is our development setup as user-facing advice: it assumes the user has a
pnpm project at their repo root, and asks them to adopt one to author a trait.

## Two things to fix, and they are separable

### 1. The packages must actually be published

Resolution cannot succeed against a registry that has never seen these names.
`0031` decides which of the five are public; this task depends on that ruling
and on the first publish. Nothing below can be tested until it lands.

### 2. Authoring deps belong to ctx, in `.ctx/`

`ctx traits init` should write `.ctx/package.json` (private, `type: "module"`)
and install the authoring dependency into `.ctx/node_modules`, pinned to a
version the installed binary knows it is compatible with.

**`.ctx/node_modules` is already on Node's natural path for trait sources.** A
source at `.ctx/traits/packages/<id>/source/index.ts` resolves its own imports
by walking up from the file: `source/` → `<id>/` → `packages/` → `traits/` →
**`.ctx/`** → `<repo>/`. So a package installed under `.ctx/` is found without
any resolver trickery, and a repo that already has a root `node_modules` (this
one) keeps working through the next step up.

The `--eval` script anchors to **cwd**, not the file, so that path needs the
cwd moved from `repo_root` to the ctx root when `.ctx/node_modules` exists —
falling back to `repo_root` when it does not, which keeps this workspace and any
existing user setup working unchanged.

Version pinning is not optional: the CLI and the CDK agree on a schema version
(this repo currently emits both `0.2` and `0.3` packages), and an `init` that
installs `latest` will eventually scaffold a package the installed binary
cannot read.

## Watch

- **Running an installed trait must never require Node.** A generated
  `index.toml` is self-contained; node is an *authoring-time* dependency only.
  That is a real product property — a consumer who only installs and runs traits
  needs no JavaScript toolchain at all — and this change must not quietly make
  `dependency add` or `run` depend on one. Keep the install in `init` and
  `build`, never in the run path.
- **Do not hard-code a package manager.** The current message names pnpm because
  that is what this repo uses. `npm` is the one every Node install has; detect
  or prefer it, and let the user override. Whatever is chosen, the failure
  message must name a command that works in a repo with no JavaScript project.
- **`.ctx/node_modules` must be gitignored**, alongside the existing entries in
  `.ctx/.gitignore`. A committed `node_modules` would be a spectacular first
  impression.
- The offline case deserves one explicit decision: authoring in a repo that
  cannot reach npm should fail with a clear statement, not a Node stack trace.
- Reusing an existing root `node_modules` when it already provides a compatible
  `@ctx-traits/cdk` avoids a redundant second copy. Worth doing, but only after
  the clean case works — the workspace case is the one that already works today
  and the one that hid this bug.

## Done when

In a directory with no `package.json` and no `node_modules`, `ctx traits init`
followed by `ctx traits new` and `ctx traits build` succeeds; the authoring
dependency lives in a gitignored `.ctx/node_modules` at a pinned, compatible
version; a repo that already has its own `node_modules` still builds unchanged;
and installing and running a published trait needs no Node at all.
