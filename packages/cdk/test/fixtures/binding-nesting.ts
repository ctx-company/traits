import { agent, port, procedure, prompt, schema, sequence, slot, trait } from "@ctx-traits/cdk";

// Pins the two nested schema-ref shapes P398 requires beyond a single-level
// `[schema:text]`/`(schema:a|schema:b)`: a nested list (`[[schema:text]]`,
// a list of lists) and a list of a union (`[(schema:a|schema:b)]`, not a
// union containing a list member). Both are exercised end to end through an
// ordinary port/slot/procedure, not declared in isolation, so a regression
// in list/union ref nesting shows up the same way any other draft-stability
// regression would.
const noteA = schema.object("binding-nesting-note-a", { kind: schema.text(), text: schema.text() });
const noteB = schema.object("binding-nesting-note-b", { kind: schema.text(), count: schema.number() });
const noteUnion = schema.union([noteA, noteB]);

const groupedLines = port.input.of({ id: "grouped-lines", schema: schema.array(schema.array(schema.text())) });
const notes = slot({ id: "notes", schema: schema.array(noteUnion) });
const notesPort = port.output.of({ id: "notes", schema: schema.array(noteUnion), value: notes });
const worker = agent("worker", { description: "Groups raw lines and classifies each into a structured note." });

export const bindingNesting = trait({
  id: "binding-nesting-fixture",
  name: "Binding Nesting Fixture",
  summary: "A nested list (`[[schema:text]]`) and a list of a union (`[(schema:a|schema:b)]`), pinned for stability.",
  port: notesPort,
  procedure: procedure({
    description: "Classify grouped raw lines into a list of structured notes.",
    sequence: sequence.prompt("classify-grouped-lines", {
      title: "Classify Grouped Lines",
      agent: worker,
      input: groupedLines,
      text: prompt.text`Classify each group in ${groupedLines} into a structured note.`,
      output: notes,
    }),
  }),
});
