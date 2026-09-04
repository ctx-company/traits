import type { SchemaHandle } from "@ctx-traits/cdk";
import { schema } from "@ctx-traits/cdk";

/** The kit's minimal produce-review verdict value: `schema.decision()`'s stable `approved|revise` vocabulary plus the blockers that justify a `revise`. */
export type ReviewerVerdictValue = {
  readonly status: "approved" | "revise";
  readonly blockers: readonly string[];
};

/**
 * The kit's typed produce-review verdict: `status` built directly from
 * `schema.decision()` (P449's rider — the shared decision vocabulary is the
 * verdict spine, not a fourth hand-rolled enum) plus a plain blocker list.
 * Deliberately poorer than the family's own `reviewVerdictSchema` (blocker
 * ids, owner-items, escalation, ...) — that richer shape stays untouched
 * for P450; this is the kit's own, smaller contract.
 */
export const reviewerVerdict: SchemaHandle<ReviewerVerdictValue> = schema.object(
  "reviewer-verdict",
  {
    status: schema.field(schema.decision(), {
      description: "approved when no blocker remains; revise when at least one blocker remains.",
    }),
    blockers: schema.field(schema.list(schema.text()), {
      description: "The blocking defects that justify a revise verdict; empty when status is approved.",
    }),
  },
  {
    description: "Minimal produce-review verdict: decision status plus the blockers that justify a revise.",
  },
);

/** The kit's feasibility triage verdict: whether a task is even worth spending a build loop on. */
export type FeasibilityVerdictValue = {
  readonly verdict: "feasible" | "blocked" | "oversized" | "ambiguous";
  readonly evidence: string;
  readonly missing: readonly string[];
  readonly "owner-action": string;
};

/**
 * A built-in triage check run BEFORE any build loop spends anything (0047
 * mechanism 1's agent layer): given the task contract, an agent audits it
 * from four angles — possible, blocked, oversized, ambiguous — verifying
 * with its own tools that everything the task references as existing
 * actually exists in this worktree. Companion to the deterministic
 * blocked-status-marker dispatch preflight (`modules/io/src/
 * dispatch_preflight.rs::blocked_status_marker`): that catches DECLARED
 * blockage for free at dispatch time; this catches UNDECLARED blockage
 * (referenced artifacts absent) that no header ever recorded, at the cost
 * of one cheap frame instead of a whole grind loop parking at exhaustion.
 */
export const feasibilityVerdictSchema: SchemaHandle<FeasibilityVerdictValue> = schema.object(
  "feasibility-verdict",
  {
    verdict: schema.field(schema.enum(["feasible", "blocked", "oversized", "ambiguous"] as const), {
      description:
        "feasible when the task can be built from inside this run as scoped; blocked when it depends on something not landed; oversized when the scope does not honestly fit one run's budget; ambiguous when it lacks a falsifiable Done-when and any implementation would be a guess.",
    }),
    evidence: schema.field(schema.text(), {
      description:
        "What was checked and how: each referenced file, command, recipe, or prerequisite the task names as existing, and whether your tools confirmed it exists. Default to feasible absent proof of a problem — an unverified claim is never grounds for a non-feasible verdict on its own.",
    }),
    missing: schema.field(schema.list(schema.text()), {
      description:
        "The concrete things missing or wrong, one per entry (a referenced file that does not exist, a prerequisite task not landed, the specific ambiguity). Empty when verdict is feasible.",
    }),
    "owner-action": schema.field(schema.text(), {
      description:
        "The one owner action that would clear a non-feasible verdict — land the prerequisite, split the task, or rewrite the Done-when. Empty string (never omitted — always return the key) when verdict is feasible.",
    }),
  },
  {
    description:
      "Typed pre-build feasibility triage: possible, blocked, oversized, or ambiguous, with tool-verified evidence and the owner action that clears a non-feasible verdict.",
  },
);

/**
 * One operational step of a blocker's fix, with its verification state.
 * The step list is the loop's cross-round progress ledger: text is frozen
 * once written, status is what moves, evidence is what justifies the move.
 */
/** One step of a blocker's required fix, as accepted values. */
export type BlockerStepValue = {
  readonly step: string;
  readonly status: string;
  readonly evidence?: string;
  readonly ruling?: string;
};

