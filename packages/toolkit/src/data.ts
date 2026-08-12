/**
 * Shared scope-split doctrine for the draft step of an implement loop: classify every checklist
 * item as agent-doable or owner-only BEFORE any work exists, so capability walls become a typed
 * owner-items receipt instead of a terminal blocker. Pure static text (no port/slot refs) — safe
 * to splice into any draft-step `input.prompt` source via `${SCOPE_SPLIT_DOCTRINE}`.
 */
export const SCOPE_SPLIT_DOCTRINE = `Open the draft with a SCOPE SPLIT section classifying EVERY checklist item and Done-when clause of the task into exactly one of two piles. AGENT-DOABLE (the default): a competent engineer with this repository and a shell could complete and verify it here. OWNER-ONLY: no amount of in-run effort can complete it, for exactly one of these reasons — gui-or-visual (requires seeing or operating a real screen), paid-or-live-execution (requires spending money or an execution only the owner may authorize), owner-decision (requires an authority call: publishing policy, credentials, a trade-off the task reserves to the owner), or contract-conflict (the item contradicts landed code or an authoritative rule, and resolving the contradiction is itself the owner's call). For each OWNER-ONLY item record: the item, its one reason class, one sentence why no in-run effort suffices, the SUBSTITUTE EVIDENCE the worker must produce in its place (the closest verification a shell allows — automated tests, dry runs, static checks; "none possible" only when truly nothing applies), and the CLOSE-OUT — the exact command the owner runs or decision the owner makes to finish the item. Classify honestly: an item that is merely hard, slow, or tedious is AGENT-DOABLE, and reviewers will promote any owner-only claim a shell could in fact satisfy. The split is the run's scope contract: the worker owes 100% of the agent-doable pile plus the named substitute evidence for the rest.`;

/**
 * Private paragraphs composed into the public doctrines below. `RECURRENCE_VERIFICATION`,
 * `BLOCKER_REPORT_FORMAT`, and `STATUS_ADVISORY_SPLIT` are shared verbatim by every doctrine so
 * the reviewer contracts cannot silently diverge on them. The consultation and blocker-definition
 * paragraphs exist in two deliberate versions: the `PHASE_`/plain forms keep the generic
 * `{phaseBrief}`/`{productBrief}` contract for families that bind their own authorities (refactor
 * binds its agreed design and architecture dialect); the `TASK_` forms are the implement-family
 * versions, where the task file from the task board is the sole contract and no separate
 * rule-authority document exists (PRODUCT.md retired 2026-07-31). Not exported — compose the
 * public doctrines below instead of splicing these directly into an `input.prompt`.
 */
