import { reviewerVerdict } from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";
import { input, slot } from "@ctx-traits/cdk";

import { reviewer, scribe, worker } from "../agent.ts";
import { commitLog, newVersion } from "../data.ts";
import { releaseManifest } from "../resource.ts";

export const changelogEntry = slot.text({
  id: "changelog-entry",
  description:
    "The drafted changelog entry, grounded solely in the commit log — every line traceable to a listed commit hash.",
});
export const changelogVerdict = slot({
  id: "changelog-verdict",
  schema: reviewerVerdict,
  description: "Reviewer's refute-mode spot-check verdict on the changelog entry.",
});
export const changelogWriteOutput = slot.text({
  id: "changelog-write-output",
  description: "One-line confirmation from the worker of the changelog file path it edited.",
});

const changelogDraftText = input.prompt`
    Draft the changelog entry for version ${newVersion} from the commit log since the last tag: ${commitLog}. Do not consult anything else.
    Every line must trace to a specific commit hash in that log — never invent, embellish, or infer beyond what a commit's own summary states.
    Write the entry in the format the manifest ${releaseManifest} declares for its changelog file, ready to insert at the top of the changelog section for unreleased entries.`;

export const draft = cdk.stage({
  agent: scribe,
  input: changelogDraftText,
  output: changelogEntry,
});

const changelogReviewText = input.prompt`
    Spot-check the changelog entry ${changelogEntry} against the commit log ${commitLog}. Open a few of the cited commits with "git show" and try to REFUTE each line — default to blocking on any line you cannot verify against an actual commit, rather than assuming it is grounded.
    A blocker is any line not traceable to a listed commit, or any commit in the log with no corresponding line.
    approved only once every line survives the refutation attempt.`;

// Unapproved spot-check: the enclosing flow.when simply closes over nothing
// further — the run parks without a commit.
export const spotCheck = cdk.stage({
  agent: reviewer,
  input: changelogReviewText,
  output: changelogVerdict,
});

const changelogWriteText = input.prompt`
    The changelog entry ${changelogEntry} for version ${newVersion} has been approved. Read the release manifest ${releaseManifest} with your tools to find the changelog file path.
    Insert the approved entry at the top of that file's unreleased/new-version section. Edit ONLY that one file — no other files.
    Return a one-line confirmation of the exact file path you edited.`;

export const write = cdk.stage({
  agent: worker,
  input: changelogWriteText,
  output: changelogWriteOutput,
});