export const blockerStepSchema: SchemaHandle<BlockerStepValue> = schema.object(
  "blocker-step",
  {
    step: schema.field(schema.text(), {
      description:
        "The operation, frozen once written: imperative, concrete, one action. Carried verbatim across rounds.",
    }),
    status: schema.field(schema.text(), {
      description:
        '"open", "done", "deferred" or "dropped". Flips to done only when the reviewer has verified the evidence against the working tree — never on the worker\'s claim alone. deferred and dropped are set only when the reviewer applies an owner annotation (recorded in the verdict\'s dispositions): deferred is the owner\'s "not now" — kept on record, not executed until the owner or its stage says so; dropped is the owner\'s "no" — kept on record, never executed. Neither blocks approval, and the reviewer never reopens either on its own.',
    }),
    evidence: schema.field(schema.text(), {
      required: false,
      description:
        "Why status is done: file:line, a test name, or a command and its observed result. Required in practice for every done step; absent while open.",
    }),
    ruling: schema.field(schema.text(), {
      required: false,
      description:
        "The owner's annotation note, verbatim, when one applies to this step. The worker executes a ruled step as the ruling says, not as the step text alone says; a ruled step done differently from its ruling is a blocker next round. Carried verbatim while the step survives.",
    }),
  },
  {
    description:
      "One step of a blocker's required fix, with verified progress state and the owner's ruling on it when one exists.",
  },
);

/** One blocking defect, as accepted values. */
export type BlockerValue = {
  readonly id: string;
  readonly where: string;
  readonly what: string;
  readonly "root-cause": string;
  readonly "required-fix": string;
  readonly steps: readonly BlockerStepValue[];
  readonly "done-when": string;
  readonly "recurrence-of"?: string;
  readonly "rule-source"?: string;
  readonly "rule-quote"?: string;
  readonly ruling?: string;
  readonly stage?: string;
};

export const blockerSchema: SchemaHandle<BlockerValue> = schema.object(
  "blocker",
  {
    id: schema.field(schema.text(), {
      description:
        "Stable kebab-case slug for this defect, chosen on first report and reused verbatim in every later round it survives (e.g. unrelated-change-guard-baseline).",
    }),
    stage: schema.field(schema.text(), {
      required: false,
      description:
        "The id of the plan stage this blocker belongs to: the stage whose goal it prevents. A blocker on the first open stage, or a regression of a done stage's goal, is the worker's work now; a blocker on a later stage is filed here and not worked until that stage is the first open one. Absent only when the plan carries no stages.",
    }),
    where: schema.field(schema.text(), {
      description: "Repo-relative files and paths involved.",
    }),
    what: schema.field(schema.text(), {
      description: "The defect and the concrete failure it causes.",
    }),
    "root-cause": schema.field(schema.text(), {
      description:
        "Why the code is wrong — the missing or broken invariant, not just where it shows — and how deep the wrong goes: a slip at the cited line, a missing invariant, or a structure the code must converge to. The depth tells the worker how big a change you expect; omitting it is how a structural demand gets answered with a point-patch.",
    }),
    "required-fix": schema.field(schema.text(), {
      description:
        "One or two sentences of intent: the invariant an acceptable fix establishes and why. The operational content — what to do, in what order — lives in `steps`, which is the field the worker executes and the field you maintain across rounds.",
    }),
    steps: schema.field(schema.list(blockerStepSchema), {
      description:
        "The fix as an ordered list of typed steps — you are the smarter model here, and this list exists to spend that intelligence on the worker's behalf (owner ruling 2026-07-30). Each step is one operation: what to create and what it owns, what must cease to exist or shrink to a delegate, which call sites to route. A destination (\"replace the separate paths with one renderer\") is not a step; the operations that reach it are. THIS LIST IS CUMULATIVE STATE, not a fresh derivation: when this blocker appeared in your prior verdict (attached), carry its steps forward VERBATIM — same order, same text — flipping status to done only where you verified the evidence with your own tools, and appending genuinely new findings as new steps at the end. Never renumber, reword, drop, or re-derive a carried step: a vanished step erases the worker's credit, a reworded one moves the target, and both teach the worker that completing steps is pointless — the four-day stall this field exists to end.",
    }),
    "done-when": schema.field(schema.text(), {
      description:
        "A falsifiable check the reviewer will apply next round to declare this blocker fixed. Wherever the check can be a COMMAND, state it as one the worker can run itself (a grep proving a function has exactly one caller, a test invocation) — the worker iterates against build-and-test signals, and a structural requirement stated only as prose is invisible to that loop: the worker stops where its own instruments read green, honestly believing the blocker addressed.",
    }),
    "recurrence-of": schema.field(schema.text(), {
      required: false,
      description:
        "id of the prior-round blocker this restates; present when the prior fix did not clear its done-when.",
    }),
    "rule-source": schema.field(schema.text(), {
      required: false,
      description:
        "Repo-relative path + section for the house rule this blocker cites, verified directly against the rule-authority resource. Present only when the blocker cites a standing product rule.",
    }),
    "rule-quote": schema.field(schema.text(), {
      required: false,
      description:
        "The exact authoritative line(s) the cited rule quotes, verified against the rule-authority resource. Present only when the blocker cites a standing product rule.",
    }),
    ruling: schema.field(schema.text(), {
      required: false,
      description:
        "The owner's annotation note, verbatim, when one applies to the whole blocker — its order, its scope, its standing. Applied by the reviewer when the annotation was made; carried verbatim while the blocker survives. Blockers are listed in the order the worker executes them, and an owner's ordering ruling is what moves a blocker in that list.",
    }),
  },
  {
    description:
      "One blocking defect: stable identity, location, root cause, the invariant an acceptable fix must establish, the falsifiable check that clears it, and the owner's ruling on it when one exists.",
  },
);

