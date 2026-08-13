import * as cdk from "@ctx-traits/cdk";

export const version = cdk.port.input.text({
  id: "version",
  description:
    'Optional explicit version override (e.g. "1.5.0"). Absent, the worker derives the bump from the commit log since the last tag and the reviewer checks the derivation.',
  optional: true,
});

export const currentBranch = cdk.slot.text({
  id: "current-branch",
  description:
    "The branch HEAD is on, captured before any bump work — checked against the manifest's declared release branch.",
});

export const commitLog = cdk.slot.text({
  id: "commit-log",
  description:
    "One-line log of every commit landed since the last tag — the sole grounding source for the changelog entry.",
});

export const newVersion = cdk.slot.text({
  id: "new-version",
  description:
    'The exact new version string the worker derived (or copied from the explicit override port), e.g. "1.4.0" — digits and dots only, no leading "v".',
  hint: "Pattern-constrained by convention, not schema: reviewed before it ever reaches the tag command's argv.",
});

export const commitOutput = cdk.slot.text({
  id: "commit-output",
  description: "Output evidence from the git commit command step: committed hash and subject.",
});

export const pushEvidence = cdk.slot.text({
  id: "push-output",
  description: "Output evidence from push.",
});

export const publishOutput = cdk.slot.text({
  id: "publish-output",
  description:
    "Output evidence from the ctx-gate-guarded publish command. Absent when the owner denies or the gate times out.",
});

export const commitReport = cdk.port.output.text({
  id: "commit-report",
  description:
    "Commit-and-tag evidence. Absent when preflight parked (dirty tree, red gate) or either review round exhausted unapproved.",
  optional: true,
  value: commitOutput,
});

export const publishReport = cdk.port.output.text({
  id: "publish-report",
  description:
    "Publish evidence from the gated publish command. Absent whenever the run parked earlier, or the owner denied either gate.",
  optional: true,
  value: publishOutput,
});
