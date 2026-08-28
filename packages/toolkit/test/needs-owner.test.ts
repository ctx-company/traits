import { input, procedure, schema, step, toDraftJson, trait } from "@ctx-traits/cdk";
import type { DeclaredSignalWithFields, SchemaFieldRecord } from "@ctx-traits/cdk";
import { needsOwnerSignal } from "@ctx-traits/toolkit";
import type { NeedsOwnerValue } from "@ctx-traits/toolkit";
import { describe, expect, it } from "vitest";

type BuiltDraft = {
  readonly procedure: {
    readonly sequence: readonly { readonly id: string; readonly kind: string; readonly when?: unknown }[];
  };
  readonly prompt?: Record<string, { readonly text?: string }>;
  readonly signal?: readonly { readonly id: string; readonly schema?: string }[];
  readonly schema?: readonly {
    readonly id: string;
    readonly fields: Record<string, SchemaFieldRecord>;
  }[];
};

describe("needsOwnerSignal (0253.5)", () => {
  it("wiring the builtin into a step.ask lowers a needs-owner signal guard, payload-field prompt reads, and its schema declaration", () => {
    const proc = procedure.from({ description: "d" }, () => {
      step.ask("Ask Owner", {
        when: needsOwnerSignal,
        input: input.prompt`Reason: ${needsOwnerSignal.reason} Question: ${needsOwnerSignal.question}`,
        output: schema.text(),
      });
    });
    const built = toDraftJson(
      trait("needs-owner-wired-fixture", { name: "Needs Owner Wired", summary: "s", procedure: proc }),
    ) as BuiltDraft;

    const item = built.procedure.sequence.find((entry) => entry.id === "ask-owner");
    expect(item).toMatchObject({ kind: "ask", when: "signal:needs-owner" });
    expect(built.prompt?.["ask-owner"]?.text).toBe(
      "Reason: {signal:needs-owner.reason} Question: {signal:needs-owner.question}",
    );

    const signalDecl = built.signal?.find((entry) => entry.id === "needs-owner");
    expect(signalDecl).toBeDefined();
    expect(signalDecl?.schema).toBe("schema:needs-owner-payload");
    const schemaDecl = built.schema?.find((entry) => entry.id === "needs-owner-payload");
    expect(schemaDecl).toBeDefined();
    expect(Object.keys(schemaDecl?.fields ?? {}).sort()).toEqual(["question", "reason"]);
    for (const fieldId of ["reason", "question"] as const) {
      const field = schemaDecl?.fields[fieldId];
      expect(field?.schema).toBe("schema:text");
      expect(field?.required).toBe(true);
      expect(field?.description).toBeTypeOf("string");
      expect(field?.description?.length).toBeGreaterThan(0);
      expect(field?.hint).toBeUndefined();
    }
  });

  it("a trait that never wires the builtin declares no needs-owner signal and no needs-owner-payload schema", () => {
    const proc = procedure.from({ description: "d" }, () => {
      step.command("Do Something", { cmd: "echo hi" });
    });
    const built = toDraftJson(
      trait("needs-owner-unwired-fixture", { name: "Needs Owner Unwired", summary: "s", procedure: proc }),
    ) as BuiltDraft;

    expect(built.signal).toBeUndefined();
    expect(built.schema).toBeUndefined();
  });

  it("the exported handle and value type are usable at their declared types", () => {
    const value: NeedsOwnerValue = { reason: "no in-run authority to decide", question: "Proceed with option A?" };
    const alias: DeclaredSignalWithFields<NeedsOwnerValue> = needsOwnerSignal;
    expect(value.reason).toBeTypeOf("string");
    expect(alias).toBe(needsOwnerSignal);
  });
});
