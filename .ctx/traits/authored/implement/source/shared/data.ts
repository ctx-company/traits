import * as cdk from "@ctx-traits/cdk";

// ---------------------------------------------------------------------------
// The step-walk types (0283), implement-local. The reviewer owns a typed list
// of open steps; the runtime walks it with for-each; the worker returns a
// typed per-step status; a blocked step carries the reviewer's typed
// resolution. No frozen fields, no proof, no stage-claim — the shared
// toolkit schemas are left untouched for other families until this proves out.
// ---------------------------------------------------------------------------

/** The reviewer's typed answer to a blocked step — always carries its method. */
const blockerResolution = cdk.schema.object(
  "blocker-resolution",
  {
    blocker: cdk.schema.field(cdk.schema.text(), {
      description: "What the worker could not get past, in one sentence — restated from its blocked return.",
    }),
    decision: cdk.schema.field(cdk.schema.text(), {
      description: "The reviewer's ruling that unblocks it: what holds, what to stop trying, which way to go — a decision, not a restatement of the problem.",
    }),
    approach: cdk.schema.field(cdk.schema.text(), {
      description: "The concrete method that follows from the decision: the specific move the next dispatch makes — files, call sites, the shape of the change. Never a bare 'proceed'.",
    }),
  },
  { description: "A blocked step's resolution: the blocker, the reviewer's decision, and the executable approach that follows from it." },
);

/**
 * One open step of the current work. The reviewer writes the list; the runtime
 * walks it; the worker is dispatched on one bound step at a time. `done-when`
 * is the acceptance the reviewer grades against — NOT frozen: the reviewer
 * revises the list only at the review handoff, never while the worker walks
 * it, so stability is structural, not enforced per field.
 */
export const stepSchema = cdk.schema.object(
  "step",
  {
    stage: cdk.schema.field(cdk.schema.text(), {
      description: "The plan stage this step belongs to (its id), so the list keeps the plan's order and grouping.",
    }),
    operation: cdk.schema.field(cdk.schema.text(), {
      description: "One imperative, concrete operation — what to create and what it owns, what to route, what must cease to exist. A destination is not a step; the operation that reaches it is.",
    }),
    "done-when": cdk.schema.field(cdk.schema.text(), {
      description: "The falsifiable acceptance for THIS step: a check the worker can run itself (a grep that must come back empty, a test that must pass, a call site that must route through X) or one precise observable sentence. What it does not ask for is not asked here; more is a NEW step.",
    }),
    resolution: cdk.schema.field(blockerResolution, {
      required: false,
      description: "Present only when a prior pass returned this step blocked and the reviewer resolved it: the decision + approach the next dispatch works from. Absent for a fresh step.",
    }),
  },
  { description: "One open step: its stage, the operation, the acceptance it is graded against, and the reviewer's resolution when it was previously blocked." },
);

/** The worker's typed return for one dispatch on one step. */
export const stepReturnSchema = cdk.schema.object(
  "step-return",
  {
    status: cdk.schema.field(cdk.schema.enum(["done", "blocked", "not-done"] as const), {
      description: "done when you verified this step's done-when holds in the tree with your own tools; blocked when you cannot proceed on it from inside this run (a real contradiction, a decision only the owner can make) — never difficulty or a red tree; not-done when you made progress but it does not yet hold, and the next dispatch should continue the SAME step. Claim only what you verified: a false 'done' comes back as the same open step.",
    }),
    summary: cdk.schema.field(cdk.schema.text(), {
      description: "What this dispatch changed and verified — files and symbols, commands run and their observed results. When status is blocked, state precisely what blocks it and why. The next dispatch reads this as its only memory of this one.",
    }),
  },
  { description: "The worker's report on one dispatch of one step: its status and what it did." },
);

// ---------------------------------------------------------------------------
// Core slots
// ---------------------------------------------------------------------------

