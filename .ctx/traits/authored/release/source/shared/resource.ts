import { resource } from "@ctx-traits/cdk";

/**
 * The release contract: version-file locations, changelog path, release
 * branch, bump rule. Agent-read (worker/reviewer/scribe open it with their
 * own tools), never inlined. FOR NOW the trait package ships the manifest
 * itself (resources/release.toml, describing this repository) so the
 * resource provably exists; the rigorous end-state moves it back to a
 * consumer-authored repo file (`root: "repo"`, .internal/release.toml) once
 * a second consumer exists — the previous declaration pointed there, but no
 * such file was ever authored and every release run would have parked on
 * activation.
 */
export const releaseManifest = resource({
  id: "release-manifest",
  path: "resources/release.toml",
  hint: "The release contract: the version-file locations to bump, the changelog file path, the release branch name, and the bump rule. Agents read it with their own tools; the trait's steps never hardcode any of it.",
  trigger: "on-activation",
});
