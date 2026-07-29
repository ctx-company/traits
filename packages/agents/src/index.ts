import type { AgentHandle, SchemaHandle } from "@ctx-traits/cdk";
import { agent, schema } from "@ctx-traits/cdk";

// NOT "./process.js": unlike @ctx-traits/cdk and @ctx-traits/config, this
// package's package.json "exports" points dev consumers straight at
// ./src/index.ts (no dist build step in the loop), so a CDK build's plain
// `node --eval` import never resolves through dist — Node only resolves a
// specifier that names a file that actually exists on disk, and dist/*.js
// is never consulted here. Verified: a "./process.js" specifier throws
// ERR_MODULE_NOT_FOUND from a real `ctx traits build` invocation.
export { commitTail, guardedProduction, reviewerVerdict } from "./process.ts";
export type {
  CommitTailOptions,
  GuardedProductionHandle,
  GuardedProductionOptions,
  GuardedProductionRole,
  ReviewerVerdictValue,
} from "./process.ts";

/**
 * Declares the shared worker role: implements a draft/design and applies reviewer fixes.
 * @param id Agent identifier.
 * @param description Trait-specific object of the work (e.g. "the draft", "the agreed refactor design").
 * @example `workerRole("worker", "Implements the draft and applies reviewer fixes.")`
 */
export function workerRole(id: string, description: string): AgentHandle {
  return agent.worker(id, { description, summary: "Implementation role." });
}

/**
 * Declares the shared scribe role: writes the commit message for a completed run.
 * @param id Agent identifier.
 * @param taskDescription What the scribe writes the message from, without the trailing git-tail clause.
 * @example `scribeRole("scribe", "Writes the commit message for the completed phase from the execution plan")`
 */
export function scribeRole(id: string, taskDescription: string): AgentHandle {
  return agent.planner(id, {
    description: `${taskDescription}; git staging and committing run as command steps.`,
    summary: "Commit-message role.",
  });
}

/**
 * Declares the shared reviewer role: drafts and/or reviews the work in a refinement loop.
 * @param id Agent identifier.
 * @param description Trait-specific scope of what this reviewer drafts and/or judges.
 * @param summary Trait-specific model-visible role label; defaults to the generic "Review role."
 * @example `reviewerRole("smart-1", "Strong model: drafts the implementation plan, and reviews the work in the refinement loop.", "Drafting and first-reviewer role.")`
 */
export function reviewerRole(id: string, description: string, summary?: string): AgentHandle {
  return agent.reviewer(id, { description, summary: summary ?? "Review role." });
}

/**
 * Declares the shared clerk role: extracts and distills context so later steps never re-read source files.
 * @param id Agent identifier.
 * @param description Trait-specific description of what this clerk extracts.
 * @example `clerkRole("clerk", "Fast extraction model: copies the phase section out of the execution plan verbatim.")`
 */
export function clerkRole(id: string, description: string): AgentHandle {
  return agent.searcher(id, { description, summary: "Context-extraction role." });
}

/**
 * Shared scope-split doctrine for the draft step of an implement loop: classify every checklist
 * item as agent-doable or owner-only BEFORE any work exists, so capability walls become a typed
 * owner-items receipt instead of a terminal blocker. Pure static text (no port/slot refs) — safe
 * to splice into any draft-step `prompt.template` source via `${SCOPE_SPLIT_DOCTRINE}`.
 */
