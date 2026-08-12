import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { CARGO_DIAGNOSTIC_REDUCER_SOURCE } from "../src/sequence/cargo-fix.ts";

function runReducer(ndjson: string): {
  readonly status: number | null;
  readonly stdout: string;
  readonly stderr: string;
} {
  const result = spawnSync("node", ["--eval", CARGO_DIAGNOSTIC_REDUCER_SOURCE, "--", "--stdin"], {
    input: ndjson,
    encoding: "utf8",
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

const scratchDirs: string[] = [];
afterEach(() => {
  for (const dir of scratchDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

/** A fake `cargo` on PATH that always exits 1, no output — this makes the reducer's own `fail()` surface the
 * *exact argv it invoked cargo with* on its stderr (`cmd + " " + args.join(" ") + " exited " + status`), which
 * proves what `-p` flag (if any) the `--scope=` argv parse produced, without running a real (slow) check/clippy. */
function withArgvRejectingCargo(): { readonly binDir: string } {
  const binDir = mkdtempSync(join(tmpdir(), "cargo-fix-argv-"));
  scratchDirs.push(binDir);
  writeFileSync(join(binDir, "cargo"), "#!/bin/sh\nexit 1\n", { mode: 0o755 });
  return { binDir };
}

function runReducerAgainstFakeCargo(scopeArg: string | undefined): {
  readonly status: number | null;
  readonly stderr: string;
} {
  const { binDir } = withArgvRejectingCargo();
  const config = JSON.stringify({ checkArgs: [], clippyArgs: null });
  const result = spawnSync(
    "node",
    ["--eval", CARGO_DIAGNOSTIC_REDUCER_SOURCE, "--", config, ...(scopeArg === undefined ? [] : [scopeArg])],
    { encoding: "utf8", env: { ...process.env, PATH: `${binDir}:${process.env.PATH}` } },
  );
  return { status: result.status, stderr: result.stderr };
}

describe("cargo diagnostic reducer", () => {
  it("filters to compiler-message, projects the primary span, dedupes across check+clippy, and sorts deterministically", () => {
    const lines = [
      JSON.stringify({ reason: "compiler-artifact", package_id: "noise" }),
      // Same diagnostic reported twice (once from check, once from clippy) — must dedupe to one.
      JSON.stringify({
        reason: "compiler-message",
        message: {
          message: "unused variable: `x`",
          level: "warning",
          code: { code: "unused_variables" },
          spans: [{ is_primary: true, file_name: "src/lib.rs", line_start: 10, column_start: 5 }],
        },
      }),
      JSON.stringify({
        reason: "compiler-message",
        message: {
          message: "unused variable: `x`",
          level: "warning",
          code: { code: "unused_variables" },
          spans: [{ is_primary: true, file_name: "src/lib.rs", line_start: 10, column_start: 5 }],
        },
      }),
      JSON.stringify({
        reason: "compiler-message",
        message: {
          message: "mismatched types",
          level: "error",
          code: { code: "E0308" },
          spans: [{ is_primary: true, file_name: "src/main.rs", line_start: 3, column_start: 1 }],
        },
      }),
      // No primary span: dropped, not a shape error.
      JSON.stringify({
        reason: "compiler-message",
        message: { message: "aborting due to previous error", level: "error", spans: [] },
      }),
      // No code at all, and sorts before the src/main.rs:3 entry above (line 1 < line 3): projects code to the empty string.
      JSON.stringify({
        reason: "compiler-message",
        message: {
          message: "unreachable statement",
          level: "warning",
          code: null,
          spans: [{ is_primary: true, file_name: "src/main.rs", line_start: 1, column_start: 1 }],
        },
      }),
    ];

    const result = runReducer(lines.join("\n") + "\n");

    expect(result.status).toBe(0);
    expect(JSON.parse(result.stdout)).toEqual([
      {
        file: "src/lib.rs",
        line: 10,
        column: 5,
        code: "unused_variables",
        level: "warning",
        message: "unused variable: `x`",
      },
      { file: "src/main.rs", line: 1, column: 1, code: "", level: "warning", message: "unreachable statement" },
      { file: "src/main.rs", line: 3, column: 1, code: "E0308", level: "error", message: "mismatched types" },
    ]);
  });

  it("produces byte-identical output across repeated runs of the same input", () => {
    const line = JSON.stringify({
      reason: "compiler-message",
      message: {
        message: "unused import",
        level: "warning",
        code: { code: "unused_imports" },
        spans: [{ is_primary: true, file_name: "src/a.rs", line_start: 1, column_start: 1 }],
      },
    });

    const first = runReducer(line + "\n");
    const second = runReducer(line + "\n");

    expect(first.status).toBe(0);
    expect(first.stdout).toBe(second.stdout);
  });

  it("degrades loudly (non-zero exit, stderr) on a compiler-message with no spans field", () => {
    const line = JSON.stringify({ reason: "compiler-message", message: { message: "broken", level: "error" } });

    const result = runReducer(line + "\n");

    expect(result.status).not.toBe(0);
    expect(result.stderr).toMatch(/spans/);
  });

  it("degrades loudly on a malformed code field", () => {
    const line = JSON.stringify({
      reason: "compiler-message",
      message: {
        message: "broken",
        level: "error",
        code: "not-an-object",
        spans: [{ is_primary: true, file_name: "src/a.rs", line_start: 1, column_start: 1 }],
      },
    });

    const result = runReducer(line + "\n");

    expect(result.status).not.toBe(0);
    expect(result.stderr).toMatch(/code/);
  });

  it("parses a fused --scope=<value> argv item: empty resolves to no -p, non-empty resolves to -p <value>, missing resolves to no -p", () => {
    const emptyScope = runReducerAgainstFakeCargo("--scope=");
    expect(emptyScope.status).toBe(1);
    expect(emptyScope.stderr).not.toMatch(/-p/);

    const namedScope = runReducerAgainstFakeCargo("--scope=ctx-fixture-agent");
    expect(namedScope.status).toBe(1);
    expect(namedScope.stderr).toMatch(/-p ctx-fixture-agent/);

    const missingScope = runReducerAgainstFakeCargo(undefined);
    expect(missingScope.status).toBe(1);
    expect(missingScope.stderr).not.toMatch(/-p/);
  });

  it("ignores non-compiler-message reasons entirely", () => {
    const lines = [
      JSON.stringify({ reason: "compiler-artifact" }),
      JSON.stringify({ reason: "build-script-executed" }),
    ];

    const result = runReducer(lines.join("\n") + "\n");

    expect(result.status).toBe(0);
    expect(JSON.parse(result.stdout)).toEqual([]);
  });
});
