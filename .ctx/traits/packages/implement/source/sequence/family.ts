// Shared implementation concepts used by the native implement family leaves.
import {
    clerkRole,
    feasibilityGate,
    feasibilityVerdictSchema,
    leftoverSchema,
    ownerItemSchema,
    reviewerRole,
    reviewVerdictSchema,
    scribeRole,
    workerRole,
} from "@ctx-traits/agents";
import type { GuardedProductionRole } from "@ctx-traits/agents";
import type {
    AgentHandle,
    ResourceHandle,
    SequenceHandle,
    SlotHandle,
} from "@ctx-traits/cdk";
import { condition, effect, flow, input, operation, port, resource, schema, signal, slot, step } from "@ctx-traits/cdk";

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
 * check-step timeout). `gateTimedOutAbortIf` is provably mutually exclusive
 * with every `guardedProduction` `until` — each conjoins `repoGatesPassed.ok
 * == true`, which a timed-out gate (`command_execution_succeeded` forces
 * `ok=false` whenever `timed_out` is true) always falsifies — so no
 * `guard-conflict` diagnostic is reachable.
 */
export const gateTimedOut = signal({
    id: "gate-timed-out",
    description: "The repository gate exceeded its declared ceiling — a repo condition no worker round can fix.",
});
export const gateTimedOutAbortIf = condition.fieldEquals(repoGatesPassed, "timed-out", true);

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
 * then parks (`onExhausted: "abort"` halts before the commit tail ever
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
        // Owner ruling 2026-07-31 (first live firing, run-bba65cb5): the
        // audit is WARNING-ONLY for now — the typed verdict is recorded for
        // the reviewer and the owner, but never stops the run.
        mode: "warn",
    });
}

/** Functional-layer form (0109 F2): `task-extraction`'s hand-chosen id never matched `idFromTitle` of its own title, so it needs the `id:` override — the same known gap as the seven baseline steps, just uncounted in the 0108 pilot since quick never called this step. */
export function taskExtractionStep(agentHandle: AgentHandle, taskBoardHandle: ResourceHandle): void {
    agentHandle.prompt("Copy the task contract (clerk)", {
        id: "task-extraction",
        input: input.prompt`
            Copy the task file for ${task} EXACTLY as written.
            Open the task-board directory named in ${taskBoardHandle} with your tools. Task files are named NNNN-kebab-slug.md; the requested task names its file by number, full name, or filename — list the directory and match it (a bare number matches its NNNN- prefix). Files under archived/ are not live tasks; match one only when the request names it explicitly.
            Return the file's entire contents, byte-for-byte — no paraphrasing, no summaries, no commentary, no added headers. Every later step works from this copy instead of the board, so anything you drop is lost.`,
        output: taskBrief,
    });
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
 *
 * Functional-layer form (0109 F3): calls `step.project`/`flow.when`
 * directly into the current build scope instead of returning handles — a
 * pre-built `SequenceHandle[]` has no way to be lifted into a functional
 * loop body. Every id/title below is auto-derived from its own title text
 * (`idFromTitle`), matching this function's own former hand-chosen ids
 * exactly, so no `id:` override is needed here.
 */
export function deriveParkReportStep(
    verdicts: SlotHandle | SlotHandle[],
    opts: { parkReportSlot?: SlotHandle } = {},
): void {
    const targetParkReport = opts.parkReportSlot ?? parkReport;
    const verdictList = Array.isArray(verdicts) ? verdicts : [verdicts];
    step.project("Park Report Clear", {
        projections: [{ source: operation.literal([]), destination: targetParkReport }],
    });
    verdictList.forEach((verdict, index) => {
        const titleSuffix = verdictList.length > 1 ? ` ${index + 1}` : "";
        flow.when(`Park Report Record${titleSuffix}`, condition.fieldEquals(verdict, "status", "revise"), () => {
            step.project(`Park Report Append${titleSuffix}`, {
                projections: [{ source: verdict, destination: operation.over(targetParkReport, operation.Append) }],
            });
        });
    });
}

/**
 * The functional-layer equivalent of `@ctx-traits/agents`' `commitTail`
 * (0109): the clean-tree-guarded git tail. Simplified to the shape every
 * caller actually uses — a caller-owned `{agent, text}` scribe role; the
 * kit's own default-scribe-prose fallback has no consumer in this family and
 * is not reproduced (leanness).
 */
export function familyCommitTail(opts: {
    id: string;
    scribe: GuardedProductionRole;
    receipt: SlotHandle;
    /** 0098: routes the commit through an external approval gate — e.g. `ctx-gate run --`. Absent, the commit command is untouched. */
    gate?: { prefix: string; title?: string; timeoutMs?: number };
}): void {
    const { id, scribe, receipt, gate } = opts;
    const status = slot.text({
        id: `${id}-status`,
        description:
            "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
        hint:
            "Empty exactly when the tree is clean; the commit tail is skipped entirely in that case, so no commit is attempted and the scribe never runs.",
    });
    const message = slot.text({ id: `${id}-message`, description: "The commit message the scribe writes for the completed work." });
    const stageOutput = slot.text({ id: `${id}-stage-output`, description: "Verbatim output of the git-add staging command." });

    step.command("Check working tree status", { id: `${id}-status`, input: input.command`git status --porcelain`, output: status });

    flow.when("Shipping Maybe Commit", condition.not(condition.equals(status, "")), () => {
        scribe.agent.prompt("Write the commit message (scribe)", { id: `${id}-message`, input: scribe.text, output: message });
        step.command("Stage all changes except runtime state", {
            id: `${id}-stage`,
            input: input.command`git add -A -- :!.agents/runs`,
            output: stageOutput,
        });
        const commitInput = gate === undefined ? input.command`git commit -m ${message}` : (() => {
            const strings = [`${gate.prefix} git commit -m `, ``];
            return input.command(Object.assign(strings, { raw: strings }) as unknown as TemplateStringsArray, message);
        })();
        step.command(gate?.title ?? "Commit the work", {
            id: `${id}-commit`,
            input: commitInput,
            output: receipt,
            ...(gate?.timeoutMs === undefined ? {} : { timeoutMs: gate.timeoutMs }),
        });
    });
}

export { ownerItemSchema };
