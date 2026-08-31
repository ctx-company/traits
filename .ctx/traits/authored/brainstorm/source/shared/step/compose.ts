// The composing seat researches online, grounds in this codebase, and
// writes BOTH artifacts itself (house doctrine: the composing seat
// writes its own deliverable):
//   .internal/brainstorms/<slug>.md    — the proposal as plain markdown
//   .internal/brainstorms/<slug>.html  — a self-contained diagram of the
//     proposed flow/architecture, following the diagram standards
//     resource (distilled, with attribution, from the effective-html
//     html-diagram skill).
// The two artifacts state the same proposal; the markdown is the
// annotation surface, the HTML is the presentation surface.
import { input } from "@ctx-traits/cdk";
import type { AgentHandle } from "@ctx-traits/cdk";

import { proposal, revisionNote, slug, topic, ownerAnswer } from "../data.ts";

export function composeStep(agent: AgentHandle, title: string): void {
  agent.prompt(title, {
    id: "research-and-compose",
    input: input.prompt`Brainstorm an implementation proposal for this system.

Topic: ${topic}

Research the topic online first: current approaches, technologies, and prior art; keep the handful of sources that actually informed you. Ground the proposal in THIS repository second: name the real modules, traits, and seams it would touch. Then decide a recommended approach and its alternatives, with tradeoffs.

Write exactly two files, creating .internal/brainstorms/ if needed, both named by the slug ${slug}:
1. .internal/brainstorms/<slug>.md — the full proposal as plain markdown: the idea, the researched grounding (with source URLs), the recommended approach, alternatives and tradeoffs, the concrete touchpoints in this repo, and open questions for the owner.
2. .internal/brainstorms/<slug>.html — one self-contained HTML file (inline CSS/JS, no external resources) presenting the proposed flow or architecture as a diagram. Follow the diagram standards at .ctx/traits/authored/brainstorm/resources/diagram-standards.md — read that file before composing. Choose the diagram form the content demands (topology, sequence, process, state); legible labels and clear direction come before any interaction.

Both artifacts must present the same proposal; where detail differs, the markdown is authoritative. Return the receipt: both artifact paths, the source URLs consulted, and a one-paragraph summary of the recommended approach.`,
    output: proposal,
  });
}

export function reviseStep(agent: AgentHandle, title: string): void {
  agent.prompt(title, {
    id: "apply-owner-annotations",
    input: input.prompt`The owner reviewed .internal/brainstorms/${slug}.md through ctx-annotate and returned this decision; its annotations reference that markdown file's line numbers and are binding corrections: ${ownerAnswer}

Apply every annotation to the markdown, then update the sibling .internal/brainstorms/${slug}.html so the diagram still presents the same (now corrected) proposal. Research further online only where an annotation demands it. Change nothing an annotation does not ask for. Report each annotation and the exact edits made to both artifacts for it.`,
    output: revisionNote,
  });
}
