import { reviewVerdictSchema } from "@ctx-traits/agents";
import { port, schema, slot } from "@ctx-traits/cdk";

export const range = port.input.text({
  id: "range",
  description:
    'A locally-resolvable git ref or range (e.g. "main...feature-x", a branch name, or a single ref meaning <merge-base of default>..ref). Preflight resolves and validates it with git rev-parse/git merge-base before anything else runs.',
});

export const rangeResolved = slot.text({
  id: "range-resolved",
  description: "The normalized two-endpoint range actually diffed and logged, after preflight resolution.",
});
export const diffStat = slot.text({
  id: "diff-stat",
  description: "git diff --stat output for the resolved range: file inventory only, never patch bodies.",
  hint: "Index pattern — the reviewer opens actual content with its own tools (git show, git diff <range> -- <file>).",
});
export const commitLog = slot.text({
  id: "commit-log",
  description: "git log --oneline output for the resolved range.",
});

export const reviewVerdict = slot({
  id: "review-verdict",
  schema: reviewVerdictSchema,
  description:
    "Reviewer's typed verdict on the range. No task board governs this run: wall-id is the empty string, remaining/owner-items are absent, and escalation is needs-owner only for authority questions the diff itself raises.",
});
export const reviewDocument = slot.text({
  id: "review-document",
  description: "Rendered human review document for the range, written by the same reviewer step.",
});

// The runtime's own `check`-step contract (P565): a check step's output
// slot must declare `ok` (schema:boolean) plus `argv` (schema:text list).
const treeStatusSchema = schema.object("review-tree-status", {
  ok: schema.field(schema.boolean(), {
    description: "True when the reviewed tree has no uncommitted changes after the review step ran.",
  }),
  argv: schema.field(schema.list(schema.text()), { description: "The exact argv that decided it." }),
});
export const treeStatus = slot({
  id: "tree-status",
  schema: treeStatusSchema,
  description: "Detects (does not prevent) whether the review step mutated the reviewed tree.",
});

export const reviewVerdictReport = port.output.of(reviewVerdictSchema, {
  id: "review-verdict-report",
  description: "The reviewer's typed verdict. Absent when preflight parked on an unresolvable range or an empty diff.",
  optional: true,
  value: reviewVerdict,
});
export const reviewDocumentReport = port.output.text({
  id: "review-document-report",
  description:
    "The rendered human review document. Absent when preflight parked on an unresolvable range or an empty diff.",
  optional: true,
  value: reviewDocument,
});
export const treeStatusReport = port.output.of(treeStatusSchema, {
  id: "tree-status-report",
  description:
    "Whether the reviewed tree came out clean. Absent when preflight parked on an unresolvable range or an empty diff.",
  optional: true,
  value: treeStatus,
});
