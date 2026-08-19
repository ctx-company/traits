# Trait Design Rubric

## Derived Kind

Kinds are derived from canonical content; no authored kind field declares one.

- A trait with one or more resources or schemas is knowledge.
- A trait with an explicit behavior block is behavior.
- A trait with a procedure section is procedure.
- More than one of those capabilities also yields mixed.
- A trait with none of those capabilities falls back to knowledge.

Prompts, activation, and intent support a trait but do not independently change its derived kind.

## Speech Acts

Classify material by what it says, then model it with the smallest supported shape.

- Definitional reference material, schemas, and rubrics are knowledge.
- Standing guidance expressed as always/never rules is behavior.
- Ordered work across roles with typed inputs and outputs is procedure.
- When classification is uncertain, default to knowledge rather than inventing runtime behavior.

## Dataflow And Control

Use only declared, typed protocol concepts.

- An output port is a trait-boundary value. Its value exposes a declared internal slot; runtime or project bindings, not atomic trait source, connect output ports to input ports.
- A slot is internal procedure ledger state. Do not present it as a public trait boundary unless an output port exposes it.
- A loop must be bounded with max-iterations; its declared exit conditions and on-failure signal make completion or exhaustion explicit.
- A signal is a declared runtime trace event or fact. Declare signals that control flow or report an outcome rather than implying undeclared events.

Do not claim that a render, host, or runtime enforces behavior unless that behavior is demonstrated by the current canonical schema and runtime surface.

## Identifier Naming

Intent and behavior ids are self-describing at a glance: the id alone is what a reader sees, so it must carry the meaning on its own, without the surrounding summary or directive for context.

- Choose an id that states the judgment it names — `one-question`, never `rule-7`.
- A positional or numeric id (`rule-7`, `item-3`, `step-two`) is never acceptable, even as a placeholder, because it carries no meaning once separated from the authoring document.
- The same standard applies to activation-rule ids and any other identifier a render surfaces on its own.