/** One plan stage's standing in the verdict's ledger, as accepted values. */
export type StageStatusValue = {
  readonly id: string;
  readonly status: "open" | "done";
  readonly evidence: string;
  readonly ruling?: string;
};

/**
 * One entry of the verdict's stage ledger: the plan's stage by id, whether
 * its goal holds, on what evidence, and the owner's ruling on it when one
 * exists. The ledger is what makes the plan survive past round 1 — the
 * first open stage is the worker's objective, and nothing outside its goal
 * and the goals of done stages is required.
 */
export const stageStatusSchema: SchemaHandle<StageStatusValue> = schema.object(
  "stage-status",
  {
    id: schema.field(schema.text(), {
      description: "The stage id exactly as the plan's stages list names it.",
    }),
    status: schema.field(schema.enum(["open", "done"] as const), {
      description:
        "done only when you verified with your own tools that the stage's goal is observable in the working tree; open otherwise. A done stage whose goal stops holding goes back to open, with the evidence that shows it — that is the only kind of regression there is.",
    }),
    evidence: schema.field(schema.text(), {
      description: "Why the status is what it is: what you ran or read. Empty only for a stage nobody has reached yet.",
    }),
    ruling: schema.field(schema.text(), {
      required: false,
      description:
        "The owner's annotation note on this stage, verbatim, when one exists (an order to start it now, to treat it as done, to change its goal). Applied by the reviewer and carried verbatim.",
    }),
  },
  {
    description:
      "One plan stage in the verdict's ledger: id, whether its goal holds, the evidence, and the owner's ruling on it when one exists.",
  },
);

/** One owner annotation and what the reviewer did with it, as accepted values. */
export type DispositionValue = {
  readonly raw: string;
  readonly text: string;
  readonly "applied-to": string;
  readonly action: "deferred" | "dropped" | "done" | "added" | "ordered" | "ruled" | "answered" | "unclear";
  readonly note: string;
};

/**
 * One owner annotation applied to a verdict: the annotated surface text
 * (the anchor — an exact substring of the verdict's own text, because the
 * surface is a verbatim copy), the owner's note, where it landed, and what
 * the reviewer did. The owner reads these on the next surface to check the
 * application and correct it by annotating again.
 */
