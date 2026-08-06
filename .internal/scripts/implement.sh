#!/usr/bin/env bash
# Batch driver for `just implement`. Ported from implement.cr 2026-08-03:
# Crystal is not installed on CI runners, so the one proof that exercises this
# contract (proof_park_honesty::batch_halts_at_the_first_blocked_phase) could
# not run there. Bash needs no toolchain on any platform we build on, which is
# the whole reason for the port — keep it dependency-free.
#
# Written for bash 3.2, the version macOS ships: no associative arrays, no
# `mapfile`, no `${var,,}`.

set -uo pipefail

trait_id="implement:gated"

usage() {
  cat <<'USAGE'
usage: implement.sh [--trait <id>] "<task>[, <task>...]"

    -t, --trait=ID   Trait or family:variant to run (default: implement:gated)
    -h, --help       Show this help
USAGE
}

# The CLI's own exit contract, not this script's. A parked task must be
# REPORTED as parked rather than surfacing as a bare non-zero code, and the
# code is re-raised unchanged so batch callers keep the distinction.
exit_meaning() {
  case "$1" in
    3) printf 'PARKED — run ended without completing (wall park, blocked, or refusal)' ;;
    4) printf 'merge parked, branch and worktree intact' ;;
    5) printf 'merge failed' ;;
    *) printf 'ctx traits run exited %s' "$1" ;;
  esac
}

raw=""
while [ $# -gt 0 ]; do
  case "$1" in
    -t|--trait) [ $# -ge 2 ] || { echo "--trait needs a value" >&2; exit 2; }; trait_id="$2"; shift 2 ;;
    --trait=*)  trait_id="${1#--trait=}"; shift ;;
    -h|--help)  usage; exit 0 ;;
    --)         shift; raw="$raw $*"; break ;;
    *)          raw="$raw $1"; shift ;;
  esac
done

# One comma-separated operand, exactly as the Crystal version accepted it.
tasks=""
task_count=0
saved_ifs="$IFS"
IFS=','
for part in $raw; do
  trimmed="$(printf '%s' "$part" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
  [ -n "$trimmed" ] || continue
  tasks="${tasks}${trimmed}"$'\n'
  task_count=$((task_count + 1))
done
IFS="$saved_ifs"

if [ "$task_count" -eq 0 ]; then
  echo "no tasks given — see --help" >&2
  exit 2
fi

if ! command -v ctx >/dev/null 2>&1; then
  echo "cannot run \`ctx\` — is it on PATH?" >&2
  exit 127
fi

# The TUI is only meaningful on a real terminal. Piped or redirected (CI, a
# test harness, `just implement ... | tee`), it collapses to a terse summary
# and the run's park/blocked detail never reaches the log — so the mode is
# chosen from the actual stream, not assumed.
if [ -t 1 ]; then progress="tui"; else progress="stream"; fi

completed=""
while IFS= read -r task; do
  [ -n "$task" ] || continue
  echo "=== implement: $task ==="

  # stdio is inherited so `--progress tui` keeps the real terminal: capturing
  # it here to inspect output would blank the live view.
  ctx traits run "$trait_id" \
    --set "task=$task" \
    --worktree \
    --merge \
    --progress "$progress" \
    --verbose
  code=$?

  if [ "$code" -eq 0 ]; then
    completed="${completed:+$completed, }$task"
    continue
  fi

  echo "=== STOPPED at $task: $(exit_meaning "$code") (exit $code) ==="
  echo "=== completed before stop: ${completed:-none} ==="
  exit "$code"
done <<EOF
$tasks
EOF

echo "=== all tasks completed: $completed ==="