const RECURRENCE_VERIFICATION = `Your own verdict from the previous round is attached as input when one exists — it is your review so far, and this round's verdict EXTENDS it rather than re-deriving it. For every carried blocker: keep its id and its steps verbatim (same order, same text); verify each open step's state directly with your tools and flip its status to done only on evidence you confirmed; append genuinely new findings as new steps at the end, or as new blockers; set recurrence-of on every carried blocker. DROP a blocker entirely once every step is done — name it in the advisory so the clearing is on record. The attached work summary is the worker's cumulative account and its claims of done are input to your verification, never a substitute for it.`;
const PHASE_CONTRACT_CONSULTATION = `                    Consult the phase contract {phaseBrief} and house rules {productBrief}, and inspect the actual working tree with your tools — never review the summary alone; run the gates named in the phase's own Definition of Done if the summary does not prove they ran.`;
const TASK_CONTRACT_CONSULTATION = `                    Consult the task contract {taskBrief} and inspect the actual working tree with your tools — never review the summary alone; run the gates named in the task's own Done-when if the summary does not prove they ran.`;
const RULE_CITATION_VERIFICATION = `                    Before citing any house rule in a blocker, verify it against the phase's declared rule-authority: open that resource yourself with your tools, confirm the rule's source path/section and quoted line(s) in {productBrief} match the authority exactly, and drop the blocker rather than raise it if you cannot verify both — an unsourced or misquoted rule is not a rule. When a blocker does cite a house rule, set its rule-source and rule-quote fields to the verified citation; leave both unset for blockers that are not rule-based (correctness bugs, duplication, and the like).`;
const BLOCKER_DEFINITION_CORE = `A BLOCKER makes the work genuinely unmergeable: a correctness bug, a house-rule violation, a gate the phase's own Definition of Done requires that is failing, clear over-build (accretion, defensive validation for impossible states, scope creep), OR un-abstracted duplication — logic re-implemented or copied where existing code should have been reused or a shared abstraction extracted. Everything else — subjective style, naming, taste, optional improvements, follow-up`;
const TASK_BLOCKER_DEFINITION_CORE = `A BLOCKER makes the work genuinely unmergeable: a correctness bug, a gate the task's own Done-when requires that is failing, clear over-build (accretion, defensive validation for impossible states, scope creep), OR un-abstracted duplication — logic re-implemented or copied where existing code should have been reused or a shared abstraction extracted. Everything else — subjective style, naming, taste, optional improvements, follow-up`;
const BLOCKER_REPORT_FORMAT = `                    Report each blocker as a typed entry: a stable kebab-case id (reused verbatim if it returns), where (paths), what (the defect and its failure), root-cause (the missing invariant, not just where it shows), required-fix (the invariant an acceptable fix must establish and what to replace or delete — never just the cited call sites), and done-when (the falsifiable check you will apply next round). A blocker with recurrence-of set is proof the prior fix treated a symptom: state in root-cause why it was symptomatic, prescribe the structural fix in required-fix, and make clear that another point-patch at the cited sites will not clear it — for recurring duplication that means consolidate into one shared path and delete the copy, and do not approve until the duplicate is gone rather than the named case merely fixed. On a recurrence, two further duties. FIRST, enumerate the COMPLETE remaining divergence now — every responsibility the shared path must still absorb — not merely the symptom that exposed it this round: a worker who satisfies a partial list meets a longer list next round and learns that complying does not clear the blocker. SECOND, the required-fix must now name symbols: the exact functions/types to create, the exact functions that must no longer exist, and the call sites to route; and done-when must include at least one command the worker can run to see the structural state red or green for itself.`;
const STATUS_ADVISORY_SPLIT = `                    Put blocking defects in the blockers field and non-blocking notes in advisory. Set status to revise only when blockers is non-empty; otherwise approved — advisory never blocks. Do not promote taste to a blocker to earn another round. Return the typed verdict.`;

/**
 * Shared blocker-reporting and escalation doctrine for the implement-family typed multi-reviewer
 * refinement loop: how to judge severity, report a blocker, and record owner-triage escalation.
 * The task file in `.internal/tasks/` is the sole contract — there is no separate rule-authority
 * document. Pure static text (no port/slot refs) — safe to splice into any `input.prompt`
 * review-step source via `${REVIEW_VERDICT_DOCTRINE}`, next to the review's own task-specific
 * opening line and `{taskBrief}` placeholder. Composed from the private paragraphs above plus
 * this doctrine's own SCOPE SPLIT, owner-items, cross-task-seam, and escalation machinery.
 */
