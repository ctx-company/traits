# 0110 — Benchmark corpus run: audit ≥1,000 real marketplace skills, commit the manifest and numbers

**Status:** ready to implement (owner-run: needs marketplace network access) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P235)

The headline "% hidden-content / drift / near-dup on real marketplace data" claim is
the launch hook, and today it is unbacked — only one fixture is committed; the audit
harness is built and self-test-green but has never seen a real corpus.

Fetch a dated, digest-pinned ≥1,000-skill corpus from the public marketplaces
(Skills.sh / ClawHub / SkillsMP), snapshot with pull date + per-source digests, run
the audit harness over local files (import refuses remote by design), and commit the
corpus **digest manifest** (digests, not payloads) plus the run output as evidence.

Driver: `scripts/audit_marketplace.py` (`discover_skills`, `audit_one` runs
`ctx traits import --source <path> --json` dry and reads
`import-report.hidden-content-findings[]` + `synth-provenance.canonical-digest`;
`aggregate` rolls %-flagged / by-code / by-severity / duplicate-by-digest);
`just audit-marketplace <corpus>`. Detectors under audit:
`modules/core/src/audit.rs` (`scan_hidden_content`) + `model_view/sanitize.rs`.

## Watch

- Import runs dry (no repo writes) and stdout stays valid JSON even when the audit
  fails the exit code — read stdout only, never merge `2>&1`, never gate on exit code.
- Byte-stable `--json` is what makes the aggregate reproducible from the committed
  manifest.
- Owner-run job (network + marketplace access), not a headless CI step.
- Feeds the launch flag-plant post and the feature matrix (0114).

## Done when

A dated, digest-pinned ≥1,000-skill corpus is audited end to end; the digest manifest
+ report are committed; the %-hidden-content / drift / near-dup headline numbers are
real and re-derivable from the committed manifest.

Full original contract: `archived/board/execution-plan.md` (Group 54, P235).
