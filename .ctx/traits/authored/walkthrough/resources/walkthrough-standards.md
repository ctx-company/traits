# Walkthrough standards

The walkthrough is a tree of explanations over real code, rendered as a
zoomable treemap. Its whole value is honesty: every tile's size and every
paragraph's claims derive from code that was actually opened. These rules
are the review bar.

## Tree shape

- Exactly one root node with no `parent`, kind `overview`. It explains the
  topic the way a senior engineer would brief a new teammate: intent,
  architecture, the two or three load-bearing ideas.
- Children partition their parent's territory: no overlaps, no gaps big
  enough to mislead. A reader who reads all children should have covered
  the parent.
- At most three levels below the root. Prefer fewer, larger, well-written
  nodes over many thin ones. Six to twenty nodes total is the usual range;
  past thirty, the walkthrough stops being a walkthrough.
- `id` is kebab-case and unique across the list; `parent` names an
  existing id. The list is flat — nesting is expressed only by links.

## Grounding (refs)

- Every node carries at least one ref: a repo-relative `path` plus a
  1-indexed inclusive `lines` span `"start-end"`.
- A ref is only legal if the file was OPENED and the span checked. A
  guessed span is a fabrication and a review blocker.
- Spans honestly cover the code the node explains — the treemap sizes
  tiles by these spans, so padding a span inflates a tile and lies to the
  reader.
- A child's spans should live inside the territory its parent's spans
  cover; a `flow` node may cut across files, and is the exception.

## Register per layer

- Root (`overview`): architecture and intent. No line-level mechanics.
- Middle (`subsystem`, `component`): responsibilities, collaborations,
  the contracts between parts, why the boundaries sit where they sit.
- Leaves (`symbol`, `flow`): the actual mechanics, precisely — what the
  function does, what it refuses, the edge it guards. Name the real
  identifiers.
- `flow` is for cross-cutting paths (a request, a lifecycle, a refusal
  chain) that no single file owns.

## Prose

- `summary`: one or two sentences, glanceable.
- `explanation`: thorough for its register, plain paragraphs separated by
  blank lines, inline backtick `code` for identifiers. No headings, no
  lists-of-lists — the treemap is the structure; the prose narrates.
- Write for a reader who will click top to bottom: each layer should make
  the next layer's tiles feel expected.
