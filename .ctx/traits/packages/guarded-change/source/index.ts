import {
    agent,
    condition,
    dependency,
    Intent,
    Method,
    port,
    procedure,
    prompt,
    ref,
    resource,
    schema,
    sequence,
    slot,
    Tone,
    trait,
    Verbosity,
} from "@ctx-traits/cdk";

// --- Agents: roles as data. The host binds each to a model/harness in ctx.toml,
// so the same trait runs across families; the two reviewers are different brains. ---
const clerk = agent("clerk", {
    description:
        "Fast extraction model: distils the change request and target into a scope brief, so no later step re-reads the raw inputs.",
    summary: "Context-extraction role.",
});
const smart1 = agent("smart-1", {
    description:
        "Strong drafting model: turns the scope brief into an implementation draft, and reviews the work in the refinement loop.",
    summary: "Drafting and first reviewer role.",
});
const smart2 = agent("smart-2", {
    description:
        "Strong review model: rewrites the draft to a higher standard, and reviews the work in the refinement loop.",
    summary: "Draft-rewriting and second reviewer role.",
});
const worker = agent("worker", {
    description: "Implements the reviewed draft, runs the project's own gates, and applies reviewer fixes.",
    summary: "Implementation role.",
});
const scribe = agent("scribe", {
    description:
        "Writes the commit message for the approved change; git staging and committing run as command steps.",
    summary: "Commit-message role.",
});

// --- Ports: the typed boundary. ---
const changeRequest = port.input.text({
    id: "change-request",
    description: "The change to make, in plain language — a bug to fix, a feature to add, a refactor to perform.",
});
const target = port.input.text({
    id: "target",
    description: "Where the change applies — the repository path, module, or area the request concerns.",
});

// --- Own resources: the trait's review rubric and definition-of-done, loaded on activation. ---
const reviewRubric = resource({
    id: "review-rubric",
    path: "resources/review-rubric.md",
    hint: "The rubric reviewers score every change against; agents read this file with their own tools.",
    trigger: "on-activation",
});
const doneChecklist = resource({
    id: "done-checklist",
    path: "resources/done-checklist.md",
    hint: "The definition of done the commit message is grounded in; agents read this file with their own tools.",
    trigger: "on-activation",
});

// --- Dependency composition: the shared, versioned standards this trait consumes by reference. ---
const codingStandards = ref.resource({ id: "coding-standards", dependency: "engineering-standards" });
const reviewGuidance = ref.resource({ id: "review-guidance", dependency: "engineering-standards" });

// --- Slots: typed internal state passed between steps. ---
const brief = slot.text({
    id: "scope-brief",
    description:
        "The request and target distilled into concrete scope + acceptance criteria; later steps work from this instead of re-reading the raw inputs.",
    hint: "What the change must accomplish, how we will know it is done, and the files/areas it touches.",
});
const draft = slot.text({
    id: "draft",
    description: "smart-1's implementation draft for the change.",
    hint: "Scope, files to touch, approach, validation plan (including the test that must fail first), risks. A plan, not an implementation.",
});
const reviewedDraft = slot.text({
    id: "reviewed-draft",
    description: "smart-2's full replacement draft — the contract the worker implements.",
});
const workSummary = slot.text({
    id: "work-summary",
    description: "Worker's report of the implemented state and gate results; revised in place after each refinement pass.",
    hint: "What changed (files), the exact gate commands run and their result, and open concerns.",
});
const commitMessage = slot.text({
    id: "commit-message",
    description:
        "Scribe's commit message for the approved change; also saved to .git/CTX_COMMITMSG for the commit command step.",
});
const stageOutput = slot.text({
    id: "stage-output",
    description: "Output evidence from the git staging command step.",
});
const commitOutput = slot.text({
    id: "commit-output",
    description: "Output evidence from the git commit command step: committed hash and subject.",
});

// --- Schema: the typed verdict the loop exits on. `status` uses the stable approved|revise vocabulary. ---
const reviewVerdict = schema.object(
    "review-verdict",
    {
        status: schema.field(schema.decision(), {
            description:
                "approved when no blocking defect remains (advisory notes may still exist); revise when at least one blocking defect remains. Set to approved if and only if blockers is empty.",
        }),
        blockers: schema.field(schema.text(), {
            required: false,
            description:
                "The blocking defects that must be fixed before merge: a correctness bug, a failing gate, a standards violation, or work the request did not ask for. Present when status is revise; empty or absent when approved.",
        }),
        advisory: schema.field(schema.text(), {
            required: false,
            description:
                "Non-blocking notes: naming, structure, taste, optional improvements, follow-up work. Never affects status and never forces another refinement round.",
        }),
    },
    {
        description:
            "Typed review verdict. The loop blocks only on blockers, so taste and cosmetics do not cause churn; status is revise if and only if a blocking defect remains.",
    },
);

