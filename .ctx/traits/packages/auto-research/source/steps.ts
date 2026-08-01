import { CODE_INTEGRITY_DOCTRINE, QUICK_VARIANT_DOCTRINE } from "@ctx-traits/agents";
import { condition, operation, prompt, sequence } from "@ctx-traits/cdk";
import type { AutoResearchAgents } from "./agent.ts";
import type { AutoResearchVariantFlags } from "./config.ts";
import type { AutoResearchSlots } from "./slots.ts";

// Retired extraction (P516, 2026-07-26): this inline `--eval` body stays
// inline. It is part of the canonical trait document, so it is already
// covered by the trait's canonical digest and machine trust approval —
// arguably more tightly bound than a file resource, which needs a separate
// pin plus spawn-time verification to reach the same guarantee. Extraction
// is a style question, not a security fix: it would move the canonical
// digest of both live `auto-research`/`benchmark-refactor` variants
// (`gitRefCapture` is shared by both, see `steps.ts:82,149`) and force human
// trust re-approval on both, right after the 2026-07-26 approval-ping-pong
// incident (P534). Four sibling inline `--eval` scripts on the
// `benchmarkRefactor` path share this shape; extracting one without the
// others would leave the package half-converted. The decision belongs to
// the Group 128 fold, which is already moving these digests.
const gitRefCapture = `
import { spawnSync } from "node:child_process";
const result = spawnSync("git", ["rev-parse", "--verify", "HEAD^{commit}"], {
    cwd: process.cwd(),
    encoding: "utf8",
});
const sha = (result.stdout ?? "").trim();
if (result.status !== 0 || !/^(?:[0-9a-f]{40}|[0-9a-f]{64})$/.test(sha)) {
    process.stderr.write(result.stderr ?? "could not capture HEAD commit");
    process.exit(1);
}
process.stdout.write(JSON.stringify({ sha }));
`.trim();

