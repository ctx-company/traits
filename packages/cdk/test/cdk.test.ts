import {
  agent,
  callout,
  condition,
  dependency,
  diagnostics,
  input,
  intent,
  isTraitFamilyHandle,
  operation,
  oracle,
  output,
  planner,
  port,
  procedure,
  prompt,
  resolveTraitFamily,
  resource,
  reviewer,
  rule,
  schema,
  searcher,
  sequence,
  session,
  signal,
  slot,
  snippet,
  steps,
  table,
  toDraftJson,
  toDraftJsonWithSourceMap,
  tone,
  trait,
  variant,
  worker,
} from "@ctx-traits/cdk";
import * as publicCdk from "@ctx-traits/cdk";
import type {
  AgentHandle,
  ArgvItem,
  ConditionHandle,
  DependencyFields,
  DraftOf,
  FieldRef,
  GuardHandle,
  OutputSinkHandle,
  PortHandle,
  PromptHandle,
  ResourceFields,
  RuleFields,
  SchemaHandle,
  SchemaUnionHandle,
  SchemaValue,
  SessionBinding,
  SessionHandle,
  SignalFields,
  SlotHandle,
} from "@ctx-traits/cdk";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, expectTypeOf, it } from "vitest";
import { z } from "zod";
import { zodToJsonSchema } from "zod-to-json-schema";
import { bindingNesting } from "./fixtures/binding-nesting.js";
import { prRiskTriage } from "./fixtures/readme-draft.js";

const testDirectory = dirname(fileURLToPath(import.meta.url));

describe("draft synthesis", () => {
  it("lowers a static input port default through the existing port builder", () => {
    const plan = port.input.text({
      id: "plan",
      description: "Path to the execution plan.",
      default: { value: ".plans/EXECUTION_PLAN.md" },
    });
    const draft = toDraftJson(
      trait({
        id: "port-default",
        name: "Port Default",
        description: "Static input default fixture.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        port: plan,
      }),
    ) as { readonly port?: readonly Record<string, unknown>[]; };

    expect(draft.port).toContainEqual(expect.objectContaining({
      id: "plan",
      default: { value: ".plans/EXECUTION_PLAN.md" },
    }));
  });

  it("resolves native family leaves without changing ordinary trait emission", async () => {
    const ordinary = trait("ordinary", { name: "Ordinary", summary: "Unchanged." });
    expect(toDraftJson(ordinary).id).toBe("ordinary");

    const family = trait("family", {
      variants: {
        default: variant({ name: "Default", summary: "The default leaf." }).default(),
        quick: variant({ name: "Quick", summary: "The quick leaf." }),
      },
    });
    expect(isTraitFamilyHandle(family)).toBe(true);
    const resolved = await resolveTraitFamily(family);
    expect(resolved.leaves.map((leaf) => leaf.path)).toEqual(["default", "quick"]);
    expect(resolved.leaves[0]?.draft).toMatchObject({ id: "family", variant: "default" });
  });

  it("is byte-stable", () => {
    const build = () =>
      trait({
        id: "stable",
        name: "Stable",
        description: "A deterministic fixture.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: [slot.text("result")],
      });
    const first = JSON.stringify(toDraftJson(build()));
    const second = JSON.stringify(toDraftJson(build()));

    expect(first).toBe(second);
    expect(first).toBe(readFileSync(resolve(testDirectory, "fixtures/stable-draft.json"), "utf8").trim());
  });

  it("keeps the representative README trait byte-stable", () => {
    const first = JSON.stringify(toDraftJson(prRiskTriage));
    const second = JSON.stringify(toDraftJson(prRiskTriage));

    expect(first).toBe(second);
    expect(first).toBe(readFileSync(resolve(testDirectory, "fixtures/readme-draft.json"), "utf8").trim());
  });

  it("keeps a nested-list (`[[schema:text]]`) and list-of-union (`[(schema:a|schema:b)]`) trait byte-stable", () => {
    const first = JSON.stringify(toDraftJson(bindingNesting));
    const second = JSON.stringify(toDraftJson(bindingNesting));

    expect(first).toBe(second);
    expect(first).toBe(readFileSync(resolve(testDirectory, "fixtures/binding-nesting-draft.json"), "utf8").trim());
    expect(first).toMatch(/"schema":"\[\[schema:text\]\]"/);
    expect(first).toMatch(
      /"schema":"\[\(schema:binding-nesting-note-a\|schema:binding-nesting-note-b\)\]"/,
    );
  });

  it("interpolates resource handles as prompt inputs", () => {
    const guide = resource.file("guide", { path: "resources/guide.md" });
    const result = slot.text("result");
    const draft = toDraftJson(
      trait({
        id: "resource-interpolation",
        name: "Resource Interpolation",
        description: "Resource interpolation fixture.",
        procedure: procedure({
          description: "Read the guide.",
          sequence: [
            sequence.prompt("read-guide", {
              title: "Read the guide",
              text: prompt.text`Open ${guide} and summarize it.`,
              output: result,
            }),
          ],
        }),
      }),
    ) as { readonly procedure?: { readonly sequence?: unknown; }; readonly prompt?: unknown; };

    expect(draft.procedure?.sequence).toEqual([
      {
        id: "read-guide",
        title: "Read the guide",
        prompt: "prompt:read-guide",
        input: ["resource:guide"],
        output: ["slot:result"],
      },
    ]);
    expect(draft.prompt).toEqual({
      "read-guide": {
        input: ["resource:guide"],
        output: ["slot:result"],
        text: "Open {resource:guide} and summarize it.",
      },
    });
  });
});

// `zod-to-json-schema` emits a top-level `$schema` key by default, outside
// the documented/enforced `schema.zod` subset; a real adapter strips it —
// matches `test/fixtures/adapter-trait.mjs`'s callback.
function zodToSupportedJsonSchema(value: z.ZodTypeAny): unknown {
  const { $schema: _schema, ...rest } = zodToJsonSchema(value);
  return rest;
}

describe("schema adapters", () => {
  const source = z.object({
    name: z.string().describe("Display name"),
    count: z.number().int().optional(),
    enabled: z.boolean(),
    tags: z.array(z.string()),
    mode: z.enum(["fast", "safe"]),
  });

  it("maps Zod and TypeBox-shaped schemas through the canonical declaration path", () => {
    const zodSchema = schema.zod("result", source, {
      toJsonSchema: (value) => zodToSupportedJsonSchema(value as z.ZodTypeAny),
    });
    const typeBoxSchema = schema.typebox("typebox-result", {
      type: "object",
      required: ["name", "enabled", "tags", "mode"],
      properties: {
        tags: { type: "array", items: { type: "string" } },
        enabled: { type: "boolean" },
        mode: { type: "string", enum: ["fast", "safe"] },
        count: { type: "integer" },
        name: { type: "string", description: "Display name" },
      },
    });

    const zodDraft = toDraftJson(
      trait({
        id: "zod-adapter",
        name: "Zod",
        description: "Adapter fixture.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: slot({ id: "zod-adapter-result", schema: zodSchema }),
      }),
    );
    const typeBoxDraft = toDraftJson(
      trait({
        id: "typebox-adapter",
        name: "TypeBox",
        description: "Adapter fixture.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: slot({ id: "typebox-adapter-result", schema: typeBoxSchema }),
      }),
    );

    expect(zodDraft).toMatchObject({
      schema: [{
        id: "result",
        fields: {
          name: { schema: "schema:text", required: true },
          count: { schema: "schema:integer", required: false },
          tags: { schema: "[schema:text]", required: true },
          mode: { allowed: ["fast", "safe"] },
        },
      }],
    });
    expect(typeBoxDraft).toMatchObject({
      schema: [{
        id: "typebox-result",
        fields: {
          name: { schema: "schema:text", required: true },
          count: { schema: "schema:integer", required: false },
        },
      }],
    });
    expect(JSON.stringify(zodDraft)).toBe(
      JSON.stringify(
        toDraftJson(
          trait({
            id: "zod-adapter",
            name: "Zod",
            description: "Adapter fixture.",
            procedure: procedure({ description: "No steps.", sequence: [] }),
            slot: slot({
              id: "zod-adapter-result",
              schema: schema.zod("result", source, {
                toJsonSchema: (value) => zodToSupportedJsonSchema(value as z.ZodTypeAny),
              }),
            }),
          }),
        ),
      ),
    );
    expect(JSON.stringify(zodDraft.schema)).not.toMatch(/zod|typebox|adapter/i);
  });

  it.each([
    [{ type: "object", properties: { nested: { type: "object", properties: {} } } }, "nested objects"],
    [{ type: "object", properties: { value: { type: "string", minLength: 1 } } }, "minLength"],
    [{ type: "object", properties: {}, additionalProperties: { type: "string" } }, "additionalProperties"],
    [{ oneOf: [] }, "oneOf"],
    [{ type: "object", properties: { value: { $ref: "#/x" } } }, "$ref"],
    [{ type: "object", $defs: { x: { type: "string" } }, properties: {} }, "$defs"],
    [{ type: "object", properties: { value: { type: "string", default: "x" } } }, "default"],
    [{ type: "string", enum: [1, 2] }, "declared scalar type"],
    [{ type: "object", properties: { value: { type: "string", enum: [1, 2] } } }, "declared scalar type"],
    [{ type: "object", required: ["missing"], properties: {} }, "unknown required property"],
    [
      { type: "object", required: ["value", "value"], properties: { value: { type: "string" } } },
      "duplicate required property",
    ],
    [{ type: "object", properties: { value: { type: "number", enum: [Number.NaN] } } }, "finite"],
    [{ type: "object", title: "Result", properties: {} }, "title"],
    [{ type: "object", examples: [{}], properties: {} }, "examples"],
    [{ type: "object", $schema: "http://json-schema.org/draft-07/schema#", properties: {} }, "$schema"],
  ])("rejects unsupported schema at a field-specific path", (value, keyword) => {
    expect(() => schema.typebox("unsupported", value)).toThrow(keyword);
  });

  it("rejects a top-level Zod refinement before the lossy toJsonSchema adapter ever runs", () => {
    expect(() =>
      schema.zod("refined", z.string().refine((value) => value.length > 0), {
        toJsonSchema: (value) => zodToSupportedJsonSchema(value as z.ZodTypeAny),
      })
    ).toThrow(/refinements\/transforms/);
  });

  it("rejects a nested Zod transform on an object field", () => {
    expect(() =>
      schema.zod(
        "transformed",
        z.object({ name: z.string().transform((value) => value.trim()) }),
        { toJsonSchema: (value) => zodToSupportedJsonSchema(value as z.ZodTypeAny) },
      )
    ).toThrow(/refinements\/transforms/);
  });

  it("rejects a refinement hidden behind an otherwise-unhandled wrapper (.brand())", () => {
    // `.brand()` (`ZodBranded`) is not itself an effect and is not one of
    // the wrapper kinds this rejection special-cases by name — its wrapped
    // `ZodEffects` must still be found by walking `_def.type` generically,
    // or the refinement would be silently erased by `toJsonSchema`.
    expect(() =>
      schema.zod(
        "branded-refined",
        z.object({ x: z.string().refine((value) => value.length > 0).brand() }),
        { toJsonSchema: (value) => zodToSupportedJsonSchema(value as z.ZodTypeAny) },
      )
    ).toThrow(/refinements\/transforms/);
  });
});