// The plan the loop operates on. Stages carry a goal only — the mechanical
// `proof` is gone (the reviewer validates, nothing self-proofs), and there is
// no per-stage commit flag (v1 commits progress whenever the tree is dirty;
// per-stage-semantic commit lands with the commit effect in 0283.1).
export const plan = cdk.slot({
  id: "plan",
  schema: cdk.schema.object(
    "implementation-plan",
    {
      scope: cdk.schema.field(cdk.schema.text(), {
        description: "The task's scope restated from the task file: what is in, what is explicitly out.",
      }),
      stages: cdk.schema.field(
        cdk.schema.list(
          cdk.schema.object(
            "stage",
            {
              id: cdk.schema.field(cdk.schema.text(), {
                description: 'Short kebab-case slug naming the stage, stable for the whole run (e.g. "resolver", "flip-context", "delete-old").',
              }),
              goal: cdk.schema.field(cdk.schema.text(), {
                description: "What must be observable when this stage is done, taken from the task's Done-when. A goal is the only requirement of its stage.",
              }),
            },
            { description: "One stage of the plan: an id and the goal that defines it." },
          ),
        ),
        {
          description:
            "The plan as an ordered list of stages, each an id and a goal. Nothing else is required of a stage while it is open: whatever a later stage's goal covers may be in any state until that stage, and that is neither a defect nor a regression. The plan must be ACTIONABLE for this run — a stage that cannot be done as written is a plan error, surfaced as blocked steps and revised, never ground against. When the task declares itself an atomic cutover that only compiles as one unit, honor that: do not carve it into stages that each demand a green intermediate. Together the stages must cover every Done-when goal of the task.",
        },
      ),
      approach: cdk.schema.field(cdk.schema.text(), {
        description: "The suggested route: files to touch, symbols, edit order, reuse opportunities. Guidance the worker may deviate from in service of a goal, reporting the deviation.",
      }),
      risks: cdk.schema.field(cdk.schema.text(), {
        description: "What could go wrong and how the route mitigates it.",
      }),
    },
    { description: "smart's implementation plan for the task — an actionable plan, not an implementation." },
  ),
  description: "The implementation plan the reviewer turns into open steps and the worker implements.",
});

// The current OPEN steps of the work, the reviewer's sole loop output. The
// runtime walks this with for-each; empty means the task is done (derived
// completeness — no stage-claim). The reviewer carries done steps out of the
// list, keeps or resolves blocked ones, and appends new findings.
export const steps = cdk.slot({
  id: "steps",
  schema: cdk.schema.list(stepSchema),
  description: "The open steps the worker still has to do, in order, written by the reviewer. The for-each walks it; an empty list means the reviewer found nothing open — the task is done.",
});

// The worker's typed per-dispatch return. The inner work loop reads `status`
// to decide same-step-again vs advance; the next dispatch on the same step
// reads the whole thing as its only memory.
export const report = cdk.slot({
  id: "work-report",
  schema: stepReturnSchema,
  description: "The worker's return on its latest dispatch: the step's status and what the dispatch did.",
});

export const diffBase = cdk.slot.text({
  id: "diff-base",
  description: "Commit the session started from — a fixed anchor the reviewer can diff against with its own tools.",
  hint: "git rev-parse HEAD captured before any work; commits during the loop never move it.",
});

export const commitMessage = cdk.slot.text({
  id: "commit-message",
  description: "Commit message for the current progress commit; injected directly into the git commit command step.",
});

export const ownerDecisions = cdk.slot.texts({
  id: "owner-decisions",
  description: "Every owner ruling made during this run — one entry per summons — the run's durable decision record, carried into the commit message.",
  hint: "One entry per ruling: the question put to the owner and the owner's answer in one or two sentences.",
});

export const commitLog = cdk.slot.text({
  id: "commit-log",
  description: "Throwaway capture of the commit command's own output — never gates anything.",
});

export const gitStatus = cdk.slot.text({
  id: "git-status",
  description: "Working-tree status captured before the commit tail: git status --porcelain output verbatim.",
});

export const stageOutput = cdk.slot.text({
  id: "stage-output",
  description: "Output evidence from the git add command step.",
});

// ---------------------------------------------------------------------------
// Owner-notification slots (notify stays a command chain in 0283; it becomes a
// typed effect in 0283.1). The digest is re-keyed to the step list.
// ---------------------------------------------------------------------------

export const notifyId = cdk.slot.text({
  id: "notify-id",
  description: "The owner-notification activity id, exactly as `begin` printed it — every later notification step addresses this.",
});

export const notifyDigest = cdk.slot({
  id: "notify-digest",
  schema: cdk.schema.object(
    "notify-digest",
    {
      badge: cdk.schema.field(cdk.schema.text(), {
        description: 'Short status badge, 64 characters at most: "review: <N> open" while steps remain, or "review: done" when the step list is empty.',
      }),
      summary: cdk.schema.field(cdk.schema.text(), {
        description: 'Two to four plain sentences for the owner\'s phone: how many steps are open, what closed this pass, whether any step is blocked, and where the run is heading. NEVER the empty string — when the list is empty, exactly "done — no steps open".',
      }),
      journal: cdk.schema.field(cdk.schema.text(), {
        description: 'Exactly "steps: <O> open • <B> blocked" and NOTHING else — O counts open steps in the list, B counts steps carrying a resolution (previously blocked). No prose, no step text.',
      }),
      surface: cdk.schema.field(cdk.schema.text(), {
        description: "The owner's annotation surface: one line 'STEPS: <O> open' then every open step COPIED VERBATIM as numbered plain lines — 'n. [<stage>] <operation> — done-when: <done-when>', with ' — resolution: <decision>' appended when the step carries one. Fidelity to the step list is the only requirement; each annotation returns carrying the exact text it was made on.",
      }),
    },
    { description: "The step list digested for the owner notification thread." },
  ),
  description: "Scribe's per-pass digest of the step list, fed directly into the notification commands.",
});

