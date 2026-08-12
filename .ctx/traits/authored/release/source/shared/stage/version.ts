import { reviewerVerdict } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";
import { input, slot } from "@ctx-traits/cdk";

import { reviewer, worker } from "../agent.ts";
import { commitLog, currentBranch, newVersion, version } from "../data.ts";
import { releaseManifest } from "../resource.ts";

export const lastTag = slot.text({
  id: "last-tag",
  description: "The most recent annotated tag reachable from HEAD, or the sentinel v0.0.0 when the repo has none yet.",
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

// First release: `lastTag` is the v0.0.0 sentinel, not a resolvable ref, so
// `lastTag..HEAD` is fatal — the commit-log capture below falls back to the
// full history, the correct grounding set for a first release.
export function captureLastTag(title: string) {
  return cdk.step.command(title, {
    input: input.command`sh -c "git describe --tags --abbrev=0 2>/dev/null || echo v0.0.0"`,
    output: lastTag,
  });
}
export function captureCommitLog(title: string) {
  return cdk.step.command(title, {
    input: input.command`sh -c 'git log "$1..HEAD" --oneline 2>/dev/null || git log --oneline' _ ${lastTag}`,
    output: commitLog,
  });
}

const versionBumpText = input.prompt`
    Read the release manifest ${releaseManifest} with your tools: the version-file locations to bump, the changelog path, and the release branch.
    The current branch is ${currentBranch}. If it does not match the manifest's declared release branch, edit nothing and say so plainly in your summary.
    ${version} is an explicit override when present; otherwise derive the next version from the commit log since the last tag (${commitLog}, following the last tag ${lastTag}) using the manifest's declared bump rule.
    Edit ONLY the version files the manifest lists — no other files.
    Return the new version string (digits and dots only, no leading "v") and a summary of what changed (files, old version to new version, how the bump was derived).`;

export const bump = cdk.stage({
  agent: worker,
  input: versionBumpText,
  output: [newVersion, versionSummary],
});

export function diff(title: string) {
  return cdk.step.command(title, { argv: ["git", "diff", "--stat"], output: versionDiff });
}

const versionReviewText = input.prompt`
    Review the version bump. Current branch: ${currentBranch}. New version: ${newVersion}. Worker's summary: ${versionSummary}. Scoped diff: ${versionDiff}.
    A blocker is: the run is on a branch other than the manifest's declared release branch, a file the manifest does not list was edited, the new version does not match the manifest's declared bump rule (or the explicit override port when one was given), or any file the manifest lists was missed.
    approved only when the bump is exactly what the manifest and commit log justify, on the correct release branch.`;

// Unapproved review: the enclosing flow.when simply closes over nothing
// further — the run parks without a commit.
export const review = cdk.stage({
  agent: reviewer,
  input: versionReviewText,
  output: versionVerdict,
});