export const SCOPE_SPLIT_DOCTRINE =
  `Open the draft with a SCOPE SPLIT section classifying EVERY checklist item and Done-when clause of the phase into exactly one of two piles. AGENT-DOABLE (the default): a competent engineer with this repository and a shell could complete and verify it here. OWNER-ONLY: no amount of in-run effort can complete it, for exactly one of these reasons — gui-or-visual (requires seeing or operating a real screen), paid-or-live-execution (requires spending money or an execution only the owner may authorize), owner-decision (requires an authority call: publishing policy, credentials, a trade-off the plan reserves to the owner), or contract-conflict (the item contradicts landed code or an authoritative rule, and resolving the contradiction is itself the owner's call). For each OWNER-ONLY item record: the item, its one reason class, one sentence why no in-run effort suffices, the SUBSTITUTE EVIDENCE the worker must produce in its place (the closest verification a shell allows — automated tests, dry runs, static checks; "none possible" only when truly nothing applies), and the CLOSE-OUT — the exact command the owner runs or decision the owner makes to finish the item. Classify honestly: an item that is merely hard, slow, or tedious is AGENT-DOABLE, and reviewers will promote any owner-only claim a shell could in fact satisfy. The split is the run's scope contract: the worker owes 100% of the agent-doable pile plus the named substitute evidence for the rest.`;

/**
 * Private paragraphs shared verbatim by `REVIEW_VERDICT_DOCTRINE` and `INTEGRITY_DOCTRINE` so the
 * two reviewer contracts cannot silently diverge: recurrence verification, phase/house-rule
 * consultation, rule-citation verification, the BLOCKER definition core, the typed blocker-report
 * format, and the status/advisory split. Not exported — compose the two public doctrines below
 * instead of splicing these directly into a `prompt.template`.
 */
const RECURRENCE_VERIFICATION =
  `If the work summary ends with a Blocker dispositions section, those ids are the blockers raised last pass: verify each disposition's quoted done-when directly with your tools FIRST; a blocker whose done-when still fails returns with recurrence-of set to its id — never silently rewritten as a new blocker.`;
const PHASE_CONTRACT_CONSULTATION =
  `                    Consult the phase contract {phaseBrief} and house rules {productBrief}, and inspect the actual working tree with your tools — never review the summary alone; run the gates named in the phase's own Definition of Done if the summary does not prove they ran.`;
const RULE_CITATION_VERIFICATION =
  `                    Before citing any house rule in a blocker, verify it against the phase's declared rule-authority: open that resource yourself with your tools, confirm the rule's source path/section and quoted line(s) in {productBrief} match the authority exactly, and drop the blocker rather than raise it if you cannot verify both — an unsourced or misquoted rule is not a rule. When a blocker does cite a house rule, set its rule-source and rule-quote fields to the verified citation; leave both unset for blockers that are not rule-based (correctness bugs, duplication, and the like).`;
const BLOCKER_DEFINITION_CORE =
  `A BLOCKER makes the work genuinely unmergeable: a correctness bug, a house-rule violation, a gate the phase's own Definition of Done requires that is failing, clear over-build (accretion, defensive validation for impossible states, scope creep), OR un-abstracted duplication — logic re-implemented or copied where existing code should have been reused or a shared abstraction extracted. Everything else — subjective style, naming, taste, optional improvements, follow-up`;
const BLOCKER_REPORT_FORMAT =
  `                    Report each blocker as a typed entry: a stable kebab-case id (reused verbatim if it returns), where (paths), what (the defect and its failure), root-cause (the missing invariant, not just where it shows), required-fix (the invariant an acceptable fix must establish and what to replace or delete — never just the cited call sites), and done-when (the falsifiable check you will apply next round). A blocker with recurrence-of set is proof the prior fix treated a symptom: state in root-cause why it was symptomatic, prescribe the structural fix in required-fix, and make clear that another point-patch at the cited sites will not clear it — for recurring duplication that means consolidate into one shared path and delete the copy, and do not approve until the duplicate is gone rather than the named case merely fixed.`;
const STATUS_ADVISORY_SPLIT =
  `                    Put blocking defects in the blockers field and non-blocking notes in advisory. Set status to revise only when blockers is non-empty; otherwise approved — advisory never blocks. Do not promote taste to a blocker to earn another round. Return the typed verdict.`;

/**
 * Shared blocker-reporting and escalation doctrine for a typed multi-reviewer refinement loop:
 * how to judge severity, report a blocker, and record owner-triage escalation. Pure static text
 * (no port/slot refs) — safe to splice into any `prompt.template` review-step source via
 * `${REVIEW_VERDICT_DOCTRINE}`, next to the review's own phase-specific opening line and
 * `{phaseBrief}`/`{productBrief}` placeholders. Composed from the private paragraphs above plus
 * this doctrine's own SCOPE SPLIT, owner-items, cross-phase-seam, and escalation machinery.
 */
