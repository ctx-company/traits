import {
  agent,
  condition,
  dependency,
  input,
  operation,
  port,
  procedure,
  ref,
  schema,
  sequence,
  slot,
  trait,
} from "@ctx-traits/cdk";

const refiner = agent("refiner", {
  description:
    "Produces source-anchored trait refinement scaffolds, then revises them against typed round diagnostics.",
  summary: "Trait refinement agent.",
});

const source = port.input.text({ id: "source", description: "Canonical source trait text." });
const sourceDigest = port.input.text({ id: "source-digest", description: "sha256 digest of the source trait." });
const sourcePath = port.input.text({
  id: "source-path",
  description: "Filesystem path whose lines anchors must reference; also the round evaluator's re-read source.",
});
const changeRequest = port.input.text({ id: "change-request", description: "Requested refinement." });
// `refine --apply` drives the guarded loop to convergence; `refine` without
// `--apply` (preview / `--out`) still runs this trait for its one produce
// call, but must never loop or make a second provider call — `single-shot`
// is the guard that stops the loop after exactly one round regardless of
// convergence, reproducing that form's prior one-call contract (task 0066.3
// approach note: this is the one behavior-preservation risk in this slice).
const singleShot = port.input.of(schema.boolean(), {
  id: "single-shot",
  description: "Stop after exactly one produce/evaluate round regardless of convergence.",
});
const agentTraitsSchema = ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" });
const designRubric = ref.resource({ id: "design-rubric", dependency: "trait-spec" });

// --- Loop-carried state: the candidate refine scaffold under construction,
// its latest round evaluation, and how many rounds have run. ---
const candidate = slot.text({
  id: "candidate",
  description: "Current-round refine scaffold JSON text: {source-trait-id, source-digest, proposed-trait, patches}.",
});

const roundReportSchema = schema.object(
  "round-report",
  {
    rung: schema.field(schema.text(), {
      description: "The last rung evaluated: the failing rung, or the ladder's final rung once converged.",
    }),
    converged: schema.field(schema.boolean(), { description: "Whether the candidate cleared every rung." }),
    diagnostics: schema.field(schema.any(), {
      required: false,
      description: "Typed diagnostics from the failing rung; empty once converged.",
    }),
  },
  { description: "One round's rung-ladder evaluation, emitted by `ctx traits refine-round`." },
);
const roundReport = slot({
  id: "round-report",
  schema: roundReportSchema,
  description: "Latest round's rung-ladder evaluation.",
});
const roundsSpent = slot.number({
  id: "rounds-spent",
  description: "Rounds evaluated so far, including the pre-loop first draft's evaluation.",
});
// Loop guards require a local `slot:*` ref, not a `port:*` ref — this
// captures `singleShot`'s bound value into a local slot once, up front.
const singleShotFlag = slot.boolean({
  id: "single-shot-flag",
  description: "Local copy of the single-shot input port for the loop guard.",
});

const produceFirstStep = sequence.prompt("produce-first", {
  title: "Draft the refinement scaffold (refiner)",
  agent: refiner,
  input: input.prompt`
        Refine ${source} at ${sourcePath} for ${changeRequest}. Preserve source identity, use ${sourceDigest} as source evidence, and anchor every patch to real one-based inclusive lines in ${sourcePath}.
        Ground it in ${agentTraitsSchema} and ${designRubric}. Return exactly one complete refine scaffold JSON text with fields source-trait-id, source-digest, proposed-trait, and patches — nothing else.`,
  output: candidate,
});

const initRoundsSpent = sequence.project("init-rounds-spent", {
  title: "Seed the rounds-spent counter",
  projections: [{ source: operation.literal(0), destination: roundsSpent }],
});

const captureSingleShot = sequence.command("capture-single-shot", {
  title: "Capture the single-shot flag into a local slot",
  argv: ["node", "--eval", "process.stdout.write(String(process.argv[1]))", singleShot],
  output: singleShotFlag,
});

// Evaluate step: the in-loop command rung, re-reading `sourcePath` itself to
// validate scaffold anchors and identity against the live source (task
// 0066.3 §2/§3) — no build rung, the candidate is already canonical JSON.
const evaluateStep = sequence.command("evaluate-round", {
  title: "Evaluate the candidate through the rung ladder",
  argv: ["ctx", "traits", "refine-round", sourcePath, candidate],
  output: roundReport,
  timeoutMs: 180_000,
});

const countRound = sequence.project("count-round", {
  title: "Advance the rounds-spent counter",
  projections: [{ source: operation.literal(1), destination: operation.over(roundsSpent, operation.Increment) }],
});

