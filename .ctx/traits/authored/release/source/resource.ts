import * as cdk from "@ctx-traits/cdk";

export const releaseManifest = cdk.resource({
  id: "release-manifest",
  path: "resources/release.toml",
  hint: "The release contract: the version-file locations to bump, the changelog file path, the release branch name, and the bump rule.",
  trigger: "on-activation",
});
