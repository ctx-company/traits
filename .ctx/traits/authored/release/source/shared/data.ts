import { port, slot } from "@ctx-traits/cdk";

export const version = port.input.text({
  id: "version",
  description:
    'Optional explicit version override (e.g. "1.5.0"). Absent, the worker derives the bump from the commit log since the last tag and the reviewer checks the derivation.',
  optional: true,
});

export const currentBranch = slot.text({
  id: "current-branch",
  description:
    "The branch HEAD is on, captured before any bump work — checked against the manifest's declared release branch.",
});
export const commitLog = slot.text({
  id: "commit-log",
  description:
    "One-line log of every commit landed since the last tag — the sole grounding source for the changelog entry.",
});

// Plain text, not an object slot: an object schema's fields are exposed
// only as condition FieldRefs, not as standalone `input.command` argv
// interpolations — the tag command below needs the version as its own
// complete argv token, so the worker writes it as its own slot.
export const newVersion = slot.text({
  id: "new-version",
  description:
    'The exact new version string the worker derived (or copied from the explicit override port), e.g. "1.4.0" — digits and dots only, no leading "v".',
  hint: "Pattern-constrained by convention, not schema: reviewed before it ever reaches the tag command's argv.",
});

export const commitOutput = slot.text({
  id: "commit-output",
  description: "Output evidence from the git commit command step: committed hash and subject.",
});
export const publishOutput = slot.text({
  id: "publish-output",
  description:
    "Output evidence from the ctx-gate-guarded publish command. Absent when the owner denies or the gate times out.",
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
