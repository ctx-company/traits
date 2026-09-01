// Owner narration for the basic variant (task 0273): every helper is a
// title-taking function that places `cdk.step.command` inline, so the
// notifier argv exists only in canonicals whose variant calls these —
// quick and complex stay free of it. Scripts are positional sh -c
// bodies (runtime text rides as argv data, never inside shell source),
// each wrapping a python body that inspects returncodes and enforces
// caught timeouts. Every path exits zero: notification trouble must
// never alter run success. The python bodies must contain no single
// quotes — they are embedded in single-quoted shell words.
import * as cdk from "@ctx-traits/cdk";

import { notifyId, notifyLog, reviewPoints, reviewStatus, task, verdict1 } from "../data.ts";

export const BEGIN_PY = [
  "import json, subprocess, sys",
  "try:",
  '    p = subprocess.run(["ctx-notify", "begin", "--json", "implement: " + sys.argv[1]],',
  "        capture_output=True, text=True, timeout=15, check=False)",
  "    if p.returncode != 0:",
  '        print("unavailable", end="")',
  "    else:",
  '        print(str(json.loads(p.stdout)["activity_id"]), end="")',
  "except Exception:",
  '    print("unavailable", end="")',
  "sys.exit(0)",
].join("\n");

export const UPDATE_PY = [
  "import subprocess, sys",
  "aid, status = sys.argv[1], sys.argv[2][:64]",
  'if aid == "unavailable":',
  '    print("skipped: no activity", end="")',
  "    sys.exit(0)",
  "try:",
  '    p = subprocess.run(["ctx-notify", "update", aid, "--quiet", "--status", status],',
  "        capture_output=True, text=True, timeout=15, check=False)",
  '    print(("ok: " if p.returncode == 0 else "skipped: ") + status, end="")',
  "except Exception:",
  '    print("skipped: " + status, end="")',
  "sys.exit(0)",
].join("\n");

export const REVIEW_PY = [
  "import json, subprocess, sys, time",
  "aid, status, points_json = sys.argv[1], sys.argv[2], sys.argv[3]",
  'if aid == "unavailable":',
  '    print("skipped: no activity", end="")',
  "    sys.exit(0)",
  "deadline = time.monotonic() + 60.0",
  "def left():",
  "    return max(1.0, min(15.0, deadline - time.monotonic()))",
  "def run(args):",
  "    try:",
  "        return subprocess.run(args, capture_output=True, text=True, timeout=left(), check=False).returncode",
  "    except Exception:",
  "        return -1",
  "opens = []",
  "try:",
  "    for blocker in (json.loads(points_json) or []):",
  '        for step in (blocker.get("steps") or []):',
  '            if isinstance(step, dict) and step.get("status") == "open":',
  '                opens.append(str(step.get("step"))[:200])',
  "except Exception:",
  "    opens = []",
  'badge = ("review: " + status)[:64]',
  'summary = (str(len(opens)) + " open points") if opens else ""',
  "markers = []",
  "if time.monotonic() < deadline:",
  '    rc = run(["ctx-notify", "update", aid, "--quiet", "--status", badge, "--summary", summary])',
  '    markers.append("update:" + ("ok" if rc == 0 else "skipped"))',
  "else:",
  '    markers.append("update:skipped")',
  "if opens:",
  "    if time.monotonic() < deadline:",
  '        rc = run(["ctx-notify", "log", aid] + opens)',
  '        markers.append("log:" + ("ok" if rc == 0 else "skipped") + ":" + str(len(opens)))',
  "    else:",
  '        markers.append("log:skipped:" + str(len(opens)))',
  'print(" ".join(markers), end="")',
  "sys.exit(0)",
].join("\n");

export const FINISH_PY = [
  "import subprocess, sys",
  "aid = sys.argv[1]",
  'if aid == "unavailable":',
  '    print("skipped: no activity", end="")',
  "    sys.exit(0)",
  "try:",
  '    p = subprocess.run(["ctx-notify", "finish", "--ok", "--quiet", aid],',
  "        capture_output=True, text=True, timeout=15, check=False)",
  '    print("finish: " + ("ok" if p.returncode == 0 else "skipped"), end="")',
  "except Exception:",
  '    print("finish: skipped", end="")',
  "sys.exit(0)",
].join("\n");

// The sh boundary: guard python3 itself, then hand every runtime value
// through as positional argv. A missing python3 degrades exactly like a
// missing notifier binary and still exits zero.
const wrap = (py: string, fallback: string, args: string): string =>
  [
    `command -v python3 >/dev/null 2>&1 || { printf '%s' '${fallback}'; exit 0; }`,
    `exec python3 -c '${py}' ${args}`,
  ].join("\n");

export const BEGIN_SH = wrap(BEGIN_PY, "unavailable", '"$1"');
export const UPDATE_SH = wrap(UPDATE_PY, "skipped: no python3", '"$1" "$2"');
export const REVIEW_SH = wrap(REVIEW_PY, "skipped: no python3", '"$1" "$2" "$3"');
export const FINISH_SH = wrap(FINISH_PY, "skipped: no python3", '"$1"');

export function begin(title: string): void {
  cdk.step.command(title, {
    id: "notify-begin",
    argv: ["sh", "-c", BEGIN_SH, "_", task],
    output: notifyId,
  });
}

export function update(title: string, status: string): void {
  cdk.step.command(title, {
    id: cdk.idFromTitle(title),
    argv: ["sh", "-c", UPDATE_SH, "_", notifyId, status],
    output: notifyLog,
  });
}

export function reviewUpdate(title: string): void {
  cdk.step.project(`${title}: project the verdict`, {
    id: "notify-project-verdict",
    projections: [
      { source: verdict1, field: "status", destination: reviewStatus },
      { source: verdict1, field: "blockers", destination: reviewPoints },
    ],
  });
  cdk.step.command(title, {
    id: "notify-review-update",
    argv: ["sh", "-c", REVIEW_SH, "_", notifyId, reviewStatus, reviewPoints],
    output: notifyLog,
  });
}

export function finish(title: string): void {
  cdk.step.command(title, {
    id: "notify-finish",
    argv: ["sh", "-c", FINISH_SH, "_", notifyId],
    output: notifyLog,
  });
}