describe("schema authoring sugar", () => {
  it("exposes an object schema handle's own enumerable keys as exactly its field ids", () => {
    const finding = schema.object("sugar-finding", {
      file: schema.text(),
      summary: schema.text(),
    });

    expect(Object.keys(finding).sort()).toEqual(["file", "summary"]);
    expect({ ...finding }).toEqual({
      file: { schema: "schema:text", required: true },
      summary: { schema: "schema:text", required: true },
    });
  });

  it("infers an object schema's represented value shape, including optional fields", () => {
    const finding = schema.object("sugar-finding-typed", {
      file: schema.text(),
      note: schema.optional(schema.text()),
    });

    const findingSlot = slot({ id: "sugar-finding-typed-slot", schema: finding });
    expectTypeOf(findingSlot.file).toMatchTypeOf<FieldRef<string>>();
    expectTypeOf(findingSlot.note).toMatchTypeOf<FieldRef<string | undefined>>();
  });

  it("composes two schemas with plain spread as last-wins", () => {
    const a = schema.object("sugar-spread-a", { shared: schema.text(), "only-a": schema.text() });
    const b = schema.object("sugar-spread-b", { shared: schema.number(), "only-b": schema.text() });
    const merged = schema.object("sugar-spread-merged", { ...a, ...b });

    const draft = toDraftJson(
      trait({
        id: "sugar-spread",
        name: "Sugar Spread",
        description: "Plain spread is last-wins.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: slot({ id: "sugar-spread-slot", schema: merged }),
      }),
    ) as { readonly schema?: readonly { readonly id: string; readonly fields?: Record<string, unknown>; }[]; };

    const mergedDeclaration = draft.schema?.find((entry) => entry.id === "sugar-spread-merged");
    expect(mergedDeclaration?.fields).toEqual({
      shared: { schema: "schema:number", required: true },
      "only-a": { schema: "schema:text", required: true },
      "only-b": { schema: "schema:text", required: true },
    });
  });

  it("rejects schema.extend on a planted duplicate field, naming the field", () => {
    const a = schema.object("sugar-extend-a", { shared: schema.text() });
    const b = schema.object("sugar-extend-b", { shared: schema.number() });

    expect(() => schema.extend(a, b)).toThrow(/"shared"/);
  });

  it("marks a field optional via schema.optional without a second field representation", () => {
    const bare = schema.optional(schema.text());
    expect(bare).toEqual({ schema: "schema:text", required: false });

    const fromField = schema.optional(schema.field(schema.text(), { description: "Optional note." }));
    expect(fromField).toEqual({ schema: "schema:text", description: "Optional note.", required: false });
  });

  it("preserves a spread field's nested declaration through schema.optional", () => {
    const base = schema.object("sugar-optional-nested-base", {
      inner: schema.object("sugar-optional-nested-inner", {
        value: schema.text(),
      }),
    });
    const wrapped = schema.object("sugar-optional-nested-wrapped", { inner: schema.optional(base.inner!) });

    const draft = toDraftJson(
      trait({
        id: "sugar-optional-nested",
        name: "Sugar Optional Nested",
        description: "schema.optional on a spread field must not drop its nested declaration.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: slot({ id: "sugar-optional-nested-slot", schema: wrapped }),
      }),
    ) as { readonly schema?: readonly { readonly id: string; }[]; };

    expect(draft.schema?.map((entry) => entry.id).sort()).toEqual([
      "sugar-optional-nested-inner",
      "sugar-optional-nested-wrapped",
    ]);
  });

  it("lowers condition.equals(slot.foo, value) byte-identically to condition.fieldEquals(slot, 'foo', value)", () => {
    const noteSchema = schema.object("sugar-condition-note", { status: schema.text() });
    const noteSlot = slot({ id: "sugar-condition-note-slot", schema: noteSchema });
    const fieldRef = noteSlot.status;
    expectTypeOf(fieldRef).toMatchTypeOf<FieldRef<string>>();

    const guardViaFieldRef = condition.equals(fieldRef, "approved");
    const guardViaFieldEquals = condition.fieldEquals(noteSlot, "status", "approved");
    expect(JSON.stringify(guardViaFieldRef)).toBe(JSON.stringify(guardViaFieldEquals));

    // @ts-expect-error 1 is not assignable to the field's inferred `string` value type.
    condition.equals(fieldRef, 1);
  });

  it("throws naming the slot and field on unknown field access", () => {
    const noteSchema = schema.object("sugar-unknown-field-note", { status: schema.text() });
    const noteSlot = slot({ id: "sugar-unknown-field-note-slot", schema: noteSchema }) as unknown as {
      readonly nope: unknown;
    };

    expect(() => noteSlot.nope).toThrow(/sugar-unknown-field-note-slot/);
    expect(() => noteSlot.nope).toThrow(/nope/);
  });

  it("recurses field refs through nested object schemas so slot.a.b.c typechecks and lowers to a dotted field", () => {
    const decisionSchema = schema.object("sugar-0085-decision", { behavior: schema.text() });
    const hookOutputSchema = schema.object("sugar-0085-hook-output", { decision: decisionSchema });
    const hookSlot = slot({ id: "sugar-0085-hook-slot", schema: hookOutputSchema });

    const fieldRef = hookSlot.decision.behavior;
    expectTypeOf(fieldRef).toMatchTypeOf<FieldRef<string>>();

    const guard = condition.equals(fieldRef, "approve");
    expect(JSON.parse(JSON.stringify(guard))).toEqual({
      slot: "slot:sugar-0085-hook-slot",
      field: "decision.behavior",
      equals: "approve",
    });

    // @ts-expect-error 1 is not assignable to the field's inferred `string` value type.
    condition.equals(fieldRef, 1);
  });

  it("does not re-expose fields on a scalar leaf", () => {
    // A scalar leaf `FieldRef` is a plain (non-proxied) object — like every
    // one-level field ref before this change — so an unknown property reads
    // as `undefined` rather than throwing; only the slot/proxy layers throw
    // on unknown access.
    const decisionSchema = schema.object("sugar-0085-scalar-leaf-decision", { behavior: schema.text() });
    const hookOutputSchema = schema.object("sugar-0085-scalar-leaf-hook-output", { decision: decisionSchema });
    const hookSlot = slot({ id: "sugar-0085-scalar-leaf-slot", schema: hookOutputSchema }) as unknown as {
      readonly decision: { readonly behavior: { readonly nope: unknown; }; };
    };

    expect(hookSlot.decision.behavior.nope).toBeUndefined();
  });

  it("stops recursion at a list field", () => {
    const itemSchema = schema.object("sugar-0085-list-item", { value: schema.text() });
    const containerSchema = schema.object("sugar-0085-list-container", { items: schema.list(itemSchema) });
    const containerSlot = slot({ id: "sugar-0085-list-slot", schema: containerSchema }) as unknown as {
      readonly items: { readonly value: unknown; };
    };

    expect(containerSlot.items.value).toBeUndefined();
  });

  it("throws naming the slot and the full dotted path on an unknown nested field", () => {
    const decisionSchema = schema.object("sugar-0085-unknown-nested-decision", { behavior: schema.text() });
    const hookOutputSchema = schema.object("sugar-0085-unknown-nested-hook-output", { decision: decisionSchema });
    const hookSlot = slot({ id: "sugar-0085-unknown-nested-slot", schema: hookOutputSchema }) as unknown as {
      readonly decision: { readonly nope: unknown; };
    };

    expect(() => hookSlot.decision.nope).toThrow(/sugar-0085-unknown-nested-slot/);
    expect(() => hookSlot.decision.nope).toThrow(/decision\.nope/);
  });

  it("emits one-level field refs byte-identically to before this change", () => {
    const noteSchema = schema.object("sugar-0085-one-level-note", { status: schema.text() });
    const noteSlot = slot({ id: "sugar-0085-one-level-slot", schema: noteSchema });

    const guardViaFieldRef = condition.equals(noteSlot.status, "approved");
    const guardViaFieldEquals = condition.fieldEquals(noteSlot, "status", "approved");
    expect(JSON.parse(JSON.stringify(guardViaFieldRef))).toEqual({
      slot: "slot:sugar-0085-one-level-slot",
      field: "status",
      equals: "approved",
    });
    expect(JSON.stringify(guardViaFieldRef)).toBe(JSON.stringify(guardViaFieldEquals));
  });

  it("flattens a union of unions into one flat ref and still rejects a duplicate after flattening", () => {
    const a = schema.object("sugar-union-a", { value: schema.text() });
    const b = schema.object("sugar-union-b", { value: schema.text() });
    const c = schema.object("sugar-union-c", { value: schema.text() });

    const inner = schema.union([a, b]);
    const outer = schema.union([inner, c]);

    expect(String(outer)).toBe("(schema:sugar-union-a|schema:sugar-union-b|schema:sugar-union-c)");
    expect(() => schema.union([inner, a])).toThrow(/duplicate member/);
  });

  it("specializes a schema.template per call with template provenance absent from draft JSON", () => {
    const noteTemplate = schema.template(
      "sugar-note-template",
      { value: schema.text(), detail: schema.text() },
      (spots) => ({
        value: schema.field(spots.value),
        detail: schema.field(spots.detail),
      }),
    );

    const textNote = noteTemplate("sugar-text-note");
    const numberNote = noteTemplate("sugar-number-note", { value: schema.number() });

    // Overriding a spot changes that specialization's field type: `value` is
    // `FieldRef<string>` on the un-overridden specialization but
    // `FieldRef<number>` once `value` is overridden with `schema.number()`
    // — spot identity is preserved per call, not fixed at `schema.template`
    // definition time.
    const textNoteSlot = slot({ id: "sugar-text-note-slot", schema: textNote });
    expectTypeOf(textNoteSlot.value).toMatchTypeOf<FieldRef<string>>();
    expectTypeOf(textNoteSlot.detail).toMatchTypeOf<FieldRef<string>>();

    const numberNoteSlot = slot({ id: "sugar-number-note-slot", schema: numberNote });
    expectTypeOf(numberNoteSlot.value).toMatchTypeOf<FieldRef<number>>();
    expectTypeOf(numberNoteSlot.detail).toMatchTypeOf<FieldRef<string>>();
    // @ts-expect-error `value` is now `FieldRef<number>`, not `FieldRef<string>`
    expectTypeOf(numberNoteSlot.value).toMatchTypeOf<FieldRef<string>>();

    expect(textNote).not.toBe(numberNote);
    const draft = toDraftJson(
      trait({
        id: "sugar-template",
        name: "Sugar Template",
        description: "Template specialization fixture.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: [textNoteSlot, numberNoteSlot],
      }),
    );

    expect(JSON.stringify(draft)).not.toMatch(/templateId|sugar-note-template/);
    expect(draft).toMatchObject({
      schema: [
        { id: "sugar-text-note", fields: { value: { schema: "schema:text" } } },
        { id: "sugar-number-note", fields: { value: { schema: "schema:number" } } },
      ],
    });
  });

  it("keeps two same-shaped template spots independently typed at specialization", () => {
    const twoTextSpots = schema.template(
      "sugar-two-spot-template",
      { first: schema.text(), second: schema.text() },
      (spots) => {
        // Same declared shape (`schema.text()` for both), but each spot now
        // carries its own name as a private, type-only brand (see
        // `TaggedSpots`/`OriginOf` in schema.ts) so a specialization can
        // resolve `first` and `second` independently even though they'd
        // otherwise be structurally identical — `toEqualTypeOf` would fail
        // here precisely because the brands differ.
        expectTypeOf(spots.first).toMatchTypeOf<SchemaValue<string>>();
        expectTypeOf(spots.second).toMatchTypeOf<SchemaValue<string>>();
        return { first: schema.field(spots.first), second: schema.field(spots.second) };
      },
    );

    // Overriding only `first` must not change `second`'s resolved schema —
    // each named spot resolves independently by name, not by shared shape.
    const specialized = twoTextSpots("sugar-two-spot-specialized", { first: schema.number() });
    const draft = toDraftJson(
      trait({
        id: "sugar-two-spot",
        name: "Sugar Two Spot",
        description: "Origin-preserving spot fixture.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: slot({ id: "sugar-two-spot-slot", schema: specialized }),
      }),
    );

    expect(draft).toMatchObject({
      schema: [{
        id: "sugar-two-spot-specialized",
        fields: {
          first: { schema: "schema:number" },
          second: { schema: "schema:text" },
        },
      }],
    });
  });

  it("resolves a template spot by origin, not by the built field's own key", () => {
    // `build` renames the `item` spot into a `value` field — a field key
    // that never equals its spot's name. Resolving overrides by matching
    // the override key against the *field* key (the pre-fix behavior) would
    // never retype `value` here, and could misfire on an unrelated field
    // that coincidentally shares a spot's name. Resolving by the field's
    // recorded spot origin instead gets this right independent of naming.
    const itemTemplate = schema.template(
      "sugar-origin-template",
      { item: schema.text() },
      (spots) => ({ value: schema.field(spots.item) }),
    );

    const defaulted = itemTemplate("sugar-origin-default");
    const overridden = itemTemplate("sugar-origin-overridden", { item: schema.number() });

    const defaultedSlot = slot({ id: "sugar-origin-default-slot", schema: defaulted });
    expectTypeOf(defaultedSlot.value).toMatchTypeOf<FieldRef<string>>();

    const overriddenSlot = slot({ id: "sugar-origin-overridden-slot", schema: overridden });
    expectTypeOf(overriddenSlot.value).toMatchTypeOf<FieldRef<number>>();
    // @ts-expect-error `value` is now `FieldRef<number>` once `item` is overridden.
    expectTypeOf(overriddenSlot.value).toMatchTypeOf<FieldRef<string>>();

    const draft = toDraftJson(
      trait({
        id: "sugar-origin",
        name: "Sugar Origin",
        description: "Origin-preserving spot-to-field-rename fixture.",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        slot: [defaultedSlot, overriddenSlot],
      }),
    );

    expect(draft).toMatchObject({
      schema: [
        { id: "sugar-origin-default", fields: { value: { schema: "schema:text" } } },
        { id: "sugar-origin-overridden", fields: { value: { schema: "schema:number" } } },
      ],
    });
  });
});

