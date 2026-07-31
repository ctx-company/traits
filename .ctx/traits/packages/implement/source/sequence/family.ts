// Shared implementation concepts used by the native implement family leaves.
import {
    clerkRole,
    commitTail,
    feasibilityGate,
    feasibilityVerdictSchema,
    guardedProduction,
    LEFTOVER_DOCTRINE,
    leftoverSchema,
    ownerItemSchema,
    reviewerRole,
    REVIEW_VERDICT_DOCTRINE,
    reviewVerdictSchema,
    scribeRole,
    SCOPE_SPLIT_DOCTRINE,
    workerRole,
} from "@ctx-traits/agents";
import type {
    AgentHandle,
    GuardValue,
    GuardedProductionHandle,
    GuardedProductionReviewSeat,
    PromptTemplate,
    ResourceHandle,
    SchemaHandle,
    SchemaObjectField,
    SequenceHandle,
    SignalHandle,
    SlotHandle,
} from "@ctx-traits/cdk";
import { condition, operation, port, prompt, resource, schema, sequence, signal, slot } from "@ctx-traits/cdk";

// P450 S3, repointed at the task board (2026-07-31): the board is the
// owner's status surface (P486) — a run never writes it at all. Progress
// lives in the work summary and the commit message.
export const TASK_WRITE_SCOPE_RIDER =
    `Never edit files under .internal/tasks/ — the task board is owner-maintained; a run records its progress in the work summary and the commit message, never in a task file.`;
// P450 S4 (P427): the model-side half of the one-turn-discipline fix.
export const ONE_TURN_DISCIPLINE =
    `End this turn with the structured output and nothing else — no prose status line after the payload.`;

export function smart1Role(description: string): AgentHandle {
    return reviewerRole("smart-1", description, "Drafting and first-reviewer role.");
}
export function smart2Role(description: string): AgentHandle {
    return reviewerRole("smart-2", description, "Second-reviewer role.");
}
export const worker = workerRole("worker", "Implements the draft and applies reviewer fixes.");
export const scribe = scribeRole("scribe", "Writes the commit message for the completed task from the task contract");
export const clerk = clerkRole(
    "clerk",
    "Fast extraction model: copies the task file out of the task board verbatim, so no later step re-reads the board.",
);

export const task = port.input.text({
    id: "task",
    description:
        'Task to implement, named by its file in .internal/tasks/ — the number ("0044"), the full name ("0044-live-view-pane-polish"), or the filename.',
});

/**
 * Declares the task-board resource: the repo-root directory holding one
 * markdown file per task. Each package instantiates its own declaration via
 * this factory (never a trait `dependency` ref — a dependency-vendored
 * root="repo" resource loses the on-demand audit exemption a package's own
 * direct declaration gets), keeping the declaration itself single-sourced.
 */
export function declareTaskBoard(): ResourceHandle {
    return resource({
        id: "task-board",
        path: ".internal/tasks",
        root: "repo",
        hint: "Repo-root directory for the task board: one markdown file per task, named NNNN-kebab-slug.md; agents read task files with their own tools and never inline them.",
        trigger: "on-demand",
    });
}

export const taskBrief = slot.text({
    id: "task-brief",
    description:
        "The task file's contents, copied exactly as written — the scope contract every later step works from instead of the board.",
    hint: "Verbatim copy of the whole task file: title, status line, body, Watch, and Done when. No paraphrasing.",
});
export const draft = slot.text({
    id: "draft",
    description: "The implementation draft for the task — the contract the produce-first build loop implements.",
    hint: "Scope, files to touch, approach, reuse/abstraction opportunities, validation plan, risks. A plan, not an implementation.",
});
export const workSummary = slot.text({
    id: "work-summary",
    description:
        "Worker's cumulative account of the implemented state, extended each produce round — the worker's cross-round memory.",
    hint: "Cumulative across rounds: your own summary from the previous round arrives as input — extend it, never restart it. Per round, append: what changed (files), how it was validated, open concerns, and per-blocker progress against the verdict's step list (which step, what changed, evidence). Compact rounds older than the attached verdict to a line each so the document stays bounded.",
});
export const leftovers = slot({
    id: "leftovers",
    schema: schema.list(leftoverSchema),
    description:
        "Adjudicated leftovers: legitimate follow-on work the shipped result does not require, surviving the reviewer's two-question test. Replaced by each review step; an empty list is a valid, signed claim that none exist.",
});
/**
 * This round's typed park record (P414): empty when every reviewed verdict
 * this round is approved; one entry — the whole `reviewVerdictSchema`-shaped
 * verdict object, copied unchanged — per reviewed verdict that is revise
 * (default/smart/strict/phase review twice a round and so may record up to
 * two; quick reviews once and records at most one). Written each round by
 * `deriveParkReportStep`'s deterministic `project` steps, never
 * model-authored, so it can never disagree with the verdict(s) it comes from
 * (see the P414 doc comment in `@ctx-traits/agents` for why the list
 * element reuses the verdict schema itself rather than a separate
 * hand-declared shape). The run parks on these entries when the build loop
 * exhausts unapproved — no commit is ever created while any reviewed
 * verdict this round is still revise.
 */
