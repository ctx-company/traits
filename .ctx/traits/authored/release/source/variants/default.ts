// release (default): preflight gates -> version bump -> changelog ->
// commit tail -> gated tag push -> gated publish. Every mutating step past
// the review loops is behind a gate; the irreversible edge (push/publish)
// waits on an explicit owner approval via ctx-gate (0098 pattern). Every
// refusal path (dirty tree, red gate, unapproved review) simply lets the
// enclosing flow.when close over nothing further — the run parks by doing
// nothing more, the same "maybe-commit" idiom every other family uses.
import { condition, defineVariant, flow, input, intent, step, useIntent, useResource } from "@ctx-traits/cdk";
import { reviewer, scribe, worker } from "../agent.ts";
import {
  changelogEntry,
  changelogVerdict,
  changelogWriteOutput,
  commitMessage,
  commitOutput,
  commitReport,
  currentBranch,
  gatePassed,
  gitStatus,
  lastTag,
  commitLog,
  newVersion,
  publishOutput,
  publishReport,
  pushOutput,
  stageOutput,
  tagOutput,
  unstageOutput,
  version,
  versionDiff,
  versionSummary,
  versionVerdict,
} from "../data.ts";
import { releaseManifest } from "../resource.ts";

const versionBumpText = input.prompt`
    Read the release manifest ${releaseManifest} with your tools: the version-file locations to bump, the changelog path, and the release branch.
    The current branch is ${currentBranch}. If it does not match the manifest's declared release branch, edit nothing and say so plainly in your summary.
    ${version} is an explicit override when present; otherwise derive the next version from the commit log since the last tag (${commitLog}, following the last tag ${lastTag}) using the manifest's declared bump rule.
    Edit ONLY the version files the manifest lists — no other files.
    Return the new version string (digits and dots only, no leading "v") and a summary of what changed (files, old version to new version, how the bump was derived).`;

const versionReviewText = input.prompt`
    Review the version bump. Current branch: ${currentBranch}. New version: ${newVersion}. Worker's summary: ${versionSummary}. Scoped diff: ${versionDiff}.
    A blocker is: the run is on a branch other than the manifest's declared release branch, a file the manifest does not list was edited, the new version does not match the manifest's declared bump rule (or the explicit override port when one was given), or any file the manifest lists was missed.
    approved only when the bump is exactly what the manifest and commit log justify, on the correct release branch.`;

const changelogDraftText = input.prompt`
    Draft the changelog entry for version ${newVersion} from the commit log since the last tag: ${commitLog}. Do not consult anything else.
    Every line must trace to a specific commit hash in that log — never invent, embellish, or infer beyond what a commit's own summary states.
    Write the entry in the format the manifest ${releaseManifest} declares for its changelog file, ready to insert at the top of the changelog section for unreleased entries.`;

const changelogReviewText = input.prompt`
    Spot-check the changelog entry ${changelogEntry} against the commit log ${commitLog}. Open a few of the cited commits with "git show" and try to REFUTE each line — default to blocking on any line you cannot verify against an actual commit, rather than assuming it is grounded.
    A blocker is any line not traceable to a listed commit, or any commit in the log with no corresponding line.
    approved only once every line survives the refutation attempt.`;

const changelogWriteText = input.prompt`
    The changelog entry ${changelogEntry} for version ${newVersion} has been approved. Read the release manifest ${releaseManifest} with your tools to find the changelog file path.
    Insert the approved entry at the top of that file's unreleased/new-version section. Edit ONLY that one file — no other files.
    Return a one-line confirmation of the exact file path you edited.`;

