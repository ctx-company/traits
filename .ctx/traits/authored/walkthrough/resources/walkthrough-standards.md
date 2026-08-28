# Walkthrough standards (exhaustive)

The walkthrough is a tree of explanations over real code, rendered as a
zoomable treemap, and it ends at code: every function, struct, enum,
trait, and module inside the topic's file closure gets its own described
node. Completeness is not your judgment — a deterministic script
enumerates every symbol in the closure and the run cannot finish while
any enumerated key lacks a node. Your job is honesty and register; the
arithmetic is handled.

## The closure and the horizon

- The closure is every source file the topic's code lives in, plus every
  same-workspace file it directly or transitively depends on, followed
  through imports/use declarations.
- Traversal stops at the horizon: std and third-party dependencies,
  unrelated subsystems, generated code. The horizon note names each cut
  and why — it renders into the walkthrough, so a dishonest horizon is a
  visible lie.
- The closure is the coverage contract. A file you list is a file whose
  every symbol you owe a description for; a file you omit is a claim the
  topic does not depend on it.

## Tree shape and id conventions

- Exactly one root, id `root`, kind `overview`, no parent.
- Areas group files by module/responsibility; every file node's parent
  is an area (or the root).
- File node id: `f-` + the path slugified — lowercase, every character
  outside `a-z0-9` becomes `-`, runs collapsed, ends trimmed.
  `modules/io/src/mcp.rs` → `f-modules-io-src-mcp-rs`.
- Symbol node id: `s-` + symbol name + `-` + definition line, kebab-case.
- Re-emitting an existing id REPLACES that node (last-wins). That is the
  only revision mechanism — never rewrite the whole tree.

## Coverage keys

- Every `type`/`function` node carries `symbol`: the index entry's key
  copied VERBATIM (`path:line:name`). Coverage joins on this exact
  string; a retyped or "corrected" key counts as undescribed.
- The index's kind is authoritative: `function` for fn/def/macro/arrow,
  `type` for struct/enum/trait/mod/impl/type/class/interface/const.
- Every enumerated symbol gets a node — including private items, test
  helpers, and tiny accessors. A one-line getter earns a one-sentence
  explanation, not an exemption.

## Grounding (refs)

- Every node carries at least one ref: repo-relative `path` plus a
  1-indexed inclusive `lines` span `"start-end"` you verified by OPENING
  the file. A guessed span is a fabrication and a review blocker.
- A symbol node's span runs from its definition line to its real end.
  The treemap sizes tiles by these spans — padding a span lies to the
  reader.

## Register per layer

- Root (`overview`): architecture and intent — the brief a senior
  engineer gives a new teammate, plus the horizon.
- `area`: responsibilities, collaborations, why the boundary sits there.
- `file`: what lives in the file, its role in the area, its internal
  organization.
- `type` / `function` (the leaves): the actual mechanics, precisely —
  what it does, what it refuses, the edge it guards, the invariant it
  maintains. Name real identifiers. One glanceable `summary` sentence;
  an `explanation` of one to three tight paragraphs scaled to the
  symbol's weight.
- `flow`: a cross-cutting path (a request, a lifecycle, a refusal chain)
  that no single file owns — the exception allowed to span files.

## Prose

- Plain paragraphs separated by blank lines; inline backtick `code` for
  identifiers. No headings, no nested lists — the treemap is the
  structure; the prose narrates.
- Write for a reader who clicks top to bottom: each layer should make
  the next layer's tiles feel expected.