export const parkReport = slot({
    id: "park-report",
    schema: schema.list(reviewVerdictSchema),
    description:
        "This round's typed park record (P414): empty when every verdict reviewed this round is approved; one entry per reviewed verdict that is revise, each copied unchanged. Written each round by deterministic project steps (deriveParkReportStep), never model-authored, so it can never disagree with the verdict(s) it comes from. The run parks on these entries when the build loop exhausts unapproved — no commit is ever created while any reviewed verdict this round is still revise.",
});
export const commitOutput = slot.text({
    id: "commit-output",
    description: "Output evidence from the git commit command step: committed hash and subject.",
});
export const reviewDiff = slot.text({
    id: "review-diff",
    description:
        "Inventory of every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. The reviewer opens whatever it needs with its own tools.",
    hint: "git diff --stat output, excluding runtime state. No patch bodies: a deleted generated artifact would otherwise contribute its entire content.",
});
// P565: a check reports the verdict AND the argv that produced it. A bare
// boolean cost three runs (P535, P552 twice): handed only `false`, the worker
// re-validated with whatever command the surrounding prose named — and when
// the docs and the declared check disagree, every round proves the wrong
// thing. The argv makes the gate self-describing, so there is no second
// source of truth for "what proves this done".
export const repoGatesPassed = slot({
    id: "repo-gates-passed",
    description:
        "This round's repository gate result: whether the repository gate chain passed, and the exact command that decided it.",
    schema: schema.object("repo-gates-result", {
        ok: schema.field(schema.boolean(), {
            description: "True when the gate command exited successfully.",
        }),
        argv: schema.field(schema.list(schema.text()), {
            description:
                "The exact argv the gate ran. This is the command that decides done-ness — re-run THIS, not a command named anywhere else.",
        }),
        "exit-code": schema.field(schema.number(), {
            required: false,
            description:
                "The gate command's exit status. Absent when the command never produced one, which itself means it could not be executed.",
        }),
        "timed-out": schema.field(schema.boolean(), {
            required: false,
            description:
                "Present and true only when the gate was killed by its own timeout rather than exiting. A timed-out gate proves nothing about the work.",
        }),
        tail: schema.field(schema.text(), {
            required: false,
            description:
                "The end of the failed gate's output — stderr when it said anything, otherwise stdout. Present ONLY when ok is false. This is the reason the gate failed: read it before attributing the failure to anything else, and never infer a cause the tail does not state.",
        }),
    }),
});

/**
 * 0047 mechanism 4: a gate the exit guard forced to `ok=false` purely by
 * timing out is a REPO CONDITION no worker round can fix — the ceiling is
 * fixed in the trait, not a defect in the work. Every family variant's
 * build loop stops on this arm the round a gate times out rather than
 * grinding toward a doomed park (measured: run-f60c3ef5's undeclared
 * check-step timeout). `gateTimedOutStopIf` is provably mutually exclusive
 * with every `guardedProduction` `until` — each conjoins `repoGatesPassed.ok
 * == true`, which a timed-out gate (`command_execution_succeeded` forces
 * `ok=false` whenever `timed_out` is true) always falsifies — so no
 * `guard-conflict` diagnostic is reachable.
 */
export const gateTimedOut = signal({
    id: "gate-timed-out",
    description: "The repository gate exceeded its declared ceiling — a repo condition no worker round can fix.",
});
export const gateTimedOutStopIf = condition.fieldEquals(repoGatesPassed, "timed-out", true);

export const commitReport = port.output.text({
    id: "commit-report",
    description:
        "Final commit evidence from the git commit command step. Absent when the clean-tree gate (P397) skipped the commit tail entirely — a clean working tree at gate time means nothing to commit.",
    optional: true,
    value: commitOutput,
});

