import * as cdk from "@ctx-traits/cdk";

import * as step from "./step/index.ts";
import * as resource from "./resource.ts";
import * as data from "./data.ts";

export default function () {
  cdk.defineTrait("Release", {
    version: "0.1.0",
    metadata: { tag: ["first-party", "release", "review", "gated"] },
    summary: "Release Procedure: preflight gates, version bump, changelog, commit & tag, with owner-approved tag push and publish.",
    description: "Release Procedure: preflight gates, version bump, changelog, commit & tag, with owner-approved tag push and publish.",
  });

  cdk.useIntent({
    require: [cdk.intent.ReviewBeforeFinal, cdk.intent.Leanness],
    avoid: [cdk.intent.RubberStampReview, cdk.intent.ScopeCreep],
  });
  cdk.useResource([resource.releaseManifest]);

  step.preflight.status("Check working tree status");
  step.preflight.branch("Capture the current branch");

  cdk.flow.when("Preflight Clean Tree", cdk.condition.empty(step.preflight.gitStatus), () => {
    step.preflight.gate("Run the repository gate chain");
  });

  cdk.flow.when("Preflight Gate Green", cdk.condition.isTrue(step.preflight.gatePassed.ok), () => {
    step.version.captureLastTag("Find the last tag");
    step.version.captureCommitLog("Capture the commit log since the last tag");
    step.version.bump("Bump the declared version files");
    step.version.extractDiff("Capture the version-file diff");
    step.version.review("Review the version bump");
  });

  cdk.flow.when("Version Bump Approved", cdk.condition.equals(step.version.verdict.status, "approved"), () => {
    step.changelog.draft("Draft the changelog entry");
    step.changelog.spotCheck("Spot-check the changelog");
    step.changelog.write("Insert the changelog entry into the changelog file");
  });

  cdk.flow.when("Changelog Approved", cdk.condition.equals(step.changelog.verdict.status, "approved"), () => {
    step.commit.writeMessage("Write the release commit message");
    step.commit.add("Stage all changes");
    step.commit.unstage("Unstage runtime state");
    step.commit.release("Commit the release");
    step.commit.tag("Tag the release (local)");
  });

  cdk.flow.when("Tagged", cdk.condition.notEmpty(data.commitOutput), () => {
    step.release.push("Push the tag (awaiting ctx-gate approval)");
    step.release.publish("Publish the release (awaiting ctx-gate approval)");
  });

  return { commitReport: data.commitReport, publishReport: data.publishReport };
}
