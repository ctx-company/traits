#!/usr/bin/env python3
"""Deterministic symbol enumeration and coverage arithmetic.

Modes:
  enumerate <closure-json-or-path>
      Print the TYPED chunk list over the closure's files: every
      fn/struct/enum/trait/mod/impl/type/class/interface definition with its
      coverage key "path:line:name", sliced into chunks of at most 12
      symbols (a chunk is one bounded describe frame's work). A closure
      path that does not exist fails loudly — closure entries are verified
      claims. Ground truth for exhaustiveness.
  coverage <chunks> <skeleton-nodes> <node-batches>
      Print the JSON coverage report: uncovered entries, unknown symbol
      keys, orphaned parents, roots, counts.
  status <chunks> <skeleton-nodes> <node-batches>
      Print exactly "complete" when every enumerated key is described and
      the tree is sound, else "incomplete:<n>".

Every payload argv element is JSON text (the runtime substitutes typed slot
values) or, as a hedge, a path to a JSON file. Same input, same output —
no clocks, no randomness, no model anywhere.
"""
import json
import os
import re
import sys


def fail(msg: str) -> None:
    print(f"symbols.py: {msg}", file=sys.stderr)
    sys.exit(1)


def load_json(arg: str, what: str):
    text = arg
    if len(arg) < 512 and os.path.exists(arg) and not arg.lstrip().startswith(("[", "{")):
        with open(arg, encoding="utf-8") as fh:
            text = fh.read()
    try:
        return json.loads(text)
    except json.JSONDecodeError as e:
        fail(f"{what}: neither JSON nor a readable JSON file: {e}")


RUST_ITEM = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r'(?:extern\s+"[^"]*"\s+)?(fn|struct|enum|trait|mod|type|static|const|union)\s+([A-Za-z_][A-Za-z0-9_]*)'
)
RUST_MACRO = re.compile(r"^\s*macro_rules!\s+([A-Za-z_][A-Za-z0-9_]*)")
RUST_IMPL = re.compile(r"^\s*impl(?:<[^>]*>)?\s+(?:.*?\bfor\s+)?([A-Za-z_][A-Za-z0-9_]*)")
TS_ITEM = re.compile(
    r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(?:async\s+)?"
    r"(function|class|interface|enum|namespace)\s+([A-Za-z_$][\w$]*)"
)
TS_TYPE = re.compile(r"^\s*(?:export\s+)?type\s+([A-Za-z_$][\w$]*)")
TS_ARROW = re.compile(
    r"^\s*(?:export\s+)?const\s+([A-Za-z_$][\w$]*)\s*(?::[^=\n]+)?=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>"
)
PY_ITEM = re.compile(r"^\s*(?:async\s+)?(def|class)\s+([A-Za-z_][A-Za-z0-9_]*)")

FUNCTION_KINDS = {"fn", "function", "def", "macro", "arrow"}


def file_symbols(path: str):
    symbols = []
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            lines = fh.read().splitlines()
    except OSError:
        return None
    ext = os.path.splitext(path)[1]
    for i, line in enumerate(lines, 1):
        stripped = line.lstrip()
        if stripped.startswith(("//", "#!")) or (ext == ".py" and stripped.startswith("#")):
            continue
        found = None
        if ext == ".rs":
            m = RUST_ITEM.match(line)
            if m:
                found = (m.group(1), m.group(2))
            else:
                m = RUST_MACRO.match(line)
                if m:
                    found = ("macro", m.group(1))
                else:
                    m = RUST_IMPL.match(line)
                    if m and "{" in line.split("//")[0]:
                        found = ("impl", m.group(1))
        elif ext in (".ts", ".tsx", ".js", ".mjs", ".jsx"):
            m = TS_ITEM.match(line)
            if m:
                found = (m.group(1), m.group(2))
            else:
                m = TS_TYPE.match(line)
                if m:
                    found = ("type", m.group(1))
                else:
                    m = TS_ARROW.match(line)
                    if m:
                        found = ("arrow", m.group(1))
        elif ext == ".py":
            m = PY_ITEM.match(line)
            if m:
                found = (m.group(1), m.group(2))
        if found:
            kind, name = found
            symbols.append(
                {
                    "key": f"{path}:{i}:{name}",
                    "kind": "function" if kind in FUNCTION_KINDS else "type",
                    "raw-kind": kind,
                    "name": name,
                    "line": i,
                }
            )
    return symbols


CHUNK_SIZE = 12


def slugify(path: str) -> str:
    out = "".join(c if c.isalnum() else "-" for c in path.lower())
    while "--" in out:
        out = out.replace("--", "-")
    return out.strip("-")


