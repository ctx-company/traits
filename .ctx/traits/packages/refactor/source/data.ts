import { port, slot } from "@ctx-traits/cdk";

export const target = port.input.text({
    id: "target",
    description:
        'Module, file path, or entity to refactor; append an optional emphasis after a dash (e.g. "modules/cli/src/app/run_view.rs", "crate::lockfile — typed errors only").',
});

export const survey = slot.text({
    id: "survey",
    description:
        "Numbered refactoring candidates for the target: cluster, why coupled, smell ids, test impact. No interface proposals.",
    hint: "The friction encountered while reading IS the signal. Deferred candidates stay listed here.",
});
export const refactorFrame = slot.text({
    id: "refactor-frame",
    description:
        "The selected candidate set plus problem framing: constraints any new boundary must satisfy, dependencies, an illustrative (non-binding) sketch.",
});
export const agreedDesign = slot.text({
    id: "agreed-design",
    description: "The design, complete enough to implement from: boundaries, types, migration steps, validation plan.",
});
export const workSummary = slot.text({
    id: "work-summary",
    description: "Worker's report of the refactored state; revised in place after each refinement pass.",
    hint: "What changed (files), how behavior was preserved, gate results, open concerns.",
});
export const commitMessage = slot.text({
    id: "commit-message",
    description: "Refactor commit message; injected directly into the git commit command step.",
});
export const stageOutput = slot.text({ id: "stage-output", description: "Output evidence from the git staging command step." });
export const commitOutput = slot.text({
    id: "commit-output",
    description: "Output evidence from the git commit command step: committed hash and subject.",
});
export const gitStatus = slot.text({
    id: "git-status",
    description: "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
    hint: "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and the scribe never runs.",
});

export const commitReport = port.output.text({
    id: "commit-report",
    description:
        "Final commit evidence from the git commit command step. Absent when the clean-tree gate (P397) skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
    optional: true,
    value: commitOutput,
});