export const dispositionSchema: SchemaHandle<DispositionValue> = schema.object(
  "disposition",
  {
    raw: schema.field(schema.text(), {
      description:
        "The annotated surface text, verbatim from the gate answer's raw field. This is the anchor: it is an exact substring of the verdict's own blocker, step, stage or advisory text.",
    }),
    text: schema.field(schema.text(), {
      description: "The owner's note, verbatim from the gate answer's text field.",
    }),
    "applied-to": schema.field(schema.text(), {
      description:
        'Where the annotation landed: a blocker id, "<blocker-id>/step <n>", "stage <id>", "advisory", or "plan".',
    }),
    action: schema.field(
      schema.enum(["deferred", "dropped", "done", "added", "ordered", "ruled", "answered", "unclear"] as const),
      {
        description:
          "What the reviewer did: deferred (the owner said not now — the step keeps status deferred until the owner or its stage brings it back); dropped (the owner said no — the step keeps status dropped, never executed, never reopened); done (the owner accepted it as is — status done with the note as evidence); added (the note asked for something new — a new step or blocker carrying the note as its ruling); ordered (the note changed what comes first — the blocker list is reordered); ruled (the note changes how a step or blocker is done — carried as its ruling and executed as ruled); answered (the note answered or asked something that changes no item — the answer is in note); unclear (the note could not be mapped or understood — note carries a one-sentence question for the owner, and nothing was changed).",
      },
    ),
    note: schema.field(schema.text(), {
      description:
        "One sentence: what changed in this verdict because of the annotation, or the question back to the owner when action is unclear.",
    }),
  },
  {
    description:
      "One owner annotation applied to the verdict by the reviewer: anchor text, note, target, action taken, and a one-sentence account. Annotations are binding edits, never arguments; where the reviewer disagrees it applies the annotation anyway and records the disagreement in the advisory.",
  },
);

/**
 * Strict-variant deviation report: one proposed departure from a verbatim plan, its rationale, and
 * its disposition. Standalone export for later families (P362/P363) to compose where they choose;
 * not attached to `reviewVerdictSchema` here.
 */
export const deviationReportSchema: SchemaHandle = schema.object(
  "deviation-report",
  {
    id: schema.field(schema.text(), {
      description: "Stable kebab-case slug for this deviation, reused verbatim if it recurs.",
    }),
    planned: schema.field(schema.text(), {
      description: "What the plan specified, verbatim or tightly paraphrased.",
    }),
    actual: schema.field(schema.text(), {
      description: "What actually happened instead.",
    }),
    rationale: schema.field(schema.text(), {
      description: "Why the departure occurred.",
    }),
    disposition: schema.field(schema.text(), {
      description: "How the departure was resolved or is being handled.",
    }),
  },
  {
    description:
      "One proposed departure from a verbatim plan under strict authority: what was planned, what actually happened, why, and its disposition.",
  },
);

/**
 * Smart-variant plan amendment: a typed record that makes a contradiction resolution the binding
 * contract going forward. Standalone export for later families (P362/P363) to compose where they
 * choose; not attached to `reviewVerdictSchema` here.
 */
export const planAmendmentSchema: SchemaHandle = schema.object(
  "plan-amendment",
  {
    change: schema.field(schema.text(), {
      description: "The change being made to the plan.",
    }),
    "contradiction-resolved": schema.field(schema.text(), {
      description: "The contradiction between the plan and the observed work that this amendment resolves.",
    }),
    rationale: schema.field(schema.text(), {
      description: "Why this change is the right resolution.",
    }),
    provenance: schema.field(schema.text(), {
      description: "Who or what authorized this amendment and how it was recorded.",
    }),
  },
  {
    description:
      "One typed amendment under smart authority: the plan change, the contradiction it resolves, its rationale, and its provenance. Becomes the binding contract once recorded.",
  },
);

/**
 * One checklist item no in-run effort can satisfy, accepted per the draft's scope split:
 * its reason class, the substitute evidence the reviewer verified, and the exact owner
 * action that closes it. The run completes; this is the honest remainder of the receipt.
 */