describe("retired canonical fields", () => {
  it("exposes built-in vocabulary only through lowercase PascalCase namespaces", () => {
    expect(tone.Direct).toBe("direct");
    // @ts-expect-error builtin leaves are PascalCase.
    expectTypeOf(tone.direct);
    // @ts-expect-error legacy uppercase namespace is not exported publicly.
    expectTypeOf(publicCdk.Tone);
    expect((tone as unknown as Record<string, unknown>).direct).toBeUndefined();
    expect((tone as unknown as Record<string, unknown>).Tone).toBeUndefined();
    expect((publicCdk as Record<string, unknown>).Tone).toBeUndefined();
  });

  it("collects a reachable schema once and omits unrelated authored schemas", () => {
    const reachable = schema.object("reachable-schema", { value: schema.text() });
    schema.object("unreachable-schema", { value: schema.text() });
    const draft = toDraftJson(
      trait("schema-reachability", {
        name: "Schema Reachability",
        slot: [
          slot({ id: "reachable-schema-a", schema: reachable }),
          slot({ id: "reachable-schema-b", schema: reachable }),
        ],
      }),
    );
    expect(draft.schema?.map((entry) => entry.id)).toEqual(["reachable-schema"]);
  });

  it("rejects removed trait schema aliases at typecheck time", () => {
    const reachable = schema.object("retired-schema-reachable", { value: schema.text() });
    // @ts-expect-error schema declarations are inferred from reachable slots and ports.
    trait({ id: "retired-schema", schema: [reachable] });
    // @ts-expect-error schemas is not a trait authoring alias.
    trait({ id: "retired-schemas", schemas: [reachable] });
    // @ts-expect-error variant fields use the same inferred schema closure.
    variant({ schema: [reachable] });
    // @ts-expect-error schemas is not a variant authoring alias.
    variant({ schemas: [reachable] });
  });

  it("rejects removed procedure contract fields at typecheck time", () => {
    const request = port.input.text({ id: "retired-contract-request" });
    const result = slot.text("retired-contract-result");
    const response = port.output.text({ id: "retired-contract-response", value: result });
    procedure({
      description: "Contracts are inferred from reachable steps.",
      sequence: sequence.prompt("retired-contract-step", { text: prompt.text`Use ${request}.`, output: result }),
      // @ts-expect-error procedure input ports are inferred from consumed refs.
      input: request,
    });
    procedure({
      description: "Contracts are inferred from reachable steps.",
      sequence: sequence.prompt("retired-contract-output-step", { text: prompt.text`Use ${request}.`, output: result }),
      // @ts-expect-error procedure output ports are inferred from produced bound slots.
      output: response,
    });
  });

  it("rejects authoring status/trust on a trait draft", () => {
    trait({
      id: "retired-fields",
      name: "Retired Fields",
      description: "Status/trust must not be authorable on the canonical document.",
      procedure: procedure({ description: "No steps.", sequence: [] }),
      // @ts-expect-error status moved to [package].status in trait.toml; never authored here.
      status: "draft",
      // @ts-expect-error trust is machine-local only; never authored here.
      trust: "local",
    });
  });
});

