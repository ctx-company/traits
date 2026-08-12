import { resource } from "@ctx-traits/cdk";

/**
 * Consumer-authored release contract: version-file locations, changelog
 * path, and release branch. Agent-read (worker/reviewer/scribe open it with
 * their own tools), never inlined — the trait carries no repo-specific
 * knowledge of where versions live or what the branch is named.
 */
export const releaseManifest = resource({
  id: "release-manifest",
  path: ".internal/release.toml",
  root: "repo",
  hint: "Repo-declared release contract: the version-file locations to bump, the changelog file path, and the release branch name. Agents read it with their own tools; the trait never hardcodes any of it.",
  trigger: "on-activation",
});
