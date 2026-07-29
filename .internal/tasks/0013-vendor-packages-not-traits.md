# 0013 — Vendor materializes PACKAGES, not traits

**Status:** ready to implement · **Convention agreed with owner 2026-07-28** · **Raised:** 2026-07-28

## The mistake, named

Vendoring was built when every package contained exactly one trait, so it flattens: `install` copies the trait's canonical to `vendor/<alias>/trait.toml` and stops. Alias happened to equal trait id every time, so the distinction was never exercised.

That flattening cannot express anything richer than one trait. A folded family has five canonicals and a `[family]` table with nowhere to live in a canonical document — so **the single most valuable thing this repo could publish, `implement`, is the one thing the current vendor layout cannot carry.** P535 (sharing the family into ctx-gate) runs through this too.

## The convention

```
.ctx/traits.toml                       # consumer: what I installed          (edited by install)
.ctx/traits.lock                       # consumer: at what version, digests  (edited by install)
.ctx/traits/vendor/<alias>/traits.toml # package: what this package provides (authored by publisher, read-only here)
.ctx/traits/vendor/<alias>/<trait>/trait.toml
.ctx/traits/vendor/<alias>/<trait>/trait.lock
```

Three scopes, three files, never two in one directory:

- **`traits.toml`** — a package: what it provides, what it depends on. At the vendor root of one package, and at the root of a published package. Same format, two scopes, like `Cargo.toml` at a workspace root versus a crate.
- **`trait.toml`** — one trait.
- **`.ctx/traits.toml`** — this repo: what is installed.

**No index at the vendor ROOT.** `.ctx/traits.lock` already records every vendored package, version and digest; a second index over the same facts is a second source of truth, and the ctx-gate incident is what that costs — a lock naming one canonical while the file held another, with nothing noticing.

**Mirror always; never flatten the single-trait case.** One layout beats two, and the "flatten when exactly one trait" branch is the one that breaks the first time someone publishes a family. The extra directory level for single-trait packages is the price.

## What is already correct — do not redesign it

**The lock format already models this.** `resolve_package_lock_paths_in` (`modules/io/src/project_lock.rs:107-125`) iterates `entry.traits` — a per-package LIST — each with its own `id` and a `canonical_path` **relative to the package's vendor root**, validated traversal-free before joining. That is exactly `vendor/<alias>/<trait>/trait.toml`. Nothing in the lock needs changing.

`vendored_package_root(alias)` is likewise already alias-keyed, not trait-keyed. The flatness is an accident of the corpus, not a decision in the format.

## What to change

- **`install`** writes the mirrored tree instead of flattening, and copies the package's own `traits.toml` in.
- **`publish`** emits `traits.toml` plus one directory per trait, and refuses when a declared trait is missing.
- **Resolution** reads `vendor/<alias>/traits.toml` to enumerate what a package provides, rather than assuming one canonical at the alias root. Eleven call sites reference the vendor layout (`vendored_package_root` / `vendor_root()`), which bounds the sweep.
- **`trait.toml` in a published package is the MANIFEST form** (`[package]` identity, status, dependencies, `[family]`), with the canonical reachable from it — not the flattened canonical the vendor tree holds today. Those are two different documents that currently share a filename: the authoring `trait.toml` has `[package]` and no `[procedure]`; the vendored one has `[[intent.require]]` and no `[package]`. The published form must be the manifest, or families cannot travel.

## `vendor` must run in a PACKAGE, not only in a consumer (owner-raised 2026-07-28)

`ctx traits vendor` resolves declared dependencies and materializes them. Today it can only do that inside a CONSUMING repo: `trait_authoring_root_path` hard-codes `repo_root.join(".ctx/traits")` (`modules/io/src/layout/mod.rs:127-130`), so discovery starts from `.ctx/traits/*` and vendoring lands in `.ctx/traits/vendor/`.

A package repo under this convention has no `.ctx/traits` at all — `traits.toml` sits at the repo root and each trait is a top-level folder. So **the tool that resolves dependencies cannot run in the layout you would author a package in.** That is the same mistake as vendoring traits instead of packages: the tooling was built for consumers and quietly excludes publishers.

Make the authoring root discovered rather than assumed: a repo with `traits.toml` at its root IS a package and resolves its traits from the root; a repo with `.ctx/traits/` is a consumer and resolves from there. One probe, two layouts, no flag — a package author runs `ctx traits vendor` and it works, which is the whole point.

**Watch:** the vendor DESTINATION differs too. A consumer vendors into `.ctx/traits/vendor/`; a package vendoring its own dependencies needs its own location, and whether that is `vendor/` at the package root or something excluded from the published tarball is a real decision — a package that ships its dependencies' copies inside itself would double-vendor on install. Decide it here rather than discovering it after the first publish.

## Watch

- **The vendor tree is DERIVED — deleting it is a no-op.** Verified 2026-07-28: removing both vendored packages blocked four traits (`guarded-change`, `refactor-{quick,smart,strict}`), and a single `ctx traits vendor` regenerated both from their sibling path dependencies and restored every check to green. So there is nothing to migrate and no compatibility shim to write: land the new materialization and the next `vendor` produces it. Do NOT write a reader for the old layout — that is the flatten-when-one branch under another name.
- Alias and trait id diverge permanently under this convention (`vendor/acme/foo` where the package is `acme` and the trait `foo`). Every path that assumed they were interchangeable needs checking, and today's corpus will not catch it — both current packages are single-trait with alias == id, so a test fixture with a genuinely multi-trait package is required or this ships untested.
- `.ctx/traits.lock` does not exist in this repo today (only `.ctx/traits.toml`, holding just `schema-version`), because both dependencies are local-path and vendored directly. A multi-trait fixture will need the lock path exercised for real.

## Done when

A package containing more than one trait installs into `vendor/<alias>/<trait>/` with the package's own `traits.toml` alongside; `publish` emits that shape; resolution enumerates traits from the package manifest rather than assuming one; a folded family can be published and consumed end to end; and no code path reads the old flattened layout.