describe("sequence.loop on-exhausted (P399)", () => {
  it("collapses continue/block/signal spellings to the canonical scalar-or-array shape", () => {
    const exhaustedOne = signal({ id: "exhausted-one", description: "Loop A ran out of budget." });
    const exhaustedTwo = signal({ id: "exhausted-two", description: "Loop B ran out of budget." });
    const bodyOf = (id: string) =>
      sequence.linear(id, [sequence.prompt(`${id}-step`, { text: prompt.text`Do work.` })]);

    const draft = toDraftJson(
      trait({
        id: "loop-on-exhausted",
        name: "Loop On Exhausted",
        description: "P399 on-exhausted spelling fixture.",
        procedure: procedure({
          description: "Exercise every on-exhausted spelling.",
          sequence: [
            sequence.loop("default-loop", { sequence: bodyOf("default-body"), iterations: 3 }),
            sequence.loop("block-loop", {
              sequence: bodyOf("block-body"),
              iterations: 3,
              onExhausted: sequence.flow.Block,
            }),
            sequence.loop("signal-loop", {
              sequence: bodyOf("signal-body"),
              iterations: 3,
              onExhausted: exhaustedOne,
            }),
            sequence.loop("multi-signal-loop", {
              sequence: bodyOf("multi-signal-body"),
              iterations: 3,
              onExhausted: [exhaustedOne, exhaustedTwo],
            }),
          ],
        }),
        signal: [exhaustedOne, exhaustedTwo],
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    const items = draft.procedure?.sequence ?? [];
    expect(items[0]?.["on-exhausted"]).toBeUndefined();
    expect(items[1]?.["on-exhausted"]).toBe("block");
    expect(items[2]?.["on-exhausted"]).toBe("signal:exhausted-one");
    expect(items[3]?.["on-exhausted"]).toEqual(["signal:exhausted-one", "signal:exhausted-two"]);
  });

  it("rejects an empty on-exhausted signal array at build time", () => {
    expect(() =>
      sequence.loop("empty-signals-loop", {
        sequence: sequence.linear("empty-signals-body", [
          sequence.prompt("empty-signals-step", { text: prompt.text`Do work.` }),
        ]),
        iterations: 3,
        onExhausted: [],
      })
    ).toThrow(/must not be empty/);
  });
});

describe("input.command", () => {
  it("splits literal words and inserts each interpolated handle as one argv element", () => {
    const message = slot.text("message");
    const template = input.command`git commit -m ${message}`;
    expect(template.argv).toEqual(["git", "commit", "-m", message]);
  });

  it("collapses repeated literal whitespace and trims leading/trailing literal segments", () => {
    const cwd = slot.text("cwd");
    const template = input.command`  git   -C ${cwd}   status  `;
    expect(template.argv).toEqual(["git", "-C", cwd, "status"]);
  });

  it("tokenizes quoted words as one argv element and unescapes double-quote escapes", () => {
    const template = input.command`git commit -m "hello world" --allow-empty-message`;
    expect(template.argv).toEqual(["git", "commit", "-m", "hello world", "--allow-empty-message"]);
    const escaped = input.command`echo "say \\"hi\\""`;
    expect(escaped.argv).toEqual(["echo", "say \"hi\""]);
  });

  it("compiles through sequence.command's input field identically to explicit argv", () => {
    const message = slot.text("message");
    const draftFor = (
      fields: { readonly argv: readonly ArgvItem[]; } | { readonly input: ReturnType<typeof input.command>; },
    ) =>
      toDraftJson(
        trait("commit-only", {
          version: "0.1.0",
          name: "Commit Only",
          procedure: procedure({ description: "No steps.", sequence: [sequence.command({ id: "commit", ...fields })] }),
          slot: [message],
        }),
      );
    expect(draftFor({ input: input.command`git commit -m ${message}` })).toEqual(
      draftFor({ argv: ["git", "commit", "-m", message] }),
    );
  });

  it("rejects shell-only syntax in a literal segment", () => {
    expect(() => input.command`git log && rm -rf /`).toThrow(/shell-only syntax/);
  });

  it("rejects unquoted shell-substitution and redirection syntax even split across words", () => {
    expect(() => input.command`echo $(whoami)`).toThrow(/shell-only syntax/);
    expect(() => input.command`echo hi > out.txt`).toThrow(/shell-only syntax/);
  });

  it("rejects an interpolated value spliced into a word", () => {
    const flag = slot.text("flag");
    expect(() => input.command`git --flag=${flag}`).toThrow(/standalone argv token/);
  });

  it("rejects an unterminated quote", () => {
    expect(() => input.command`git commit -m "unterminated`).toThrow(/shell-only syntax/);
  });

  it("rejects an empty template", () => {
    expect(() => input.command``).toThrow(/argv must not be empty/);
  });
});

describe("input.text / output.text / output.of (0045)", () => {
  it("accepts a PromptTemplate as a prompt step's input: field, same as text:", () => {
    const diff = slot.text("io-diff");
    const step = sequence.prompt("io-review", { input: input.text`Review ${diff}.` });
    const draft = toDraftJson(
      trait({
        id: "io-input-text",
        name: "IO Input Text",
        description: "input.text fixture.",
        procedure: procedure({ description: "Review.", sequence: [step] }),
      }),
    ) as { readonly procedure?: { readonly sequence?: unknown; }; readonly prompt?: unknown; };

    expect(draft.procedure?.sequence).toEqual([
      { id: "io-review", title: "Io Review", prompt: "prompt:io-review", input: ["slot:io-diff"] },
    ]);
    expect(draft.prompt).toEqual({
      "io-review": { input: ["slot:io-diff"], text: "Review {slot:io-diff}." },
    });
  });

  it("output.text auto-declares a schema:text slot at the step id and renders a return-format instruction", () => {
    const step = sequence.prompt("io-summarize", {
      input: input.text`Summarize the work so far.`,
      output: output.text`Write a one-paragraph work summary.`,
    });
    const draft = toDraftJson(
      trait({
        id: "io-output-text",
        name: "IO Output Text",
        description: "output.text fixture.",
        procedure: procedure({ description: "Summarize.", sequence: [step] }),
      }),
    ) as {
      readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; };
      readonly prompt?: Record<string, { readonly text?: string; }>;
      readonly slot?: readonly { readonly id?: string; readonly schema?: string; }[];
    };

    expect(draft.procedure?.sequence?.[0]).toMatchObject({ output: ["slot:io-summarize"] });
    expect(draft.slot).toContainEqual(expect.objectContaining({ id: "io-summarize", schema: "schema:text" }));
    expect(draft.prompt?.["io-summarize"]?.text).toBe(
      "Summarize the work so far.\n\nWrite a one-paragraph work summary."
        + "\n\nReturn plain text only, with no surrounding commentary.",
    );
  });

  it("output.of auto-declares a schema-typed slot and names the schema in the rendered instruction", () => {
    const verdictSchema = schema.text();
    const step = sequence.prompt("io-verdict", {
      input: input.text`Review the change.`,
      output: output.of(verdictSchema)`Your verdict, citing every finding.`,
    });
    const draft = toDraftJson(
      trait({
        id: "io-output-of",
        name: "IO Output Of",
        description: "output.of fixture.",
        procedure: procedure({ description: "Verdict.", sequence: [step] }),
      }),
    ) as {
      readonly prompt?: Record<string, { readonly text?: string; }>;
      readonly slot?: readonly { readonly id?: string; readonly schema?: string; }[];
    };

    expect(draft.slot).toContainEqual(expect.objectContaining({ id: "io-verdict", schema: "schema:text" }));
    expect(draft.prompt?.["io-verdict"]?.text).toBe(
      "Review the change.\n\nYour verdict, citing every finding.\n\nReturn JSON matching schema:text, with no surrounding commentary.",
    );
  });

  it("a second anonymous output on the same step takes the <step-id>-2 auto id", () => {
    const step = sequence.prompt("io-multi", {
      input: input.text`Do the work.`,
      output: [output.text`Summary one.`, output.text`Summary two.`],
    });
    const draft = toDraftJson(
      trait({
        id: "io-output-multi",
        name: "IO Output Multi",
        description: "multi-output fixture.",
        procedure: procedure({ description: "Multi.", sequence: [step] }),
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    expect(draft.procedure?.sequence?.[0]).toMatchObject({ output: ["slot:io-multi", "slot:io-multi-2"] });
  });

  it("an interpolated instruction-output resolves to the attaching step's slot once authored produce-then-consume", () => {
    const summary = output.text`A one-paragraph summary.`;
    const producer = sequence.prompt("io-produce", {
      input: input.text`Summarize the diff.`,
      output: summary,
    });
    const consumer = sequence.prompt("io-consume", {
      input: input.text`Given ${summary}, write a title.`,
    });
    const draft = toDraftJson(
      trait({
        id: "io-consume-fixture",
        name: "IO Consume Fixture",
        description: "consume fixture.",
        procedure: procedure({ description: "Both.", sequence: [producer, consumer] }),
      }),
    ) as { readonly prompt?: Record<string, { readonly input?: readonly string[]; }>; };

    expect(draft.prompt?.["io-consume"]?.input).toEqual(["slot:io-produce"]);
  });

  it("interpolating an instruction-output before its producing step is authored fails", () => {
    const summary = output.text`A one-paragraph summary.`;
    expect(() => input.text`Given ${summary}, write a title.`).toThrow(/before the step/);
  });

  it("collision between an auto-declared and hand-declared slot of the same id cites both names", () => {
    const clashing = slot.text("io-clash");
    expect(() =>
      sequence.prompt("io-clash", {
        input: input.text`Do the work.`,
        output: [clashing, output.text`Summary.`],
      })
    ).toThrow(/io-clash/);
  });

  it("warns when an explicit input: entry duplicates a ref already interpolated into the template", () => {
    const diff = slot.text("io-dup-diff");
    const step = sequence.prompt("io-dup", { input: input.text`Review ${diff}.`, include: [diff] });
    const { diagnostics: warnings } = toDraftJsonWithSourceMap(
      trait({
        id: "io-duplicate-ref",
        name: "IO Duplicate Ref",
        description: "duplicate-ref fixture.",
        procedure: procedure({ description: "Review.", sequence: [step] }),
      }),
    );

    expect(warnings).toContainEqual(
      expect.objectContaining({ code: "cdk-duplicate-input", fieldPath: "sequence.input" }),
    );
  });
});

describe("sequence.command / sequence.check include", () => {
  it("collects a plain resource declaration reachable only through include", () => {
    const guide = resource.inline("guide", "Prefer minimal diffs.");
    const draft = toDraftJson(
      trait("commit-only", {
        version: "0.1.0",
        name: "Commit Only",
        procedure: procedure({
          description: "No steps.",
          sequence: [sequence.command({ id: "commit", cmd: "git commit", include: guide })],
        }),
      }),
    ) as {
      readonly resource?: unknown;
      readonly procedure?: { readonly sequence?: readonly { readonly input?: unknown; }[]; };
    };

    expect(draft.resource).toEqual([{ id: "guide", content: "Prefer minimal diffs." }]);
    expect(draft.procedure?.sequence?.[0]?.input).toEqual(["resource:guide"]);
  });

  it("collects a slot declaration reachable only through include", () => {
    const scope = slot.text("scope");
    const draft = toDraftJson(
      trait("check-only", {
        version: "0.1.0",
        name: "Check Only",
        procedure: procedure({
          description: "No steps.",
          sequence: [
            sequence.check("verify", { cmd: "git diff --quiet", output: slot.boolean("clean"), include: scope }),
          ],
        }),
      }),
    ) as {
      readonly slot?: readonly { readonly id: string; }[];
      readonly procedure?: { readonly sequence?: readonly { readonly input?: unknown; }[]; };
    };

    expect(draft.slot?.some((entry) => entry.id === "scope")).toBe(true);
    expect(draft.procedure?.sequence?.[0]?.input).toEqual(["slot:scope"]);
  });

  it("keeps honoring a legacy input: dependency list on a cmd command step (P447 surface)", () => {
    // The landed optional-input proof authors exactly this shape: a `cmd:`
    // command step declaring optional deps through `input:`. Sources are
    // evaluated, never type-checked, so this list must never be silently
    // dropped from the step's input evidence.
    const verdict = slot.text("verdict");
    const marker = slot.text("marker");
    const draft = toDraftJson(
      trait("legacy-input", {
        version: "0.1.0",
        name: "Legacy Input",
        procedure: procedure({
          description: "No steps.",
          sequence: [
            sequence.command({
              id: "peek",
              cmd: "printf peeked",
              input: [input.optional(verdict)],
              output: marker,
            }),
          ],
        }),
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly { readonly input?: unknown; }[]; }; };

    expect(draft.procedure?.sequence?.[0]?.input).toEqual([{ slot: "slot:verdict", optional: true }]);
  });

  it("merges a legacy input: list before include: entries on the same step", () => {
    const verdict = slot.text("verdict");
    const scope = slot.text("scope");
    const marker = slot.text("marker");
    const draft = toDraftJson(
      trait("legacy-merge", {
        version: "0.1.0",
        name: "Legacy Merge",
        procedure: procedure({
          description: "No steps.",
          sequence: [
            sequence.command({
              id: "peek",
              cmd: "printf peeked",
              input: [input.optional(verdict)],
              include: scope,
              output: marker,
            }),
          ],
        }),
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly { readonly input?: unknown; }[]; }; };

    expect(draft.procedure?.sequence?.[0]?.input).toEqual([
      { slot: "slot:verdict", optional: true },
      "slot:scope",
    ]);
  });

  it("collects a slot declaration reachable only through include(input.optional(slot))", () => {
    const scope = slot.text("scope");
    const draft = toDraftJson(
      trait("check-optional", {
        version: "0.1.0",
        name: "Check Optional",
        procedure: procedure({
          description: "No steps.",
          sequence: [
            sequence.check("verify", {
              cmd: "git diff --quiet",
              output: slot.boolean("clean"),
              include: input.optional(scope),
            }),
          ],
        }),
      }),
    ) as { readonly slot?: readonly { readonly id: string; }[]; };

    expect(draft.slot?.some((entry) => entry.id === "scope")).toBe(true);
  });
});

describe("CDK grammar v2 shape (P458 S1)", () => {
  it("lowers sequence.ask with its declared direct signal guard", () => {
    const needsHuman = signal({ id: "needs-human-input", description: "Needs a human answer." });
    const question = slot.text("human-question");
    const answer = slot.text("human-answer");
    const ask = sequence.ask("ask-reviewer", {
      prompt: prompt.text`Question: ${question}`,
      when: needsHuman,
      output: answer,
    });
    const draft = toDraftJson(trait("ask", {
      version: "0.1.0",
      name: "Ask",
      procedure: procedure({ description: "Ask a human.", sequence: [ask] }),
    })) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };
    expect(draft.procedure?.sequence?.[0]).toMatchObject({
      kind: "ask",
      when: "signal:needs-human-input",
      output: ["slot:human-answer"],
      input: ["slot:human-question"],
    });
  });

  it("lowers sequence.gate to the existing loop, ask, branch, and project grammar", () => {
    const proposal = slot.text("proposal");
    const ready = signal({ id: "proposal-ready", description: "A proposal is ready for review." });
    const producer = sequence.prompt("produce", {
      text: prompt.text`Produce a proposal.`,
      output: proposal,
      emits: ready,
    });
    const draft = toDraftJson(trait("approval-gate", {
      version: "0.1.0",
      name: "Approval Gate",
      procedure: procedure({
        description: "Review a proposal.",
        sequence: [sequence.gate(producer, { proposal, when: ready, rounds: 2 })],
      }),
    })) as {
      readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; };
      readonly sequence?: Record<string, { readonly sequence?: readonly Record<string, unknown>[]; }>;
    };

    expect(draft.procedure?.sequence?.[0]).toMatchObject({
      id: "produce-gate",
      kind: "loop",
      "max-iterations": 2,
      "on-exhausted": "block",
    });
    const body = draft.sequence?.["produce-gate-body"]?.sequence ?? [];
    expect(body.map((item) => item.kind)).toEqual(["project", undefined, "ask", "branch"]);
    expect(body[2]).toMatchObject({ kind: "ask", input: ["slot:proposal"] });
    expect(JSON.stringify(draft)).not.toContain("\"kind\":\"gate\"");
  });

  it("accepts both direct and derived producer signal emissions", () => {
    const proposal = slot.text("signal-proposal");
    const ready = signal({ id: "signal-ready", description: "Proposal ready." });
    const direct = sequence.prompt("signal-direct", {
      text: prompt.text`Produce.`,
      output: proposal,
      emits: ready,
    });
    const derived = sequence.prompt("signal-derived", {
      text: prompt.text`Produce.`,
      output: proposal,
      emits: { signal: ready, when: condition.equals(proposal, "ready") },
    });

    expect(() => sequence.gate(direct, { proposal, when: ready, rounds: 1 })).not.toThrow();
    expect(() => sequence.gate(derived, { proposal, when: ready, rounds: 1 })).not.toThrow();
    expect(() =>
      sequence.gate(direct, {
        proposal,
        when: signal({ id: "other-ready", description: "Other." }),
        rounds: 1,
      })
    ).toThrow(/must declare signal:other-ready/);
  });

  it("rejects a custom edit arm that does not replace the proposal", () => {
    const proposal = slot.text("edit-proposal");
    const ready = signal({ id: "edit-ready", description: "Proposal ready." });
    const producer = sequence.prompt("edit-produce", {
      text: prompt.text`Produce.`,
      output: proposal,
      emits: ready,
    });
    const unrelated = sequence.project("unrelated-edit", {
      projections: [{ source: operation.literal("unchanged"), destination: slot.text("other-proposal") }],
    });

    expect(() =>
      sequence.gate(producer, {
        proposal,
        when: ready,
        rounds: 1,
        editStep: unrelated,
      })
    ).toThrow(/editStep must replace slot:edit-proposal/);
  });

  it("lowers sequence.branch's gate-step check to [check-step, branch] with the check emitted exactly once", () => {
    const ready = slot.boolean("ready");
    const gate = sequence.check("gate", { cmd: "test -f ready", output: ready });
    const mergeStep = sequence.command("merge", { cmd: "git merge --no-ff" });
    const draft = toDraftJson(
      trait("gate-branch", {
        version: "0.1.0",
        name: "Gate Branch",
        procedure: procedure({
          description: "Gate then branch.",
          sequence: [
            sequence.branch("route", { check: gate, success: [mergeStep] }),
          ],
        }),
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    const items = draft.procedure?.sequence ?? [];
    expect(items.filter((item) => item.id === "gate")).toHaveLength(1);
    expect(items[0]?.id).toBe("gate");
    expect(items[0]?.kind).toBe("check");
    expect(items[1]?.id).toBe("route");
    expect(items[1]?.kind).toBe("branch");
    expect(items[1]?.when).toEqual({ slot: "slot:ready", equals: true });
  });

  it("does not leak sequence.check's .pass convenience field into the built canonical", () => {
    const ready = slot.boolean("ready");
    const gate = sequence.check("gate", { cmd: "test -f ready", output: ready });
    expect(gate.pass).toBe(ready);
    expect(Object.keys(gate)).not.toContain("pass");
    expect(Object.prototype.propertyIsEnumerable.call(gate, "pass")).toBe(false);

    const draft = toDraftJson(
      trait("standalone-check", {
        version: "0.1.0",
        name: "Standalone Check",
        procedure: procedure({ description: "One check step.", sequence: [gate] }),
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    expect(draft.procedure?.sequence?.[0]).not.toHaveProperty("pass");
  });

  it("accepts success-only and failure-only branches, and rejects neither given", () => {
    const step = sequence.command("noop-step", { cmd: "true" });
    const draftFor = (fields: { readonly success?: readonly unknown[]; readonly failure?: readonly unknown[]; }) =>
      toDraftJson(
        trait("branch-arms", {
          version: "0.1.0",
          name: "Branch Arms",
          procedure: procedure({
            description: "One arm each.",
            sequence: [
              sequence.branch(
                "route",
                {
                  check: condition.output.is("approved"),
                  ...fields,
                } as Parameters<typeof sequence.branch>[1],
              ),
            ],
          }),
        }),
      ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    expect(() => draftFor({ success: [step] })).not.toThrow();
    expect(() => draftFor({ failure: [step] })).not.toThrow();
    expect(() =>
      sequence.branch("route", { check: condition.output.is("approved") } as Parameters<typeof sequence.branch>[1])
    ).toThrow(/branch requires success, failure, or both/);
  });

  it("builds sequence.when identically to sequence.branch for the equivalent guard/arms", () => {
    const mergeStep = sequence.command("merge", { cmd: "git merge --no-ff" });
    const reviseStep = sequence.command("revise", { cmd: "revise" });
    const draftFor = (items: readonly unknown[]) =>
      toDraftJson(
        trait("when-vs-branch", {
          version: "0.1.0",
          name: "When Vs Branch",
          procedure: procedure({ description: "Route.", sequence: items as never }),
        }),
      );
    const viaWhen = draftFor([
      sequence.when("route", {
        if: condition.output.is("approved"),
        // oxlint-disable-next-line unicorn/no-thenable -- BranchSequenceFields.then is a step ref, not a promise.
        then: [mergeStep],
        otherwise: [reviseStep],
      }),
    ]);
    const viaBranch = draftFor([
      sequence.branch("route", {
        check: condition.output.is("approved"),
        success: [mergeStep],
        failure: [reviseStep],
      }),
    ]);
    expect(viaBranch).toEqual(viaWhen);
  });

  it("emits idle-timeout-ms on a command step that declares it, and omits it otherwise", () => {
    const draftFor = (idleTimeoutMs: number | undefined) =>
      toDraftJson(
        trait("idle-timeout", {
          version: "0.1.0",
          name: "Idle Timeout",
          procedure: procedure({
            description: "One command step.",
            sequence: [
              sequence.command("gate", { cmd: "true", ...(idleTimeoutMs === undefined ? {} : { idleTimeoutMs }) }),
            ],
          }),
        }),
      ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    expect(draftFor(5_000).procedure?.sequence?.[0]).toMatchObject({ command: { "idle-timeout-ms": 5_000 } });
    const withoutIdle = draftFor(undefined).procedure?.sequence?.[0] as { readonly command?: Record<string, unknown>; };
    expect(withoutIdle?.command).not.toHaveProperty("idle-timeout-ms");
  });

  it("auto-names an inline loop body <step-id>-body", () => {
    const bodyStep = sequence.prompt("refine", { text: prompt.text`Refine the work.` });
    const draft = toDraftJson(
      trait("inline-loop-body", {
        version: "0.1.0",
        name: "Inline Loop Body",
        procedure: procedure({
          description: "Loop with an inline body.",
          sequence: [
            sequence.loop("refinement-loop", { body: [bodyStep], iterations: 3 }),
          ],
        }),
      }),
    ) as {
      readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; };
      readonly sequence?: Record<string, unknown>;
    };

    expect(draft.procedure?.sequence?.[0]?.sequence).toBe("sequence:refinement-loop-body");
    expect(draft.sequence?.["refinement-loop-body"]).toBeDefined();
  });

  it("auto-names an inline forEach body <step-id>-body", () => {
    const files = slot.texts("files");
    const currentFile = slot.text("current-file");
    const bodyStep = sequence.prompt("review-one", { text: prompt.text`Review a file.` });
    const draft = toDraftJson(
      trait("inline-foreach-body", {
        version: "0.1.0",
        name: "Inline ForEach Body",
        procedure: procedure({
          description: "ForEach with an inline body.",
          sequence: [
            sequence.forEach("review-each", { over: files, item: currentFile, body: [bodyStep] }),
          ],
        }),
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    expect(draft.procedure?.sequence?.[0]?.sequence).toBe("sequence:review-each-body");
  });

  it("infers ports through named control-flow sequences and discovers bound output ports", () => {
    const request = port.input.text({ id: "contract-request", optional: true });
    const result = slot.text("contract-result");
    const response = port.output.of({ id: "contract-response", schema: "schema:text", value: result });
    const work = sequence.linear("contract-work", [
      sequence.prompt("contract-prompt", { text: prompt.text`Use ${request}.`, output: result }),
    ]);
    const draft = toDraftJson(
      trait("contract-inference", {
        name: "Contract Inference",
        procedure: procedure({
          description: "Traverse a named sequence inside a loop.",
          sequence: sequence.loop("contract-loop", { sequence: work, iterations: 1 }),
        }),
        port: response,
      }),
    );

    expect(draft.procedure?.input).toEqual(["port:contract-request"]);
    expect(draft.procedure?.output).toEqual(["port:contract-response"]);
    expect(draft.port).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: "contract-request" }),
      expect.objectContaining({ id: "contract-response", value: "slot:contract-result" }),
    ]));
  });

  it("infers a bound output produced by a project step", () => {
    const source = slot.text("project-source");
    const result = slot.text("project-result");
    const response = port.output.of({ id: "project-response", schema: "schema:text", value: result });
    const draft = toDraftJson(trait("project-contract", {
      name: "Project Contract",
      port: response,
      procedure: procedure({
        description: "Project a result to the response boundary.",
        sequence: [sequence.project("project-result", {
          projections: [{ source, destination: result }],
        })],
      }),
    }));

    expect(draft.procedure?.output).toEqual(["port:project-response"]);
  });

  it("infers only leaf-local ports in authored order, including direct output sinks", async () => {
    const request = port.input.text({ id: "ordered-request", default: { cmd: "default-request" } });
    const result = slot.text("ordered-result");
    const direct = port.output.text({ id: "direct-result" });
    const bound = port.output.of({ id: "bound-result", schema: "schema:text", value: result });
    const unproduced = port.output.text({ id: "unproduced-result" });
    const nested = sequence.linear("nested-contract", [
      sequence.prompt("produce-contract", { text: prompt.text`Use ${request}.`, output: result }),
    ]);
    const family = trait("isolated-contract", {
      version: "0.1.0",
      variants: {
        one: variant({
          name: "One",
          procedure: procedure({
            description: "Produce nested and direct results.",
            sequence: sequence.branch("route-contract", {
              check: condition.output.is("yes"),
              success: [sequence.loop("nested-loop", { sequence: nested, iterations: 1 })],
              failure: [sequence.prompt("produce-direct", { text: prompt.text`Finish.`, output: direct })],
            }),
          }),
          port: [bound, direct, unproduced, request],
        }).default(),
        two: variant({
          name: "Two",
          procedure: procedure({
            description: "Do not use the sibling boundary.",
            sequence: sequence.prompt("other", { text: prompt.text`Other.` }),
          }),
          port: port.output.text({ id: "bound-result" }),
        }),
      },
    });
    const resolved = await resolveTraitFamily(family);
    const one = resolved.leaves.find((leaf) => leaf.path === "one")?.draft;
    const two = resolved.leaves.find((leaf) => leaf.path === "two")?.draft;

    expect(one?.procedure?.input).toEqual(["port:ordered-request"]);
    expect(one?.procedure?.output).toEqual(["port:direct-result", "port:bound-result"]);
    expect(one?.port?.map(({ id }) => id)).toEqual([
      "ordered-request",
      "direct-result",
      "bound-result",
      "unproduced-result",
    ]);
    expect(two?.procedure?.output).toBeUndefined();
  });

  it("keeps inferred output boundaries local when a procedure handle is reused", () => {
    const result = slot.text("shared-result");
    const shared = procedure({
      description: "Produce one shared result.",
      sequence: sequence.prompt("produce-shared", { text: prompt.text`Produce.`, output: result }),
    });
    const first = port.output.of({ id: "first-output", schema: "schema:text", value: result });
    const second = port.output.of({ id: "second-output", schema: "schema:text", value: result });

    const a = toDraftJson(trait("first-contract", { name: "First", procedure: shared, port: first }));
    const b = toDraftJson(trait("second-contract", { name: "Second", procedure: shared, port: second }));

    expect(a.procedure?.output).toEqual(["port:first-output"]);
    expect(b.procedure?.output).toEqual(["port:second-output"]);
  });

  it("matches the former explicit contracts across inputs, outputs, and source-map traversal", () => {
    const required = port.input.text({ id: "parity-required" });
    const optional = port.input.text({ id: "parity-optional", optional: true });
    const defaulted = port.input.text({ id: "parity-defaulted", default: { cmd: "default-value" } });
    const promptResult = slot.text("parity-prompt-result");
    const projectResult = slot.text("parity-project-result");
    const promptOutput = port.output.of({ id: "parity-prompt-output", schema: "schema:text", value: promptResult });
    const projectOutput = port.output.of({ id: "parity-project-output", schema: "schema:text", value: projectResult });
    const directOutput = port.output.text({ id: "parity-direct-output" });
    const unproduced = port.output.of({
      id: "parity-unproduced",
      schema: "schema:text",
      value: slot.text("parity-never-produced"),
    });
    const nested = sequence.linear("parity-nested", [
      sequence.prompt("parity-prompt", {
        text: prompt.text`Use ${required}, ${optional}, and ${defaulted}.`,
        output: promptResult,
      }),
      sequence.project("parity-project", {
        projections: [{ source: promptResult, destination: projectResult }],
      }),
    ]);
    const { draft, __map } = toDraftJsonWithSourceMap(
      trait("contract-parity", {
        name: "Contract Parity",
        port: [promptOutput, unproduced, required, directOutput, optional, projectOutput, defaulted],
        procedure: procedure({
          description: "Exercise every inferred contract boundary.",
          sequence: sequence.branch("parity-branch", {
            check: condition.output.is("continue"),
            success: [sequence.loop("parity-loop", { sequence: nested, iterations: 1 })],
            failure: [sequence.prompt("parity-direct", { text: prompt.text`Finish.`, output: directOutput })],
          }),
        }),
      }),
    );

    // This is the exact order and membership formerly authored on procedure().
    expect(draft.procedure?.input).toEqual([
      "port:parity-required",
      "port:parity-optional",
      "port:parity-defaulted",
    ]);
    expect(draft.procedure?.output).toEqual([
      "port:parity-prompt-output",
      "port:parity-project-output",
      "port:parity-direct-output",
    ]);
    expect(draft.procedure?.output).not.toContain("port:parity-unproduced");
    expect(__map["port:parity-prompt-output"]).toBeDefined();
    expect(__map["port:parity-project-output"]).toBeDefined();
  });

  it("rejects a loop/forEach declaring both sequence and body, or neither", () => {
    const named = sequence.linear("named-body", [sequence.prompt("step", { text: prompt.text`Do work.` })]);
    expect(() => sequence.loop("both", { sequence: named, body: [], iterations: 1 })).toThrow(
      /expected exactly one of sequence or body/,
    );
    expect(() => sequence.loop("neither", { iterations: 1 } as Parameters<typeof sequence.loop>[1])).toThrow(
      /expected exactly one of sequence or body/,
    );
  });

  it("synthesizes the forEach closure's item slot as <step-id>-item regardless of the callback parameter name", () => {
    const files = slot.texts("files");
    const draft = toDraftJson(
      trait("foreach-closure", {
        version: "0.1.0",
        name: "ForEach Closure",
        procedure: procedure({
          description: "ForEach via closure.",
          sequence: [
            sequence.forEach("review-each", files, (whateverNameYouLike) => ({
              body: [sequence.prompt("review-one", { text: prompt.text`Review ${whateverNameYouLike}.` })],
            })),
          ],
        }),
      }),
    ) as { readonly procedure?: { readonly sequence?: readonly Record<string, unknown>[]; }; };

    expect(draft.procedure?.sequence?.[0]?.item).toBe("slot:review-each-item");
  });

  it("produces identical drafts from positional and object sequence.command forms", () => {
    const draftFor = (step: unknown) =>
      toDraftJson(
        trait("command-forms", {
          version: "0.1.0",
          name: "Command Forms",
          procedure: procedure({ description: "One command.", sequence: [step as never] }),
        }),
      );
    expect(draftFor(sequence.command("commit", { cmd: "git commit" }))).toEqual(
      draftFor(sequence.command({ id: "commit", cmd: "git commit" })),
    );
  });
});

describe("public brands and values", () => {
  it("preserves handle brands and values", () => {
    const textPort = port.input.text({ id: "request" });
    const textSlot = slot.text("result");
    const listSlot = slot.texts("results");
    const promptHandle = {} as unknown as PromptHandle<string, number>;

    expectTypeOf(textPort).toMatchTypeOf<PortHandle<string>>();
    expectTypeOf(textSlot).toMatchTypeOf<SlotHandle<string>>();
    expectTypeOf(promptHandle).toMatchTypeOf<PromptHandle<string, number>>();
    expectTypeOf(operation.over(listSlot, operation.Append)).toMatchTypeOf<OutputSinkHandle<string>>();

    // @ts-expect-error Slots are not ports.
    expectTypeOf(textSlot).toMatchTypeOf<PortHandle<string>>();
    // @ts-expect-error Append requires an array-valued slot.
    operation.over(textSlot, operation.Append);
  });
});

it("keeps the README example synchronized with its compiled fixture", () => {
  const readme = readFileSync(resolve(testDirectory, "../README.md"), "utf8");
  const example = readme.match(/```ts\n([\s\S]*?)```/)?.[1];
  expect(example).toBe(readFileSync(resolve(testDirectory, "fixtures/readme-example.ts"), "utf8"));
});

it("passes metadata family/variant and facets through the draft unchanged", () => {
  const draft = toDraftJson(
    trait("plan-quick", {
      version: "0.1.0",
      name: "Plan (Quick)",
      metadata: { family: "plan", variant: "quick", tag: ["first-party"] },
      procedure: procedure({ description: "No steps.", sequence: [] }),
      slot: [slot.text("result")],
    }),
  ) as { metadata?: Record<string, unknown>; };

  expect(draft.metadata).toEqual({ family: "plan", variant: "quick", tag: ["first-party"] });
});

describe("agent.* templates vs deprecated bare exports (P459)", () => {
  it.each([
    ["worker", agent.worker, worker] as const,
    ["reviewer", agent.reviewer, reviewer] as const,
    ["planner", agent.planner, planner] as const,
    ["oracle", agent.oracle, oracle] as const,
    ["searcher", agent.searcher, searcher] as const,
  ])("%s: namespaced and bare exports emit an identical declaration", (_name, namespaced, bare) => {
    const namespacedHandle = namespaced("role", { description: "Custom description." });
    const bareHandle = bare("role", { description: "Custom description." });

    expect(toDraftJson(bareHandle)).toEqual(
      toDraftJson(namespacedHandle),
    );
  });

  it("reports a deprecation diagnostic only for the bare export", () => {
    const namespacedHandle = agent.worker("role", { description: "Custom description." });
    const bareHandle = worker("role", { description: "Custom description." });

    expect(diagnostics(namespacedHandle)).toEqual([]);
    expect(diagnostics(bareHandle)).toEqual([
      {
        severity: "warning",
        code: "cdk.agent-template.bare-export-deprecated",
        fieldPath: "agent.role",
        message: "Bare `worker(...)` is deprecated; use `agent.worker(...)` instead.",
      },
    ]);
  });
});

describe("session", () => {
  it("a named session bound to two agents emits the same session:<id> ref on both", () => {
    const shared = session("shared-scan", { description: "Shared scan session." });
    const first = agent("first", { description: "First agent.", session: shared });
    const second = agent.reviewer("second", { session: shared });

    const firstDraft = toDraftJson(first);
    const secondDraft = toDraftJson(second);

    expect(firstDraft.session).toBe("session:shared-scan");
    expect(secondDraft.session).toBe(firstDraft.session);

    // P481 Done-when #1: tsc catches a misspelled canonical key with no cast.
    // @ts-expect-error "sesion" is a misspelling of the canonical "session" key.
    expect(firstDraft.sesion).toBeUndefined();
  });

  it("a named session bound only to agents (no explicit root session field) still emits one top-level session declaration", () => {
    const shared = session("shared-scan", { description: "Shared scan session." });
    const first = agent("first", { description: "First agent.", session: shared });
    const secondAgent = agent.reviewer("second", { session: shared });

    const draft = toDraftJson(trait("session-sharing", {
      summary: "Two agents sharing one session.",
      agent: [first, secondAgent],
    }));

    expect(draft.session).toEqual([{ id: "shared-scan", description: "Shared scan session." }]);
    expect(draft.agent?.map((a) => a["session"])).toEqual(["session:shared-scan", "session:shared-scan"]);
  });

  it("repeated toDraftJson calls on the same session-bearing agent are byte-identical", () => {
    const shared = session("byte-stable", { description: "Byte-stability check." });
    const withSession = agent("byte-stable-agent", { description: "Agent.", session: shared });

    expect(JSON.stringify(toDraftJson(withSession))).toBe(
      JSON.stringify(toDraftJson(withSession)),
    );
  });

  it("session.PerFrame/session.Persistent lower to bare lifecycle scalars, not a declaration", () => {
    const perFrame = agent("frame-scoped", { description: "Per-frame agent.", session: session.PerFrame });
    const persistent = agent("lifetime-scoped", { description: "Persistent agent.", session: session.Persistent });

    expect(toDraftJson(perFrame).session).toBe("per-frame");
    expect(toDraftJson(persistent).session).toBe("persistent");
  });

  it("two agents bound to session.Persistent do not become one shared session", () => {
    const a = agent("agent-a", { description: "A.", session: session.Persistent });
    const b = agent("agent-b", { description: "B.", session: session.Persistent });

    const aDraft = toDraftJson(a);
    const bDraft = toDraftJson(b);

    // Both lower to the same bare lifecycle scalar, but a bare lifecycle
    // value is always agent-local — there is no declared session for two
    // independently-scoped agents to accidentally collide on.
    expect(aDraft.session).toBe("persistent");
    expect(bDraft.session).toBe("persistent");
  });

  it("session.id rejects a non-slug id", () => {
    expect(() => session("Not A Slug")).toThrow(/slug/);
  });

  it("all five role templates and their deprecated bare aliases accept a session binding", () => {
    const shared = session("shared-role-scan");
    const namespacedHandles = [
      agent.worker("w", { session: shared }),
      agent.reviewer("r", { session: shared }),
      agent.planner("p", { session: shared }),
      agent.oracle("o", { session: shared }),
      agent.searcher("s", { session: shared }),
    ];
    const bareHandles = [
      worker("w", { session: shared }),
      reviewer("r", { session: shared }),
      planner("p", { session: shared }),
      oracle("o", { session: shared }),
      searcher("s", { session: shared }),
    ];

    for (
      const [namespaced, bare] of namespacedHandles.map((handle, index) => [handle, bareHandles[index]!] as const)
    ) {
      expect(toDraftJson(namespaced).session).toBe(
        "session:shared-role-scan",
      );
      expect(toDraftJson(namespaced)).toEqual(toDraftJson(bare));
    }
  });

  it("session is optional and omits the field entirely when unset", () => {
    const bare = agent("no-session", { description: "No session." });
    expect(toDraftJson(bare).session).toBeUndefined();
  });

  it("type-accepts a named handle or a lifecycle constant, and rejects an arbitrary object", () => {
    const shared = session("typed-shared");
    expectTypeOf(shared).toMatchTypeOf<SessionBinding>();
    expectTypeOf(session.PerFrame).toMatchTypeOf<SessionBinding>();
    expectTypeOf(session.Persistent).toMatchTypeOf<SessionBinding>();

    expect(() => {
      // @ts-expect-error an arbitrary object is not a SessionHandle or lifecycle constant.
      agent("bad", { description: "Bad.", session: { id: "not-a-handle" } });
    }).toThrow(/expected typed ref handle or ref string/);
    // @ts-expect-error an arbitrary string is not a recognized lifecycle constant.
    agent("bad-string", { description: "Bad.", session: "some-other-value" });

    const typedHandle: SessionHandle = shared;
    void typedHandle;
  });
});

describe("typed rule/signal/dependency (P459)", () => {
  it("maps camelCase rule fields to canonical kebab-case, always-array predicates, dropping unset fields", () => {
    const fields: RuleFields = {
      id: "rust-only",
      reason: "Rust-specific standards apply.",
      fileGlob: ["**/*.rs"],
      taskKeyword: "refactor",
      excludeFileGlob: "**/*.md",
      weight: 2,
      loadLevel: "summary",
    };

    expect(rule(fields)).toEqual({
      id: "rust-only",
      reason: "Rust-specific standards apply.",
      "file-glob": ["**/*.rs"],
      "task-keyword": ["refactor"],
      "exclude-file-glob": ["**/*.md"],
      weight: 2,
      "load-level": "summary",
    });

    // @ts-expect-error rule() rejects unknown/misspelled fields at tsc, not just at runtime.
    rule({ id: "typo", fileGlobb: "**/*.rs" });
    // @ts-expect-error canonical schema requires rule.id; tsc must reject its absence too.
    rule({ reason: "Missing id." });
    // @ts-expect-error canonical schema requires rule.reason; tsc must reject its absence too.
    rule({ id: "missing-reason" });
    // @ts-expect-error load-level is a closed vocabulary; tsc must reject values outside it.
    rule({ id: "bad-level", reason: "Bad load level.", loadLevel: "verbose" });
  });

  it("requires signal.description at the type level", () => {
    const fields: SignalFields = { id: "approved", description: "The reviewer approved the change." };
    const draft = toDraftJson(
      trait("signal-fixture", {
        version: "0.1.0",
        name: "Signal Fixture",
        procedure: procedure({ description: "No steps.", sequence: [] }),
        signal: [signal(fields)],
      }),
    ) as { readonly signal?: readonly { readonly id: string; readonly description: string; }[]; };

    expect(draft.signal).toEqual([{ id: "approved", description: "The reviewer approved the change." }]);

    // @ts-expect-error canonical schema requires signal.description; tsc must reject its absence too.
    signal({ id: "approved" });
  });

  it("requires dependency.alias/id/version and accepts exactly one source shape", () => {
    const fields: DependencyFields = {
      alias: "shared",
      id: "ctx/shared",
      version: "0.1.0",
      source: { path: "../shared" },
    };
    expect(dependency(fields)).toEqual({
      alias: "shared",
      id: "ctx/shared",
      version: "0.1.0",
      source: { path: "../shared" },
    });

    // @ts-expect-error a bare string is no longer accepted; the alias-only shorthand had zero users.
    dependency("shared");
    // @ts-expect-error dependency() rejects unknown/misspelled fields at tsc.
    dependency({ alias: "shared", id: "ctx/shared", version: "0.1.0", sourcee: { path: "../shared" } });

    const npmFields: DependencyFields = {
      alias: "shared",
      id: "ctx/shared",
      version: "0.1.0",
      source: { package: "@ctx/shared" },
    };
    expect(dependency(npmFields)).toEqual({
      alias: "shared",
      id: "ctx/shared",
      version: "0.1.0",
      source: { package: "@ctx/shared" },
    });
    const npmWithRegistry: DependencyFields = {
      alias: "shared",
      id: "ctx/shared",
      version: "0.1.0",
      source: { package: "@ctx/shared", registry: "npm" },
    };
    expect(npmWithRegistry.source).toEqual({ package: "@ctx/shared", registry: "npm" });

    // @ts-expect-error a Git source cannot also carry npm's package field.
    const mixedSource: DependencyFields["source"] = { git: "https://example.com/shared.git", package: "@ctx/shared" };
    void mixedSource;
  });
});

describe("TraitFields declaration field typing (P482)", () => {
  it("rejects a raw object literal in agent: position", () => {
    // @ts-expect-error a plain object literal is not an AgentHandle; author via agent(...)/worker(...)/etc.
    trait("bad-agent", { agent: [{ id: "not-a-handle" }] });
  });

  it("rejects a wrong-kind handle in port: position", () => {
    const notAPort = slot.text("oops");
    // @ts-expect-error a SlotHandle is not a PortHandle.
    trait("bad-port", { port: [notAPort] });
  });

  it("rejects a misspelled key inside activation", () => {
    // @ts-expect-error "minScore" misspells the canonical "min-score" key.
    trait("bad-activation", { activation: { minScore: 1 } });
  });

  it("rejects a raw JsonValue in relations:", () => {
    // @ts-expect-error relations must be a CanonicalRelations object, not an arbitrary JSON value.
    trait("bad-relations", { relations: "not-an-object" });
  });
});

describe("guards are handles, not JsonObjects (P483)", () => {
  it("rejects a raw object literal in until:/if: guard positions", () => {
    // @ts-expect-error a plain object literal is not a GuardValue; author via condition.*.
    sequence.loop("bad-loop-until", { sequence: "sequence:noop", iterations: 3, until: { slot: "x", equals: 1 } });

    // @ts-expect-error a plain object literal is not a GuardValue; author via condition.*.
    // oxlint-disable-next-line unicorn/no-thenable -- BranchSequenceFields.then is a step ref, not a promise.
    sequence.when("bad-branch-if", { if: { slot: "x", equals: 1 }, then: "sequence:noop" });
  });

  it("rejects a misspelled predicate key passed to condition.not", () => {
    // @ts-expect-error "eqals" misspells the canonical "equals" key.
    condition.not({ slot: "x", eqals: 1 });
  });

  it("accepts every sanctioned GuardValue form: ref strings, a named condition handle, nested arrays, and condition.unsafe", () => {
    const named = condition.all("guard-fixture-named", [condition.signal("signal:review-complete")]);

    sequence.loop("guard-fixture-loop", {
      sequence: "sequence:noop",
      iterations: 3,
      until: "condition:guard-fixture-named",
    });
    // oxlint-disable unicorn/no-thenable -- BranchSequenceFields.then is a step ref, not a promise.
    sequence.when("guard-fixture-branch-signal", { if: "signal:review-complete", then: "sequence:noop" });
    sequence.when("guard-fixture-branch-named", { if: named, then: "sequence:noop" });
    sequence.when("guard-fixture-branch-nested", {
      if: [condition.signal("signal:review-complete"), [condition.unsafe({ empty: "slot:findings" })]],
      then: "sequence:noop",
    });
    // oxlint-enable unicorn/no-thenable
  });

  it("condition.iteration/iterationAtLeast round-trip the closed canonical fields", () => {
    expect(condition.iteration(3)).toEqual({ iteration: 3 });
    expect(condition.iterationAtLeast(3)).toEqual({ "iteration-at-least": 3 });
  });

  it("condition.present emits a bare present leaf, or a present+field leaf when field is given", () => {
    expect(condition.present("port:cost-cap")).toEqual({ present: "port:cost-cap" });
    expect(condition.present("slot:cap-report", { field: "cost-report" })).toEqual({
      present: "slot:cap-report",
      field: "cost-report",
    });
  });

  it("condition.absent lowers unconditionally to not(present(...)), never a separate canonical form", () => {
    expect(condition.absent("port:cost-cap")).toEqual({ not: { present: "port:cost-cap" } });
    expect(condition.absent("slot:cap-report", { field: "cost-report" })).toEqual({
      not: { present: "slot:cap-report", field: "cost-report" },
    });
  });
});

describe("value-type preservation, handle assignability, resource field lockdown (P484)", () => {
  it("narrows schema.enum/oneOf/rating/union declared forms and checks condition.equals against them", () => {
    const severity = schema.enum("value-preservation-severity", ["blocker", "advisory"] as const);
    expectTypeOf(severity).toMatchTypeOf<SchemaHandle<"blocker" | "advisory">>();

    const oneOfDeclared = schema.oneOf("value-preservation-one-of", ["a", "b"] as const);
    expectTypeOf(oneOfDeclared).toMatchTypeOf<SchemaHandle<"a" | "b">>();

    const declaredRating = schema.rating("value-preservation-rating", 1, 5);
    expectTypeOf(declaredRating).toMatchTypeOf<SchemaHandle<number>>();

    const unionHandle = schema.union([schema.text(), schema.number()]);
    expectTypeOf(unionHandle).toMatchTypeOf<SchemaUnionHandle<string | number>>();

    const severitySlot = slot({ id: "value-preservation-severity-slot", schema: severity });
    condition.equals(severitySlot, "blocker");
    // @ts-expect-error "urgent" is not a member of the declared severity vocabulary.
    condition.equals(severitySlot, "urgent");

    const noteSchema = schema.object("value-preservation-note", { severity });
    const noteSlot = slot({ id: "value-preservation-note-slot", schema: noteSchema });
    condition.equals(noteSlot.severity, "advisory");
    // @ts-expect-error "urgent" is not a member of the declared severity vocabulary, through a field ref too.
    condition.equals(noteSlot.severity, "urgent");
  });

  it("narrows verdict/decision/yesNo to their fixed vocabularies", () => {
    const verdictHandle = schema.verdict();
    const decisionHandle = schema.decision();
    const yesNoHandle = schema.yesNo();
    expectTypeOf(verdictHandle).toMatchTypeOf<SchemaHandle<"pass" | "fail" | "needs-changes">>();
    expectTypeOf(decisionHandle).toMatchTypeOf<SchemaHandle<"approved" | "revise">>();
    expectTypeOf(yesNoHandle).toMatchTypeOf<SchemaHandle<"yes" | "no">>();

    const verdictSlot = slot({ id: "value-preservation-verdict-slot", schema: verdictHandle });
    condition.equals(verdictSlot, "pass");
    // @ts-expect-error "approved" is a decision member, not a verdict member.
    condition.equals(verdictSlot, "approved");
  });

  it("accepts every non-trait branded handle into toDraftJson with zero casts (P481 mechanism, verified here)", () => {
    const agentHandle = agent("value-preservation-agent", { description: "Agent." });
    const sessionHandle = session("value-preservation-session");
    const conditionHandle = condition.all("value-preservation-condition", [
      condition.signal("signal:value-preservation-review-complete"),
    ]);
    const guardHandle = condition.signal("signal:value-preservation-review-complete");

    expectTypeOf(toDraftJson(agentHandle)).toEqualTypeOf<DraftOf<AgentHandle>>();
    expectTypeOf(toDraftJson(sessionHandle)).toEqualTypeOf<DraftOf<SessionHandle>>();
    expectTypeOf(toDraftJson(conditionHandle)).toEqualTypeOf<DraftOf<ConditionHandle>>();
    expectTypeOf(toDraftJson(guardHandle)).toEqualTypeOf<DraftOf<GuardHandle>>();

    expect(toDraftJson(agentHandle)).toMatchObject({ id: "value-preservation-agent" });
    expect(toDraftJson(sessionHandle)).toMatchObject({ id: "value-preservation-session" });
  });

  it("closes ResourceFields to the canonical resource key set, still accepting trait-spec's hint/trigger usage", () => {
    const fields: ResourceFields = {
      id: "value-preservation-resource",
      content: "Body.",
      hint: "Advisory hint.",
      trigger: "on-activation",
    };
    expect(toDraftJson(resource(fields))).toMatchObject({ id: "value-preservation-resource" });

    // @ts-expect-error "hnit" misspells the canonical "hint" key; ResourceFields no longer has an index signature to absorb it.
    resource({ id: "value-preservation-typo", hnit: "Advisory hint." });
  });
});

describe("markdown doc-resource helpers (P459)", () => {
  it("steps renders a numbered list under a level-two heading", () => {
    const draft = toDraftJson(
      trait("steps-fixture", {
        version: "0.1.0",
        name: "Steps Fixture",
        procedure: procedure({
          description: "No steps.",
          sequence: [sequence.command({
            id: "commit",
            cmd: "git commit",
            include: steps({
              id: "validation-steps",
              items: ["Run the build", "Run the tests"],
            }),
          })],
        }),
      }),
    ) as { readonly resource?: readonly { readonly id: string; readonly content: string; }[]; };

    expect(draft.resource?.find((entry) => entry.id === "validation-steps")?.content).toBe(
      "## Steps\n\n1. Run the build\n2. Run the tests\n",
    );
  });

  it("table escapes pipes/newlines and callout quotes every body line", () => {
    const draft = toDraftJson(
      trait("table-callout-fixture", {
        version: "0.1.0",
        name: "Table Callout Fixture",
        procedure: procedure({
          description: "No steps.",
          sequence: [sequence.command({
            id: "commit",
            cmd: "git commit",
            include: [
              table({ id: "gates", headers: ["Gate", "Command"], rows: [["build", "pnpm build | ok\nnext"]] }),
              callout({ id: "note", kind: "warning", body: "Line one\n\nLine two" }),
            ],
          })],
        }),
      }),
    ) as { readonly resource?: readonly { readonly id: string; readonly content: string; }[]; };

    expect(draft.resource?.find((entry) => entry.id === "gates")?.content).toBe(
      "| Gate | Command |\n| --- | --- |\n| build | pnpm build \\| ok<br>next |\n",
    );
    expect(draft.resource?.find((entry) => entry.id === "note")?.content).toBe(
      "> [!WARNING]\n> Line one\n> \n> Line two\n",
    );
  });

  it("snippet widens its fence past any run of backticks already in the code", () => {
    const handle = snippet({ id: "example", lang: "ts", code: "```nested```" });
    expect(toDraftJson(handle)).toMatchObject({
      content: "````ts\n```nested```\n````\n",
    });
  });
});

describe("vocabulary casing", () => {
  it("exposes only qualified PascalCase built-in leaves", () => {
    expect(tone.Direct).toBe("direct");
    expect(intent.avoid.ScopeCreep).toBe("scope-creep");
    expect((tone as unknown as Record<string, unknown>).direct).toBeUndefined();
    expect((tone as unknown as Record<string, unknown>).DIRECT).toBeUndefined();
    expect((intent as unknown as Record<string, unknown>).ScopeCreep).toBeUndefined();
  });
});