const converged = condition.fieldEquals(roundReport, "converged", true);
const shouldStop = condition.any([converged, condition.equals(singleShotFlag, true)]);
const notConverged = condition.not(shouldStop);

const reviseStep = sequence.prompt("revise", {
  title: "Revise the candidate against its round diagnostics (refiner)",
  agent: refiner,
  input: input.prompt`
        Round evaluation of the refinement for ${changeRequest} failed at rung ${roundReport}. Current candidate scaffold: ${candidate}.
        Fix exactly what the diagnostics in ${roundReport} name; never change the trait's identity or drop its procedure — refinement edits a trait, it does not swap or gut it. Keep everything else. Return the complete corrected refine scaffold JSON text only, nothing else.`,
  output: candidate,
});

// Guarded, not unconditional: a converged (or single-shot) round must exit
// the loop with `candidate` still exactly the text `evaluate-round` scored.
const reviseIfNotConverged = sequence.branch("revise-if-not-converged", {
  check: notConverged,
  success: [reviseStep],
});

const ROUND_BOUND = 3;

const guardedLoop = sequence.loop("guarded-loop", {
  title: "Guarded refine loop",
  sequence: sequence.linear("round", [evaluateStep, countRound, reviseIfNotConverged]),
  until: shouldStop,
  iterations: ROUND_BOUND,
  onExhausted: "continue",
});

const envelopeSchema = schema.object(
  "refine-envelope",
  {
    converged: schema.field(schema.boolean(), {
      description: "Whether the loop exited because a candidate converged.",
    }),
    "rounds-spent": schema.field(schema.integer(), {
      description: "Rounds evaluated, including the first draft's evaluation.",
    }),
    "rounds-bound": schema.field(schema.integer(), {
      description: "The declared round bound the loop is guarded against.",
    }),
    "failing-rung": schema.field(schema.text(), {
      required: false,
      description: "The rung the last round failed at; absent when converged.",
    }),
    diagnostics: schema.field(schema.any(), {
      required: false,
      description: "The last round's diagnostics; empty when converged.",
    }),
    candidate: schema.field(schema.text(), { description: "The last candidate scaffold JSON text produced." }),
  },
  { description: "Terminal report the CLI handler drives its write-once/non-convergence decision from." },
);
const envelope = slot({
  id: "envelope",
  schema: envelopeSchema,
  description: "Terminal round-evidence envelope.",
});

const deriveEnvelopeScript = `
const [roundReportText, candidateText, roundsSpentText, roundsBoundText] = process.argv.slice(1);
const report = JSON.parse(roundReportText);
process.stdout.write(JSON.stringify({
    converged: report.converged,
    "rounds-spent": Number(roundsSpentText),
    "rounds-bound": Number(roundsBoundText),
    "failing-rung": report.converged ? undefined : report.rung,
    diagnostics: report.diagnostics ?? [],
    candidate: candidateText,
}));
`.trim();

const deriveEnvelopeStep = sequence.command("derive-envelope", {
  title: "Assemble the terminal round-evidence envelope",
  argv: [
    "node",
    "--input-type=module",
    "--eval",
    deriveEnvelopeScript,
    roundReport,
    candidate,
    roundsSpent,
    String(ROUND_BOUND),
  ],
  output: envelope,
});

const envelopePort = port.output.of({
  id: "result",
  schema: ref.schema("refine-envelope"),
  description:
    "Terminal round-evidence envelope: convergence, rounds spent, failing rung, diagnostics, and the last candidate scaffold.",
  value: envelope,
});

export default trait("refine-trait", {
  version: "0.1.0",
  name: "Refine Trait",
  description:
    "Drives a bounded produce/evaluate loop that refines a trait candidate against the deterministic rung ladder (with an identity rung refinement must never cross), revising on its own typed diagnostics.",
  metadata: { tag: ["first-party", "meta-trait", "refinement"] },
  dependency: dependency({
    alias: "trait-spec",
    id: "trait-spec",
    version: "0.1.0",
    source: { path: "../trait-spec" },
  }),
  port: envelopePort,
  procedure: procedure({
    description:
      "Produce a refinement scaffold, evaluate it through the deterministic rung ladder, and revise within a declared round bound; the handler writes only a converged result. `single-shot` stops after exactly one round for the non-`--apply` preview path.",
    sequence: [produceFirstStep, initRoundsSpent, captureSingleShot, guardedLoop, deriveEnvelopeStep],
  }),
});
