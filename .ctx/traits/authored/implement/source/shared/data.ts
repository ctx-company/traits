import * as cdk from "@ctx-traits/cdk";
import * as agents from "@ctx-traits/agents";

// The plan the loop operates on (0281.2). Prose everywhere except `stages`:
// an ordered list the verdict's stage ledger walks, so the plan survives
// past round 1. The permissive rule lives in the stages description and
// nowhere else: a goal is the only requirement of its stage, nothing
// unstated is a restriction.
export const draft = cdk.slot({
  id: "draft",
  schema: cdk.schema.object(
    "implementation-draft",
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
                description:
                  "What must be observable when this stage is done, taken from the task's Done-when. A goal is the only requirement of its stage.",
              }),
            },
            { description: "One stage of the plan: an id and the goal that defines it." },
          ),
        ),
        {
          description:
            "The plan as an ordered list of stages, each an id and a goal. Nothing else is required of a stage while it is open: whatever a later stage's goal covers may be in any state until that stage, and that is neither a defect nor a regression. Restrictions exist only as goals; anything unstated is allowed. A done stage's goal must keep holding, and a standing property (a safety invariant, a contract) is the goal of the stage that establishes it — nothing more is needed to keep it. The stage is the unit of completeness: the reviewer grades the stage, never a round, and a round is one attempt at finishing the current stage — the worker attempts the whole stage and, when it cannot finish, reports what is left. A cutover is stages, not a cliff: its last stages are the deletion and the green gates. Together the stages must cover every Done-when goal of the task.",
        },
      ),
      approach: cdk.schema.field(cdk.schema.text(), {
        description:
          "The suggested route: files to touch, symbols, edit order, reuse opportunities, validation plan. Guidance the worker may deviate from in service of a goal, reporting the deviation in the work summary.",
      }),
      risks: cdk.schema.field(cdk.schema.text(), {
        description: "What could go wrong and how the route mitigates it.",
      }),
    },
    { description: "smart-1's implementation plan for the task — what the loop operates on. A plan, not an implementation." },
  ),
  description: "smart-1's implementation plan for the task — the plan the worker implements and the verdict's stage ledger walks.",
});

export const workSummary = cdk.slot.text({
  id: "work-summary",
  description: "Worker's account of what this round changed.",
  hint: "What changed, how it was validated, and open concerns.",
});

export const changedFiles = cdk.slot.text({
  id: "changed-files",
  description:
    "Names and change-kinds of every file changed since the session base — an index of where the work landed, never the work itself. The reviewer opens whatever it needs with its own tools.",
  hint: "git diff --name-status against slot:diff-base; covers committed and uncommitted changes, omits untracked files.",
});

export const diffBase = cdk.slot.text({
  id: "diff-base",
  description: "Commit the session started from — the fixed base every changed-files capture diffs against.",
  hint: "git rev-parse HEAD captured before any work; worker commits during the loop never move it.",
});

export const verdict1 = cdk.slot({
  id: "review-verdict-1",
  schema: agents.reviewVerdictSchema,
  description: "First reviewer's verdict for the current work state.",
});

export const verdict2 = cdk.slot({
  id: "review-verdict-2",
  schema: agents.reviewVerdictSchema,
  description: "Second, independent reviewer's verdict for the current work state.",
});

export const commitMessage = cdk.slot.text({
  id: "commit-message",
  description: "Commit message for the completed task; injected directly into the git commit command step.",
});

export const ownerDecisions = cdk.slot.texts({
  id: "owner-decisions",
  description:
    "Every owner ruling made during this run — one formatted entry per summons, one gate answer per annotated verdict — the run's durable decision record, carried into the commit message. Annotations reach the seats through the verdict the reviewer applied them to, not through this record.",
  hint: "One entry per ruling: a summons entry is the question and the owner's answer in one or two sentences; a gate entry is the ctx-annotate decision JSON verbatim.",
});

export const commitLog = cdk.slot.text({
  id: "commit-log",
  description: "Throwaway capture of the commit command's own output — not a run output port, never gates anything.",
});