export const notifyBadge = cdk.slot.text({ id: "notify-badge", description: "The digest's badge field, carried as text for command argv." });
export const notifySummary = cdk.slot.text({ id: "notify-summary", description: "The digest's summary field, carried as text for command argv." });
export const notifyJournal = cdk.slot.text({ id: "notify-journal", description: "The digest's journal field, carried as text for command argv." });
export const notifyLog = cdk.slot.text({ id: "notify-log", description: "The notifier's own JSON receipt for the most recent notification command." });

export const gateSurface = cdk.slot.text({ id: "gate-surface", description: "The digest's annotation surface, carried as text for the gate command." });

export const gateAnswer = cdk.slot.text({
  id: "gate-answer",
  description: "The owner's verdict-gate outcome: the literal 'accepted' when the owner had no annotations (or the gate is off); otherwise the ctx-annotate decision JSON. Each annotation carries `raw` (the exact surface text annotated) and `text` (the note). Annotations are binding edits to the step list: the reviewer applies them on the next pass.",
});

// The plan gate (unchanged): the plan is corrected at round 0 before any work.
export const planDigest = cdk.slot({
  id: "plan-digest",
  schema: cdk.schema.object(
    "plan-digest",
    {
      surface: cdk.schema.field(cdk.schema.text(), {
        description: "The owner's plan surface: the plan COPIED VERBATIM as plain numbered lines: 'SCOPE:' then the scope; 'STAGES:' then one line per stage as '<id>: <goal>' in order; 'APPROACH:' then the approach; 'RISKS:' then the risks.",
      }),
    },
    { description: "The drafted plan digested for the owner's annotation pass." },
  ),
  description: "Scribe's digest of the drafted plan into the owner's annotation surface.",
});

export const planSurface = cdk.slot.text({ id: "plan-surface", description: "The plan digest's surface, carried as text for the gate command." });

export const planAnswer = cdk.slot.text({
  id: "plan-answer",
  description: "The owner's plan-gate outcome: the literal 'accepted' when the owner had no annotations (or the gate is off); otherwise the ctx-annotate decision JSON. Annotations are binding corrections: the plan is drafted again with them until accepted.",
});

// The tree lane (implement:annotate): the owner's annotations over the working
// tree are the task. No task port, no plan gate, no verdict gate.
export const annotations = cdk.slot.text({
  id: "annotations",
  description: "The owner's annotations from ctx-annotate's tree view over the run's working tree: the decision JSON. In the tree lane this is the task — what to change and where. The literal 'none' when the owner closed the tree without annotating.",
});

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

export const task = cdk.port.input.text({
  id: "task",
  description: 'Task to implement, named by its file in .internal/tasks/ — the key ("0044" or "0044.2"), the full name, or the filename.',
});

export const ownerRuling = cdk.port.input.text({
  id: "owner-ruling",
  description: "Owner summons transport: 'on' (default) parks the run on reviewer-raised owner questions; 'off' skips the summons branch for unattended runs — escalations stay in the step's resolution for morning review.",
  optional: true,
  default: { value: "on" },
});

export const ownerGate = cdk.port.input.text({
  id: "owner-gate",
  description: "Owner annotation transport for the step list: 'annotate' (default) pipes each pass's surface to ctx-annotate; 'off' auto-accepts for unattended runs.",
  optional: true,
  default: { value: "annotate" },
});

export const planGate = cdk.port.input.text({
  id: "plan-gate",
  description: "Owner annotation transport for the drafted plan before any work: 'annotate' (default) pipes the plan surface to ctx-annotate and redrafts with corrections until accepted; 'off' auto-accepts for unattended runs.",
  optional: true,
  default: { value: "annotate" },
});

export const port = { task, ownerGate, ownerRuling, planGate };

export const slot = {
  plan,
  steps,
  report,
  diffBase,
  commitMessage,
  ownerDecisions,
  gitStatus,
  commitLog,
  stageOutput,
  notifyId,
  notifyDigest,
  notifyBadge,
  notifySummary,
  notifyJournal,
  gateSurface,
  gateAnswer,
  notifyLog,
  planDigest,
  planSurface,
  planAnswer,
  annotations,
};
