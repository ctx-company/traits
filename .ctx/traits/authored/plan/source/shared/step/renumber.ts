// The end-of-run key assignment: symbolic KEY<n> tokens become real board
// numbers, derived from the board AS IT IS when the plan is done — not as it
// was when the run (or its worktree) started. The model never sees numbers;
// this step never exercises judgment.
import { step } from "@ctx-traits/cdk";

import { keyMap } from "../data.ts";

/**
 * Deterministic renumber, POSIX sh (dash-safe: no bashisms, no printf
 * escapes). Why the origin lookup: a worktree's board is frozen at cut
 * time, so numbering from it races every task that lands on the origin
 * checkout while the run is live — the exact collision that motivated
 * symbolic keys. `git worktree list` names the origin checkout whether or
 * not this run is in a worktree (outside one, origin == cwd and the two
 * listings coincide); the base key is the max across BOTH boards.
 *
 * Mapping is ascending (KEY1 gets the first free number); MAX_SLICES <= 8
 * keeps tokens single-digit so no token prefixes another. Three passes,
 * strictly ordered: (1) build the full map, touching nothing; (2) apply
 * EVERY substitution to every symbolic file's content while all files
 * still carry symbolic names — a later slice's file may reference an
 * earlier key and vice versa, so contents finish before any rename;
 * (3) rename. Glob existence is probed with [ -e ] per candidate, never
 * `ls` exit codes — `ls a b` fails when either pattern is empty, which
 * would silently skip a bare-task slice that has no children. Files
 * without a KEY-prefixed name are never touched. Zero symbolic files
 * fails the step: a plan that wrote nothing is a failed plan, not a
 * quiet success.
 *
 * No command substitution anywhere — the hidden-content audit flags every
 * `$(` including arithmetic `$((`, same reason derive.ts avoids it — so
 * ALL computation (base-key max, token detection, counting, zero-padding)
 * lives in one awk program that emits "i key" pairs; sh consumes them via
 * read-in-pipeline blocks and uses only parameter expansion. The __LOCAL__
 * marker scopes KEY-token detection to this checkout's board (a leftover
 * symbolic file on the origin must never be adopted), while the numeric
 * max spans both listings.
 */
const RENUMBER_SCRIPT = `set -eu
git worktree list --porcelain 2>/dev/null | awk '/^worktree /{sub(/^worktree /,"");print;exit}' | {
  IFS= read -r main || main=.
  [ -n "$main" ] || main=.
  { ls "$main/.internal/tasks" "$main/.internal/tasks/archived" .internal/tasks .internal/tasks/archived 2>/dev/null; echo __LOCAL__; ls .internal/tasks 2>/dev/null; } | awk -F'[.-]' '
    $0 == "__LOCAL__" { in_local=1; next }
    { n=$1+0; if (n>m) m=n }
    in_local && /^KEY[1-8][.-]/ { seen[substr($0,4,1)]=1 }
    END { k=m; for (i=1;i<=8;i++) if (seen[i]) { k++; printf "%d %04d\\n", i, k } }
  ' | {
    map=''
    while read -r i key; do
      map="$map $i:$key"
    done
    [ -n "$map" ] || { echo "no symbolic KEY task files found on the board" >&2; exit 1; }
    for pair in $map; do
      i=\${pair%%:*}
      key=\${pair#*:}
      for f in .internal/tasks/KEY*.toml; do
        [ -e "$f" ] || continue
        sed "s/KEY$i/$key/g" "$f" > "$f.tmp" && mv "$f.tmp" "$f"
      done
    done
    for pair in $map; do
      i=\${pair%%:*}
      key=\${pair#*:}
      for f in .internal/tasks/KEY$i-*.toml .internal/tasks/KEY$i.*.toml; do
        [ -e "$f" ] || continue
        rest=\${f#.internal/tasks/KEY$i}
        mv "$f" ".internal/tasks/$key$rest"
      done
      echo "KEY$i -> $key"
    done
  }
}`;

export function finalKeysStep(): void {
  step.command("Assign final board keys", {
    id: "assign-keys",
    argv: ["sh", "-c", RENUMBER_SCRIPT],
    output: keyMap,
  });
}