/**
 * Terminal typed boundary for the adjudicated leftover list (P409): the same
 * `leftovers` slot each build round's review replaces, exposed as an
 * optional structured output port. `slot:leftovers` stays the required
 * reviewer output — an empty list remains an explicit signed ledger claim —
 * while this port only ever surfaces in the run's structured final outputs
 * when the adjudicated list is non-empty (runtime-enforced, not
 * schema-enforced: see `session.rs::final_outputs`'s empty-array omission).
 */
export const leftoversPort = port.output.of(schema.list(leftoverSchema), {
    id: "leftovers",
    title: "Leftovers",
    description:
        "Adjudicated leftovers: legitimate follow-on work the shipped result does not require, surviving the reviewer's two-question test. Omitted from the run's structured final outputs when empty; slot:leftovers always carries the signed evidence either way.",
    optional: true,
    value: leftovers,
    format: ["structured", "table"],
});

/**
 * Terminal typed boundary for the park report (P414): the same `parkReport`
 * slot each review step replaces, exposed as an optional structured output
 * port. Non-empty only when the build loop exhausted unapproved — the run
 * then parks (`onExhausted: "block"` halts before the commit tail ever
 * runs) and this port is the durable, dispatch-preflight-readable evidence
 * of why. Empty (and thus omitted from structured final outputs) on every
 * approved run.
 */
export const parkReportPort = port.output.of(schema.list(reviewVerdictSchema), {
    id: "park-report",
    title: "Park Report",
    description:
        "Typed park record for an unapproved run (P414): one entry per reviewed verdict that was still revise in the final round, each with the wall citation (if any), the exact blockers, and escalation state. Present in the run's persisted output-port evidence only when the build loop exhausted without approval — the run parks and no commit is created. A dispatch-time preflight refuses a sibling task that explicitly cites the same wall-id while this stands unforced.",
    optional: true,
    value: parkReport,
    format: ["structured", "table"],
});

/**
 * 0047 mechanism 1's agent layer: a shared pre-build feasibility triage —
 * possible, blocked, oversized, ambiguous — audited BEFORE any planning or
 * build round spends anything. Complementary to the deterministic
 * blocked-status-marker dispatch preflight (which catches DECLARED
 * blockage for free): this catches UNDECLARED blockage (a referenced
 * artifact that is simply absent) no header ever recorded.
 */
export const feasibility = slot({
    id: "feasibility",
    schema: feasibilityVerdictSchema,
    description:
        "Pre-build feasibility triage verdict (0047): audited once before any planning or build round spends anything. feasible lets the run continue; any other verdict is the park evidence for why it stopped here.",
});
export const taskNotFeasible = signal({
    id: "task-not-feasible",
    description:
        "The pre-build feasibility gate found the task blocked, oversized, or ambiguous before any build work began; the run parks on the typed verdict instead of grinding toward a doomed park.",
});
export const feasibilityPort = port.output.of(feasibilityVerdictSchema, {
    id: "feasibility",
    title: "Feasibility Verdict",
    description:
        "Typed pre-build feasibility triage verdict (0047), for discoverability in the run's structured final outputs.",
    optional: true,
    value: feasibility,
    format: ["structured", "table"],
});
/**
 * Build the feasibility-gate step for one variant's own drafting/reviewer
 * agent — a single shared builder so every adopter gets byte-identical
 * doctrine and guard wiring; only the agent and the contract ref vary.
 */
export function feasibilityStep(agentHandle: AgentHandle, contract: ResourceHandle | SlotHandle = taskBrief): SequenceHandle {
    return feasibilityGate({
        id: "feasibility",
        agent: agentHandle,
        task,
        contract,
        output: feasibility,
        onStop: taskNotFeasible,
    });
}

/**
 * Build a review-verdict schema for one variant: the shared `reviewVerdictSchema`
 * base (status, blockers, advisory, escalation, remaining, owner-items) plus
 * any variant-specific extra typed fields (smart's amendments). default and
 * strict pass no extra fields and reuse the base schema unchanged — a fresh
 * identically-shaped schema is not declared for them.
 */
export function verdictSchemaFor(variantId: string, extraFields: Record<string, SchemaObjectField> = {}): SchemaHandle {
    if (Object.keys(extraFields).length === 0) return reviewVerdictSchema;
    return schema.object(`review-verdict-${variantId}`, schema.extend(reviewVerdictSchema, extraFields));
}

export function verdictSlot(id: string, reviewerLabel: string, schemaHandle: SchemaHandle): SlotHandle {
    return slot({
        id,
        schema: schemaHandle,
        description: `${reviewerLabel}'s refinement verdict for the current work state.`,
    });
}

