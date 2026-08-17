// Deterministic command-step derivations: board numbering and the Raised
// date are facts about the repository and the clock, never agent judgment.
import { input, step } from "@ctx-traits/cdk";

import { nextKey, raisedDate } from "../data.ts";

/**
 * Next free board key, zero-padded: scans .internal/tasks/ and its archived/
 * for the highest leading NNNN, so a plan never reuses or renumbers an
 * existing key. One awk pass does everything — the [.-] field split
 * collapses dotted child keys to their parent, non-numeric first fields
 * (ls headers, blank lines, stray names) coerce to 0, and awk's numeric
 * coercion reads zero-padded keys as decimal, never octal. No command
 * substitution, so the hidden-content audit stays clean.
 */
export function nextKeyStep(): void {
  step.command("Derive next board key", {
    id: "next-key",
    input: input.command`sh -c "ls .internal/tasks .internal/tasks/archived 2>/dev/null | awk -F'[.-]' '{n=\\$1+0; if (n>m) m=n} END {printf \\"%04d\\", m+1}'"`,
    output: nextKey,
  });
}

/** Today's date for every task's Raised stamp. */
export function raisedDateStep(): void {
  step.command("Derive raised date", {
    id: "raised-date",
    input: input.command`sh -c "date +%Y-%m-%d"`,
    output: raisedDate,
  });
}
