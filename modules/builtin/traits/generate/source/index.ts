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

const generator = agent("generator", {
  description:
    "Produces one complete trait draft from a concise authoring goal, then revises it against typed round diagnostics.",
  summary: "Trait draft generator.",
});

const name = port.input.text({ id: "name", description: "Human-readable trait name." });
const brief = port.input.text({ id: "brief", description: "Goal and constraints for the new trait." });
const traitId = port.input.text({
  id: "trait-id",
  description:
    'Slugified trait ID the candidate must declare (`trait("<trait-id>", ...)`); also keys the round evaluator\'s scratch package.',
});
const designRubric = ref.resource({ id: "design-rubric", dependency: "spec" });

// --- Loop-carried state: the candidate authoring source under construction,
// its latest round evaluation, and how many rounds have run. ---
const candidateSource = slot.text({
  id: "candidate-source",
  description: "Current-round authoring source (TypeScript, @ctx-traits/cdk) for the trait under construction.",
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
  { description: "One round's rung-ladder evaluation, emitted by `ctx traits generate-round`." },
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

// A schema-typed instruction-output: the return-format instruction folds
// into the step's compiled prompt, and its slot (auto-declared at the
// step's own id) backs the first-draft candidate source below.
const produceFirstStep = sequence.prompt("produce-first", {
  title: "Draft the trait candidate (generator)",
  agent: generator,
  input: input.prompt`
        Create exactly one complete trait authoring source in TypeScript, using the @ctx-traits/cdk builder, for ${name} from ${brief}. It must declare trait("${traitId}", ...) with a non-empty procedure sequence — a candidate that compiles but declares no procedure never converges.
        Follow ${designRubric}: classify the material by what it says before choosing a shape, model dataflow only with declared ports and slots, and give every identifier a name that carries its meaning alone. Return the complete TypeScript source text only, nothing else.`,
  output: candidateSource,
});

const initRoundsSpent = sequence.project("init-rounds-spent", {
  title: "Seed the rounds-spent counter",
  projections: [{ source: operation.literal(0), destination: roundsSpent }],
});

// Evaluate step: the in-loop command rung. Invokes the CLI's single-round
// evaluator directly with the candidate source as one argv item — no
// stdin/file channel for command steps today (on record as a primitive gap,
// task 0066.1 §5), so the candidate rides one argv element.
const evaluateStep = sequence.command("evaluate-round", {
  title: "Evaluate the candidate through the rung ladder",
  argv: ["ctx", "traits", "generate-round", traitId, candidateSource],
  output: roundReport,
  // Node builds are slow; an undeclared timeout is a 120s kill.
  timeoutMs: 180_000,
});

const countRound = sequence.project("count-round", {
  title: "Advance the rounds-spent counter",
  projections: [{ source: operation.literal(1), destination: operation.over(roundsSpent, operation.Increment) }],
});

// Revise step: skipped once converged, so the candidate the loop exits on
// is exactly the one the last `evaluate-round` call scored (its scratch
// copy under `ctx_traits_io::layout::trait_protocol_root()` is what a
// non-converging run's error message preserves and reports).
const reviseStep = sequence.prompt("revise", {
  title: "Revise the candidate against its round diagnostics (generator)",
  agent: generator,
  input: input.prompt`
        Round evaluation of trait ${traitId} for ${brief} failed at rung ${roundReport}. Current candidate source: ${candidateSource}.
        Fix exactly what the diagnostics in ${roundReport} name; keep everything else. Return the complete corrected TypeScript source text only, nothing else.`,
  output: candidateSource,
});

// Guarded, not unconditional: a converged round must exit the loop with
// `candidate-source` still exactly the text `evaluate-round` scored — the
// handler's write-once-on-convergence path writes this envelope field
// straight to the real package, so it must never be a further, unevaluated
// revision.
const reviseIfNotConverged = sequence.branch("revise-if-not-converged", {
  check: condition.not(condition.fieldEquals(roundReport, "converged", true)),
  success: [reviseStep],
});

const ROUND_BOUND = 3;

const guardedLoop = sequence.loop("guarded-loop", {
  title: "Guarded generate loop",
  sequence: sequence.linear("round", [evaluateStep, countRound, reviseIfNotConverged]),
  until: condition.fieldEquals(roundReport, "converged", true),
  iterations: ROUND_BOUND,
  onExhausted: "continue",
});

// Deterministic envelope assembly — no model call, no gate re-evaluation:
// the round-evidence envelope proper is 0066.2, this stays the minimal shape
// `handle_generate` needs to guard on `converged` and report the rest.
const envelopeSchema = schema.object(
  "generate-envelope",
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
    "candidate-source": schema.field(schema.text(), { description: "The last candidate produced." }),
  },
  { description: "Terminal report the CLI handler drives its write-once/non-convergence decision from." },
);
const envelope = slot({
  id: "envelope",
  schema: envelopeSchema,
  description: "Terminal round-evidence envelope.",
});

const deriveEnvelopeScript = `
const [roundReportText, candidateSourceText, roundsSpentText, roundsBoundText] = process.argv.slice(1);
const report = JSON.parse(roundReportText);
process.stdout.write(JSON.stringify({
    converged: report.converged,
    "rounds-spent": Number(roundsSpentText),
    "rounds-bound": Number(roundsBoundText),
    "failing-rung": report.converged ? undefined : report.rung,
    diagnostics: report.diagnostics ?? [],
    "candidate-source": candidateSourceText,
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
    candidateSource,
    roundsSpent,
    String(ROUND_BOUND),
  ],
  output: envelope,
});

const envelopePort = port.output.of({
  id: "result",
  schema: ref.schema("generate-envelope"),
  description:
    "Terminal round-evidence envelope: convergence, rounds spent, failing rung, diagnostics, and the last candidate source.",
  value: envelope,
});

export default trait("generate", {
  version: "0.1.0",
  name: "Generate Trait",
  description:
    "Drives a bounded produce/evaluate loop that generates a trait candidate against the deterministic rung ladder, revising on its own typed diagnostics.",
  metadata: { tag: ["first-party", "meta-trait", "generation"] },
  dependency: dependency({
    alias: "spec",
    id: "spec",
    version: "0.1.0",
    source: { path: "../spec" },
  }),
  // `procedure(...)` has no `input`/`output` fields of its own — the
  // procedure's contract is inferred from declared ports: input ports the
  // sequence consumes, and an output port (declared here, at trait level)
  // whose `value` names the slot the sequence produces.
  port: envelopePort,
  procedure: procedure({
    description:
      "Produce a trait candidate, evaluate it through the deterministic rung ladder, and revise within a declared round bound; the handler writes only a converged result.",
    sequence: [produceFirstStep, initRoundsSpent, guardedLoop, deriveEnvelopeStep],
  }),
});
