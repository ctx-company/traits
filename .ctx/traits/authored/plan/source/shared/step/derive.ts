// Deterministic command-step derivations: board numbering and the Raised
// date are facts about the repository and the clock, never agent judgment.
import { input, step } from "@ctx-traits/cdk";

import { boardSnapshot, raisedDate } from "../data.ts";

/**
 * Run-start checksum of the board listing (archived included), the
 * reference half of the pre-write guard in writeSlices.ts. A missing board
 * directory and an empty one checksum identically, so a repo planning its
 * first board passes the guard exactly like an established one.
 */
export function boardSnapshotStep(): void {
  step.command("Snapshot the board", {
    id: "board-snapshot",
    input: input.command`sh -c "ls .internal/tasks .internal/tasks/archived 2>/dev/null | sort | cksum"`,
    output: boardSnapshot,
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
