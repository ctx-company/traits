# 0065 — Authoring assist writes CDK source; only `build` writes canonical

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-03 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

First slice of the authoring-assist arc. `generate`, `refine --apply` and
`import --llm-assisted` currently write `.ctx/traits/<id>/generated/index.toml` — the *build
output* — because they predate the CDK. The authoring reality is `source/index.ts` →
`ctx traits build` → `generated/index.toml` + `generated/index.map`. Three consequences today: a
generated package has no TS source and therefore cannot be rebuilt or maintained; it has no source
map, so `critique` cannot anchor a finding to it; and `refine --apply` edits a file the next build
overwrites. `refine`'s own help text already claims it "never edits generated exports directly"
while `resolve_trait_path` sends it exactly there.

## Decisions

- **The candidate is TypeScript authoring source.** The meta-traits return the text of
  `source/index.ts`, not a canonical trait object. `trait-draft` (a bare `schema.any()` trait blob)
  is retired for `generate-trait` and `import-trait`; `refine-scaffold` anchors its patches to the
  authoring source instead of the canonical document.
- **`build` is the only writer of `generated/`.** No assist path may write a canonical document,
  not even a valid one. This is the whole point: one producer for the artifact everything else
  digests, locks and approves.
- **Gates evaluate the BUILT canonical, never raw model output.** Parse/normalize/audit/check keep
  their present meaning; what they consume is the build's output, and the build becomes the first
  rung. Auditing the TS source for hidden content stays — it is now a *second* audit surface, not a
  replacement.
- **The model authors into a scaffold, not into an empty directory.** Reuse the deterministic
  package scaffolder `ctx traits new` already runs (`package.toml`, `source/`, layout), so the
  model writes one file and never invents package structure.
- **Grounding moves to the CDK authoring API.** `agent-traits.schema.json` describes the *output*
  of the pipeline, which is the wrong artifact to write against. `trait-spec` gains a curated
  authoring resource; the canonical schema stays for `critique` and for readers, not for authors.
- **`refine` operates on `source/index.ts`** — its source digest, its patch anchors and its
  `--apply` target all move there. Trait identity preservation is unchanged.
- **`import --llm-assisted` enriches into TS.** The deterministic import scaffold remains the
  authoritative baseline and provenance carrier; it is simply emitted as source.
- **`critique` does not change.** Canonical + `generated/index.map` is exactly the pair the source
  map exists for; it starts working on assist-produced packages as a side effect of this task,
  which is the tell that the shape is right.

## Scope

The three meta-trait packages (`generate-trait`, `refine-trait`, `import-trait` under
`modules/core/builtins/traits/`) change their output schema, prompt and grounding resources.
`trait-spec` gains the authoring resource. The CLI handlers (`generate.rs`, `refine.rs`,
`import_handlers.rs`) write `source/index.ts` through the safe writer and then drive the existing
`cdk_build` path; `trait_package_output_paths` stops routing assist through
`package_manifest_write_path`.

## Watch

- **Grounding size is a real constraint.** `packages/cdk/dist/*.d.ts` is ~5 200 lines — far too
  large to hand a model. The README is 199. The authoring resource must be curated, and it becomes
  load-bearing: every CDK API change is now a resource change too, and a stale one produces
  confident source that does not compile.
- **Assist inherits the node/CDK dependency.** `build` shells out with `@ctx-traits/cdk` resolved
  from `.ctx/node_modules` (0043). `generate` stops working where those are absent and must say
  exactly that — the existing "run `ctx traits init`" diagnostic — rather than failing obscurely
  three layers down.
- **Digest meaning shifts.** `raw_output_digest` is now over TS text; `canonical_digest` is over an
  artifact the model never saw. Keep both and label which is which in the candidate envelope, or
  provenance quietly starts claiming the model produced bytes it did not.
- **`--candidate <file>` must keep working.** It now supplies TS, and offline replay includes a
  real build — still zero provider calls, which is the property that makes gate tests cheap.

## Done when

`ctx traits generate` leaves a package that `ctx traits build` rebuilds byte-identically;
`ctx traits critique` runs on that package with no `--source-map` argument; `refine --apply` edits
`source/index.ts` and leaves `generated/` untouched until a build runs; `import --llm-assisted`
produces buildable source with its deterministic provenance intact; and no assist code path is
able to write a canonical document.
