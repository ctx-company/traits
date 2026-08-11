# How ctx compares

This matrix is generated against the same claim gate that governs ctx's own
launch copy (`ctx traits claim-gate`). No ctx cell here asserts more than that
gate's `allowed-wording` for the matching row — where the row is `gated` or the
capability isn't shipped, the cell says so instead of rounding up.

Legend: **✓** shipped and evidenced · **partial** shipped with material gaps ·
**gated** built but not launch-approved · **planned** not shipping yet ·
**—** not offered / not publicly documented.

| Capability | ctx | SkillFortify | Skills Are Not Islands | MCP | Ruler | Agent Skills | AGENTS.md | Cursor rules |
|---|---|---|---|---|---|---|---|---|
| typed | ✓ [^1] | partial [^c1] | — [^c2] | partial [^c3] | — [^c4] | partial [^c5] | — [^c6] | partial [^c7] |
| versioned | ✓ [^2] | partial [^c1] | — [^c2] | partial [^c3] | — [^c4] | — [^c5] | — [^c6] | — [^c7] |
| lockfile | ✓ [^3] | partial [^c1] | — [^c2] | — [^c3] | — [^c4] | — [^c5] | — [^c6] | — [^c7] |
| transitive-audit | **planned** [^4] | partial [^c1] | partial [^c2] | — [^c3] | — [^c4] | — [^c5] | — [^c6] | — [^c7] |
| drift | ✓ [^5] | — [^c1] | — [^c2] | — [^c3] | partial [^c4] | — [^c5] | — [^c6] | — [^c7] |
| multi-harness | **gated** [^6] | — [^c1] | — [^c2] | ✓ [^c3] | ✓ [^c4] | partial [^c5] | ✓ [^c6] | — [^c7] |
| render-to-host | ✓ [^7] | — [^c1] | — [^c2] | — [^c3] | ✓ [^c4] | — [^c5] | — [^c6] | — [^c7] |

## Rebuttals

**SkillFortify** (arXiv 2603.00195) is a formal-analysis paper, not a shipping
tool: DY-Skill attacker model, Trust Score Algebra, and an Agent Dependency
Graph with lockfile *semantics* are proposed and benchmarked, not distributed
as an installable dependency manager. The gap isn't the theory — it's that
nothing in the paper is a `trait.toml` or `trait.lock` you can commit and
diff today.

**Skills Are Not Islands** (arXiv 2607.01136) is a measurement study of
1.43M skills that documents the problem — "dependency identities, versions,
and provenance remain implicit" — and calls for typed manifests and
lockfile-like records. It doesn't ship them; SkillDepAnalyzer is a one-shot
research analyzer, not a lifecycle tool a team runs on every change.

**MCP** standardizes the wire protocol between a host and a running tool/resource
server. It says nothing about how the *skill itself* is authored, typed,
versioned, or reviewed before it's exposed over that protocol — it's a
transport layer, not a context-artifact lifecycle. Orthogonal, not competing;
ctx could render an MCP-facing surface without touching MCP's own scope.

**Ruler** solves real-world distribution — one instruction set, rendered into
many AI tool config files — which is genuinely adjacent to ctx's render/export
path. What it doesn't have is a typed source of truth: no package identity, no
version, no lockfile, no drift detection between the rendered files and the
source once a team starts hand-editing the generated output.

**Agent Skills** (the `SKILL.md` convention) adds lightweight frontmatter
(name, description) but no version field, no lockfile, and no drift check
between what's declared and what's active. It also doesn't render to
non-Claude hosts — the same authoring format isn't portable the way a canonical
package with export profiles is.

**AGENTS.md** is a prose convention adopted by many tools precisely because it
has no schema, no version, and no lock semantics — that's what makes it easy to
adopt and also what makes it impossible to audit or diff mechanically. It's a
convention, not a lifecycle.

**Cursor rules** are host-native and stay that way: no export path to other
harnesses, no lockfile, no drift report against a canonical source. Some rule
files carry lightweight frontmatter (description, glob scoping), which is why
that cell reads partial rather than —, but the format doesn't travel outside
Cursor.

## Evidence footnotes (ctx cells)

Every ✓/gated/planned cell below maps to a row in `ctx traits claim-gate` and a
command you can run in this repository to reproduce it.

[^1]: claim-gate row `package/version`, implemented/source-approved: "canonical
    trait packages carry typed identity and version metadata." Evidence: any
    canonical `trait.toml`, e.g. `.ctx/traits/packages/research/trait.toml`,
    plus `ctx traits check research`.
[^2]: same claim-gate row as [^1] (`package/version`) — version metadata is part
    of the same typed identity.
[^3]: claim-gate row `check`, implemented/source-approved: "check combines
    validation, audit, resource, render, eval, and lock drift evidence."
    Evidence: committed lock files, e.g.
    `.ctx/traits/packages/research/trait.lock`, plus
    `ctx traits export --update-skill-lock`.
[^4]: the shipped claim-gate row `audit` covers only direct hidden-content and
    advisory findings ("audit reports review findings and advisory risks; it is
    not a security certificate") — it does not walk a dependency graph.
    Transitive (multi-hop) dependency audit has no queued shipping task, so
    this row is marked **planned**, not ✓.
[^5]: claim-gate rows `check` and `diff`, implemented/source-approved: "diff
    shows canonical/model-view/resource/policy/export evidence drift where
    available." Evidence: `ctx traits check <package>` drift sections.
[^6]: claim-gate row `multi-agent/multi-harness runtime` (`runtime_claim_row()`
    in `modules/core/src/launch/claim_evidence.rs`), gated/blocked-pending-
    runtime-family-approval: "runtime surfaces are available for controlled
    dogfood only; do not present multi-harness runtime as launch-approved
    until this row is unblocked."
[^7]: claim-gate row `render/export`, implemented/source-approved: "render/export
    produces reviewable host files with explicit semantic-loss warnings" —
    the wording explicitly excludes "host-native enforcement." Evidence:
    `ctx traits export` output and its warnings.

## Competitor citations

[^c1]: Varun Pratap Bhardwaj, "Formal Analysis and Supply Chain Security for
    Agentic AI Skills" (SkillFortify), arXiv:2603.00195, 2026-02-27.
[^c2]: Changguo Jia, Tianqi Zhao, Runzhi He, Minghui Zhou, "Skills Are Not
    Islands: Measuring Dependency and Risk in Agent Skill Supply Chains",
    arXiv:2607.01136, 2026-07-01.
[^c3]: Model Context Protocol specification, modelcontextprotocol.io.
[^c4]: Ruler, github.com/intellectronica/ruler.
[^c5]: Anthropic Agent Skills documentation (`SKILL.md` format), Claude Docs
    "Agent Skills".
[^c6]: agents.md — the AGENTS.md open convention.
[^c7]: Cursor documentation, "Rules" (docs.cursor.com/context/rules).
