// release (default): preflight gates -> version bump -> changelog ->
// commit tail -> gated tag push -> gated publish. Every mutating step past
// the review loops is behind a gate; the irreversible edge (push/publish)
// waits on an explicit owner approval via ctx-gate (0098 pattern). Every
// refusal path (dirty tree, red gate, unapproved review) simply lets the
// enclosing flow.when close over nothing further — the run parks by doing
// nothing more, the same "maybe-commit" idiom every other family uses.
import { condition, defineVariant, flow, intent, useIntent, useResource } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

export default function () {
  defineVariant("default", {
    name: "Release (Default)",
    summary:
      "Gated release procedure: preflight gates, version bump, commit-grounded changelog, commit and local tag, then owner-approved tag push and publish.",
    metadata: { tag: ["first-party", "release", "review", "gated"] },
    description:
      "Preflight (clean tree, repo-declared gate) -> version bump with review -> commit-grounded changelog with refute-mode spot-check -> commit and local tag -> ctx-gate-approved push and publish.",
  });
  useIntent({
    require: [intent.require.ReviewBeforeFinal, intent.require.Leanness],
    avoid: [intent.avoid.RubberStampReview, intent.avoid.ScopeCreep],
  });
  useResource([shared.resource.releaseManifest]);

  shared.stage.preflight.status("Check working tree status");
  shared.stage.preflight.branch("Capture the current branch");

  flow.when("Preflight Clean Tree", condition.equals(shared.stage.preflight.gitStatus, ""), () => {
    shared.stage.preflight.gate("Run the repository gate chain");

    flow.when("Preflight Gate Green", condition.fieldEquals(shared.stage.preflight.gatePassed, "ok", true), () => {
      shared.stage.version.captureLastTag("Find the last tag");
      shared.stage.version.captureCommitLog("Capture the commit log since the last tag");

      shared.stage.version.bump("Bump the declared version files");
      shared.stage.version.diff("Capture the version-file diff");
      shared.stage.version.review("Review the version bump");

      flow.when(
        "Version Bump Approved",
        condition.fieldEquals(shared.stage.version.versionVerdict, "status", "approved"),
        () => {
          shared.stage.changelog.draft("Draft the changelog entry");
          shared.stage.changelog.spotCheck("Spot-check the changelog");

          flow.when(
            "Changelog Approved",
            condition.fieldEquals(shared.stage.changelog.changelogVerdict, "status", "approved"),
            () => {
              shared.stage.changelog.write("Insert the changelog entry into the changelog file");
              shared.stage.commit.writeMessage("Write the release commit message");
              shared.stage.commit.add("Stage all changes");
              shared.stage.commit.unstage("Unstage runtime state");
              shared.stage.commit.commitRelease("Commit the release");
              shared.stage.commit.tag("Tag the release (local)");

              flow.when("Tagged", condition.not(condition.equals(shared.data.commitOutput, "")), () => {
                shared.stage.push.push("Push the tag (awaiting ctx-gate approval)");
                shared.stage.publish.publish("Publish the release (awaiting ctx-gate approval)");
              });
            },
          );
        },
      );
    });
  });

  return { commitReport: shared.data.commitReport, publishReport: shared.data.publishReport };
}
