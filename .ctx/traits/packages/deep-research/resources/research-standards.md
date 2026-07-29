# Deep Research Standards

This resource is the canonical model-facing doctrine for the deep-research package. Runtime-owned retries, heartbeats, task registries, completion markers, journals, locks, and watchdog files are intentionally absent.

## Operating Defaults

- Use four to six non-overlapping research streams; use three for a focused question.
- Use a 36-month recency window for most topics unless the user requests historical coverage.
- Require citations for every factual claim and every statistic.
- Target 60-150 sources when appropriate; prioritize quality over quantity.
- Aim for at least eight citations across two independent source types per key question unless the manifest sets a different threshold.
- Prefer A- and B-rated sources. Use C-rated sources for context. Treat D and E as leads, not evidence for critical claims.
- Record uncertainty, inaccessible sources, contradictory evidence, and unresolved gaps explicitly.
- Budget 20-40 minutes for standard quality in fast harnesses, several hours for publication-ready quality, and longer for field-tested or population-specific variants.

## Clarification Contract

Before planning, ask three to five questions that establish:

1. The core research question and decision it informs.
2. Scope boundaries, geography, timeframe, and exclusions.
3. Required outputs and depth.
4. Source preferences, recency, and inaccessible-source constraints.
5. Audience, purpose, and expected actionability.
6. Domain-specific safety, regulatory, ethical, or formatting requirements.

Offer a fast response shape and an explicit option to proceed with documented assumptions. Do not begin evidence collection until the response is available.

## Eight-Phase Process

1. **Clarify:** establish the research question, boundaries, audience, quality target, depth, outputs, source preferences, and assumptions.
2. **Plan:** create typed streams and deliverables manifests with non-overlapping scopes and measurable success criteria.
3. **Collect:** research each stream serially in v1, write only to its declared target, and return cited findings.
4. **Triangulate:** compare independent sources, reconcile contradictions, rate source quality, and identify gaps.
5. **Gate:** compute the evidence score, normalize it to 0-10, and compare it with the selected target before synthesis.
6. **Synthesize:** combine supported findings into an executive summary, full report, source record, and useful tables or Mermaid diagrams.
7. **Verify:** apply Chain-of-Verification to critical claims and revise until approved or the bounded loop is exhausted.
8. **Package:** verify deliverables, organize the output tree, write navigation and methodology, and report limitations and remaining gaps.

## Runtime-Native Output Tree

```text
research/
|-- [topic]/
    |-- README.md
    |-- 00_plan/
    |   +-- 00_research_plan.md
    |-- 01_executive_summary/
    |   +-- 01_executive_summary.md
    |-- 02_full_report/
    |   +-- 02_full_report.md
    |-- 03_findings/
    |   +-- [stream].md
    |-- 04_sources/
    |   |-- bibliography.md
    |   +-- source_ratings.md
    |-- 05_visuals/
    |   +-- diagrams.md
    +-- 06_metadata/
        |-- methodology.md
        |-- evidence_table.csv
        |-- gap_report.md
        |-- quality_verdict.json
        +-- deliverables/
            +-- manifest.json
```

Only README.md may sit beside the numbered folders. All paths are relative to research/<topic>/; use forward slashes and kebab-case names. The runtime ledger replaces tasks/, heartbeats/, results/, logs, lock files, and gate.ok.

## Graph of Thoughts

Use Graph of Thoughts as a reasoning method, not a second runtime state store:

1. **Generate:** propose multiple research angles or explanations.
2. **Aggregate:** combine compatible evidence and remove redundant branches.
3. **Refine:** deepen promising branches and seek disconfirming evidence.
4. **Score:** judge relevance, evidence strength, novelty, and decision value.
5. **Prune:** drop unsupported, duplicated, or low-value branches.
6. **Frontier:** retain the strongest unresolved branches for the next research action.

Do not emit graph_state.json; typed manifests and the session ledger are the durable state.

## Citation Requirements

- Every factual claim must have a verifiable citation.
- Prefer inline citations immediately after the supported claim.
- Include direct URLs or DOI identifiers and retrieval dates for mutable web sources.
- Cite original sources rather than secondary summaries whenever possible.
- Verify that links resolve and the cited source actually supports the claim.
- Keep one citation style throughout each deliverable; use citation-style.md for source-type formats.

Core formats retained from the source:

```text
Web source format:
(Organization, Year, "Section Title")
Full: Organization. (Year). "Source Title." Retrieved [date] from https://example.com/page

Academic source format:
(Author et al., Year, p. XX)
Full: Author, A., et al. (Year). "Title." Journal, volume(issue), pages. https://doi.org/...

Direct quotes:
"Exact quote from source" (Author, Year, p. XX)
```

## Canonical Source Quality Ratings

| Rating | Use |
|---|---|
| A | Systematic reviews, strong RCTs, top peer-reviewed work, primary regulatory records, official standards, and transparent authoritative datasets. |
| B | Credible guidelines, cohort or case-control studies, established technical white papers, official reports, and reputable expert analysis with disclosed methods. |
| C | Reputable journalism, substantive company engineering material, analyst reports, expert talks, and other contextual sources requiring corroboration. |
| D | Preprints without validation, preliminary results, weak methodology, opinion, and promotional material; use only as a lead with strong caveats. |
| E | Anonymous, fabricated, unverifiable, deceptive, or materially conflicted sources; do not use for claims. |