export function buildSteps(
    agents: AutoResearchAgents,
    slots: AutoResearchSlots,
    { benchmarkRefactor }: AutoResearchVariantFlags,
) {
    const { worker, proposer, summarizer, brSmart1, brWorker } = agents;
    const {
        objective,
        experimentCommand,
        metricField,
        maxExperiments,
        target,
        benchmarkCommand,
        improvementTarget,
        noiseThreshold,
        maxRounds,
        timeLimitSeconds,
        maxWallTime,
        maxTokenEstimate,
        maxCost,
        maxDelta,
        summaryReplay,
        readinessSlot,
        bestRef,
        capturedRef,
        commandReceipt,
        baselineResult,
        candidateResult,
        bestMetric,
        history,
        experimentCount,
        keptCount,
        decisions,
        proposal,
        applyReceipt,
        summary,
        scopeSlot,
        draftSlot,
        implementReceipt,
        reviewVerdict,
        marginResult,
        roundComplete,
        reviews,
    } = slots;

    // The single port every variant's shared worktree-preflight/abort text
    // names: `target` for benchmark-refactor, `objective` otherwise.
    const focusPort = benchmarkRefactor ? target! : objective!;
    const setupWorker = benchmarkRefactor ? brWorker! : worker;
    const setupStep = sequence.prompt("setup", {
        title: "Prepare and verify the isolated workbench",
        agent: setupWorker,
        text: prompt.text`
            Prepare the workbench for ${focusPort}. Confirm this execution is inside an isolated Git worktree and inspect the current tracked state.
            Return status ready only when the tracked workbench is clean and destructive reset is confined to that worktree. Otherwise return abort with the concrete reason.
            Do not modify files yet and do not invent metric evidence.`,
        output: readinessSlot,
    });
    const captureInitialRef = sequence.command({
        id: "capture-initial-ref",
        title: "Capture the immutable baseline commit",
        argv: ["node", "--input-type=module", "--eval", gitRefCapture],
        output: capturedRef,
    });
    const captureBestRef = sequence.project("capture-best-ref", {
        title: "Capture the fixed reset ref",
        projections: [{ source: capturedRef, field: "sha", destination: bestRef }],
    });
    const measureBaseline = sequence.command({
        id: "measure-baseline",
        title: "Measure the baseline",
        argvFrom: benchmarkRefactor ? benchmarkCommand! : experimentCommand!,
        output: baselineResult,
    });
    const seedBest = sequence.project("seed-best", {
        title: "Seed trusted best state and history",
        projections: [
            { source: baselineResult, field: "metric", destination: bestMetric },
            { source: baselineResult, destination: operation.over(history, operation.Append) },
            { source: operation.literal(0), destination: experimentCount },
            { source: operation.literal(0), destination: keptCount },
            { source: operation.literal("baseline"), destination: operation.over(decisions, operation.Append) },
        ],
    });
    const proposeStep = benchmarkRefactor
        ? undefined
        : sequence.prompt("propose", {
            title: "Propose one fresh bounded experiment",
            agent: proposer,
            text: prompt.text`
            Propose one small, falsifiable experiment for ${objective!}.
            The lower-is-better metric is ${metricField!}; current best is ${bestMetric}; trusted prior measurements are ${history}.
            Use only this typed handoff. Do not infer hidden conversation, claim a result, or decide keep/discard.
            Return a title, hypothesis, and exact bounded change.`,
            output: proposal,
        });
    const applyStep = benchmarkRefactor
        ? undefined
        : sequence.prompt("apply", {
            title: "Apply the proposed candidate",
            agent: worker,
            text: prompt.text`
            Apply exactly the bounded change in ${proposal} for ${objective!} inside the isolated worktree.
            Inspect the current files first, preserve unrelated work, and do not run or report the experiment metric. The runtime executes the trusted measurement command next.
            Return a concise receipt naming the files changed.`,
            output: applyReceipt,
        });
    const measureCandidate = sequence.command({
        id: "measure-candidate",
        title: "Measure the candidate",
        argvFrom: benchmarkRefactor ? benchmarkCommand! : experimentCommand!,
        output: candidateResult,
    });
    const stageCandidate = sequence.command({
        id: "stage-candidate",
        title: "Stage the kept candidate",
        argv: ["git", "add", "-A"],
        output: commandReceipt,
    });
    const commitCandidate = sequence.command({
        id: "commit-candidate",
        title: "Commit the kept candidate",
        argv: ["git", "commit", "-m", "auto-research: keep improved experiment"],
        output: commandReceipt,
    });
    const captureCommittedRef = sequence.command({
        id: "capture-committed-ref",
        title: "Capture the kept commit SHA",
        argv: ["node", "--input-type=module", "--eval", gitRefCapture],
        output: capturedRef,
    });
    const advanceBest = sequence.project("advance-best", {
        title: "Advance trusted best state after commit",
        projections: [
            { source: candidateResult, field: "metric", destination: bestMetric },
            { source: capturedRef, field: "sha", destination: bestRef },
        ],
    });
    const resetCandidate = sequence.command({
        id: "reset-candidate",
        title: "Discard the non-improving candidate",
        argv: ["git", "reset", "--hard", bestRef],
        output: commandReceipt,
    });
    const cleanCandidate = sequence.command({
        id: "clean-candidate",
        title: "Remove untracked candidate files",
        argv: ["git", "clean", "-fd"],
        output: commandReceipt,
    });
    const recordKeptCandidate = sequence.project("record-kept-candidate", {
        title: "Record the kept candidate",
        projections: [
            { source: operation.literal(1), destination: operation.over(experimentCount, operation.Increment) },
            { source: operation.literal(1), destination: operation.over(keptCount, operation.Increment) },
            { source: candidateResult, destination: operation.over(history, operation.Append) },
            { source: operation.literal("kept"), destination: operation.over(decisions, operation.Append) },
        ],
    });
    const recordDiscardedCandidate = sequence.project("record-discarded-candidate", {
        title: "Record the discarded candidate",
        projections: [
            { source: operation.literal(1), destination: operation.over(experimentCount, operation.Increment) },
            { source: candidateResult, destination: operation.over(history, operation.Append) },
            { source: operation.literal("discarded"), destination: operation.over(decisions, operation.Append) },
        ],
    });

    // Each optional cap contributes one composed arm: absent -> passes
    // automatically; present -> the candidate must also report the capped
    // field, and clear it. Supplied-but-unmeasurable therefore discards
    // (the inner `present` on candidateResult.field is Unmeasurable, so the
    // `all` arm cannot match and the `absent` arm is not-matched either).
    const capGuard = (cap: NonNullable<typeof maxWallTime>, field: string) =>
        condition.any([
            condition.absent(cap),
            condition.all([
                condition.present(cap),
                condition.present(candidateResult, { field }),
                condition.fieldLte(candidateResult, field, cap),
            ]),
        ]);
    const keepConditions = benchmarkRefactor
        ? [
            condition.fieldEquals(candidateResult, "status", "ok"),
            condition.fieldLt(candidateResult, "metric", bestMetric),
        ]
        : [
            condition.fieldEquals(candidateResult, "status", "ok"),
            condition.fieldLt(candidateResult, "metric", bestMetric),
            capGuard(maxWallTime!, "wall-time-ms"),
            capGuard(maxTokenEstimate!, "token-estimate"),
            capGuard(maxCost!, "cost-microusd"),
            capGuard(maxDelta!, "delta-lines"),
        ];
    const keepCandidate = sequence.when("decide-candidate", {
        title: "Apply the declared keep/discard decision",
        if: condition.all(keepConditions),
        then: sequence.linear("keep-candidate", [
            stageCandidate,
            commitCandidate,
            captureCommittedRef,
            advanceBest,
            recordKeptCandidate,
        ]),
        otherwise: sequence.linear("discard-candidate", [
            resetCandidate,
            cleanCandidate,
            recordDiscardedCandidate,
        ]),
    });
    const experimentLoop = benchmarkRefactor
        ? undefined
        : sequence.loop("experiments", {
            title: "Run the declared experiment budget",
            sequence: sequence.linear("experiment-round", [
                proposeStep!,
                applyStep!,
                measureCandidate,
                keepCandidate,
            ]),
            iterations: maxExperiments!,
            onExhausted: "continue",
        });
    const summarizeStep = benchmarkRefactor
        ? undefined
        : sequence.command({
            id: "derive-summary",
            title: "Derive the typed experiment summary",
            argv: [
                "node",
                summaryReplay,
                experimentCount,
                keptCount,
                bestMetric,
                history,
                decisions,
            ],
            output: summary,
        });
    const abortSummary = sequence.prompt("abort-summary", {
        title: "Report the preflight abort",
        agent: summarizer,
        text: prompt.text`
            Report an aborted auto-research run for ${focusPort} from ${readinessSlot}.
            Return status aborted, experiments 0, kept 0, no best, empty rows, and the exact readiness reason.`,
        output: summary,
    });
    const baselineFailureSummary = sequence.prompt("baseline-failure-summary", {
        title: "Report the unusable baseline",
        agent: summarizer,
        text: prompt.text`
            Report an aborted auto-research run for ${focusPort}. The trusted baseline command returned ${baselineResult}, whose status was not ok.
            Return status aborted, experiments 0, kept 0, no best, one baseline row containing the unchanged measurement with decision baseline, and the concrete measurement failure.`,
        output: summary,
    });

    // benchmark-refactor's own terminal-abort shape: neither preflight nor
    // baseline gating has a "rounds"/"kept"/"reviews" story yet (seed-best/
    // seed-reviews have not run), so these are typed
    // deterministic command steps — never an agent-authored summary — that
    // fill every field the benchmark-refactor-summary schema requires:
    // status "aborted", reason "aborted", zero rounds/kept, and empty
    // rows/reviews (baseline-failure keeps the one failing measurement as an
    // honest baseline row).
    const abortSummaryBr = benchmarkRefactor
        ? sequence.command({
            id: "abort-summary",
            title: "Report the preflight abort",
            argv: [
                "node",
                "--input-type=module",
                "--eval",
                `
const [readinessText] = process.argv.slice(1);
const readiness = JSON.parse(readinessText);
process.stdout.write(JSON.stringify({
    status: "aborted",
    reason: "aborted",
    rounds: 0,
    kept: 0,
    rows: [],
    reviews: [],
    detail: readiness.detail,
}));
                `.trim(),
                readinessSlot,
            ],
            output: summary,
        })
        : undefined;
    const baselineFailureSummaryBr = benchmarkRefactor
        ? sequence.command({
            id: "baseline-failure-summary",
            title: "Report the unusable baseline",
            argv: [
                "node",
                "--input-type=module",
                "--eval",
                `
const [baselineText] = process.argv.slice(1);
const baseline = JSON.parse(baselineText);
process.stdout.write(JSON.stringify({
    status: "aborted",
    reason: "aborted",
    rounds: 0,
    kept: 0,
    rows: [{ measurement: baseline, decision: "baseline" }],
    reviews: [],
    detail: "trusted baseline command did not return status ok: " + JSON.stringify(baseline),
}));
                `.trim(),
                baselineResult,
            ],
            output: summary,
        })
        : undefined;

    const resetRound = benchmarkRefactor
        ? sequence.project("reset-round", {
            title: "Mark the round open",
            projections: [{ source: operation.literal("open"), destination: roundComplete! }],
        })
        : undefined;
    const scopeStep = benchmarkRefactor
        ? sequence.prompt("scope", {
            title: "Scope one bounded benchmark-improvement attempt (smart-1)",
            agent: brSmart1!,
            text: prompt.text`
            Scope one small, falsifiable attempt to improve the lower-is-better benchmark over ${target!}.
            Current best metric is ${bestMetric}; trusted prior measurements are ${history}.
            Return the exact bounded area to touch and why it should improve the benchmark, staying inside ${target!}.`,
            output: scopeSlot!,
        })
        : undefined;
    const draftStep = benchmarkRefactor
        ? sequence.prompt("draft", {
            title: "Turn the scope into a concrete worker draft (smart-1)",
            agent: brSmart1!,
            text: prompt.text`
            Turn the scope ${scopeSlot!} for ${target!} into a concrete, actionable draft the worker can implement directly: exact files/areas and the precise change. Keep it to one bounded attempt.`,
            output: draftSlot!,
        })
        : undefined;
    const implementStep = benchmarkRefactor
        ? sequence.prompt("implement", {
            title: "Implement the draft (worker)",
            agent: brWorker!,
            text: prompt.text`
            Implement the draft ${draftSlot!} for ${target!} inside the isolated worktree.
            Do not run or report the benchmark; the runtime executes the trusted benchmark command next.
            Return a concise receipt naming the files changed.`,
            output: implementReceipt!,
        })
        : undefined;
    const reviewStep = benchmarkRefactor
        ? sequence.prompt("review", {
            title: "Review the implemented candidate (smart-1)",
            agent: brSmart1!,
            text: prompt.text(
                `Review the implemented candidate for {target} against the draft {draft}. Work summary: {implementReceipt}.
                    A BLOCKER always includes a behavior break, a new smell, or an interface widened to make a caller compile. ${QUICK_VARIANT_DOCTRINE}
                    This is the ONLY review pass for this round — an unapproved candidate is reverted, never repaired.
                    Wherever the authority rule above names REVIEW_VERDICT_DOCTRINE, it means the composed baseline stated immediately below.
                    ${CODE_INTEGRITY_DOCTRINE}`,
                {
                    target: target!,
                    draft: draftSlot!,
                    implementReceipt: implementReceipt!,
                },
            ),
            output: reviewVerdict!,
            input: [target!, draftSlot!, implementReceipt!],
        })
        : undefined;
    const deriveMargin = benchmarkRefactor
        ? sequence.command({
            id: "derive-margin",
            title: "Derive whether the candidate clears the noise threshold",
            argv: [
                "node",
                "--input-type=module",
                "--eval",
                `
const [candidateText, bestText, thresholdText] = process.argv.slice(1);
const candidate = JSON.parse(candidateText);
const best = Number(bestText);
const threshold = Number(thresholdText);
const meets = Boolean(candidate) && candidate.status === "ok"
    && Number.isFinite(candidate.metric) && Number.isFinite(best)
    && Number.isFinite(threshold) && threshold >= 0
    && (best - candidate.metric) > threshold;
process.stdout.write(JSON.stringify(meets));
                `.trim(),
                candidateResult,
                bestMetric,
                noiseThreshold!,
            ],
            output: marginResult!,
        })
        : undefined;
    const recordKeptCandidateBr = benchmarkRefactor
        ? sequence.project("record-kept-candidate", {
            title: "Record the kept candidate",
            projections: [
                { source: operation.literal(1), destination: operation.over(experimentCount, operation.Increment) },
                { source: operation.literal(1), destination: operation.over(keptCount, operation.Increment) },
                { source: candidateResult, destination: operation.over(history, operation.Append) },
                { source: reviewVerdict!, field: "status", destination: operation.over(reviews!, operation.Append) },
                { source: operation.literal("kept"), destination: operation.over(decisions, operation.Append) },
                { source: operation.literal("complete"), destination: roundComplete! },
            ],
        })
        : undefined;
    const recordDiscardedCandidateBr = benchmarkRefactor
        ? sequence.project("record-discarded-candidate", {
            title: "Record the discarded candidate",
            projections: [
                { source: operation.literal(1), destination: operation.over(experimentCount, operation.Increment) },
                { source: candidateResult, destination: operation.over(history, operation.Append) },
                { source: reviewVerdict!, field: "status", destination: operation.over(reviews!, operation.Append) },
                { source: operation.literal("discarded"), destination: operation.over(decisions, operation.Append) },
                { source: operation.literal("complete"), destination: roundComplete! },
            ],
        })
        : undefined;
    const recordReviewRejected = benchmarkRefactor
        ? sequence.project("record-review-rejected", {
            title: "Record the review-rejected round",
            projections: [
                { source: operation.literal(1), destination: operation.over(experimentCount, operation.Increment) },
                { source: reviewVerdict!, field: "status", destination: operation.over(reviews!, operation.Append) },
                { source: operation.literal("review-rejected"), destination: operation.over(decisions, operation.Append) },
                { source: operation.literal("complete"), destination: roundComplete! },
            ],
        })
        : undefined;
    const keepCandidateBr = benchmarkRefactor
        ? sequence.when("decide-candidate", {
            title: "Apply the declared keep/discard decision",
            if: condition.all([
                condition.fieldEquals(candidateResult, "status", "ok"),
                condition.fieldLt(candidateResult, "metric", bestMetric),
                condition.equals(marginResult!, true),
            ]),
            then: sequence.linear("keep-candidate", [
                stageCandidate,
                commitCandidate,
                captureCommittedRef,
                advanceBest,
                recordKeptCandidateBr!,
            ]),
            otherwise: sequence.linear("discard-candidate", [
                resetCandidate,
                cleanCandidate,
                recordDiscardedCandidateBr!,
            ]),
        })
        : undefined;
    const reviewRejectedArm = benchmarkRefactor
        ? sequence.linear("review-rejected", [resetCandidate, cleanCandidate, recordReviewRejected!])
        : undefined;
    const reviewGate = benchmarkRefactor
        ? sequence.when("review-gate", {
            title: "Route on the quick-authority review verdict",
            if: condition.fieldEquals(reviewVerdict!, "status", "approved"),
            then: sequence.linear("approved-candidate", [measureCandidate, deriveMargin!, keepCandidateBr!]),
            otherwise: reviewRejectedArm!,
        })
        : undefined;
    const roundLoop = benchmarkRefactor
        ? sequence.loop("rounds", {
            title: "Run the bounded benchmark-refactor round budget",
            sequence: sequence.linear("round", [
                resetRound!,
                scopeStep!,
                draftStep!,
                implementStep!,
                reviewStep!,
                reviewGate!,
            ]),
            iterations: maxRounds!,
            onExhausted: "continue",
            until: condition.all([
                condition.equals(roundComplete!, "complete"),
                condition.any([
                    condition.lte(bestMetric, improvementTarget!),
                    condition.elapsedAtLeast(timeLimitSeconds!),
                ]),
            ]),
        })
        : undefined;
    // The completion cause is never reconstructed from correlated counters
    // (a soft deadline hit on the very last allowed round must read
    // budget-reached, not round-limit-reached): each reason is a distinct
    // command step whose only difference is the literal `reason` argv it
    // supplies, selected by re-evaluating the same runtime conditions that
    // could have ended the loop, in priority order (target, then elapsed
    // budget, then pure round exhaustion).
    const deriveSummaryScript = `
const [reason, roundsText, keptText, bestText, decisionsText, measurementsText, reviewsText] = process.argv.slice(1);
const rounds = Number(roundsText);
const kept = Number(keptText);
const best = Number(bestText);
const decisions = JSON.parse(decisionsText);
const measurements = JSON.parse(measurementsText);
const reviews = JSON.parse(reviewsText);
if (!["target-reached", "budget-reached", "round-limit-reached"].includes(reason)
    || !Number.isSafeInteger(rounds) || rounds < 0
    || !Number.isSafeInteger(kept) || kept < 0
    || !Number.isFinite(best)
    || !Array.isArray(decisions) || !Array.isArray(measurements) || !Array.isArray(reviews)
    || decisions.length !== rounds + 1
    || decisions[0] !== "baseline"
    || decisions.filter((decision) => decision === "kept").length !== kept) {
    process.exit(1);
}
let measurementIndex = 0;
const rows = decisions.map((decision) => {
    if (decision === "review-rejected") return { decision };
    const measurement = measurements[measurementIndex];
    measurementIndex += 1;
    return { measurement, decision };
});
if (measurementIndex !== measurements.length) process.exit(1);
process.stdout.write(JSON.stringify({
    status: "completed",
    reason,
    rounds,
    kept,
    best,
    rows,
    reviews,
    detail: "Decisions and counts projected from runtime-owned keep/discard/review branches; stop reason preserved from the guard/exhaustion branch that actually terminated the run.",
}));
    `.trim();
    const deriveSummaryStep = (id: string, reason: "target-reached" | "budget-reached" | "round-limit-reached") =>
        sequence.command({
            id,
            title: `Derive the typed benchmark-refactor summary (${reason})`,
            argv: [
                "node",
                "--input-type=module",
                "--eval",
                deriveSummaryScript,
                reason,
                experimentCount,
                keptCount,
                bestMetric,
                decisions,
                history,
                reviews!,
            ],
            output: summary,
        });
    const deriveSummaryTargetReached = benchmarkRefactor
        ? deriveSummaryStep("derive-summary-target-reached", "target-reached")
        : undefined;
    const deriveSummaryBudgetReached = benchmarkRefactor
        ? deriveSummaryStep("derive-summary-budget-reached", "budget-reached")
        : undefined;
    const deriveSummaryRoundLimitReached = benchmarkRefactor
        ? deriveSummaryStep("derive-summary-round-limit-reached", "round-limit-reached")
        : undefined;
    const elapsedReasonGate = benchmarkRefactor
        ? sequence.when("elapsed-reason-gate", {
            title: "Distinguish a soft time-budget stop from pure round exhaustion",
            if: condition.elapsedAtLeast(timeLimitSeconds!),
            then: sequence.linear("budget-reached", [deriveSummaryBudgetReached!]),
            otherwise: sequence.linear("round-limit-reached", [deriveSummaryRoundLimitReached!]),
        })
        : undefined;
    const reasonGate = benchmarkRefactor
        ? sequence.when("reason-gate", {
            title: "Deterministically branch on the actual completion cause",
            if: condition.lte(bestMetric, improvementTarget!),
            then: sequence.linear("target-reached", [deriveSummaryTargetReached!]),
            otherwise: sequence.linear("check-elapsed-reason", [elapsedReasonGate!]),
        })
        : undefined;
    const seedReviews = benchmarkRefactor
        ? sequence.project("seed-reviews", {
            title: "Seed empty review history",
            projections: [{ source: operation.literal([]), destination: reviews! }],
        })
        : undefined;
    const targetAlreadyMetGate = benchmarkRefactor
        ? sequence.when("target-gate", {
            title: "Complete immediately when the baseline already meets the target",
            if: condition.lte(bestMetric, improvementTarget!),
            then: sequence.linear("already-met", [deriveSummaryTargetReached!]),
            otherwise: sequence.linear("run-rounds", [roundLoop!, reasonGate!]),
        })
        : undefined;

    const runAfterBaseline = sequence.when("baseline-gate", {
        title: "Require a usable trusted baseline",
        if: condition.fieldEquals(baselineResult, "status", "ok"),
        then: benchmarkRefactor
            ? sequence.linear("run-rounds-after-baseline", [seedBest, seedReviews!, targetAlreadyMetGate!])
            : sequence.linear("run-experiments", [
                seedBest,
                experimentLoop!,
                summarizeStep!,
            ]),
        otherwise: sequence.linear("abort-baseline", [benchmarkRefactor ? baselineFailureSummaryBr! : baselineFailureSummary]),
    });
    const runWhenReady = sequence.when("readiness-gate", {
        title: "Gate mutation on isolated-worktree readiness",
        if: condition.fieldEquals(readinessSlot, "status", "ready"),
        then: sequence.linear("initialize-and-run", [
            captureInitialRef,
            captureBestRef,
            measureBaseline,
            runAfterBaseline,
        ]),
        otherwise: sequence.linear("abort-preflight", [benchmarkRefactor ? abortSummaryBr! : abortSummary]),
    });

    return [setupStep, runWhenReady];
}