const commitMessageText = input.prompt`
    The version-bump and changelog reviews have both approved. Write the release commit message: subject line "release: v${newVersion}", then the changelog entry ${changelogEntry} as the body.
    Return exactly that message. Do not run git commands and do not write files.`;

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
  useResource([releaseManifest]);

  step.command("Check working tree status", {
    argv: ["git", "status", "--porcelain"],
    output: gitStatus,
  });
  step.command("Capture the current branch", {
    argv: ["git", "rev-parse", "--abbrev-ref", "HEAD"],
    output: currentBranch,
  });

  flow.when("Preflight Clean Tree", condition.equals(gitStatus, ""), () => {
    // Fixed argv, mirroring `[merge] gate = [["just", "test"]]`: the
    // trait never composes a gate command from model output.
    step.check("Run the repository gate chain", {
      argv: ["just", "test"],
      output: gatePassed,
      timeoutMs: 600_000,
    });

    flow.when("Preflight Gate Green", condition.fieldEquals(gatePassed, "ok", true), () => {
      step.command("Find the last tag", {
        input: input.command`sh -c "git describe --tags --abbrev=0 2>/dev/null || echo v0.0.0"`,
        output: lastTag,
      });
      step.command("Capture the commit log since the last tag", {
        // First release: `lastTag` is the v0.0.0 sentinel, not a
        // resolvable ref, so `lastTag..HEAD` is fatal — fall back to
        // the full history, the correct grounding set for a first
        // release.
        input: input.command`sh -c 'git log "$1..HEAD" --oneline 2>/dev/null || git log --oneline' _ ${lastTag}`,
        output: commitLog,
      });

      worker.prompt("Bump the declared version files", {
        input: versionBumpText,
        output: [newVersion, versionSummary],
      });
      step.command("Capture the version-file diff", {
        argv: ["git", "diff", "--stat"],
        output: versionDiff,
      });
      reviewer.prompt("Review the version bump", {
        input: versionReviewText,
        output: versionVerdict,
      });

      flow.when("Version Bump Approved", condition.fieldEquals(versionVerdict, "status", "approved"), () => {
        scribe.prompt("Draft the changelog entry", {
          input: changelogDraftText,
          output: changelogEntry,
        });
        reviewer.prompt("Spot-check the changelog", {
          input: changelogReviewText,
          output: changelogVerdict,
        });

        flow.when("Changelog Approved", condition.fieldEquals(changelogVerdict, "status", "approved"), () => {
          worker.prompt("Insert the changelog entry into the changelog file", {
            input: changelogWriteText,
            output: changelogWriteOutput,
          });
          scribe.prompt("Write the release commit message", {
            input: commitMessageText,
            output: commitMessage,
          });
          // Two steps, not one excluding pathspec — a pathspec that
          // mentions a gitignored `.agents` exits 1 (run-42bd7fb2).
          step.command("Stage all changes", { argv: ["git", "add", "-A"], output: stageOutput });
          step.command("Unstage runtime state", {
            argv: ["git", "reset", "-q", "--", ".agents/runs"],
            output: unstageOutput,
          });
          step.command("Commit the release", {
            argv: ["git", "commit", "-m", commitMessage],
            output: commitOutput,
          });
          // Local annotated tag only — still reversible, not gated.
          step.command("Tag the release (local)", {
            input: input.command`sh -c 'git tag -a "v$1" -m "$2"' _ ${newVersion} ${commitMessage}`,
            output: tagOutput,
          });

          flow.when("Tagged", condition.not(condition.equals(commitOutput, "")), () => {
            // Irreversible edge, owner-gated (0098): fixed argv,
            // the trait never composes shell from model output.
            step.command("Push the tag (awaiting ctx-gate approval)", {
              argv: ["ctx-gate", "run", "--", "git", "push", "--follow-tags"],
              output: pushOutput,
              timeoutMs: 28_800_000,
            });
            step.command("Publish the release (awaiting ctx-gate approval)", {
              argv: ["ctx-gate", "run", "--", "just", "release-publish"],
              output: publishOutput,
              timeoutMs: 28_800_000,
            });
          });
        });
      });
    });
  });

  return { commitReport, publishReport };
}