export const REVIEW_VERDICT_DOCTRINE = `${RECURRENCE_VERIFICATION}
${PHASE_CONTRACT_CONSULTATION}
${RULE_CITATION_VERIFICATION}
                    Judge the work SOLELY against this phase's stated scope and Definition of Done. A phase is complete when its own DoD holds — even if the overall project does not yet build or pass whole-project gates, which belong to later phases. Do not invent acceptance criteria the phase does not state, and do not fail the work for deliverables or gates scoped to a different phase.
                    Hold the work to correctness, robustness, pragmatism, elegance, leanness, and reuse. ${BLOCKER_DEFINITION_CORE}, or gates that belong to later phases — is ADVISORY.
${BLOCKER_REPORT_FORMAT}
                    The phase contract is the standard you measure against, split by the draft's SCOPE SPLIT: withhold approval while any agent-doable checklist item or Done-when of THIS phase is unimplemented or unverifiable in the working tree, or while an owner-only item lacks the substitute evidence its class allows. Verify each item yourself with your tools; every unimplemented or partially implemented agent-doable item is a BLOCKER (one typed entry naming the item), not a note. Audit the split itself with one test: if a competent engineer with this repository and a shell — but no screen, no payment authority, and no owner authority — could complete and verify the item, it is agent-doable no matter what the draft claims: PROMOTE it and raise the blocker. You may promote owner-only to agent-doable; you may NEVER demote an item the draft classified doable. When the work summary claims a NEW wall discovered mid-run, apply the same test with heightened suspicion and accept it only with a named reason class and substitute evidence you verified. Record every accepted owner-only item in the owner-items field (item, class, reason, the substitute evidence you verified, close-out); an owner-item never justifies skipping doable work, and an owner-item whose substitute evidence is missing or unverified is a BLOCKER, not an owner-item. Cross-phase seams are the other exception: when a missing counterpart (the other side of a wire contract, the consumer of a new API, the native half of a typed bridge) is assigned by the execution plan to a DIFFERENT phase, SEARCH the execution-plan resource, confirm that phase, and judge the seam by failure-safety — if the intermediate state fails SAFELY (a typed or clearly-handled error, no data corruption, no silent wrong behavior), the seam is not a defect: cite the owning phase by id in the remaining field. Neither exception ever converts an implementable item of this phase's own checklist. Block on a seam when it fails unsafely or no other phase owns it.
                    The loop is bounded, so spend your rounds where they change the outcome. The refinement loop exits early the moment every reviewer approves; a revise verdict NEVER lands a commit — if the loop uses its full budget with blockers still open, the run PARKS instead: no commit is created, and your final verdict plus a typed park report become the durable, honest record of what is still wrong (P414). That makes your verdict a park report, not an advisory note on a landed commit: rank blockers by real consequence, state each one once and precisely, and never repeat a blocker the worker cannot act on merely to withhold approval — a blocker whose only possible resolution is an owner decision belongs in owner-items or escalation-reason, and repeating it costs a round and changes nothing. Approve when the phase's own DoD holds; withhold approval when it does not, knowing the work parks and your objections travel with it as the park report.
                    Escalation: set escalation to needs-owner if and only if the RUN AS A WHOLE cannot reach an approvable state — the phase contract is a placeholder or lacks a falsifiable Done-when (or the draft opens with CONTRACT PROBLEM and you confirm it), a prerequisite phase named in Deps has not landed in this tree, the group is marked SUPERSEDED or CANCELLED, or a contradiction poisons the entire phase leaving no separable agent-doable work. A single item outside in-run capability or authority is NOT escalation — it is an owner-item with substitute evidence, and the run still completes. Escalation flags a park for owner triage; it never lands a commit early or on its own, and your flag plus escalation-reason (one sentence naming the owner action that would clear it) are recorded on the park report. Never escalate merely because the work is large or incomplete; judge only what is in front of you. Otherwise set escalation to none and omit escalation-reason.
                    Wall citation: when the phase contract carries an explicit "**Wall:** <id>" label, copy that id verbatim into wall-id whenever you set status to revise — this is the ONLY way a park can ever refuse a sibling run citing the same wall; an id you infer from prose similarity, or a blocker you judge related without that literal label, must never populate wall-id. Leave wall-id as an empty string when the phase contract carries no such label, even if escalation is needs-owner — never a placeholder or inferred value.
${STATUS_ADVISORY_SPLIT}`;

