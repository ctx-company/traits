import * as agents from "@ctx-traits/agents";
import * as cdk from "@ctx-traits/cdk";

import { reviewer, scribe, worker } from "../agent.ts";
import { commitLog, newVersion } from "../data.ts";
import { releaseManifest } from "../resource.ts";

export const entry = cdk.slot.text({
  id: "changelog-entry",
  description: "The drafted changelog entry, grounded solely in the commit log — every line traceable to a listed commit hash.",
});

export const verdict = cdk.slot({
  id: "changelog-verdict",
  schema: agents.reviewerVerdict,
  description: "Reviewer's refute-mode spot-check verdict on the changelog entry.",
});

export const writeOutput = cdk.slot.text({
  id: "changelog-write-output",
  description: "One-line confirmation from the worker of the changelog file path it edited.",
});

export const draft = cdk.step({
  agent: scribe,
  input: cdk.input.prompt`
    Draft the changelog entry for version ${newVersion} from the commit log since the last tag: ${commitLog}. Do not consult anything else.
    Every line must trace to a specific commit hash in that log — never invent, embellish, or infer beyond what a commit's own summary states.
    Write the entry in the format the manifest ${releaseManifest} declares for its changelog file, ready to insert at the top of the changelog section for unreleased entries.`,
  output: entry,
});

export const spotCheck = cdk.step({
  agent: reviewer,
  input: cdk.input.prompt`
    Spot-check the changelog entry ${entry} against the commit log ${commitLog}.
    Open a few of the cited commits with "git show" and try to REFUTE each line (default to blocking on what you cannot verify).
    A blocker is any line not traceable to a listed commit, or any commit in the log with no corresponding line.`,
  output: verdict,
});

export const write = cdk.step({
  agent: worker,
  input: cdk.input.prompt`
    The changelog entry ${entry} for version ${newVersion} has been approved.
    Read the release manifest ${releaseManifest} with your tools to find the changelog file path.
    Insert the approved entry at the top of that file's unreleased/new-version section.`,
  output: writeOutput,
});