export const REVIEW_VERDICT_DOCTRINE = `${RECURRENCE_VERIFICATION}
${TASK_CONTRACT_CONSULTATION}
                    Judge the work SOLELY against this task's stated scope and Done-when. A task is complete when its own Done-when holds — even if the overall project does not yet build or pass whole-project gates, which belong to later tasks. Do not invent acceptance criteria the task does not state, and do not fail the work for deliverables or gates scoped to a different task.
                    Hold the work to correctness, robustness, pragmatism, elegance, leanness, and reuse. ${TASK_BLOCKER_DEFINITION_CORE}, or gates that belong to later tasks — is ADVISORY.
${BLOCKER_REPORT_FORMAT}
                    The task contract is the standard you measure against, split by the draft's SCOPE SPLIT: withhold approval while any agent-doable checklist item or Done-when of THIS task is unimplemented or unverifiable in the working tree, or while an owner-only item lacks the substitute evidence its class allows. Verify each item yourself with your tools; every unimplemented or partially implemented agent-doable item is a BLOCKER (one typed entry naming the item), not a note. Audit the split itself with one test: if a competent engineer with this repository and a shell — but no screen, no payment authority, and no owner authority — could complete and verify the item, it is agent-doable no matter what the draft claims: PROMOTE it and raise the blocker. You may promote owner-only to agent-doable; you may NEVER demote an item the draft classified doable. When the work summary claims a NEW wall discovered mid-run, apply the same test with heightened suspicion and accept it only with a named reason class and substitute evidence you verified. Record every accepted owner-only item in the owner-items field (item, class, reason, the substitute evidence you verified, close-out); an owner-item never justifies skipping doable work, and an owner-item whose substitute evidence is missing or unverified is a BLOCKER, not an owner-item. Cross-task seams are the other exception: when a missing counterpart (the other side of a wire contract, the consumer of a new API, the native half of a typed bridge) belongs to a DIFFERENT task on the task board, SEARCH the task-board resource, confirm that task file, and judge the seam by failure-safety — if the intermediate state fails SAFELY (a typed or clearly-handled error, no data corruption, no silent wrong behavior), the seam is not a defect: cite the owning task by its file name in the remaining field. Neither exception ever converts an implementable item of this task's own checklist. Block on a seam when it fails unsafely or no other task owns it.
                    The loop is bounded, so spend your rounds where they change the outcome. The refinement loop exits early the moment every reviewer approves; a revise verdict NEVER lands a commit — if the loop uses its full budget with blockers still open, the run PARKS instead: no commit is created, and your final verdict plus a typed park report become the durable, honest record of what is still wrong (P414). That makes your verdict a park report, not an advisory note on a landed commit: rank blockers by real consequence, state each one once and precisely, and never repeat a blocker the worker cannot act on merely to withhold approval — a blocker whose only possible resolution is an owner decision belongs in owner-items or escalation-reason, and repeating it costs a round and changes nothing. Approve when the task's own Done-when holds; withhold approval when it does not, knowing the work parks and your objections travel with it as the park report.
                    Escalation: set escalation to needs-owner if and only if the RUN AS A WHOLE cannot reach an approvable state — the task file is a placeholder or lacks a falsifiable Done-when (or the draft opens with CONTRACT PROBLEM and you confirm it), a prerequisite task it names has not landed in this tree, the task is marked superseded or cancelled (or has been moved to the board's archived/ directory), or a contradiction poisons the entire task leaving no separable agent-doable work. A single item outside in-run capability or authority is NOT escalation — it is an owner-item with substitute evidence, and the run still completes. Escalation flags a park for owner triage; it never lands a commit early or on its own, and your flag plus escalation-reason (one sentence naming the owner action that would clear it) are recorded on the park report. Never escalate merely because the work is large or incomplete; judge only what is in front of you. Otherwise set escalation to none and omit escalation-reason.
                    Wall citation: when the task file carries an explicit "**Wall:** <id>" label, copy that id verbatim into wall-id whenever you set status to revise — this is the ONLY way a park can ever refuse a sibling run citing the same wall; an id you infer from prose similarity, or a blocker you judge related without that literal label, must never populate wall-id. Leave wall-id as an empty string when the task file carries no such label, even if escalation is needs-owner — never a placeholder or inferred value.
${STATUS_ADVISORY_SPLIT}`;

