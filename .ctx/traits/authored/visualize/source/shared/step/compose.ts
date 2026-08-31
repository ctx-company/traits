// The composing seat investigates the repository at whatever depth it
// needs, then writes BOTH artifacts itself (house doctrine: the
// composing seat writes its own deliverable):
//   .internal/visualizations/<slug>.md    — the walkthrough as plain markdown
//   .internal/visualizations/<slug>.html  — a self-contained diagram of the
//     functionality's flows, following the diagram standards resource
//     (distilled, with attribution, from the effective-html html-diagram
//     skill).
// The investigation is code-level; the deliverable is NOT — it explains
// functionalities, flows, and states in product terms. The markdown is
// the annotation surface, the HTML is the presentation surface.
import { input } from "@ctx-traits/cdk";
import type { AgentHandle } from "@ctx-traits/cdk";

import { overview, ownerAnswer, revisionNote, slug, topic } from "../data.ts";

export function composeStep(agent: AgentHandle, title: string): void {
  agent.prompt(title, {
    id: "investigate-and-compose",
    input: input.prompt`Compose a non-code walkthrough of an existing functionality of this system.

Topic: ${topic}

Investigate the repository first to establish ground truth: read whatever code, docs, and configuration you need until you are certain how this functionality actually behaves today — including its refusal and failure paths. The investigation is code-level; the deliverable is not.

Present strictly at the functionality level: what the capability does, what triggers it, the flows it runs (steps, decisions, hand-offs, failure and refusal paths), the states it moves through, and where it borders other parts of the system. Name components by their plain-language role; a module name may appear only as orientation. No line numbers, no symbol inventories, no file-by-file tours.

Write exactly two files, creating .internal/visualizations/ if needed, both named by the slug ${slug}:
1. .internal/visualizations/<slug>.md — the walkthrough as plain markdown: a short orientation, each flow narrated step by step (including what can go wrong and how the system answers), the states and their transitions, the boundaries and hand-offs, and open questions where behavior surprised you. This file is the annotation surface and the authoritative text.
2. .internal/visualizations/<slug>.html — one self-contained HTML file (inline CSS/JS, no external resources) presenting the same walkthrough visually. Follow the diagram standards at .ctx/traits/authored/visualize/resources/diagram-standards.md — read that file before composing. Choose the diagram form the content demands (process, sequence, and state models usually fit flows); legible labels and clear direction come before any interaction.

Both artifacts must present the same walkthrough; where detail differs, the markdown is authoritative. Return the receipt: both artifact paths, the repository areas consulted, and a one-paragraph summary of the functionality as explained.`,
    output: overview,
  });
}

export function reviseStep(agent: AgentHandle, title: string): void {
  agent.prompt(title, {
    id: "apply-owner-annotations",
    input: input.prompt`The owner reviewed .internal/visualizations/${slug}.md through ctx-annotate and returned this decision; its annotations reference that markdown file's line numbers and are binding: ${ownerAnswer}

Each annotation is one of two kinds — read it and decide which:
- A correction: the walkthrough misstates how the system behaves. Re-verify against the code and fix it.
- A deepening request: the owner wants more depth on the annotated part. Investigate the code further to get the facts, then EXTEND both artifacts — new subsections or steps in the markdown, new or expanded regions in the diagram — still at the functionality level unless the annotation explicitly asks for code-level detail.

Apply every annotation to the markdown, then update the sibling .internal/visualizations/${slug}.html so the diagram still presents the same (now corrected or deepened) walkthrough. Change nothing an annotation does not ask for. Report each annotation and the exact edits made to both artifacts for it.`,
    output: revisionNote,
  });
}
