import type { AgentHandle, PortHandle, SchemaHandle, SequenceHandle } from "@ctx-traits/cdk";
import { condition, input, schema, sequence, slot } from "@ctx-traits/cdk";

/** One deduplicated cargo `check`/`clippy` diagnostic, projected from a `compiler-message`'s primary span. */
export type CargoDiagnostic = {
  readonly file: string;
  readonly line: number;
  readonly column: number;
  readonly code: string;
  readonly level: string;
  readonly message: string;
};

export const cargoDiagnosticSchema: SchemaHandle<CargoDiagnostic> = schema.object(
  "cargo-diagnostic",
  {
    file: schema.field(schema.text(), {
      description: "message.spans[].file_name of the diagnostic's primary span.",
    }),
    line: schema.field(schema.integer(), { description: "message.spans[].line_start of the primary span." }),
    column: schema.field(schema.integer(), { description: "message.spans[].column_start of the primary span." }),
    code: schema.field(schema.text(), {
      description: "message.code.code, or the empty string when cargo reports no lint/error code.",
    }),
    level: schema.field(schema.text(), { description: "message.level verbatim (e.g. \"error\", \"warning\")." }),
    message: schema.field(schema.text(), { description: "message.message verbatim." }),
  },
  {
    description:
      "One `reason == \"compiler-message\"` diagnostic from a cargo check/clippy run, deduplicated across both on (file, line, column, code) and sorted by that same key.",
  },
);

/**
 * The deterministic capture+reduce script, run via
 * `node --eval CARGO_DIAGNOSTIC_REDUCER_SOURCE -- <config-json> [--scope=<value>]`.
 * No model is ever on this path (task 0119's parsing constraint). Two runtime modes, selected by argv:
 * - Default: spawns `cargo check`/`cargo clippy --message-format json` itself, treating cargo exit 0 and 101 both
 *   as capture success (any other exit — toolchain missing, ICE — fails the step).
 * - `--stdin`: reads NDJSON from stdin instead of spawning cargo, the test seam for the reducer core.
 * Either way it prints exactly one JSON array (the reduced, sorted `CargoDiagnostic[]`) to stdout, and degrades
 * loudly (stderr + non-zero exit) on a `compiler-message` line whose shape doesn't match the pinned fields —
 * a fix loop that quietly drops diagnostics is worse than one that fails.
 */
export const CARGO_DIAGNOSTIC_REDUCER_SOURCE = `
"use strict";
const cp = require("node:child_process");
const fs = require("node:fs");

function fail(message) {
  process.stderr.write("cargo-fix: " + message + "\\n");
  process.exit(1);
}

function readCargoLines(cmd, args) {
  const result = cp.spawnSync(cmd, args, { encoding: "utf8", maxBuffer: 1024 * 1024 * 256 });
  if (result.error) fail(cmd + " " + args.join(" ") + " failed to spawn: " + result.error.message);
  if (result.status !== 0 && result.status !== 101) {
    fail(cmd + " " + args.join(" ") + " exited " + String(result.status) + ": " + String(result.stderr).slice(0, 2000));
  }
  return String(result.stdout).split("\\n").filter(Boolean);
}

function extractDiagnostic(parsed) {
  if (parsed === null || typeof parsed !== "object") { fail("compiler-message line is not an object"); }
  const inner = parsed.message;
  if (inner === null || typeof inner !== "object") { fail("compiler-message.message missing or malformed"); }
  if (typeof inner.message !== "string") { fail("compiler-message.message.message missing"); }
  if (typeof inner.level !== "string") { fail("compiler-message.message.level missing"); }
  const spans = inner.spans;
  if (!Array.isArray(spans)) { fail("compiler-message.message.spans missing or not an array"); }
  let primary = null;
  for (const span of spans) {
    if (span && span.is_primary === true) { primary = span; break; }
  }
  if (primary === null) return null;
  if (typeof primary.file_name !== "string" || typeof primary.line_start !== "number" || typeof primary.column_start !== "number") {
    fail("compiler-message primary span missing file_name/line_start/column_start");
  }
  let code = "";
  if (inner.code !== null && inner.code !== undefined) {
    if (typeof inner.code !== "object" || typeof inner.code.code !== "string") {
      fail("compiler-message.message.code malformed");
    }
    code = inner.code.code;
  }
  return {
    file: primary.file_name,
    line: primary.line_start,
    column: primary.column_start,
    code: code,
    level: inner.level,
    message: inner.message,
  };
}

function main() {
  const args = process.argv.slice(1);
  const stdinMode = args.indexOf("--stdin") !== -1;
  let lines;
  if (stdinMode) {
    lines = fs.readFileSync(0, "utf8").split("\\n").filter(Boolean);
  } else {
    const config = JSON.parse(args[0]);
    const scopeArg = args.find(function (arg) { return arg.indexOf("--scope=") === 0; });
    const scope = scopeArg === undefined ? "" : scopeArg.slice("--scope=".length);
    const scopeArgs = scope === "" ? [] : ["-p", scope];
    lines = readCargoLines("cargo", ["check", "--quiet", "--message-format", "json"].concat(config.checkArgs).concat(scopeArgs));
    if (config.clippyArgs !== null) {
      lines = lines.concat(
        readCargoLines("cargo", ["clippy", "--quiet", "--message-format", "json"].concat(config.clippyArgs).concat(scopeArgs)),
      );
    }
  }

  const deduped = new Map();
  for (const line of lines) {
    let parsed;
    try {
      parsed = JSON.parse(line);
    } catch (error) {
      fail("unparseable JSON line: " + line);
      return;
    }
    if (parsed.reason !== "compiler-message") continue;
    const diagnostic = extractDiagnostic(parsed);
    if (diagnostic === null) continue;
    const key = diagnostic.file + "\\u0000" + String(diagnostic.line) + "\\u0000" + String(diagnostic.column) + "\\u0000" + diagnostic.code;
    deduped.set(key, diagnostic);
  }

  const sorted = Array.from(deduped.values()).sort(function (a, b) {
    if (a.file !== b.file) return a.file < b.file ? -1 : 1;
    if (a.line !== b.line) return a.line - b.line;
    if (a.column !== b.column) return a.column - b.column;
    if (a.code !== b.code) return a.code < b.code ? -1 : 1;
    return 0;
  });

  process.stdout.write(JSON.stringify(sorted));
}

main();
`;

