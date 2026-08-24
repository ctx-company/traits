import * as agents from "@ctx-traits/agents";
import { port as cdkPort, schema, slot as cdkSlot } from "@ctx-traits/cdk";

export const targetSchema = schema.object("architect-target", {
  key: schema.field(schema.text(), { description: "The resolved task's immutable board key." }),
  file: schema.field(schema.text(), { description: "Repo-relative path to the one resolved task TOML file." }),
});

export const receiptSchema = schema.object("architect-receipt", {
  key: schema.field(schema.text(), { description: "The rewritten task's immutable board key." }),
  file: schema.field(schema.text(), { description: "Repo-relative path to the rewritten task file." }),
  "status-before": schema.field(schema.text(), { description: "The task status before architect rewrote it." }),
  "status-after": schema.field(schema.text(), { description: "The task status after architect rewrote it." }),
  "relations-correction": schema.optional(
    schema.field(schema.text(), { description: "Code-proven dependency correction, when architect changed relations." }),
  ),
});

const taskCheckSchema = schema.object("task-unchanged", {
  ok: schema.field(schema.boolean(), { description: "True only when the target file exists and still matches its snapshot." }),
  argv: schema.field(schema.list(schema.text()), { description: "The exact argv that verified the target snapshot." }),
});

export const target = cdkSlot({ id: "target", schema: targetSchema, description: "The one unambiguously resolved board task." });
export const targetFile = cdkSlot.text({
  id: "target-file",
  description: "The resolved target.file duplicated as a text slot for command argv interpolation.",
});
export const grounding = cdkSlot.text({
  id: "grounding",
  description: "Read-only evidence about the target, its board context, relevant code, invariants, reuse points, and existing commands.",
});
export const taskSnapshot = cdkSlot.text({
  id: "task-snapshot",
  description: "Content-only cksum captured immediately after resolution and refreshed only after architect rewrites the target.",
});
export const taskCheck = cdkSlot({ id: "task-check", schema: taskCheckSchema, description: "Result of a target content checksum guard." });
export const criticVerdict = cdkSlot({
  id: "critic-verdict",
  schema: agents.reviewVerdictSchema,
  description: "Open-endedness-only critic verdict for the rewritten task.",
});
export const receipt = cdkSlot({ id: "receipt", schema: receiptSchema, description: "Receipt for architect's one in-place rewrite." });

export const task = cdkPort.input.text({
  id: "task",
  description: "One task to architect, named by its key, name, or filename in .internal/tasks/.",
});
export const receipts = cdkPort.output.of("receipts", receiptSchema, {
  description: "The resolved task's in-place rewrite receipt.",
  value: receipt,
});

export const port = { task, receipts };
export const slot = { target, targetFile, grounding, taskSnapshot, taskCheck, criticVerdict, receipt };
