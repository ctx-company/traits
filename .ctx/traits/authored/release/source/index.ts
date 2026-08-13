import * as cdk from "@ctx-traits/cdk";

import * as stage from "./stage/index.ts";
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
    require: [cdk.intent.require.ReviewBeforeFinal, cdk.intent.require.Leanness],
    avoid: [cdk.intent.avoid.RubberStampReview, cdk.intent.avoid.ScopeCreep],
  });
  cdk.useResource([resource.releaseManifest]);

  stage.preflight.status("Check working tree status");
  stage.preflight.branch("Capture the current branch");

  cdk.flow.when("Preflight Clean Tree", cdk.condition.empty(stage.preflight.gitStatus), () => {
    stage.preflight.gate("Run the repository gate chain");
  });

  cdk.flow.when("Preflight Gate Green", cdk.condition.isTrue(stage.preflight.gatePassed.ok), () => {
    stage.version.captureLastTag("Find the last tag");
    stage.version.captureCommitLog("Capture the commit log since the last tag");
    stage.version.bump("Bump the declared version files");
    stage.version.extractDiff("Capture the version-file diff");
    stage.version.review("Review the version bump");
  });

  cdk.flow.when("Version Bump Approved", cdk.condition.equals(stage.version.verdict.status, "approved"), () => {
    stage.changelog.draft("Draft the changelog entry");
    stage.changelog.spotCheck("Spot-check the changelog");
    stage.changelog.write("Insert the changelog entry into the changelog file");
  });

  cdk.flow.when("Changelog Approved", cdk.condition.equals(stage.changelog.verdict.status, "approved"), () => {
    stage.commit.writeMessage("Write the release commit message");
    stage.commit.add("Stage all changes");
    stage.commit.unstage("Unstage runtime state");
    stage.commit.release("Commit the release");
    stage.commit.tag("Tag the release (local)");
  });

  cdk.flow.when("Tagged", cdk.condition.notEmpty(data.commitOutput), () => {
    stage.release.push("Push the tag (awaiting ctx-gate approval)");
    stage.release.publish("Publish the release (awaiting ctx-gate approval)");
  });

  return { commitReport: data.commitReport, publishReport: data.publishReport };
}
