import { reviewerVerdict } from "@ctx-traits/agents";
import { port, schema, slot } from "@ctx-traits/cdk";

export const version = port.input.text({
    id: "version",
    description:
        "Optional explicit version override (e.g. \"1.5.0\"). Absent, the worker derives the bump from the commit log since the last tag and the reviewer checks the derivation.",
    optional: true,
});

export const gitStatus = slot.text({
    id: "git-status",
    description: "Working-tree status captured before any work begins: git status --porcelain output verbatim.",
    hint: "Non-empty means the tree is dirty; the run parks before any bump when this is non-empty.",
});
// The runtime's own `check`-step contract, not a bare boolean (P565): a
// check step's output slot must declare `ok` (schema:boolean) plus `argv`
// (schema:text list) — the argv makes the gate self-describing so there is
// no second source of truth for "what proves this done".
export const gatePassed = slot({
    id: "gate-passed",
    schema: schema.object("release-gate-result", {
        ok: schema.field(schema.boolean(), { description: "True when the gate command exited successfully." }),
        argv: schema.field(schema.list(schema.text()), { description: "The exact argv the gate ran." }),
    }),
    description: "Whether the repo-declared preflight gate ([\"just\", \"test\"]) exited successfully, and the exact argv that decided it.",
});
export const currentBranch = slot.text({
    id: "current-branch",
    description: "The branch HEAD is on, captured before any bump work — checked against the manifest's declared release branch.",
});
export const lastTag = slot.text({
    id: "last-tag",
    description: "The most recent annotated tag reachable from HEAD, or the sentinel v0.0.0 when the repo has none yet.",
});
export const commitLog = slot.text({
    id: "commit-log",
    description: "One-line log of every commit landed since the last tag — the sole grounding source for the changelog entry.",
});

// Plain text, not an object slot: an object schema's fields are exposed
// only as condition FieldRefs, not as standalone `input.command` argv
// interpolations — the tag command below needs the version as its own
// complete argv token, so the worker writes it as its own slot.
export const newVersion = slot.text({
    id: "new-version",
    description:
        "The exact new version string the worker derived (or copied from the explicit override port), e.g. \"1.4.0\" — digits and dots only, no leading \"v\".",
    hint: "Pattern-constrained by convention, not schema: reviewed before it ever reaches the tag command's argv.",
});
export const versionSummary = slot.text({
    id: "version-summary",
    description: "What changed: files bumped, old version to new version, and how the bump was derived.",
});
export const versionDiff = slot.text({
    id: "version-diff",
    description: "Scoped diff of the version-file edits, for the reviewer to open with its own tools.",
});
export const versionVerdict = slot({
    id: "version-verdict",
    schema: reviewerVerdict,
    description: "Reviewer's verdict on the version bump.",
});

export const changelogEntry = slot.text({
    id: "changelog-entry",
    description: "The drafted changelog entry, grounded solely in the commit log — every line traceable to a listed commit hash.",
});
export const changelogVerdict = slot({
    id: "changelog-verdict",
    schema: reviewerVerdict,
    description: "Reviewer's refute-mode spot-check verdict on the changelog entry.",
});
export const changelogWriteOutput = slot.text({
    id: "changelog-write-output",
    description: "One-line confirmation from the worker of the changelog file path it edited.",
});

export const stageOutput = slot.text({
    id: "stage-output",
    description: "Verbatim output of the git-add staging command.",
});
export const unstageOutput = slot.text({
    id: "unstage-output",
    description: "Verbatim output of the runtime-state unstage command (git reset -- .agents/runs); empty when nothing was staged there.",
});
export const commitMessage = slot.text({
    id: "commit-message",
    description: "Release commit message; injected directly into the git commit command step.",
});
export const commitOutput = slot.text({
    id: "commit-output",
    description: "Output evidence from the git commit command step: committed hash and subject.",
});
export const tagOutput = slot.text({
    id: "tag-output",
    description: "Output evidence from the local annotated-tag command.",
});
export const pushOutput = slot.text({
    id: "push-output",
    description: "Output evidence from the ctx-gate-guarded tag push. Absent when the owner denies or the gate times out.",
});
export const publishOutput = slot.text({
    id: "publish-output",
    description: "Output evidence from the ctx-gate-guarded publish command. Absent when the owner denies or the gate times out.",
});

export const commitReport = port.output.text({
    id: "commit-report",
    description:
        "Commit-and-tag evidence. Absent when preflight parked (dirty tree, red gate) or either review round exhausted unapproved.",
    optional: true,
    value: commitOutput,
});
export const publishReport = port.output.text({
    id: "publish-report",
    description:
        "Publish evidence from the gated publish command. Absent whenever the run parked earlier, or the owner denied either gate.",
    optional: true,
    value: publishOutput,
});
