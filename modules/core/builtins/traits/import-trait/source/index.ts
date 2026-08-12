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
    "Enriches a deterministic import scaffold with source-grounded trait content, then revises it against typed round diagnostics.",
  summary: "Trait import enrichment agent.",
});

const scaffold = port.input.text({
  id: "scaffold",
  description: "Deterministic import scaffold (trait.toml text) to use as the authoritative baseline.",
});
const source = port.input.text({
  id: "source",
  description: "Raw source text (e.g. SKILL.md) to ground enrichment in.",
});
const sourceProfile = port.input.text({
  id: "source-profile",
  description: "Profile describing the source format and its known fields.",
});
const traitId = port.input.text({
  id: "trait-id",
  description:
    "Trait id the imported package must keep; also keys the round evaluator's scratch package and the persisted baseline it converges against.",
});
const targetSchema = port.input.text({
  id: "target-schema",
  description: "Target canonical schema (agent-traits/canonical-trait) the draft must conform to.",
});
const agentTraitsSchema = ref.resource({ id: "agent-traits-schema", dependency: "trait-spec" });
const designRubric = ref.resource({ id: "design-rubric", dependency: "trait-spec" });

// --- Loop-carried state: the candidate trait draft under construction, its
// latest round evaluation, and how many rounds have run. ---
const candidate = slot.text({
  id: "candidate",
  description: "Current-round trait draft JSON text: {trait: <canonical-trait-draft>}.",
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
  { description: "One round's rung-ladder evaluation, emitted by `ctx traits import-round`." },
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

const produceFirstStep = sequence.prompt("produce-first", {
  title: "Draft the trait candidate (generator)",
  agent: generator,
  input: input.prompt`
        Treat ${scaffold} as the authoritative baseline for trait ${traitId}, imported from a ${sourceProfile} source. Preserve trait identity and import provenance exactly as given in ${scaffold}. Enrich only intent, behavior, metadata, and descriptions that ${source} actually supports; never invent claims the source does not make.
        Conform to ${targetSchema}, grounding the result in ${agentTraitsSchema} and ${designRubric}. Return exactly one complete trait draft JSON text — a single trait field holding the canonical trait draft — nothing else.`,
  output: candidate,
});

const initRoundsSpent = sequence.project("init-rounds-spent", {
  title: "Seed the rounds-spent counter",
  projections: [{ source: operation.literal(0), destination: roundsSpent }],
});

// Evaluate step: the in-loop command rung, re-loading the deterministic
// scaffold baseline `handle_import` persisted to the scratch package before
// driving this loop (task 0066.3 §3) — no build rung, the candidate is
// already canonical JSON.
const evaluateStep = sequence.command("evaluate-round", {
  title: "Evaluate the candidate through the rung ladder",
  argv: ["ctx", "traits", "import-round", traitId, candidate],
  output: roundReport,
  timeoutMs: 180_000,
});

const countRound = sequence.project("count-round", {
  title: "Advance the rounds-spent counter",
  projections: [{ source: operation.literal(1), destination: operation.over(roundsSpent, operation.Increment) }],
});

const reviseStep = sequence.prompt("revise", {
  title: "Revise the candidate against its round diagnostics (generator)",
  agent: generator,
  input: input.prompt`
        Round evaluation of the import for trait ${traitId} failed at rung ${roundReport}. Current candidate draft: ${candidate}.
        Fix exactly what the diagnostics in ${roundReport} name; never invent claims ${source} does not make, and never drop a declaration the ${scaffold} baseline established — enrichment adds, it never deletes. Keep everything else. Return the complete corrected trait draft JSON text only, nothing else.`,
  output: candidate,
});

// Guarded, not unconditional: a converged round must exit the loop with
// `candidate` still exactly the text `evaluate-round` scored.
const reviseIfNotConverged = sequence.branch("revise-if-not-converged", {
  check: condition.not(condition.fieldEquals(roundReport, "converged", true)),
  success: [reviseStep],
});

const ROUND_BOUND = 3;

const guardedLoop = sequence.loop("guarded-loop", {
  title: "Guarded import loop",
  sequence: sequence.linear("round", [evaluateStep, countRound, reviseIfNotConverged]),
  until: condition.fieldEquals(roundReport, "converged", true),
  iterations: ROUND_BOUND,
  onExhausted: "continue",
});

const envelopeSchema = schema.object(
  "import-envelope",
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
    candidate: schema.field(schema.text(), { description: "The last candidate trait draft JSON text produced." }),
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
  schema: ref.schema("import-envelope"),
  description:
    "Terminal round-evidence envelope: convergence, rounds spent, failing rung, diagnostics, and the last candidate trait draft.",
  value: envelope,
});

export default trait("import-trait", {
  version: "0.1.0",
  name: "Import Trait",
  description:
    "Drives a bounded produce/evaluate loop that enriches a deterministic import scaffold with source-grounded trait content, converging toward the imported contract: builds and checks clean while remaining recognizably the scaffold baseline.",
  metadata: { tag: ["first-party", "meta-trait", "import"] },
  dependency: dependency({
    alias: "trait-spec",
    id: "trait-spec",
    version: "0.1.0",
    source: { path: "../trait-spec" },
  }),
  port: envelopePort,
  procedure: procedure({
    description:
      "Produce a trait draft grounded in the deterministic scaffold and raw source, evaluate it through the deterministic rung ladder (with a baseline-retention rung the draft must never cross), and revise within a declared round bound; the handler writes only a converged result.",
    sequence: [produceFirstStep, initRoundsSpent, guardedLoop, deriveEnvelopeStep],
  }),
});