/**
 * Lean, family-compatible integrity fragment: the unconditional part of `REVIEW_VERDICT_DOCTRINE`
 * that applies to any typed review verdict — recurrence verification, house-rule citation
 * verification, the BLOCKER definition (correctness, house-rule, required-gate, over-build,
 * duplication), the typed blocker-report format, and the status/advisory split. Deliberately
 * drops that doctrine's SCOPE SPLIT, owner-items, cross-phase-seam, and escalation machinery
 * (implement-phase-specific fields no other verdict schema exposes) and its "loop is advisory"
 * exhaustion framing (wrong for a family that blocks on exhaustion, e.g. refactor's strict variant).
 * Pure static text (no port/slot/family refs) — safe to splice into any `prompt.template` review-step
 * source via `${INTEGRITY_DOCTRINE}`, next to the review's own phase-specific opening line and
 * `{phaseBrief}`/`{productBrief}` placeholders. When a `VARIANT_DOCTRINE` fragment references
 * `REVIEW_VERDICT_DOCTRINE`, that means this composed `INTEGRITY_DOCTRINE` baseline wherever a
 * family (e.g. refactor) splices `INTEGRITY_DOCTRINE` instead of the implement-phase doctrine —
 * the two share every paragraph the fragment's rule actually depends on. Composed from the
 * private paragraphs above; carries no paragraph not already in `REVIEW_VERDICT_DOCTRINE`.
 */
export const INTEGRITY_DOCTRINE = `${RECURRENCE_VERIFICATION}
${PHASE_CONTRACT_CONSULTATION}
${RULE_CITATION_VERIFICATION}
                    ${BLOCKER_DEFINITION_CORE} — is ADVISORY.
${BLOCKER_REPORT_FORMAT}
${STATUS_ADVISORY_SPLIT}`;

/**
 * Leaner still than `INTEGRITY_DOCTRINE`: for a review step whose trait supplies no genuine
 * phase-contract or rule-authority resource to consult (e.g. a generic code-focused loop
 * reviewing its own implementation against no external plan document). Drops
 * `PHASE_CONTRACT_CONSULTATION` and `RULE_CITATION_VERIFICATION` entirely rather than binding
 * their `{phaseBrief}`/`{productBrief}` placeholders to a substitute value that isn't actually a
 * phase contract or a house-rules document — that binding is itself dishonest and tells the
 * reviewer to verify rules against something that isn't a rule-authority source. Carries no
 * paragraph not already in `INTEGRITY_DOCTRINE`. Pure static text (no port/slot/family refs) —
 * safe to splice into any `prompt.template` review-step source via `${CODE_INTEGRITY_DOCTRINE}`.
 */
export const CODE_INTEGRITY_DOCTRINE = `${RECURRENCE_VERIFICATION}
                    Inspect the actual working tree with your tools — never review the summary alone; run the gates named in the phase's own Definition of Done if the summary does not prove they ran.
                    ${BLOCKER_DEFINITION_CORE} — is ADVISORY.
${BLOCKER_REPORT_FORMAT}
${STATUS_ADVISORY_SPLIT}`;