export function taskExtractionStep(agent: AgentHandle, taskBoard: ResourceHandle) {
    return sequence.prompt("task-extraction", {
        title: "Copy the task contract (clerk)",
        agent,
        text: prompt.text`
            Copy the task file for ${task} EXACTLY as written.
            Open the task-board directory named in ${taskBoard} with your tools. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — list the directory and match it (a bare number matches its NNNN- prefix). Files under archived/ are not live tasks; match one only when the request names it explicitly.
            Return the file's entire contents, byte-for-byte — no paraphrasing, no summaries, no commentary, no added headers. Every later step works from this copy instead of the board, so anything you drop is lost.`,
        output: taskBrief,
        input: [task, taskBoard],
    });
}

export function buildDraftPromptText(): PromptTemplate {
    return prompt.template(
        `Create an implementation draft for {task}.
            Work from the task contract {taskBrief} — a verbatim copy of the task file, your scope contract. Do not re-read the board.
            FIRST verify the contract is implementable: it states a falsifiable Done-when (not a one-line placeholder or a file marked "detail pending"), it is not marked superseded or cancelled, every prerequisite task it names is already landed in this tree, and the scope honestly fits one run. If any of these fail, open the draft with "CONTRACT PROBLEM:" naming exactly what is missing or conflicting — reviewers escalate on that evidence in round 1 instead of round 10 — then still draft whatever subset is honestly implementable.
            ${SCOPE_SPLIT_DOCTRINE}
            After the SCOPE SPLIT, the draft must cover: scope (exactly what the task asks, nothing more), files to touch, approach, validation plan (the repo's standard gates), and risks.
            Favor the leanest correct approach: the minimal, robust, elegant change that satisfies the task — no speculative generality, no gold-plating, no defensive armor for states that cannot occur. Explicitly call out where existing code should be REUSED or a shared abstraction EXTRACTED rather than re-implemented or copied beside. Reference repo files by path; do not inline documents. Do not implement anything.
            ${ONE_TURN_DISCIPLINE}`,
        { task, taskBrief },
    );
}

export function draftStep(agent: AgentHandle) {
    return sequence.prompt("draft-writing", {
        title: "Draft the work (smart-1)",
        agent,
        text: buildDraftPromptText(),
        output: draft,
        input: [task, taskBrief],
    });
}

/**
 * The plan-review seat for the `planning` `guardedProduction` composite
 * (P450 §4.2): a second-reviewer pass over the draft, cheap and
 * non-blocking on exhaustion (`onExhausted: "continue"` on the composite
 * itself) — an unapproved plan still informs the build loop, whose own
 * review is the binding gate.
 */
export function buildPlanReviewText(): PromptTemplate {
    return prompt.template(
        `Review the implementation draft {draft} for {task} against the task contract {taskBrief}.
            A BLOCKER is a draft that: omits an agent-doable checklist item or Done-when of the task, proposes an approach that cannot satisfy the task contract, is missing a required SCOPE SPLIT classification, or scopes in work the task does not ask for. Everything else — style, phrasing, alternate valid approaches — is advisory.
            Return the typed verdict (status: approved when no blocker remains, revise otherwise; blockers: the list that justifies revise).
            ${ONE_TURN_DISCIPLINE}`,
        { draft, task, taskBrief },
    );
}

export const repoGatesStep = sequence.check("repo-gates", {
    title: "Run the repository gate chain",
    // Runs BEFORE the diff capture: cargo build can legitimately rewrite
    // Cargo.lock, and reviewer evidence must reflect the tree as gated, not
    // a pre-gate snapshot the gate step itself invalidates.
    //
    // P569: repointed from the retired `implement-phase-gates` recipe to the
    // repository's single gate. The argv is carried in this step's own output
    // record, so whatever this names is what the worker is told to re-run —
    // there is no second place a gate command can be declared and drift.
    argv: ["just", "test"],
    // 2026-07-31: the full gate (sdk-check, format-check, clippy, workspace
    // cargo test) takes ~10 minutes even on a warm worktree clone. With no
    // declared ceiling the runtime's built-in command default applied and
    // killed it every round — `ok` could then never be true, and the exit
    // guard (`alsoRequire`) parked an otherwise-approved run at exhaustion
    // (run-f60c3ef5, task 0046). Thirty minutes covers a cold first round.
    timeoutMs: 1_800_000,
    output: repoGatesPassed,
});