export const ownerItemSchema: SchemaHandle = schema.object(
  "owner-item",
  {
    item: schema.field(schema.text(), {
      description: "The checklist item or Done-when clause, quoted or tightly paraphrased.",
    }),
    class: schema.field(
      schema.enum(["gui-or-visual", "paid-or-live-execution", "owner-decision", "contract-conflict"] as const),
      {
        description:
          "Why no in-run effort can satisfy it: needs a real screen; needs money or an owner-authorized live execution; needs an owner authority call; or contradicts landed code/authoritative rule and the resolution is the owner's.",
      },
    ),
    reason: schema.field(schema.text(), {
      description: "One sentence: why this item is outside in-run capability or authority.",
    }),
    "substitute-evidence": schema.field(schema.text(), {
      description:
        'The closest in-run verification actually produced and verified by the reviewer (tests, dry runs, static checks); "none possible" only when truly nothing applies.',
    }),
    "close-out": schema.field(schema.text(), {
      description: "The exact command the owner runs or decision the owner makes to finish the item.",
    }),
  },
  {
    description:
      "A checklist item outside in-run capability or authority, with verified substitute evidence and the owner action that closes it.",
  },
);

/**
 * One typed leftover: legitimate follow-on work the shipped result does not
 * require to stand alone, proposed by the worker and adjudicated by
 * reviewers per `LEFTOVER_DOCTRINE` before it survives into `slot:leftovers`.
 */
export const leftoverSchema: SchemaHandle = schema.object(
  "leftover",
  {
    what: schema.field(schema.text(), {
      description: "The follow-on work item, stated concretely in exactly one sentence.",
    }),
    reason: schema.field(schema.enum(["needs-unlanded", "needs-human"] as const), {
      description:
        "Why this follow-on work is outside the run: needs-unlanded (depends on work not yet landed) or needs-human (requires an owner or authority action this run cannot take).",
    }),
    needs: schema.field(schema.list(schema.text()), {
      description: "What must happen before this item can be done; may be empty.",
    }),
    evidence: schema.field(schema.text(), {
      description: "Why the shipped result stands alone without this item — the second question's verified answer.",
    }),
    "done-when": schema.field(schema.text(), {
      description: "The falsifiable check that will confirm this item is complete once someone takes it up.",
    }),
  },
  {
    description:
      "One adjudicated leftover: real follow-on work that the shipped result does not require, with its reason class, prerequisites, standalone-evidence, and completion check.",
  },
);

/**
 * The typed review-verdict schema shared by reviewed multi-agent refinement loops: a blocking
 * defect list against the shared blocker schema, non-blocking advisory notes, owner-triage
 * escalation, and the owner-items remainder accepted per the draft's scope split.
 */
/**
 * The family review verdict, as accepted values. Typing the handle is what
 * gives `slot({ schema: reviewVerdictSchema }).status` a real `FieldRef`
 * instead of `unknown` — the bare `SchemaHandle` annotation silently threw
 * the inference away and broke `condition.equals(verdict.status, ...)`
 * under tsc in every consuming trait package.
 */
export type ReviewVerdictValue = {
  readonly status: "approved" | "revise";
  readonly blockers: readonly BlockerValue[];
  readonly advisory?: string;
  readonly escalation: "none" | "needs-owner";
  readonly "escalation-reason": string;
  readonly "wall-id": string;
  readonly remaining?: string;
  readonly "owner-items"?: readonly unknown[];
  readonly dispositions?: readonly DispositionValue[];
  readonly stages?: readonly StageStatusValue[];
};