const verdict1 = slot({
    id: "review-verdict-1",
    schema: reviewVerdict,
    description: "smart-1's refinement verdict for the current work state.",
});
const verdict2 = slot({
    id: "review-verdict-2",
    schema: reviewVerdict,
    description: "smart-2's refinement verdict for the current work state.",
});

const commitReport = port.output.text({
    id: "commit-report",
    description: "Final commit evidence from the git commit command step.",
    value: commitOutput,
});

export default trait("guarded-change", {
    version: "0.1.0",
    name: "Guarded Change",
    summary:
        "Implement a requested change across two model families, gated by the project's own tests and a typed two-reviewer approval, then commit — with a receipt.",
    metadata: {
        tag: ["first-party", "implementation", "review", "multi-agent"],
    },
    dependency: dependency({
        alias: "engineering-standards",
        id: "engineering-standards",
        version: "0.1.0",
        source: { path: "../engineering-standards" },
    }),
    behavior: {
        tone: [Tone.direct, Tone.technical],
        method: Method.evidenceFirst,
        verbosity: Verbosity.brief,
    },
    intent: {
        require: [Intent.require.reviewBeforeFinal, Intent.require.gatesGreenBeforeCommit, Intent.require.boundedRefinement, Intent.require.roleAttributedOutput],
        avoid: [Intent.avoid.unboundedLoop, Intent.avoid.rubberStampReview, Intent.avoid.weakenTestsToPass, Intent.avoid.tasteOnlyBlocking],
    },
    resource: [reviewRubric, doneChecklist],
    procedure: procedure({
        description:
            "Implement a requested change end to end with cross-model drafting, review, project-gate enforcement, bounded refinement, and a committed summary.",
        input: [changeRequest, target],
        output: commitReport,
        sequence: [
            sequence.prompt("understand", {
                title: "Distil the request (clerk)",
                agent: clerk,
                text: prompt.text`
                    Distil the request "${changeRequest}" for ${target} into a scope brief.
                    State, concretely: what the change must accomplish, its acceptance criteria (how we will know it is done), and the specific files or areas in ${target} it touches. Inspect ${target} with your tools to ground this — do not guess.
                    Every later step works from this brief instead of the raw request, so anything you leave out is lost. Return only the brief.`,
                output: brief, input: [changeRequest, target],
            }),
            sequence.prompt("draft", {
                title: "Draft the change (smart-1)",
                agent: smart1,
                text: prompt.text`
                    Create an implementation draft for the change described in ${brief}.
                    Hold it to the shared standards ${codingStandards} and the review rubric ${reviewRubric}.
                    The draft must cover: scope (exactly what the request asks, nothing more), files to touch, approach, the validation plan (which of the project's own tests/build/lint prove it — including a test that fails before the change and passes after), and risks.
                    Reference files by path; do not inline documents. Do not implement anything yet.`,
                output: draft, input: [brief, codingStandards, reviewRubric],
            }),
            sequence.prompt("review-draft", {
                title: "Rewrite the draft (smart-2)",
                agent: smart2,
                text: prompt.text`
                    Review the draft ${draft} for the change in ${brief} against the standards ${codingStandards} and the review rubric ${reviewRubric}.
                    Return a complete NEW draft of strictly higher quality — the improved draft itself, not review notes.
                    Tighten scope to the request, correct anything that violates the standards, and make the validation plan explicit: name the test that will fail before the change and pass after it.`,
                output: reviewedDraft, input: [brief, draft, codingStandards, reviewRubric],
            }),
            sequence.prompt("implement", {
                title: "Implement (worker)",
                agent: worker,
                text: prompt.text`
                    Implement the change in ${brief} following the agreed draft ${reviewedDraft}.
                    Hold to the shared standards ${codingStandards}: never weaken or delete a test to pass, add no unjustified dependency, keep the diff scoped to the request.
                    Run the project's own gates — its tests, build, and linter — with your tools before reporting; include a test that fails before your change and passes after it.
                    Return a work summary: what changed (files), the exact gate commands you ran and their result, and any open concern.`,
                output: workSummary, input: [brief, reviewedDraft, codingStandards],
            }),
            sequence.loop("refinement-loop", {
                title: "Doubly-reviewed refinement",
                sequence: sequence.linear("refine-work", [
                    sequence.prompt("review-1", {
                        title: "Review work (smart-1)",
                        agent: smart1,
                        text: prompt.text`
                            Review the implemented work for the change in ${brief} against the agreed draft ${reviewedDraft}. Current work summary: ${workSummary}.
                            Consult the standards ${codingStandards} and the review guidance ${reviewGuidance}, score against the rubric ${reviewRubric}, and inspect the actual working tree with your tools — never review the summary alone; re-run the project's gates if the summary does not prove they pass.
                            Classify every finding by severity per ${reviewGuidance}: a BLOCKER makes the change genuinely unmergeable (a correctness bug, a failing gate, a standards violation, or work the request did not ask for); everything else — naming, structure, taste, optional improvements, follow-up — is ADVISORY.
                            Put blocking defects in the blockers field and non-blocking notes in advisory. Set status to revise only when blockers is non-empty; otherwise approved — advisory never blocks. Do not promote taste to a blocker to earn another round. Return the typed verdict.`,
                        output: verdict1, input: [brief, reviewedDraft, workSummary, codingStandards, reviewGuidance, reviewRubric],
                    }),
                    sequence.prompt("review-2", {
                        title: "Review work (smart-2)",
                        agent: smart2,
                        text: prompt.text`
                            Independently review the implemented work for the change in ${brief} against the agreed draft ${reviewedDraft}. Current work summary: ${workSummary}.
                            Consult the standards ${codingStandards} and the review guidance ${reviewGuidance}, score against the rubric ${reviewRubric}, and inspect the actual working tree with your tools — never review the summary alone.
                            Classify every finding by severity per ${reviewGuidance}: a BLOCKER makes the change genuinely unmergeable (a correctness bug, a failing gate, a standards violation, or work the request did not ask for); everything else is ADVISORY.
                            Put blocking defects in the blockers field and non-blocking notes in advisory. Set status to revise only when blockers is non-empty; otherwise approved — advisory never blocks. Do not promote taste to a blocker to earn another round. Return the typed verdict.`,
                        output: verdict2, input: [brief, reviewedDraft, workSummary, codingStandards, reviewGuidance, reviewRubric],
                    }),
                    sequence.prompt("apply-fixes", {
                        title: "Apply reviewer fixes (worker)",
                        agent: worker,
                        text: prompt.text`
                            Apply the reviewer findings for the change in ${brief}: ${verdict1} and ${verdict2}.
                            Fix every BLOCKER named in either verdict — those are mandatory to merge. Address advisory notes only when the fix is cheap and safe; leaving advisory items for later is acceptable and never a blocker.
                            Where the two reviews conflict, prefer correctness and the standards ${codingStandards}.
                            Re-run the project's gates. Then return the full updated work summary replacing ${workSummary}: what changed (files), the gate commands and their result, open concerns.`,
                        output: workSummary, input: [brief, verdict1, verdict2, workSummary, codingStandards],
                    }),
                ]),
                until: condition.all([
                    condition.fieldEquals(verdict1, "status", "approved"),
                    condition.fieldEquals(verdict2, "status", "approved"),
                ]),
                iterations: 6,
            }),
            sequence.prompt("commit-message", {
                title: "Write the commit message (scribe)",
                agent: scribe,
                text: prompt.text`
                    The refinement loop for the change in ${brief} has ended and the change is being committed. The final reviewer verdicts are ${verdict1} and ${verdict2} — read their status fields FIRST. The loop exits early once both approve, so a verdict still set to revise means the rounds ran out with that reviewer unsatisfied: say so plainly rather than implying a clean approval.
                    Write a concise commit message grounded in the done-checklist ${doneChecklist} and those verdicts: a short subject line naming the change, then a one-paragraph summary of what the verdicts certify changed and how it was validated.
                    If either verdict status is revise, end the message with an "Unresolved reviewer blockers:" section listing each open blocker in one line — the honest record that the budget ran out before the reviewers were satisfied. Never write that section when there are none.
                    Save exactly that message to the file .git/CTX_COMMITMSG at the repo root (create or overwrite it), and return the same message as your output.
                    Do not run any git commands; staging and committing happen in later runtime steps.`,
                output: commitMessage, input: [brief, doneChecklist, verdict1, verdict2],
            }),
            sequence.command({
                id: "git-stage",
                title: "Stage all changes except runtime state",
                argv: ["git", "add", "-A", "--", ":(exclude).agents/runs"],
                output: stageOutput,
            }),
            sequence.command({
                id: "git-commit",
                title: "Commit the change",
                argv: ["git", "commit", "-F", ".git/CTX_COMMITMSG"],
                input: [commitMessage],
                output: commitOutput,
            }),
        ],
    }),
});