export const captureDiffStep = sequence.command({
    id: "capture-diff",
    title: "Capture the changed-file inventory",
    // `--stat` ONLY, never `--patch` (changed 2026-07-28). A patch prints the
    // full body of every deleted file, so a round that removes generated
    // artifacts pays for them in full: run-e20bc187's round-2 diff was 471,929
    // bytes / ~125k tokens, of which ~6,700 lines were the `generated/`
    // index.toml and index.map bodies of five folded packages — machine output
    // no reviewer needs to read, drowning 331 lines of actual change. It blew
    // the 256 KiB capture limit, and a truncated slot-feeding capture is a
    // typed failure (run.rs:1901), so the run died holding a completed fold.
    //
    // The inventory is the evidence now: every changed path with its
    // insertion/deletion delta, no exclusions. The reviewer picks what to open
    // with its own tools — it can see that a `generated/` file changed without
    // being handed six thousand lines of it, and it is told plainly below that
    // this is an index, not the change itself.
    argv: ["git", "diff", "--stat", "--", ".", ":(exclude).agents/runs"],
    output: reviewDiff,
});

/**
 * The produce-first merged produce prompt (P450 §4.3): implements the draft
 * when no verdict is attached to this frame; when one is (round 2+), fixes
 * every in-scope BLOCKER, carries adjudicated leftovers forward verbatim,
 * honors `recurrence-of` as proof a prior fix was symptomatic, repairs a red
 * gate from its command evidence, and returns the FULL updated work summary
 * — never a diff against the prior one. Never interpolates the verdict, the
 * leftovers, or the gate result directly (`{...}`): the kit's own
 * `guardedProduction` already attaches them as `input.optional`
 * hidden-until-produced context, and a duplicate literal ref trips
 * `cdk-duplicate-input` and drops the round-1-hidden semantics.
 */
const DEFAULT_PRODUCE_RETURN_CONTRACT =
    `Return the full updated work summary: what changed (files), what was reused or abstracted, how it was validated, substitute evidence per owner-only item, open concerns, a "Leftovers proposed" section, and — once a verdict has ever been attached — a "Blocker dispositions" section listing every blocker id from that verdict with its done-when quoted verbatim, what changed, where, and how it was verified (or the out-of-scope justification); for a blocker with recurrence-of set, open its disposition with one paragraph stating the structural design you implemented.`;

/**
 * `opts.extraFixInstruction` appends a variant-specific sentence to the
 * shared round-1/fix-round paragraph (e.g. smart's amendment-conflict
 * rule) — never a reason to fork the whole prompt. `opts.returnContract`
 * overrides the final return-contract line for a variant whose produce
 * step returns more than `work-summary` (e.g. smart's `draft` +
 * `work-summary`); every other paragraph, including the task-board rider
 * and one-turn discipline, is shared verbatim by every caller.
 */
export function buildProducePrompt(
    opts: { extraFixInstruction?: string; returnContract?: string } = {},
): PromptTemplate {
    const { extraFixInstruction, returnContract } = opts;
    return prompt.template(
        `Produce the work for {task} against the draft {draft}.
            If no reviewer verdict is attached to this frame, this is round 1: implement the draft in full. If a reviewer verdict IS attached, this is a fix round: fix every BLOCKER it names that falls within this task's stated scope and Done-when — those are mandatory to merge; a blocker demanding a gate or deliverable that belongs to a later task is out of scope, note it as a follow-up and do not expand this task to satisfy it; address advisory notes only when the fix is cheap and safe. Treat any blocker with recurrence-of set as proof your prior fix treated a symptom: do the root fix that establishes the required-fix invariant — extending or widening the previously patched check is not an acceptable resolution — even when that is larger than fixing the specific cases named. Where multiple reviewers conflict, prefer correctness and the task contract.${
            extraFixInstruction ? ` ${extraFixInstruction}` : ""
        }
            A leftovers list from this round's most recent review, if any, is attached as context: carry every still-valid entry into your revised "Leftovers proposed" section verbatim — never re-copy from memory. If your fixes invalidate one of those entries' evidence, say so explicitly instead of silently dropping or silently keeping it. Any new leftover your fixes surfaced is proposed separately, in the same section, for the next review round to adjudicate.
            A repository gate result from this round's most recent evidence, if any, is attached as context. When its ok field is false, re-run its argv verbatim — that exact command is what decides done-ness, whatever any document or reviewer calls the gate — and use what it reports Its tail is the gate's own output and states the actual reason — start there rather than guessing. A tail showing the gate could not run at all (missing recipe, command not found) is a repository misconfiguration you cannot fix from inside the run: report it in the work summary instead of attempting a code change. A gate result whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never your blocker to fix; report it and stop, do not spend the round chasing it.
            Respect the task contract {taskBrief}.
            Prefer the minimal, correct, robust implementation: add no validation, abstraction, or defensive code beyond what the task requires. Reuse before you write — when logic duplicates or closely resembles code that already exists, unify it: extract the shared abstraction and call it, never re-implement or copy the logic beside the original. Favor elegance and leanness over cleverness or exhaustive edge-case armor.
            You owe 100% of the draft's agent-doable pile. For each owner-only item in the draft's SCOPE SPLIT, produce the named substitute evidence (run the tests, dry runs, or static checks it names) — an owner-only classification is never license to skip work a shell can do. If you hit a wall the draft did not classify, flag it in your work summary as a proposed owner-item (item, reason class, why no in-run effort suffices, the substitute evidence you produced) and expect reviewers to challenge it.
            ${TASK_WRITE_SCOPE_RIDER}
            Run the gates named in this task's own Done-when before reporting — not whole-project gates that later tasks will satisfy.
            Propose leftovers explicitly: end the work summary with a "Leftovers proposed:" section listing any legitimate follow-on work (what — one sentence —, reason — needs-unlanded or needs-human —, needs, evidence it does not block this result standing alone, done-when), or state explicitly that none exist. Reviewers adjudicate these, not you.
            ${returnContract ?? DEFAULT_PRODUCE_RETURN_CONTRACT}
            ${ONE_TURN_DISCIPLINE}`,
        { task, draft, taskBrief },
    );
}

