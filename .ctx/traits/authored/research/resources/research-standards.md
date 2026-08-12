# Research Standards

Canonical model-facing doctrine for the research family. The variant is the
structural guarantee — quick/default/deep fix how many streams, how many
reviewers, and how many loop rounds run; this resource never restates a
numeric quality target, a campaign vocabulary, or a manifest schema. Runtime-
owned retries, heartbeats, task registries, completion markers, journals,
locks, and watchdog files are absent by design.

## Output Layout

Every deliverable lands flat under `{output-dir}/<topic-slug>/` — one
directory, no numbered subfolders, no manifests, no heartbeats:

```text
{output-dir}/
`-- <topic-slug>/
    |-- brief.md            (quick)
    |-- sources.md           (quick)
    |-- report.md             (default, deep)
    |-- bibliography.md       (default, deep)
    |-- evidence.csv          (default, deep)
    `-- verification.md       (deep only)
```

`<topic-slug>` is derived deterministically from the topic by a command
step, never by agent prose — every path in this layout is predictable before
any research runs. Use forward slashes and kebab-case names throughout.

## Operating Defaults

- Require a citation for every factual claim and every statistic.
- Prefer A- and B-rated sources; use C-rated sources for context only. Treat
  D and E as leads, never as evidence for a critical claim.
- Record uncertainty, inaccessible sources, contradictory evidence, and
  unresolved gaps explicitly — never synthesize past a gap silently.
- Keep every stream's scope non-overlapping; a stream owns its question and
  no other stream's.

## Canonical Source Quality Ratings

| Rating | Use                                                                                                                                                              |
| ------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A      | Systematic reviews, strong RCTs, top peer-reviewed work, primary regulatory records, official standards, and transparent authoritative datasets.                 |
| B      | Credible guidelines, cohort or case-control studies, established technical white papers, official reports, and reputable expert analysis with disclosed methods. |
| C      | Reputable journalism, substantive company engineering material, analyst reports, expert talks, and other contextual sources requiring corroboration.             |
| D      | Preprints without validation, preliminary results, weak methodology, opinion, and promotional material; use only as a lead with strong caveats.                  |
| E      | Anonymous, fabricated, unverifiable, deceptive, or materially conflicted sources; do not use for claims.                                                         |

Clinical practice guidelines are B-rated by default. Upgrade only the
underlying primary evidence, not the guideline label itself. See
`source-quality-guide.md` for domain examples subordinate to this table.

## Citation Requirements

- Cite every factual claim, immediately after the claim it supports.
- Include a direct URL or DOI and a retrieval date for mutable web sources.
- Cite the original source rather than a secondary summary whenever
  possible; verify that a cited link actually resolves and supports the
  claim it is attached to.
- Keep one citation style throughout each deliverable — see
  `citation-style.md` for source-type formats.

## Chain of Verification

Deep's produce round applies this method to every critical claim before
writing `verification.md`:

1. Generate the claim from the triangulated findings.
2. Write a verification question that would falsify the claim if it were
   wrong.
3. Independently answer that question against the cited sources — not by
   re-reading the claim's own prose.
4. Revise the claim (or drop it) based on the independent answer.

`verification.md` records, per critical claim: the claim, its verification
question, the independent answer, and the disposition (kept, revised, or
dropped). Repeat until every critical claim in the delivered report has a
recorded pass; a claim without one is a reviewer blocker.

## Counterevidence Streams

Deep's plan requires at least one stream of kind `counterevidence`: its
focus is deliberately searching for disconfirming evidence and competing
conclusions against the other streams' emerging findings — not a skeptical
restatement of a primary question. Triangulation must state what the
counterevidence stream surfaced, even when it found nothing that survived
scrutiny.

## Source Access And Contradictions

When sources conflict:

1. Identify the exact contradiction.
2. Assess source quality for each side.
3. Search for independent corroboration before picking a side.
4. Present both viewpoints with context in the delivered report; never
   silently resolve a contradiction by omission.

For paywalled sources, record full citation metadata, search for an
open-access alternative or author manuscript, and do not make an
unsupported claim from material you could not access.

## Do / Do Not

Do:

- Give each stream one complete, non-overlapping contract.
- Search for disconfirming evidence and explain contradictions.
- Distinguish source fact, inference, and recommendation.
- Preserve uncertainty and known limitations in the delivered report.

Do not:

- Invent citations, quotes, metrics, source access, or consensus.
- Cite a source you did not inspect, or treat a search snippet as source
  evidence.
- Hide conflicting evidence or an unresolved gap.
- Write outside the flat `{output-dir}/<topic-slug>/` layout, or leave a
  TODO/FIXME/placeholder marker in a deliverable.