/**
 * Shared reviewer-authority ladder: four static prompt fragments, one per variant, each stating
 * how much a reviewer may forgive or amend plan fidelity WITHOUT ever excusing a genuine defect.
 * Each fragment opens with the same precedence prefix (authored once below, not duplicated per
 * fragment): when composed alongside `REVIEW_VERDICT_DOCTRINE`, the fragment's own plan-fidelity
 * rule is authoritative over that doctrine's default completion-criteria paragraph — no separate
 * generic checklist formula is layered on top — while genuine-defect, house-rule, required-gate,
 * and duplication blocking stay unconditional exactly as `REVIEW_VERDICT_DOCTRINE` states them.
 * Pure static text (no port/slot/family refs) — safe to splice a selected fragment unchanged into
 * any review-step `prompt.template` source via `${QUICK_VARIANT_DOCTRINE}` (or
 * `DEFAULT_VARIANT_DOCTRINE`/`STRICT_VARIANT_DOCTRINE`), alongside `REVIEW_VERDICT_DOCTRINE`.
 *
 * Exported as four plain named consts, not a `VARIANT_DOCTRINE` taxonomy map keyed on variant
 * names (P450 dissolved that map): every fragment has a live non-implement second consumer
 * (`quick` — `refactor-quick`, `auto-research`; `default` — `refactor-default`; `smart` —
 * `refactor-smart`; `strict` — `refactor-strict`), so each fragment's text stays here, shared,
 * rather than being duplicated into every consumer package.
 */
const VARIANT_PRECEDENCE_PREFIX =
  `PLAN-FIDELITY AUTHORITY: this fragment's rule below is authoritative over REVIEW_VERDICT_DOCTRINE's own default completion-criteria paragraph for how strictly the plan and Definition of Done must be matched — apply this fragment's rule directly, with no separate generic checklist formula layered on top. This never touches genuine-defect, house-rule, required-gate, or duplication blocking, which stay unconditional BLOCKERs exactly as REVIEW_VERDICT_DOCTRINE requires. `;

export const QUICK_VARIANT_DOCTRINE = VARIANT_PRECEDENCE_PREFIX
  + `Quick authority: forgive a plan-fidelity gap ONLY when you record the reason it was reasonable to diverge; a genuine correctness defect is never forgivable regardless of reason, and remains a BLOCKER exactly as REVIEW_VERDICT_DOCTRINE requires. Forgiveness excuses missing or altered plan steps that changed nothing an observer could break; it never excuses a bug, a house-rule violation, or un-abstracted duplication.`;
export const DEFAULT_VARIANT_DOCTRINE = VARIANT_PRECEDENCE_PREFIX
  + `Default authority: enforce all and only the phase's declared MUST and MUST-NOT lists exactly as stated, with no forgiveness for divergence from any listed item and no invented plan requirement beyond those lists. Hold the plan to its own text, not to a stricter standard you infer — plan fidelity is judged solely by the explicit MUST/MUST-NOT lists, while any other plan content and every genuine correctness defect are judged independently under REVIEW_VERDICT_DOCTRINE regardless of what the lists do or do not declare.`;
export const SMART_VARIANT_DOCTRINE = VARIANT_PRECEDENCE_PREFIX
  + `Smart authority: when the plan and the observed work genuinely contradict, resolve the contradiction ONLY by producing a typed amendment record — the change, the contradiction it resolves, the rationale, and its provenance — which then becomes the binding contract going forward; an unrecorded departure is still a plan-fidelity gap, not an amendment. Amending fidelity never amends correctness: a genuine defect remains a BLOCKER under REVIEW_VERDICT_DOCTRINE no matter how the plan was amended.`;
export const STRICT_VARIANT_DOCTRINE = VARIANT_PRECEDENCE_PREFIX
  + `Strict authority: require verbatim execution of the plan. Record every proposed departure as a typed deviation report — the id, what was planned, what actually happened, the rationale, and its disposition — rather than silently accepting it, and ABORT when the plan itself is unsatisfiable as written rather than improvising around it. Verbatim execution governs fidelity only; a genuine correctness defect is still a BLOCKER under REVIEW_VERDICT_DOCTRINE even in a plan executed exactly as written.`;