/**
 * Lean, family-compatible integrity fragment for families that bind their OWN authorities to the
 * generic `{phaseBrief}`/`{productBrief}` placeholders (refactor binds its agreed design and
 * architecture dialect): recurrence verification, rule-citation verification against the bound
 * authority, the BLOCKER definition (correctness, house-rule, required-gate, over-build,
 * duplication), the typed blocker-report format, and the status/advisory split. Deliberately
 * drops `REVIEW_VERDICT_DOCTRINE`'s SCOPE SPLIT, owner-items, cross-task-seam, and escalation
 * machinery (implement-family-specific fields no other verdict schema exposes) and its "loop is
 * advisory" exhaustion framing (wrong for a family that blocks on exhaustion, e.g. refactor's
 * strict variant). Since the 2026-07-31 task-board migration this doctrine also DIVERGES from
 * `REVIEW_VERDICT_DOCTRINE` by design: implement retired its rule-authority document, so only
 * this fragment still carries `RULE_CITATION_VERIFICATION` and the `{productBrief}` binding.
 * Pure static text (no port/slot/family refs) — safe to splice into any `input.prompt`
 * review-step source via `${INTEGRITY_DOCTRINE}`, next to the review's own opening line and
 * `{phaseBrief}`/`{productBrief}` placeholders. When a `VARIANT_DOCTRINE` fragment references
 * `REVIEW_VERDICT_DOCTRINE`, that means this composed `INTEGRITY_DOCTRINE` baseline wherever a
 * family (e.g. refactor) splices `INTEGRITY_DOCTRINE` instead of the implement-family doctrine.
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
 * safe to splice into any `input.prompt` review-step source via `${CODE_INTEGRITY_DOCTRINE}`.
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
 * any review-step `input.prompt` source via `${QUICK_VARIANT_DOCTRINE}` (or
 * `DEFAULT_VARIANT_DOCTRINE`/`STRICT_VARIANT_DOCTRINE`), alongside `REVIEW_VERDICT_DOCTRINE`.
 *
 * Exported as four plain named consts, not a `VARIANT_DOCTRINE` taxonomy map keyed on variant
 * names (P450 dissolved that map): every fragment has a live non-implement second consumer
 * (`quick` — `refactor-quick`, `auto-research`; `default` — `refactor-default`; `smart` —
 * `refactor-smart`; `strict` — `refactor-strict`), so each fragment's text stays here, shared,
 * rather than being duplicated into every consumer package.
 */
const VARIANT_PRECEDENCE_PREFIX = `PLAN-FIDELITY AUTHORITY: this fragment's rule below is authoritative over REVIEW_VERDICT_DOCTRINE's own default completion-criteria paragraph for how strictly the plan and Definition of Done must be matched — apply this fragment's rule directly, with no separate generic checklist formula layered on top. This never touches genuine-defect, house-rule, required-gate, or duplication blocking, which stay unconditional BLOCKERs exactly as REVIEW_VERDICT_DOCTRINE requires. `;

export const QUICK_VARIANT_DOCTRINE =
  VARIANT_PRECEDENCE_PREFIX +
  `Quick authority: forgive a plan-fidelity gap ONLY when you record the reason it was reasonable to diverge; a genuine correctness defect is never forgivable regardless of reason, and remains a BLOCKER exactly as REVIEW_VERDICT_DOCTRINE requires. Forgiveness excuses missing or altered plan steps that changed nothing an observer could break; it never excuses a bug, a house-rule violation, or un-abstracted duplication.`;
export const DEFAULT_VARIANT_DOCTRINE =
  VARIANT_PRECEDENCE_PREFIX +
  `Default authority: enforce all and only the plan's declared MUST and MUST-NOT lists exactly as stated, with no forgiveness for divergence from any listed item and no invented plan requirement beyond those lists. Hold the plan to its own text, not to a stricter standard you infer — plan fidelity is judged solely by the explicit MUST/MUST-NOT lists, while any other plan content and every genuine correctness defect are judged independently under REVIEW_VERDICT_DOCTRINE regardless of what the lists do or do not declare.`;
