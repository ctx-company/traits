# Deep Research Quality Rubric

This is reviewer guidance, not a competing schema. Score only the canonical quality-verdict dimensions from 0 to 3, then normalize their average to 0-10 by multiplying by 3.33.

## Score Anchors

| Score | Meaning |
|---:|---|
| 3 | Fully met: strong, specific evidence and no material unresolved defect. |
| 2 | Mostly met: useful and supportable, with bounded gaps that do not reverse the conclusion. |
| 1 | Partly met: material omissions, weak support, or unresolved contradictions limit use. |
| 0 | Not met: absent, misleading, unverifiable, or unfit for the decision. |

## Canonical Dimensions

### Citation Density

- 3: Every factual claim and statistic has a valid, correctly placed citation.
- 2: Nearly all material claims are cited; omissions are minor.
- 1: Several important claims lack direct support or citations are hard to verify.
- 0: Citations are absent, fabricated, or materially disconnected from claims.

### Source Quality

- 3: Critical claims rely on appropriate A-B sources with transparent provenance.
- 2: Source mix is credible but includes avoidable C-rated support or limited primary evidence.
- 1: Important claims depend on weak, conflicted, stale, or secondary sources.
- 0: Sources are unverifiable or unsuitable for the claims made.

### Claim Verification

- 3: Critical claims passed independent Chain-of-Verification and counterevidence review.
- 2: Most critical claims were independently checked; a small number carry explicit caveats.
- 1: Verification is mostly self-confirming or contradictions remain unresolved.
- 0: No meaningful verification occurred or known falsehoods remain.

### Completeness

- 3: All scoped questions, required deliverables, and decision criteria are addressed.
- 2: The decision can be made, but a bounded secondary question or artifact is incomplete.
- 1: Material scope or required output is missing.
- 0: The work does not answer the core question.

### Coherence

- 3: Evidence, analysis, conclusions, and limitations form a consistent argument.
- 2: The narrative is usable with minor repetition or weak transitions.
- 1: Conclusions are difficult to trace to evidence or sections conflict.
- 0: The report is internally contradictory or unintelligible.

### Clarity

- 3: Audience-appropriate language, defined terms, navigable structure, and precise claims.
- 2: Generally clear with isolated ambiguity or unnecessary complexity.
- 1: Repeated ambiguity, jargon, or poor organization obscures meaning.
- 0: The intended audience cannot reliably interpret the result.

### Depth

- 3: Coverage matches the requested depth and examines mechanisms, tradeoffs, and edge cases.
- 2: Solid coverage with one meaningful area underdeveloped.
- 1: Surface summary where analysis was requested.
- 0: No substantive analysis.

### Actionability

- 3: Recommendations are evidence-linked, prioritized, feasible, and explicit about risks.
- 2: Recommendations are useful but need some local adaptation or prioritization.
- 1: Recommendations are generic or weakly connected to evidence.
- 0: No usable decision support is provided.

## Mapping From The Source Rubric

The source rubric's differently named dimensions are advisory checks only:

- Scope Clarity informs completeness, clarity, and depth.
- Methodology Rigor informs claim verification.
- Evidence Quality informs source quality and claim verification.
- Citation Completeness informs citation density.
- Synthesis Quality informs coherence and actionability.
- Factual Accuracy informs claim verification.
- Completeness informs completeness and depth.
- Reproducibility informs clarity and claim-verification notes.

Do not calculate a second composite score from these labels.

## Red Flags

- Fabricated or unverifiable citations.
- Unsupported certainty or hidden contradictory evidence.
- Source-quality labels without rationale.
- A high overall score inconsistent with dimension notes.
- Missing required deliverables or a report that promises absent content.
- Recommendations that exceed the cited evidence.
