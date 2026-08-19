import * as agents from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { reviewer, worker } from "../agent.ts";
import { commitLog, currentBranch, newVersion, version } from "../data.ts";
import { releaseManifest } from "../resource.ts";

export const lastTag = cdk.slot.text({
  id: "last-tag",
  description: "The most recent annotated tag reachable from HEAD, or the sentinel v0.0.0 when the repo has none yet.",
});

export const summary = cdk.slot.text({
  id: "version-summary",
  description: "What changed: files bumped, old version to new version, and how the bump was derived.",
});

export const diff = cdk.slot.text({
  id: "version-diff",
  description: "Scoped diff of the version-file edits, for the reviewer to open with its own tools.",
});

export const verdict = cdk.slot({
  id: "version-verdict",
  schema: agents.reviewerVerdict,
  description: "Reviewer's verdict on the version bump.",
});

export function captureLastTag(title: string) {
  return cdk.step.command(title, {
    input: cdk.input.command`sh -c "git describe --tags --abbrev=0 2>/dev/null || echo v0.0.0"`,
    output: lastTag,
  });
}

export function captureCommitLog(title: string) {
  return cdk.step.command(title, {
    input: cdk.input.command`sh -c 'git log "$1..HEAD" --oneline 2>/dev/null || git log --oneline' _ ${lastTag}`,
    output: commitLog,
  });
}

export const bump = cdk.step({
  agent: worker,
  input: cdk.input.prompt`
    Read the release manifest ${releaseManifest} with your tools: the version-file locations to bump, the changelog path, and the release branch.
    The current branch is ${currentBranch}. If it does not match the manifest's declared release branch, edit nothing and say so plainly in your summary.
    If present, the version-file override is ${version}, otherwise derive the next version from the commit log since the last tag (${commitLog}.
  `,
  output: [newVersion, summary],
});

export function extractDiff(title: string) {
  return cdk.step.command(title, { argv: ["git", "diff", "--stat"], output: diff });
}

export const review = cdk.step({
  agent: reviewer,
  input: cdk.input.prompt`
    Review the release's version bump.
    Information:
      - Current branch: ${currentBranch}
      - New version: ${newVersion}
      - Worker's summary: ${summary}
      - Scoped diff: ${diff}
    Blocker Guidelines:
      - the run is on a branch other than the manifest's declared release branch
      - a file the manifest does not list was edited
      - the new version does not match the manifest's declared bump rule
      - the new version does not match the explicit override port when one was given
      - the new version does not match any file the manifest lists was missed
  `,
  output: verdict,
});