/**
 * One reviewer's pass over the produced state, returned as a caller-owned
 * `guardedProduction` review seat (agent, prompt text, and its own verdict
 * slot/schema/extra-outputs) — never a whole `sequence.prompt` itself, so
 * the kit owns step construction and id numbering. `variantDoctrine` is a
 * plain-string authority fragment (e.g. `DEFAULT_VARIANT_DOCTRINE`),
 * spliced directly alongside `REVIEW_VERDICT_DOCTRINE` — its own
 * plan-fidelity paragraph is authoritative over that doctrine's default
 * completion criteria. `opts.deviationReport` adds strict's recorded-
 * deviation slot and its verification line; `opts.extraInstruction` adds a
 * variant-specific blocker rule that doesn't belong in the shared doctrine
 * text. Every pass writes two typed outputs each round — the verdict, and
 * `leftovers` (per `LEFTOVER_DOCTRINE`): the first seat's pass writes both
 * fresh; a second seat reads that same-round state (never a prior round's)
 * and independently replaces both.
 */
export function reviewSeat(
    agent: AgentHandle,
    reviewerNumber: 1 | 2,
    variantDoctrine: string,
    verdictSlotHandle: SlotHandle,
    opts: { deviationReport?: SlotHandle; extraInstruction?: string; reviewRubric?: ResourceHandle } = {},
): GuardedProductionReviewSeat {
    const { deviationReport, extraInstruction, reviewRubric } = opts;
    return {
        agent,
        verdictSlot: verdictSlotHandle,
        extraOutputs: [leftovers],
        text: prompt.template(
            `${reviewerNumber === 2 ? "Independently review" : "Review"} the produced work for {task} against the draft {draft}. Current work summary: {workSummary}.
            {reviewDiff} lists every file changed this round with its insertion/deletion counts — an index of where the work landed, never the work itself. Open whatever it points at with your own tools; it omits untracked files. {repoGatesPassed} is this round's repository gate verdict (build, clippy, fmt, CDK typecheck); a false verdict is grounds for a blocker even if the diff otherwise looks correct. When it failed it carries a tail of the gate's own output: that tail IS the reason — cite it, and never attribute the failure to a cause it does not state. If the tail shows the gate could not run at all (a missing recipe, a command not found), that is a misconfigured repository, not a defect in the work, and it is not the worker's blocker to clear: the argv is fixed in the trait and no amount of implementation changes it. A gate whose timed-out field is true is a repo condition — the ceiling is fixed in the trait — never grounds for a blocker on its own; the loop stops on it before another round is spent.${
                deviationReport
                    ? " Recorded deviations: {deviationReport}. Verify every recorded deviation is a genuinely rejected-but-considered proposal — the actual implementation still matches the draft verbatim on that point — not a departure that was actually taken; silent adaptation not recorded here is itself a BLOCKER."
                    : ""
            }${
                reviewRubric
                    ? " Score the work against the review rubric {reviewRubric}: report scope, correctness, and gates-ran, each as a mandatory first finding plus any further findings, every entry citing a concrete repo-relative file and line; a failed dimension is also recorded as a typed BLOCKER, never left as evidence-only."
                    : ""
            }
            ${variantDoctrine}${extraInstruction ? `\n            ${extraInstruction}` : ""}
            ${REVIEW_VERDICT_DOCTRINE}
${
                reviewerNumber === 2
                    ? "            The first reviewer's leftovers this same round: {leftovers}. Independently reapply both questions to every entry: keeping one co-signs it into your own output leftovers list, rejecting one requires a BLOCKER instead — never silently drop or silently keep an entry.\n"
                    : ""
            }            ${LEFTOVER_DOCTRINE}
            ${ONE_TURN_DISCIPLINE}`,
            {
                task,
                draft,
                workSummary,
                taskBrief,
                reviewDiff,
                repoGatesPassed,
                ...(deviationReport ? { deviationReport } : {}),
                ...(reviewRubric ? { reviewRubric } : {}),
                ...(reviewerNumber === 2 ? { leftovers } : {}),
            },
        ),
    };
}