def enumerate_mode(closure_arg: str) -> None:
    closure = load_json(closure_arg, "closure")
    if not isinstance(closure, list):
        fail("closure must be a JSON list of {path, role} entries")
    seen, files, missing = set(), [], []
    for entry in closure:
        path = str(entry.get("path", "")).strip() if isinstance(entry, dict) else str(entry).strip()
        if not path or path in seen:
            continue
        seen.add(path)
        symbols = file_symbols(path)
        if symbols is None:
            missing.append(path)
        else:
            files.append({"path": path, "symbols": symbols})
    if missing:
        fail(
            "closure names files that do not exist (closure entries are verified claims): " + ", ".join(missing)
        )
    chunks = []
    for f in files:
        symbols = f["symbols"]
        parts = [symbols[i : i + CHUNK_SIZE] for i in range(0, len(symbols), CHUNK_SIZE)] or []
        for i, part in enumerate(parts, 1):
            chunks.append(
                {
                    "id": f"c-{slugify(f['path'])}-{i}",
                    "path": f["path"],
                    "part": f"{i}/{len(parts)}",
                    "symbols": [{"key": s["key"], "kind": s["kind"]} for s in part],
                }
            )
    if len(chunks) > 90:
        total = sum(len(c["symbols"]) for c in chunks)
        fail(
            f"{len(chunks)} chunks ({total} symbols) — over the run's frame budget; "
            "the closure is too wide for one walkthrough. Narrow the topic or split it, then rerun."
        )
    out = json.dumps(chunks, ensure_ascii=False, separators=(",", ":"))
    if len(out.encode("utf-8")) > 300_000:
        fail(
            f"chunk list is {len(out.encode('utf-8'))} bytes — over the command-capture safety margin; "
            "the closure is too wide for one walkthrough. Narrow the topic or split it, then rerun."
        )
    print(out)


def merged_nodes(skeleton_arg: str, batches_arg: str):
    skeleton = load_json(skeleton_arg, "skeleton-nodes")
    batches = load_json(batches_arg, "node-batches")
    if not isinstance(skeleton, list):
        fail("skeleton-nodes must be a JSON list")
    if not isinstance(batches, list):
        fail("node-batches must be a JSON list")
    flat = list(skeleton)
    for batch in batches:
        if isinstance(batch, dict) and isinstance(batch.get("nodes"), list):
            flat.extend(n for n in batch["nodes"] if isinstance(n, dict))
        elif isinstance(batch, list):
            flat.extend(n for n in batch if isinstance(n, dict))
        elif isinstance(batch, dict):
            flat.append(batch)
    by_id, order = {}, []
    for n in flat:
        nid = str(n.get("id", "")).strip()
        if not nid:
            continue
        if nid not in by_id:
            order.append(nid)
        by_id[nid] = n
    return [by_id[nid] for nid in order]


def analyze(chunks_arg: str, skeleton_arg: str, batches_arg: str):
    chunks = load_json(chunks_arg, "symbol-chunks")
    if not isinstance(chunks, list):
        fail("symbol-chunks must be a JSON list")
    nodes = merged_nodes(skeleton_arg, batches_arg)
    index_entries = []
    for c in chunks:
        index_entries.extend(c.get("symbols", []))
    index_keys = {e["key"] for e in index_entries}
    covered, unknown = set(), []
    for n in nodes:
        key = str(n.get("symbol") or "").strip()
        if not key:
            continue
        if key in index_keys:
            covered.add(key)
        else:
            unknown.append({"node": n.get("id"), "symbol": key})
    uncovered = [e for e in index_entries if e["key"] not in covered]
    ids = {n["id"] for n in nodes}
    orphans = sorted({str(n.get("parent")) for n in nodes if n.get("parent") and str(n.get("parent")) not in ids})
    roots = [n["id"] for n in nodes if not n.get("parent")]
    return {
        "uncovered": uncovered[:400],
        "uncovered-count": len(uncovered),
        "unknown-symbol-keys": unknown[:100],
        "orphan-parents": orphans,
        "roots": roots,
        "described": len(covered),
        "indexed": len(index_keys),
        "nodes": len(nodes),
    }


def main() -> None:
    if len(sys.argv) < 2:
        fail("usage: symbols.py enumerate|coverage|status <args...>")
    mode = sys.argv[1]
    if mode == "enumerate":
        if len(sys.argv) != 3:
            fail("usage: symbols.py enumerate <closure>")
        enumerate_mode(sys.argv[2])
        return
    if mode in ("coverage", "status"):
        if len(sys.argv) != 5:
            fail(f"usage: symbols.py {mode} <chunks> <skeleton> <batches>")
        report = analyze(sys.argv[2], sys.argv[3], sys.argv[4])
        if mode == "coverage":
            print(json.dumps(report, ensure_ascii=False, separators=(",", ":")))
        else:
            problems = (
                report["uncovered-count"]
                + len(report["unknown-symbol-keys"])
                + len(report["orphan-parents"])
                + (0 if len(report["roots"]) == 1 else 1)
            )
            # No trailing newline: text-slot command output is captured
            # VERBATIM, and the Covering loop's exit is a string equality
            # against exactly "complete" — a print() here made the loop
            # unsatisfiable and exhausted every fully-covered run.
            sys.stdout.write("complete" if problems == 0 else f"incomplete:{problems}")
        return
    fail(f"unknown mode {mode!r}")


main()
