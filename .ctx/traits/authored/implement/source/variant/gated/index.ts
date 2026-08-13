// implement-gated: quick's lean grind with the owner inside it. Same
// draft -> worker/reviewer loop -> commit spine as implement-quick, plus the
// three plannotate grafts (approved 2026-08-06): the owner annotates the plan
// in plannotator before any work starts; the owner holds a gate the machine
// reviewer cannot open — summoned only on a round the reviewer already
// approved with the repository gate green, briefed by a fresh smart frame
// grounded in the working tree; and the approved work is recorded as a
// tracked brief that ships in the same commit.
//
// Hard dependency: the third-party `plannotator` binary. The preflight step
// fails the run before any model call is spent when it is missing — the one
// doctrinal line this variant knowingly crosses (plannotate stayed a
// standalone package for exactly this reason).
//
// Owner rulings 2026-08-06: every plannotator invocation selects its port
// from a range (PORT_SELECTION below) so concurrent runs' UIs coexist on
// their own URLs instead of replacing each other; and a gated briefing the
// owner closes without deciding ("dismissed") fails the summon step and
// halts the run instead of burning another worker round.
//
// DELIBERATELY LEAN, like quick: no recurrence breaker, no earned-exit
// carry/min-rounds, no leftovers adjudication, no spliced doctrine blocks.
// The loop IS the mechanism — the reviewer says "not yet" and the worker goes
// again — and the owner's denial is just one more typed blocker for the next
// round.
import { condition, defineVariant, effect, flow, step, useBehavior, useIntent } from "@ctx-traits/cdk";

import * as shared from "#trait/shared/index.ts";

import {
  brief,
  briefDisplayOutput,
  briefMarkdown,
  briefReport,
  briefSlug,
  briefWriteOutput,
  commitMessage,
  commitOutput,
  commitReport,
  ownerDecision,
  parkReport,
  parkReportPort,
  planDecision,
  preflightOutput,
  shippingStatus,
  stageOutput,
  unstageOutput,
  verdict,
} from "./data.ts";
import * as stage from "./stage/index.ts";

// A human annotates/reads in a browser; each plannotator step's own bound
// must beat the repository's `command-idle-seconds` (a step that declares a
// bound owns it — run.rs::resolve_command_bounds).
const HUMAN_IDLE_CEILING_MS = 4 * 60 * 60 * 1000;

// Every plannotator invocation gets a port from this range (first free wins)
// unless the owner's own PLANNOTATOR_PORT is set. Without it, remote
// (SSH_TTY/SSH_CONNECTION) sessions pin plannotator's single fixed default
// port, so concurrent runs replace each other's UI instead of coexisting;
// with a range each live gate binds its own port and URL (`plannotator
// sessions` lists them). Locally plannotator already defaults to an
// ephemeral port — the range just makes URLs predictable there too.
const PORT_SELECTION = 'PLANNOTATOR_PORT="${PLANNOTATOR_PORT:-7411-7460}"';