/**
 * The blocking-defect schema shared by reviewed multi-agent refinement loops: stable identity,
 * location, root cause, the invariant an acceptable fix must establish, and the falsifiable
 * check that clears it.
 */
export const blockerSchema: SchemaHandle = schema.object(
  "blocker",
  {
    id: schema.field(schema.text(), {
      description:
        "Stable kebab-case slug for this defect, chosen on first report and reused verbatim in every later round it survives (e.g. unrelated-change-guard-baseline).",
    }),
    where: schema.field(schema.text(), {
      description: "Repo-relative files and paths involved.",
    }),
    what: schema.field(schema.text(), {
      description: "The defect and the concrete failure it causes.",
    }),
    "root-cause": schema.field(schema.text(), {
      description: "Why the code is wrong — the missing or broken invariant, not just where it shows.",
    }),
    "required-fix": schema.field(schema.text(), {
      description:
        "The shape of an acceptable fix: the invariant it must establish and what to replace or delete — never just the cited call sites.",
    }),
    "done-when": schema.field(schema.text(), {
      description: "A falsifiable check the reviewer will apply next round to declare this blocker fixed.",
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
  },
  {
    description:
      "One blocking defect: stable identity, location, root cause, the invariant an acceptable fix must establish, and the falsifiable check that clears it.",
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
        "The closest in-run verification actually produced and verified by the reviewer (tests, dry runs, static checks); \"none possible\" only when truly nothing applies.",
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
 * Shared leftover-review doctrine: the two questions a reviewer applies to
 * every worker-proposed leftover before it may enter `slot:leftovers`. Pure
 * static text (no port/slot/family refs) — safe to splice into any
 * `prompt.template` review-step source via `${LEFTOVER_DOCTRINE}`. Kept
 * separate from `INTEGRITY_DOCTRINE`/`CODE_INTEGRITY_DOCTRINE` so families
 * that do not adopt the leftover contract (refactor, auto-research) are
 * never silently bound to this second typed output.
 */
export const LEFTOVER_DOCTRINE =
  `                    The worker proposes leftovers explicitly in its work summary — including an explicit empty list when none exist; a leftover is never something you invent on the worker's behalf. Each proposal's "what" is exactly one sentence. Adjudicate every proposal with two questions, in order: first, could a competent engineer have done this work inside THIS task? If yes, it is not a leftover — it is unfinished scope, and you raise it as a BLOCKER instead. Second, does the shipped result stand alone without it? If the result does not stand alone — a caller is left broken, a contract is left half-honored, a claim is left unverified — approval is withheld until the gap closes. Only a proposal that survives both questions enters slot:leftovers, reproduced with its full typed fields (what, reason, needs, evidence, done-when). Leftovers do not change the approval verdict or become blockers. Exit is separate: an empty carried list may exit immediately; a non-empty carried list cannot exit before the configured minimum round; and every carried entry must name a prerequisite in needs, so an entry with empty needs prevents exit. Return the leftovers list as a required typed output alongside your verdict; an empty list is a valid, signed claim that none exist — never omit the field.`;

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
export const reviewVerdictSchema: SchemaHandle = schema.object(
  "review-verdict",
  {
    status: schema.field(schema.enum(["approved", "revise"] as const), {
      description:
        "approved when no blocking defect remains (advisory notes may still exist); revise only when at least one blocking defect remains. Set to approved if and only if blockers is empty.",
    }),
    blockers: schema.field(schema.list(blockerSchema), {
      description:
        "The blocking defects that must be fixed before merge: correctness bugs, house-rule violations (core purity, a new dependency, a new #[allow], required byte-stability broken), failing validation gates, clear over-build (accretion, defensive validation for states that cannot occur, scope creep beyond the phase), OR un-abstracted duplication — logic that duplicates or closely resembles code elsewhere and should be unified into a shared abstraction instead of re-implemented or copied beside. Each entry carries a stable id, root cause, the required-fix invariant, and a falsifiable done-when. Non-empty when status is revise; an empty list (never omitted — always return the key) when approved. Always present so the runtime can deterministically copy it into a park report without a missing-field failure.",
    }),
    advisory: schema.field(schema.text(), {
      required: false,
      description:
        "Non-blocking notes: subjective style, naming, taste, optional improvements, follow-up work. Never affects status and never forces another refinement round.",
    }),
    escalation: schema.field(schema.enum(["none", "needs-owner"] as const), {
      description:
        "needs-owner if and only if at least one blocker cannot be cleared by the worker from inside this run: the phase contract is a placeholder or lacks a falsifiable Done-when; a prerequisite phase named in Deps has not landed in this tree; the group is marked SUPERSEDED or CANCELLED; resolving it requires an owner/authority decision; or a required gate fails for reasons outside the phase's scope (tooling conflict). Escalation is RECORDED for owner triage — it does not stop the loop; refinement continues on every fixable blocker. none otherwise.",
    }),
    "escalation-reason": schema.field(schema.text(), {
      description:
        "One plain sentence: WHY the flagged blocker set is outside this run's authority and what owner action would clear it. Non-empty when escalation is needs-owner; an empty string (never omitted — always return the key) otherwise. Always present so the runtime can deterministically copy it into a park report without a missing-field failure.",
    }),
    "wall-id": schema.field(schema.text(), {
      description:
        "Stable wall id copied VERBATIM from an explicit \"**Wall:** <id>\" label in the phase contract, non-empty only when status is revise and that label exists — never inferred from prose similarity or blocker content. Enables cross-run standing-wall refusal (P414); an empty string here never blocks a sibling run no matter how related its blockers look. Always present (never omitted) so the runtime can deterministically copy it into a park report without a missing-field failure.",
    }),
    remaining: schema.field(schema.text(), {
      required: false,
      description:
        "Cross-phase seam citations ONLY: counterpart work the execution plan assigns to a DIFFERENT phase, each citing that phase's id. Never in-phase scope — an unimplemented item of THIS phase's checklist is a blocker, and unimplemented in-phase scope recorded here instead of in blockers falsifies the verdict. Absent when no cross-phase seam exists.",
    }),
    "owner-items": schema.field(schema.list(ownerItemSchema), {
      required: false,
      description:
        "Checklist items of THIS phase that no in-run effort can satisfy, accepted per the draft's SCOPE SPLIT (or a verified mid-run wall claim), each with its reason class, the substitute evidence you verified, and the owner close-out. Approving with this list certifies every agent-doable item is 100% implemented. Never doable work — promote any owner-only claim a shell could satisfy to a blocker; an entry with missing or unverified substitute evidence belongs in blockers, not here. Absent when every item is agent-doable.",
    }),
  },
  {
    description:
      "Typed review verdict. The loop blocks on genuine defects, clear over-build, and un-abstracted duplication — not on taste; status is revise if and only if a blocking defect remains; escalation flags run-level owner blockers for triage without stopping the loop; owner-items records the verified outside-capability remainder so the run can complete honestly.",
  },
);

/**
 * P414's typed park record for an unapproved implement-family run is
 * DELIBERATELY not a distinct schema here: a `project` step can only copy a
 * whole source value (`replace`/`append`), never assemble one field at a
 * time (the CDK's `project` step rejects `set-field`/`merge` — those exist
 * only for a prompt/command step's own `output:` sink). So each variant's
 * park-report list reuses that SAME variant's own review-verdict schema
 * (`schema.list(reviewVerdictSchema)` in `implement-default/source/
 * shared.ts`, `schema.list(reviewVerdict)` in `implement-quick/source/
 * slot.ts`) as its list-element type, and `deriveParkReportStep` (shared.ts)
 * appends the verdict slot's own accepted value onto it UNCHANGED, in full,
 * via one deterministic `project` step guarded on `status == "revise"` —
 * never a second, model-authored classification, so a park report can never
 * disagree with the verdict it comes from. Which phase a park belongs to is
 * `port:phase`, already recorded once, structurally, in every session's own
 * accepted port values — a dispatch-time preflight reads it directly rather
 * than trusting a second, potentially-divergent copy inside the verdict.
 */