/**
 * Deterministic, runtime-only derivation of this round's typed park-report
 * entries (P414) from each given verdict's own accepted value — never a
 * second, model-authored classification, so a mismatched
 * blocker/wall-id/escalation can never be accepted (park-report simply is
 * not independent model output any more). A `project` step can only copy a
 * whole source value (`replace`/`append`) — the CDK rejects
 * `set-field`/`merge` for `project` (those exist only for a prompt/command
 * step's own `output:` sink) — so this never builds park-report
 * field-by-field; it copies each verdict's own accepted value onto
 * `parkReportSlot` UNCHANGED, in full. Every round: `park-report-clear`
 * replaces `parkReportSlot` with `[]` unconditionally; one
 * `park-report-record` step per given verdict then runs only when that
 * verdict's own status is `revise`, appending its whole value onto the
 * (now-empty, or already-appended-to) list — so runtime approval being a
 * conjunction of every given verdict is reflected in the park report as a
 * conjunction too: a single still-revise verdict among several always
 * survives into the park report, even when a later verdict in the same
 * round is approved. This is why `parkReportSlot`'s declared schema must be
 * `schema.list(<verdict's own schema>)` — the SAME schema as each verdict,
 * not a separately hand-declared shape, so the whole-value append always
 * validates. Callers pass one verdict (quick's single-review procedure) or
 * every verdict reviewed this round (default/smart/strict/phase's dual
 * review) through the same helper shape.
 */
export function deriveParkReportStep(
    verdicts: SlotHandle | SlotHandle[],
    opts: { parkReportSlot?: SlotHandle } = {},
): SequenceHandle[] {
    const targetParkReport = opts.parkReportSlot ?? parkReport;
    const verdictList = Array.isArray(verdicts) ? verdicts : [verdicts];
    return [
        sequence.project("park-report-clear", {
            projections: [{ source: operation.literal([]), destination: targetParkReport }],
        }),
        ...verdictList.map((verdict, index) => {
            const suffix = verdictList.length > 1 ? `-${index + 1}` : "";
            return sequence.when(`park-report-record${suffix}`, {
                if: condition.fieldEquals(verdict, "status", "revise"),
                then: [
                    sequence.project(`park-report-append${suffix}`, {
                        projections: [{ source: verdict, destination: operation.over(targetParkReport, operation.Append) }],
                    }),
                ],
            });
        }),
    ];
}

/**
 * The family scribe prompt (P450 S3): writes the commit message from the
 * final verdict(s). It never writes the task board — the board is the
 * owner's status surface (P486).
 */
export function buildScribeText(verdict1: SlotHandle, verdict2: SlotHandle): PromptTemplate {
    return prompt.template(
        `The build loop for {task} has ended in dual approval and the work is being committed. The final reviewer verdicts are {verdict1} and {verdict2} — the sole authority on what was implemented (P414: this step is unreachable while either status is still revise; that case parks instead, with a typed park report, and never reaches here).
            Write a concise commit message for the completed work based only on the task contract {taskBrief} and those verdicts: a short subject line naming the task, then a one-paragraph summary of what the verdicts certify implemented — never work belonging to other tasks.
            Return exactly that message as your output; the runtime injects it into the git commit step.
            Do not run any git commands and do not write any files for the message; staging and committing happen in later runtime steps.
            Do NOT record task status anywhere. ${TASK_WRITE_SCOPE_RIDER}
            Compose the tail of the commit message from the verdicts, including only the sections the verdicts actually support — never write a "none" placeholder for a section with no entries. If a verdict carries owner-items, add "Owner items:" quoting each entry (item, reason class, close-out command or decision). If a verdict carries remaining (cross-task seam citations only), add "Cross-task seams:" quoting it verbatim. The final adjudicated leftovers are attached as context: when non-empty, add a "Leftovers:" section reproducing every entry's typed fields (what, reason, needs, evidence, done-when) verbatim — never paraphrased, never invented; omit the section entirely when the list is empty. A non-empty leftovers list never affects approval or is written as if it were a blocker.`,
        { task, taskBrief, verdict1, verdict2 },
    );
}