export const SMART_VARIANT_DOCTRINE =
  VARIANT_PRECEDENCE_PREFIX +
  `Smart authority: when the plan and the observed work genuinely contradict, resolve the contradiction ONLY by producing a typed amendment record — the change, the contradiction it resolves, the rationale, and its provenance — which then becomes the binding contract going forward; an unrecorded departure is still a plan-fidelity gap, not an amendment. Amending fidelity never amends correctness: a genuine defect remains a BLOCKER under REVIEW_VERDICT_DOCTRINE no matter how the plan was amended.`;
export const STRICT_VARIANT_DOCTRINE =
  VARIANT_PRECEDENCE_PREFIX +
  `Strict authority: require verbatim execution of the plan. Record every proposed departure as a typed deviation report — the id, what was planned, what actually happened, the rationale, and its disposition — rather than silently accepting it, and ABORT when the plan itself is unsatisfiable as written rather than improvising around it. Verbatim execution governs fidelity only; a genuine correctness defect is still a BLOCKER under REVIEW_VERDICT_DOCTRINE even in a plan executed exactly as written.`;

/**
 * Shared leftover-review doctrine: the two questions a reviewer applies to
 * every worker-proposed leftover before it may enter `slot:leftovers`. Pure
 * static text (no port/slot/family refs) — safe to splice into any
 * `input.prompt` review-step source via `${LEFTOVER_DOCTRINE}`. Kept
 * separate from `INTEGRITY_DOCTRINE`/`CODE_INTEGRITY_DOCTRINE` so families
 * that do not adopt the leftover contract (refactor, auto-research) are
 * never silently bound to this second typed output.
 */
export const LEFTOVER_DOCTRINE = `                    The worker proposes leftovers explicitly in its work summary — including an explicit empty list when none exist; a leftover is never something you invent on the worker's behalf. Each proposal's "what" is exactly one sentence. Adjudicate every proposal with two questions, in order: first, could a competent engineer have done this work inside THIS task? If yes, it is not a leftover — it is unfinished scope, and you raise it as a BLOCKER instead. Second, does the shipped result stand alone without it? If the result does not stand alone — a caller is left broken, a contract is left half-honored, a claim is left unverified — approval is withheld until the gap closes. Only a proposal that survives both questions enters slot:leftovers, reproduced with its full typed fields (what, reason, needs, evidence, done-when). Leftovers do not change the approval verdict or become blockers. Exit is separate: an empty carried list may exit immediately; a non-empty carried list cannot exit before the configured minimum round; and every carried entry must name a prerequisite in needs, so an entry with empty needs prevents exit. Return the leftovers list as a required typed output alongside your verdict; an empty list is a valid, signed claim that none exist — never omit the field.`;

/**
 * Static prompt doctrine for the feasibility gate's four audit angles.
 * Pure static text (no port/slot/family refs) — safe to splice into any
 * `input.prompt` review-step source via `${FEASIBILITY_DOCTRINE}`.
 */
export const FEASIBILITY_DOCTRINE = `Audit the task from exactly four angles before any build work begins. POSSIBLE: can this be done at all from inside a run — the capability, authority, and tooling a shell here actually has? BLOCKED: does it depend on something not landed? For every file, command, fixture, prerequisite API, or recipe the task references as EXISTING, verify with your own tools that it actually exists in this worktree — a task that says "extend X" where X is absent is blocked, not implementable, however small the rest of the scope looks. OVERSIZED: does the scope honestly fit one run's budget, or does it need splitting into smaller tasks first? AMBIGUOUS: does the task state a falsifiable Done-when, or would any implementation here be a guess at what "done" means? Default to feasible: raise a non-feasible verdict only on evidence you actually checked, never on suspicion or scope alone. Return the typed verdict: the tool-checked evidence, every missing thing named as its own entry (empty when feasible), and the one owner action that would clear a non-feasible verdict (empty string when feasible).`;