export const reviewVerdictSchema: SchemaHandle<ReviewVerdictValue> = schema.object(
  "review-verdict",
  {
    status: schema.field(schema.enum(["approved", "revise"] as const), {
      description:
        "approved when no step in any blocker has status open (advisory notes, deferred steps and dropped steps may still exist); revise when at least one step is open. A step the owner deferred or dropped never keeps status at revise.",
    }),
    blockers: schema.field(schema.list(blockerSchema), {
      description:
        "The blocking defects that must be fixed before merge: correctness bugs, failing validation gates, clear over-build (accretion, defensive validation for states that cannot occur, scope creep beyond the task), OR un-abstracted duplication — logic that duplicates or closely resembles code elsewhere and should be unified into a shared abstraction instead of re-implemented or copied beside. Each entry carries a stable id, root cause, the required-fix invariant, and a falsifiable done-when. Listed in the order the worker executes them — the plan's order, then the owner's ordering rulings; never sorted by how cheap a fix is. Non-empty when status is revise. When approved the list holds no open step: a blocker whose steps are all done or dropped is removed and named in the advisory, a blocker still holding a deferred step stays listed so the deferral is on record. Always return the key (an empty list, never omitted) so the runtime can deterministically copy it into a park report without a missing-field failure.",
    }),
    advisory: schema.field(schema.text(), {
      required: false,
      description:
        "Non-blocking notes: subjective style, naming, taste, optional improvements, follow-up work. Never affects status and never forces another refinement round.",
    }),
    escalation: schema.field(schema.enum(["none", "needs-owner"] as const), {
      description:
        "needs-owner if and only if at least one blocker cannot be cleared by the worker from inside this run: the task file is a placeholder or lacks a falsifiable Done-when; a prerequisite task it names has not landed in this tree; the task is marked superseded or cancelled; resolving it requires an owner/authority decision; or a required gate fails for reasons outside the task's scope (tooling conflict). Escalation is RECORDED for owner triage — it does not stop the loop; refinement continues on every fixable blocker. none otherwise.",
    }),
    "escalation-reason": schema.field(schema.text(), {
      description:
        "One plain sentence: WHY the flagged blocker set is outside this run's authority and what owner action would clear it. Non-empty when escalation is needs-owner; an empty string (never omitted — always return the key) otherwise. Always present so the runtime can deterministically copy it into a park report without a missing-field failure.",
    }),
    "wall-id": schema.field(schema.text(), {
      description:
        'Stable wall id copied VERBATIM from an explicit "**Wall:** <id>" label in the task file, non-empty only when status is revise and that label exists — never inferred from prose similarity or blocker content. Enables cross-run standing-wall refusal (P414); an empty string here never blocks a sibling run no matter how related its blockers look. Always present (never omitted) so the runtime can deterministically copy it into a park report without a missing-field failure.',
    }),
    remaining: schema.field(schema.text(), {
      required: false,
      description:
        "Cross-task seam citations ONLY: counterpart work belonging to a DIFFERENT task on the task board, each citing that task's file name. Never in-task scope — an unimplemented item of THIS task's checklist is a blocker, and unimplemented in-task scope recorded here instead of in blockers falsifies the verdict. Absent when no cross-task seam exists.",
    }),
    "owner-items": schema.field(schema.list(ownerItemSchema), {
      required: false,
      description:
        "Checklist items of THIS task that no in-run effort can satisfy, accepted per the draft's SCOPE SPLIT (or a verified mid-run wall claim), each with its reason class, the substitute evidence you verified, and the owner close-out. Approving with this list certifies every agent-doable item is 100% implemented. Never doable work — promote any owner-only claim a shell could satisfy to a blocker; an entry with missing or unverified substitute evidence belongs in blockers, not here. Absent when every item is agent-doable.",
    }),
    dispositions: schema.field(schema.list(dispositionSchema), {
      required: false,
      description:
        "One entry per owner annotation applied to this verdict, in the order annotated. Present exactly when the gate answer carried annotations; absent otherwise. Every annotation gets exactly one entry — none dropped, none merged into another, none reworded — and the edit each entry describes is visible in this same verdict (a status, a ruling, an order, a new step). The next surface shows these lines so the owner can check the application and correct it by annotating again. A later round carries earlier rulings in the items they landed on, not here: this list is this application only.",
    }),
    stages: schema.field(schema.list(stageStatusSchema), {
      required: false,
      description:
        "The plan's stage ledger: every stage of the plan's stages list, in plan order, carried verbatim across rounds with only status, evidence and ruling moving. The first open stage is the worker's objective; blockers name the stage they belong to; nothing outside the first open stage's goal and the goals of done stages is required, so a red suite or a failing gate a later stage covers is neither a defect nor a regression and never a blocker. In the first round, also verify that the stages cover every Done-when goal of the task file (read it with your tools) and block any uncovered goal as a blocker on the last stage. Absent only when the plan carries no stages.",
    }),
  },
  {
    description:
      "Typed review verdict. The loop blocks on genuine defects, clear over-build, and un-abstracted duplication — not on taste; status is revise if and only if an open step remains; escalation flags run-level owner blockers for triage without stopping the loop; owner-items records the verified outside-capability remainder so the run can complete honestly; owner annotations are applied to this verdict by the reviewer and accounted for in dispositions — they are binding edits, never arguments, and a step the owner deferred or dropped is never reopened by the reviewer.",
  },
);