export const gitStatus = cdk.slot.text({
  id: "git-status",
  description: "Working-tree status captured immediately before the commit tail: git status --porcelain output verbatim.",
});

export const notifyId = cdk.slot.text({
  id: "notify-id",
  description:
    "The owner-notification activity id, exactly as `begin` printed it — every later notification step addresses this.",
});

export const notifyDigest = cdk.slot({
  id: "notify-digest",
  schema: cdk.schema.object(
    "notify-digest",
    {
      badge: cdk.schema.field(cdk.schema.text(), {
        description:
          'Short status badge for the activity, 64 characters at most, always "review: <verdict status>".',
      }),
      summary: cdk.schema.field(cdk.schema.text(), {
        description:
          'Multi-line prose block for the owner\'s phone, 4096 characters at most: the verdict in two to four plain sentences — how many blocker steps are open, what closed this round, and where the run is heading. NEVER the empty string (an empty value aborts the command it feeds): when the verdict is approved, exactly "approved — all points closed".',
      }),
      journal: cdk.schema.field(cdk.schema.text(), {
        description:
          'Exactly "stage <k>/<n> \u2022 <X> open \u2022 <Y> closed" and NOTHING else \u2014 k is the 1-based position of the first open stage in the verdict\'s stage ledger (n when every stage is done), n the number of stages, X counts steps with status "open" and Y counts steps with status "done" or "dropped", across every blocker in the verdict. When the verdict carries no stage ledger, exactly "<X> open \u2022 <Y> closed". No step texts, no prose, no punctuation beyond the bullets.',
      }),
      surface: cdk.schema.field(cdk.schema.text(), {
        description:
          "The owner's annotation surface. The verdict status on the first line. Then, when the verdict carries a stage ledger, one line per stage under 'STAGES:' as '<id> (<status>): <evidence>' with its ruling when present, in ledger order. Then every blocker and every step COPIED VERBATIM — never paraphrased, shortened, or reordered — as plain numbered lines, one step per line, blockers separated by a blank line and introduced by 'BLOCKER n (stage, status):'; each step line carries the step text, then its status, then its explanation VERBATIM, then its guidance when present, then its ruling when present, separated by ' — '. After the blockers: the advisory VERBATIM under a line reading 'ADVISORY:', then — when the verdict carries dispositions — one line per disposition under 'RULINGS APPLIED:', each as '<action> — <applied-to> — <note>'. This text is what the owner annotates, and each annotation comes back carrying the exact text it was made on, so fidelity to the verdict is the only requirement.",
      }),
    },
    { description: "The reviewer verdict digested for the owner notification thread." },
  ),
  description: "Scribe's per-round digest of the review verdict, fed directly into the notification commands.",
});

export const notifyBadge = cdk.slot.text({
  id: "notify-badge",
  description: "The digest's badge field, carried as text for command argv.",
});

export const notifySummary = cdk.slot.text({
  id: "notify-summary",
  description: "The digest's summary field, carried as text for command argv.",
});

export const notifyJournal = cdk.slot.text({
  id: "notify-journal",
  description: "The digest's journal field, carried as text for command argv.",
});

export const gateSurface = cdk.slot.text({
  id: "gate-surface",
  description: "The digest's annotation surface, carried as text for the gate command.",
});

export const gateAnswer = cdk.slot.text({
  id: "gate-answer",
  description:
    "The owner's verdict-gate outcome: the literal 'accepted' when the owner had no annotations (or the gate is off); otherwise the ctx-annotate decision JSON. Each of its annotations carries `raw` — the exact surface text the owner annotated, an exact substring of the verdict — and `text`, the owner's note. Annotations are binding edits to the verdict: the reviewer applies them and accounts for each in the verdict's dispositions.",
});

export const ownerRulingMode = cdk.slot.text({
  id: "owner-ruling-mode",
  description: "The owner-ruling port carried as a slot so the summons branch condition can read it.",
});