export default function () {
  defineVariant("Gated", {
    name: "Implement (Gated)",
    description:
      "Quick's implementation loop with the owner inside it: draft the approach, have the owner annotate the plan in plannotator, then grind a single reviewer loop where every gate-green approved round summons the owner to a gated briefing — on double approval, record a tracked brief and commit.",
    metadata: { tag: ["dogfood", "implementation", "review", "lean", "human-in-the-loop", "plannotator"] },
  });

  useBehavior(shared.metadata.FAMILY_BEHAVIOR);
  useIntent(shared.intent.GATED_INTENT);

  // First step, before any prompt frame: a missing binary fails here,
  // not after a model call has already been spent.
  step.command("Confirm plannotator is installed", {
    argv: ["sh", "-c", "command -v plannotator"],
    output: preflightOutput,
  });

  stage.plan.draftStage("Draft the work");

  // A command step has no stdin channel, so piping the hook envelope
  // into plannotator rides `sh -c`; the draft reaches the shell as a
  // positional `$1` argument, never spliced into the script string.
  // Closing the plan UI without deciding cannot halt here: plannotator's
  // plan-mode hook collapses a UI exit into a plain deny ("Plan changes
  // requested"), indistinguishable from a real annotation — so a closed
  // plan window flows into plan-refine as a deny, and only the gated
  // briefing steps below carry the distinguishable "dismissed" verdict.
  step.command("Annotate the plan (plannotator, plan mode)", {
    argv: [
      "sh",
      "-c",
      `printf %s "$1" | jq -n --arg plan "$(cat)" '{hook_event_name: "PreToolUse", tool_name: "ExitPlanMode", tool_input: {plan: $plan}}' | PLANNOTATOR_ORIGIN=implement-gated ${PORT_SELECTION} plannotator`,
      "sh",
      "{slot:draft}",
    ],
    idleTimeoutMs: HUMAN_IDLE_CEILING_MS,
    output: planDecision,
  });

  stage.plan.refine("Refine the plan against the owner's annotations");

  flow.loop("Building", (loop) => {
    loop.maxIterations(10, { onExhausted: "abort" });

    stage.build.produce("Building Produce");

    shared.gateAndDiffEvidence({ gatePassed: shared.data.repoGatesPassed, diff: shared.data.reviewDiff });

    stage.build.review("Building Review");

    shared.deriveParkReportStep(verdict, { parkReportSlot: parkReport });

    flow.when("Gate Timed Out", shared.data.gateTimedOutAbortIf, flow.Abort);
    effect.onAbort(shared.data.gateTimedOut);

    // The owner is summoned only on a round the reviewer already
    // approved WITH the repository gate green — a summon on a red
    // gate would ask the owner to confirm a round the loop cannot
    // exit anyway (extends plannotate's reviewer-approved-only rule).
    flow.when(
      "Owner Gate",
      condition.all([condition.equals(verdict.status, "approved"), condition.equals(shared.data.repoGatesPassed.ok, true)]),
      () => {
        stage.build.briefing("Write the round briefing");
        // plannotator's `annotate` requires a real `.md` file path
        // (no stdin form), so the briefing goes through a transient
        // temp file created and removed inside this one step;
        // plannotator's stdout is the only stdout, so the gate JSON
        // lands in the output slot clean. A "dismissed" verdict —
        // the owner closed the briefing via Exit without deciding —
        // exits 65: the step fails and the run halts instead of
        // spinning another worker round nobody asked for; the
        // decision JSON is echoed first so the failure evidence
        // carries the verdict.
        step.command("Summon the owner (plannotator, gated)", {
          argv: [
            "sh",
            "-c",
            `d=$(mktemp -d) && printf %s "$1" > "$d/briefing.md" && ${PORT_SELECTION} plannotator annotate "$d/briefing.md" --gate --json > "$d/decision.json"; s=$?; [ -s "$d/decision.json" ] && cat "$d/decision.json"; if [ $s -eq 0 ] && grep -q '"decision":"dismissed"' "$d/decision.json"; then echo "owner closed the briefing without a decision — halting the run" >&2; s=65; fi; rm -rf "$d"; exit $s`,
            "sh",
            "{slot:owner-briefing}",
          ],
          idleTimeoutMs: HUMAN_IDLE_CEILING_MS,
          output: ownerDecision,
        });
      },
    );

    flow.until(
      condition.all([
        condition.equals(verdict.status, "approved"),
        condition.equals(shared.data.repoGatesPassed.ok, true),
        condition.equals(ownerDecision.decision, "approved"),
      ]),
    );
  });

  // The commit tail, clean-tree-guarded like quick's, with plannotate's
  // brief grafts inside the guard: the brief is written and displayed
  // only when there is something to commit, and ships in the same
  // commit as the work it describes.
  step.command("Check working tree status", {
    argv: ["git", "status", "--porcelain"],
    output: shippingStatus,
  });

  flow.when("Shipping Maybe Commit", condition.not(condition.equals(shippingStatus, "")), () => {
    stage.git.briefStage("Write the brief and commit message");
    step.project("Lift Brief", {
      projections: [
        { source: brief, field: "slug", destination: briefSlug },
        { source: brief, field: "markdown", destination: briefMarkdown },
        { source: brief, field: "commit-message", destination: commitMessage },
      ],
    });
    step.command("Save the brief to .internal/briefs/", {
      argv: [
        "sh",
        "-c",
        'mkdir -p .internal/briefs && printf %s "$1" > ".internal/briefs/$2.md"',
        "sh",
        "{slot:brief-markdown}",
        "{slot:brief-slug}",
      ],
      output: briefWriteOutput,
    });
    // No `--gate`: the brief is a record, not a decision — closing it
    // is the step's normal end, never a halt.
    step.command("Show the brief to the owner (plannotator, display only)", {
      argv: ["sh", "-c", `${PORT_SELECTION} plannotator annotate "$1"`, "sh", ".internal/briefs/{slot:brief-slug}.md"],
      include: [briefWriteOutput],
      idleTimeoutMs: HUMAN_IDLE_CEILING_MS,
      output: briefDisplayOutput,
    });
    // Two steps, not one excluding add — a pathspec that mentions a
    // gitignored `.agents` exits 1 (see familyCommitTail, run-42bd7fb2).
    // The brief under .internal/briefs/ is inside this stage — it
    // ships in the same commit as the work it describes.
    step.command("Stage all changes", {
      argv: ["git", "add", "-A"],
      output: stageOutput,
    });
    step.command("Unstage runtime state", {
      argv: ["git", "reset", "-q", "--", ".agents/runs"],
      output: unstageOutput,
    });
    step.command("Commit the work", {
      argv: ["git", "commit", "-m", "{slot:commit-message}"],
      input: [commitMessage],
      output: commitOutput,
    });
  });

  return { commitReport, parkReport: parkReportPort, briefReport };
}