/**
 * A `port.input.text(...)` port declared elsewhere in the same trait, for `CargoFixLoopOptions.scope`. Carries
 * both the handle (so `sequence.command`'s `include:` keeps the port's own declaration reachable in the
 * canonical output) and its bare `id` (so the factory can build the fused `--scope={port:<id>}` argv text — a
 * `PortHandle` exposes no public accessor for its own id/ref text, so the id is passed alongside instead of
 * read back off the handle).
 */
export type CargoFixLoopScope = {
  readonly id: string;
  readonly port: PortHandle<string>;
};

export type CargoFixLoopOptions = {
  /** Slug prefix for every step/slot this factory invents; defaults to `"cargo-fix"`. */
  readonly id?: string;
  /** The role that reads the reduced diagnostic list and fixes it each round. */
  readonly worker: AgentHandle;
  /** Extra argv appended to `cargo check --quiet --message-format json`. */
  readonly check?: readonly string[];
  /** Extra argv appended to `cargo clippy --quiet --message-format json`, or `false` to skip clippy entirely. */
  readonly clippy?: readonly string[] | false;
  /** Fix-round budget; defaults to 4. */
  readonly rounds?: number;
  /** Default policy is errors-block/warnings-report (the surviving diagnostics list IS the warnings report). `true` widens the exit guard to require an empty list. */
  readonly denyWarnings?: boolean;
  /** Forwarded to both capture steps; defaults to 600000 (cold clippy on a real repo outruns the undeclared 120s kill). */
  readonly timeoutMs?: number;
  /** Optional runtime `-p <scope>` passed to both cargo invocations. Whole-workspace (no `-p`) when omitted or when the port resolves to `""`. */
  readonly scope?: CargoFixLoopScope;
};

/**
 * The compiler-is-the-reviewer fix loop (task 0119): capture+reduce a real `cargo check` + `cargo clippy` run into
 * a deduplicated, deterministically ordered diagnostic list, then drive `worker` through bounded fix rounds —
 * each followed by a fresh recapture — until the list is clean. A round that never clears it blocks
 * (`onExhausted: "abort"`) rather than letting an unresolved compiler diagnostic reach a commit.
 *
 * Skips the loop entirely when the initial capture is already clean: `until` is evaluated only after a loop
 * body step runs, so without this guard an already-clean repo would still spend one wasted fix frame.
 * @example
 * ```ts
 * const worker = rustWorkerRole("worker", "Fixes cargo diagnostics.");
 * sequence: [...cargoFixLoop({ worker, rounds: 4 })]
 * ```
 */
export function cargoFixLoop(options: CargoFixLoopOptions): readonly SequenceHandle[] {
  const {
    id = "cargo-fix",
    worker,
    check = [],
    clippy = [],
    rounds = 4,
    denyWarnings = false,
    timeoutMs = 600_000,
    scope,
  } = options;

  const config = JSON.stringify({ checkArgs: [...check], clippyArgs: clippy === false ? null : [...clippy] });
  const diagnostics = slot.array(cargoDiagnosticSchema, `${id}-diagnostics`);

  // Fused into ONE argv item (not a standalone `{port:scope}` element): the runtime rejects a slot/port-
  // substituted argv item that resolves to an empty string, and an empty scope ("" = whole workspace) is the
  // legal default. `--scope=` (the empty-scope substitution) stays non-empty because of the literal prefix.
  const scopeArgv = scope === undefined ? [] : [`--scope={port:${scope.id}}`];

  const captureStep = (stepId: string) =>
    sequence.command(stepId, {
      title: "Capture and reduce cargo check + clippy diagnostics",
      argv: [
        "node",
        "--eval",
        CARGO_DIAGNOSTIC_REDUCER_SOURCE.trim(),
        "--",
        config,
        ...scopeArgv,
      ],
      // Keeps the scope port's own declaration reachable in the canonical output: the fused argv string above
      // carries only its textual ref, not a link back to the `port.input.text(...)` declaration itself.
      ...(scope === undefined ? {} : { include: scope.port }),
      output: diagnostics,
      timeoutMs,
    });

  const initialCapture = captureStep(`${id}-capture`);

  const fixSummary = slot.text(`${id}-fix-summary`);
  const fixStep = sequence.prompt(`${id}-fix`, {
    title: "Fix cargo diagnostics (worker)",
    agent: worker,
    text: input.prompt`
      Fix every diagnostic in ${diagnostics}. Each carries the file, line, column, and cargo/clippy code the compiler reported, plus its level and message.
      Change only what each diagnostic requires.
      Return a short summary of what you changed.`,
    output: fixSummary,
  });
  const recaptureStep = captureStep(`${id}-recapture`);

  const clean = denyWarnings
    ? condition.empty(diagnostics)
    : condition.count(diagnostics).where("level", "error").equals(0);

  const loopStep = sequence.loop(`${id}-loop`, {
    body: [fixStep, recaptureStep],
    until: clean,
    iterations: rounds,
    onExhausted: "abort",
  });

  return [
    initialCapture,
    sequence.branch(`${id}-maybe-fix`, {
      check: condition.not(clean),
      success: [loopStep],
    }),
  ];
}