// The plan gate (0281.3): the same gate shape as the verdict gate, pointed
// at the drafted plan before round 1, so the plan is corrected at round 0
// for free instead of at round 3 for money.
export const planDigest = cdk.slot({
  id: "plan-digest",
  schema: cdk.schema.object(
    "plan-digest",
    {
      surface: cdk.schema.field(cdk.schema.text(), {
        description:
          "The owner's plan surface: the plan COPIED VERBATIM — never paraphrased, shortened, or reordered — as plain numbered lines: 'SCOPE:' then the scope text; 'STAGES:' then one line per stage as '<id>: <goal>' in plan order; 'APPROACH:' then the approach text; 'RISKS:' then the risks text. This text is what the owner annotates, and each annotation comes back carrying the exact text it was made on, so fidelity to the plan is the only requirement.",
      }),
    },
    { description: "The drafted plan digested for the owner's annotation pass." },
  ),
  description: "Scribe's digest of the drafted plan into the owner's annotation surface.",
});

export const planSurface = cdk.slot.text({
  id: "plan-surface",
  description: "The plan digest's surface, carried as text for the gate command.",
});

export const planAnswer = cdk.slot.text({
  id: "plan-answer",
  description:
    "The owner's plan-gate outcome: the literal 'accepted' when the owner had no annotations (or the gate is off); otherwise the ctx-annotate decision JSON. Each of its annotations carries `raw` — the exact plan text the owner annotated — and `text`, the owner's note. Annotations are binding corrections: the plan is drafted again with them as input until the owner accepts it.",
});

export const notifyLog = cdk.slot.text({
  id: "notify-log",
  description: "The notifier's own JSON receipt for the most recent notification command.",
});

export const stageOutput = cdk.slot.text({
  id: "stage-output",
  description: "Output evidence from the git add command step.",
});

export const task = cdk.port.input.text({
  id: "task",
  description: 'Task to implement, named by its file in .internal/tasks/ — the key ("0044" or "0044.2"), the full name, or the filename.',
});

export const ownerRuling = cdk.port.input.text({
  id: "owner-ruling",
  description:
    "Owner summons transport: 'on' (default) parks the run on reviewer-raised owner questions; 'off' skips the summons branch entirely for unattended runs — escalations stay recorded in the verdict for morning review, the run proceeds.",
  optional: true,
  default: { value: "on" },
});

export const ownerGate = cdk.port.input.text({
  id: "owner-gate",
  description:
    "Owner annotation transport for review verdicts: 'annotate' (default) pipes each verdict's surface to ctx-annotate so the owner can accept, overrule, or extend it; 'off' auto-accepts for unattended runs.",
  optional: true,
  default: { value: "annotate" },
});

// The tree lane (implement:annotate): the owner's annotations over the
// working tree are the task. No task port, no plan gate, no verdict gate —
// ctx-annotate is used once, at the start.
export const annotations = cdk.slot.text({
  id: "annotations",
  description:
    "The owner's annotations from ctx-annotate's tree view over the run's working tree: the decision JSON, each entry naming a file, its lines, the exact annotated text (raw) and the owner's note (text). In the tree lane this is the task — what to change and where — and the plan's goals are cut from it. The literal 'none' when the owner closed the tree without annotating.",
});

export const planGate = cdk.port.input.text({
  id: "plan-gate",
  description:
    "Owner annotation transport for the drafted plan before round 1: 'annotate' (default) pipes the plan's surface to ctx-annotate so the owner can accept it or correct it, and the plan is redrafted with the corrections until accepted; 'off' auto-accepts for unattended runs.",
  optional: true,
  default: { value: "annotate" },
});

export const port = { task, ownerGate, ownerRuling, planGate };

export const slot = {
  draft,
  workSummary,
  changedFiles,
  diffBase,
  verdict1,
  verdict2,
  commitMessage,
  ownerDecisions,
  gitStatus,
  commitLog,
  stageOutput,
  ownerRulingMode,
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