Clinical practice guidelines are B-rated by default. Upgrade only the underlying primary evidence, not the guideline label itself. See source-quality-guide.md for domain examples subordinate to this table.

## Evidence Quality Gate

Retain the source formula exactly:

```text
quality_score = (citation_density * 0.4) + (source_triangulation * 0.3) + (topic_depth * 0.3)

citation_density = min(total_citations / target_citations, 1.0)
source_triangulation = min(distinct_relevant_source_types / target_source_types, 1.0)
topic_depth = min(avg_citations_per_key_question / target_citations_per_question, 1.0)
```

Normalize quality_score to the verdict's 0-10 scale by multiplying by 10. Thresholds are basic = 7.0, standard = 8.5, and rigorous = 9.2. If evidence is below target, record the gap and the specific additional research needed; do not silently synthesize as if the target passed.

Quality and depth levels (`basic`, `standard`, `rigorous`) are a campaign-wide setting, entirely independent of the per-source A-E rating scale below: a `rigorous` campaign can still cite a C-rated source for context, and a `basic` campaign can still cite an A-rated one.

## Chain of Verification

1. Generate initial findings.
2. Create verification questions for each claim.
3. Search for evidence answering those questions.
4. Revise findings based on verification.
5. Repeat until all claims are supported or removed.

## Chain of Density

1. First pass: extract key points.
2. Second pass: add supporting details.
3. Third pass: compress while preserving critical information.
4. Final pass: maximize useful density with citations.

## ReAct Research Loop

1. Reason about what information is needed.
2. Act by searching, fetching, reading, or calculating.
3. Observe results.
4. Reason about gaps.
5. Repeat until evidence is sufficient.

## Source Access And Contradictions

When sources conflict:

1. Identify the exact contradiction.
2. Assess source quality for each claim.
3. Search for independent corroboration.
4. Present both viewpoints with context.
5. Recommend a resolution or document uncertainty.

For paywalled sources, record full citation and DOI metadata, search for open-access alternatives or author manuscripts, accept owner-provided lawful excerpts, and do not make unsupported claims from inaccessible material.

## Decomposition Patterns

- Dimensional: technical, business, regulatory, social or ethical, and verification.
- Temporal: historical context, current state, future projections, and cross-verification.
- Comparative: one stream per option plus a cross-option synthesis and verification stream.
- Multi-part: research bounded periods or subproblems independently, then synthesize them with one shared evidence standard.
- Update: review the existing report, preserve its structure and provenance, search only for material developments, and mark what changed.

## Time Expectations

| Topic Type | Streams | Typical Time | Output Depth |
|---|---:|---:|---|
| Narrow technical | 3-4 | 15-30 min | Focused to standard |
| Standard research | 4-5 | 30-90 min | Standard |
| Comprehensive | 5-6 | 1-4 hours | Comprehensive |
| Publication-grade | 6+ | Several hours+ | Comprehensive with extensive verification |

Actual timing depends on harness speed, source availability, web access, and verification depth.

## Mermaid Safety

- Use Mermaid only when a diagram improves comprehension.
- Double-quote every node label, especially labels containing punctuation, parentheses, slashes, colons, or brackets.
- Prefer simple, valid flowcharts over decorative complexity.
- Check that prose does not claim a diagram exists when no Mermaid block is present.

## Research Rules

Tips for best results:

1. Be specific in the initial request.
2. Confirm quality and depth before research begins.
3. Keep worker scopes non-overlapping.
4. Include at least one verification stream for comprehensive research.
5. Prefer authoritative and recent sources.
6. Document limitations and contradictions explicitly.
7. Run deliverable gates before final delivery.
8. Always apply human judgment for high-stakes decisions.

Do:

- Give each stream one complete, non-overlapping contract.
- Keep claims, quotes, paraphrases, source IDs, links, confidence, and counterevidence connected in the evidence table.
- Search for disconfirming evidence and explain contradictions.
- Distinguish source fact, inference, and recommendation.
- Use exact dates and quantities where they matter.
- Preserve uncertainty and known limitations.
- Keep deliverables navigable and audience-appropriate.

Do not:

- Invent citations, quotes, metrics, source access, or consensus.
- Cite a source you did not inspect.
- Treat search snippets as source evidence.
- Hide conflicting evidence or unresolved gaps.
- Use inaccessible or broken links without saying so.
- Leave TODO, FIXME, XXX, placeholder, or continuation markers in deliverables.
- Use emoji in professional research deliverables.

## Final Checklist

- Scope, audience, quality target, depth, and assumptions are explicit.
- Every required deliverable exists at its manifest path.
- Every factual claim and statistic is cited.
- Critical claims have independent corroboration or an explicit limitation.
- Sources have canonical A-E ratings with rationale.
- Contradictions and counterevidence are represented fairly.
- Chain-of-Verification completed for critical claims.
- The quality verdict uses the canonical eight dimensions and one 0-10 score.
- No banned markers, fabricated citations, broken internal links, or promised-but-missing diagrams remain.
- The executive summary, report, findings, bibliography, source ratings, methodology, and navigation agree.

## Gap Report

When a gate fails, report the missing or incomplete deliverables, failure reasons, evidence gaps, corrections already attempted, and recommended next actions. The runtime owns retry state; do not recreate an attempt registry in the report.