/**
 * The complete family procedure (P450): task extraction, a cheap
 * reviewed planning composite, a produce-first doubly-reviewed build
 * composite, and the commit tail — built entirely from the
 * `guardedProduction`/`commitTail` kit in `@ctx-traits/agents`.
 * `implement-phase` (the dogfood entry point) calls this exact function
 * rather than restoring a second copy; it differs from `implement-default`
 * in package identity, metadata, and the optional `reviewRubric`/`stopIf`/
 * `onStop`/`reviewExtraInstruction` opts, never in the procedure's own
 * structure.
 */
export function familyProcedure(opts: {
    clerk: AgentHandle;
    smart1: AgentHandle;
    smart2: AgentHandle;
    worker: AgentHandle;
    scribe: AgentHandle;
    taskBoard: ResourceHandle;
    variantDoctrine: string;
    verdict1: SlotHandle;
    verdict2: SlotHandle;
    reviewRubric?: ResourceHandle;
    parkReportOutput?: SlotHandle;
    stopIf?: GuardValue;
    onStop?: SignalHandle | readonly SignalHandle[];
    reviewExtraInstruction?: string;
    planRounds?: number;
    buildRounds?: number;
}): readonly SequenceHandle[] {
    const {
        clerk,
        smart1,
        smart2,
        worker,
        scribe,
        taskBoard,
        variantDoctrine,
        verdict1,
        verdict2,
        reviewRubric,
        parkReportOutput,
        stopIf,
        onStop,
        reviewExtraInstruction,
        planRounds,
        buildRounds,
    } = opts;
    const parkReportSlot = parkReportOutput ?? parkReport;
    // 0047 mechanism 4: every variant stops on a timed-out gate, merged
    // alongside whatever early-stop arm the caller (e.g. phase's recurrence
    // breaker) already declares — `condition.any` so either arm alone stops
    // the loop, `onStop` carrying every signal the matching arm could be.
    const combinedStopIf = stopIf === undefined ? gateTimedOutStopIf : condition.any([stopIf, gateTimedOutStopIf]);
    const priorOnStop = onStop === undefined ? [] : Array.isArray(onStop) ? onStop : [onStop];
    const combinedOnStop = [...priorOnStop, gateTimedOut];

    const planning: GuardedProductionHandle = guardedProduction({
        id: "planning",
        produces: draft,
        produce: { agent: smart1, text: buildDraftPromptText() },
        review: { agent: smart2, text: buildPlanReviewText() },
        rounds: planRounds ?? 2,
        onExhausted: "continue",
    });

    const building: GuardedProductionHandle = guardedProduction({
        id: "building",
        produces: workSummary,
        produce: {
            agent: worker,
            text: buildProducePrompt(),
            optionalInputs: [leftovers, repoGatesPassed],
        },
        review: [
            reviewSeat(smart1, 1, variantDoctrine, verdict1, { reviewRubric, extraInstruction: reviewExtraInstruction }),
            reviewSeat(smart2, 2, variantDoctrine, verdict2, { reviewRubric, extraInstruction: reviewExtraInstruction }),
        ],
        evidence: [repoGatesStep, captureDiffStep],
        afterReview: deriveParkReportStep([verdict1, verdict2], { parkReportSlot }),
        alsoRequire: condition.equals(repoGatesPassed.ok, true),
        carry: leftovers,
        minRounds: 3,
        rounds: buildRounds ?? 5,
        onExhausted: "block",
        stopIf: combinedStopIf,
        onStop: combinedOnStop,
    });

    return [
        taskExtractionStep(clerk, taskBoard),
        feasibilityStep(smart1),
        planning,
        building,
        ...commitTail({
            id: "shipping",
            scribe: { agent: scribe, text: buildScribeText(verdict1, verdict2) },
            summary: workSummary,
            verdict: building.verdict,
            receipt: commitOutput,
        }),
    ];
}

export { ownerItemSchema };
