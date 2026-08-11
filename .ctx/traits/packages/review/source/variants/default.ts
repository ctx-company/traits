// review (default): preflight resolves the range -> deterministic evidence
// capture (stat inventory + commit log, never patch bodies) -> single
// reviewer pass -> read-only assertion. No refinement loop, no worker, no
// commit tail: there is nothing to fix here, only to judge. An unresolvable
// range or an empty diff simply lets the enclosing flow.when close over
// nothing further — the run parks by doing nothing more, the same
// maybe-commit idiom every other family uses.
import { CODE_INTEGRITY_DOCTRINE } from "@ctx-traits/agents";
import { condition, defineVariant, flow, input, intent, step, useIntent } from "@ctx-traits/cdk";
import { reviewer } from "../agent.ts";
import {
    commitLog,
    diffStat,
    range,
    rangeResolved,
    reviewDocument,
    reviewDocumentReport,
    reviewVerdict,
    reviewVerdictReport,
    treeStatus,
    treeStatusReport,
} from "../data.ts";

const reviewText = input.prompt(
    `Review the range {rangeResolved} (from the range/ref you were given: {range}). Commit log: {commitLog}. File-level diff inventory (never the full patch): {diffStat}.
    Open the actual content yourself with your own tools — "git show", "git diff {rangeResolved} -- <file>" — the inventory above is only an index of where the changes landed.
    No task board governs this run: leave "wall-id" the empty string, omit "remaining" and "owner-items", and set "escalation" to "needs-owner" only for authority questions the diff itself raises, never for ordinary code-review blockers.
    Edit nothing — you are read-only. Open files, do not write them.
    ${CODE_INTEGRITY_DOCTRINE}
    Write both the typed verdict and a rendered human review document: what the range does, the blockers you found (if any) and why each is one, and anything advisory.`,
    { range, rangeResolved, commitLog, diffStat },
);

export default function () {
    defineVariant("default", {
        name: "Review (Default)",
        summary: "Points a single reviewer at an arbitrary locally-resolvable git ref/range and returns a typed verdict plus a rendered review document, with zero mutation of the reviewed tree.",
        metadata: { tag: ["first-party", "review", "read-only"] },
        procedure: "Preflight range resolution -> deterministic evidence capture (diff --stat, log --oneline) -> single reviewer pass -> read-only assertion.",
    });
    useIntent({
        require: [intent.require.ReviewBeforeFinal, intent.require.Leanness],
        avoid: [intent.avoid.RubberStampReview, intent.avoid.ScopeCreep],
    });

    step.command("Resolve the range", {
        input: input.command`sh -c 'r="$1"; case "$r" in *...*|*..*) resolved="$r" ;; *) git rev-parse --verify "$r" >/dev/null 2>&1 || exit 0; base=$(git merge-base HEAD "$r" 2>/dev/null) || exit 0; resolved="$base..$r" ;; esac; git rev-list "$resolved" >/dev/null 2>&1 && echo "$resolved"; exit 0' _ ${range}`,
        output: rangeResolved,
    });

    flow.when("Range Resolved", condition.not(condition.equals(rangeResolved, "")), () => {
        step.command("Capture the diff inventory", {
            input: input.command`sh -c 'git diff --stat "$1"' _ ${rangeResolved}`,
            output: diffStat,
        });
        step.command("Capture the commit log", {
            input: input.command`sh -c 'git log --oneline "$1"' _ ${rangeResolved}`,
            output: commitLog,
        });

        flow.when("Diff Non-Empty", condition.not(condition.equals(diffStat, "")), () => {
            reviewer.prompt("Review the range", {
                input: reviewText,
                output: [reviewVerdict, reviewDocument],
            });
            // Detection, not prevention: the harness confinement layer
            // deliberately allows writes inside the run worktree, so a true
            // deny-all-writes mode is runtime work, filed as a leftover
            // rather than built here (ship-pragmatic/park-rigorous).
            step.check("Assert the reviewed tree stayed clean", {
                argv: ["sh", "-c", "test -z \"$(git status --porcelain)\""],
                output: treeStatus,
            });
        });
    });

    return { reviewVerdictReport, reviewDocumentReport, treeStatusReport };
}
