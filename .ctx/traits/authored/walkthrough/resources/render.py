#!/usr/bin/env python3
"""Deterministic walkthrough renderer.

argv[1] — the skeleton node list: JSON text (the runtime substitutes the
          typed slot value), or, as a hedge, a path to a JSON file.
argv[2] — the node batches (list of batch objects, appended per frame),
          same JSON-or-path convention. Flattened in order and deduped by
          id, last occurrence winning (append-only revisions).
argv[3] — the horizon note text (rendered on the root node's panel).
argv[4] — repo-relative output path for the self-contained HTML.

No model output enters this file's logic: the HTML shell is fixed, the data
is embedded verbatim, and tile sizes derive from the nodes' own line spans.
No timestamps — the artifact is reproducible from the slot value alone.
"""
import json
import os
import sys


def fail(msg: str) -> None:
    print(f"render.py: {msg}", file=sys.stderr)
    sys.exit(1)


def load_nodes(arg: str):
    text = arg
    if len(arg) < 512 and os.path.exists(arg) and not arg.lstrip().startswith(("[", "{")):
        with open(arg, encoding="utf-8") as fh:
            text = fh.read()
    try:
        data = json.loads(text)
    except json.JSONDecodeError as e:
        fail(f"argv[1] is neither JSON nor a readable JSON file: {e}")
    if isinstance(data, dict) and isinstance(data.get("nodes"), list):
        data = data["nodes"]
    if not isinstance(data, list) or not data:
        fail("expected a non-empty JSON list of walkthrough nodes")
    return data


def span_lines(ref) -> int:
    lines = str(ref.get("lines", "")).strip()
    total = 0
    for part in lines.split(","):
        part = part.strip()
        if not part:
            continue
        bits = part.split("-", 1)
        try:
            start = int(bits[0])
            end = int(bits[1]) if len(bits) > 1 else start
        except ValueError:
            continue
        total += max(1, end - start + 1)
    return max(1, total)


def normalize(nodes):
    by_id, order = {}, []
    for raw in nodes:
        if not isinstance(raw, dict):
            fail("every node must be an object")
        nid = str(raw.get("id", "")).strip()
        if not nid:
            fail("a node is missing its id")
        if nid in by_id:
            fail(f"duplicate node id {nid!r}")
        node = {
            "id": nid,
            "parent": (str(raw.get("parent")).strip() if raw.get("parent") else None),
            "symbol": (str(raw.get("symbol")).strip() if raw.get("symbol") else None),
            "title": str(raw.get("title", nid)),
            "kind": str(raw.get("kind", "component")),
            "summary": str(raw.get("summary", "")),
            "explanation": str(raw.get("explanation", "")),
            "refs": [
                {"path": str(r.get("path", "")), "lines": str(r.get("lines", ""))}
                for r in (raw.get("refs") or [])
                if isinstance(r, dict)
            ],
        }
        node["own"] = sum(span_lines(r) for r in node["refs"]) or 1
        by_id[nid] = node
        order.append(node)

    roots = [n for n in order if not n["parent"]]
    for n in order:
        if n["parent"] and n["parent"] not in by_id:
            n["parent"] = None
            roots.append(n)
    if not roots:
        fail("no root node (every node has a parent) — the tree has a cycle at the top")
    if len(roots) == 1:
        root = roots[0]
    else:
        root = {
            "id": "__root",
            "parent": None,
            "title": "Walkthrough",
            "kind": "overview",
            "summary": "Synthesized root: the investigation returned several top-level areas.",
            "explanation": "",
            "refs": [],
            "own": 1,
        }
        by_id[root["id"]] = root
        for n in roots:
            n["parent"] = root["id"]
        order.insert(0, root)

    children = {n["id"]: [] for n in order}
    for n in order:
        if n["parent"]:
            children[n["parent"]].append(n["id"])

    # Cycle guard: anything not reachable from the root gets re-parented onto it.
    reachable = set()
    stack = [root["id"]]
    while stack:
        cur = stack.pop()
        if cur in reachable:
            continue
        reachable.add(cur)
        stack.extend(children[cur])
    for n in order:
        if n["id"] not in reachable:
            n["parent"] = root["id"]
            children[root["id"]].append(n["id"])
            reachable.add(n["id"])

    memo = {}

    def total(nid: str) -> int:
        if nid in memo:
            return memo[nid]
        memo[nid] = by_id[nid]["own"] + sum(total(c) for c in children[nid])
        return memo[nid]

    for n in order:
        n["total"] = total(n["id"])
        n["children"] = children[n["id"]]
    return order, root["id"]


TEMPLATE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__</title>
<style>
@import url('https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&family=IBM+Plex+Sans:wght@400;500;600&display=swap');
:root{
  --canvas:#faf9f7; --card:#fdfcfb; --surface:#f5f3f1; --surface-raised:#f3f1ef; --row-open:#f7f5f3; --button:#fdfcfb;
  --border:#f0eeea; --border-soft:#e8e5e1; --border-strong:#d3cec7; --crumb:#d9d5cf;
  --text:#454340; --text-bright:#2f2d2a; --text-heading:#262421; --text-secondary:#6a6762;
  --text-ghost:#96928c; --text-muted:#a5a099; --text-faint:#bcb8b0; --text-fact:#8c8881; --text-session:#5b5854;
  --accent:#4479db; --accent-bright:#2f66cc; --accent-dim:#a9c4f0;
  --ok:#3f9a68; --warn:#ab7f2a; --warn-dim:#ddc79e; --danger:#c96552; --review:#7a52cc; --review-dim:#c3b2e8;
  --mono:"IBM Plex Mono",ui-monospace,"SF Mono",Menlo,Consolas,monospace;
  --sans:"IBM Plex Sans",-apple-system,"Segoe UI",Helvetica,Arial,sans-serif;
}
@media (prefers-color-scheme: dark){
  :root{
    --canvas:#0c0d10; --card:#0e0f12; --surface:#14161b; --surface-raised:#16181d; --row-open:#101215; --button:#1c1e24;
    --border:#1a1c21; --border-soft:#23262d; --border-strong:#2b2e35; --crumb:#2c2f36;
    --text:#d7dae0; --text-bright:#eceef2; --text-heading:#f2f4f7; --text-secondary:#8b909c;
    --text-ghost:#7f8590; --text-muted:#585d68; --text-faint:#4d525c; --text-fact:#6b7078; --text-session:#a7abb5;
    --accent:#7fb2ff; --accent-bright:#a8ccff; --accent-dim:#5778ac;
    --ok:#7fc79a; --warn:#d8b46a; --warn-dim:#917a4a; --danger:#e08b7f; --review:#c49af0; --review-dim:#8469a2;
  }
}
:root[data-theme="light"]{
  --canvas:#faf9f7; --card:#fdfcfb; --surface:#f5f3f1; --surface-raised:#f3f1ef; --row-open:#f7f5f3; --button:#fdfcfb;
  --border:#f0eeea; --border-soft:#e8e5e1; --border-strong:#d3cec7; --crumb:#d9d5cf;
  --text:#454340; --text-bright:#2f2d2a; --text-heading:#262421; --text-secondary:#6a6762;
  --text-ghost:#96928c; --text-muted:#a5a099; --text-faint:#bcb8b0; --text-fact:#8c8881; --text-session:#5b5854;
  --accent:#4479db; --accent-bright:#2f66cc; --accent-dim:#a9c4f0;
  --ok:#3f9a68; --warn:#ab7f2a; --warn-dim:#ddc79e; --danger:#c96552; --review:#7a52cc; --review-dim:#c3b2e8;
}
:root[data-theme="dark"]{
  --canvas:#0c0d10; --card:#0e0f12; --surface:#14161b; --surface-raised:#16181d; --row-open:#101215; --button:#1c1e24;
  --border:#1a1c21; --border-soft:#23262d; --border-strong:#2b2e35; --crumb:#2c2f36;
  --text:#d7dae0; --text-bright:#eceef2; --text-heading:#f2f4f7; --text-secondary:#8b909c;
  --text-ghost:#7f8590; --text-muted:#585d68; --text-faint:#4d525c; --text-fact:#6b7078; --text-session:#a7abb5;
  --accent:#7fb2ff; --accent-bright:#a8ccff; --accent-dim:#5778ac;
  --ok:#7fc79a; --warn:#d8b46a; --warn-dim:#917a4a; --danger:#e08b7f; --review:#c49af0; --review-dim:#8469a2;
}
*{box-sizing:border-box}
html,body{margin:0;padding:0;height:100%}
body{background:var(--canvas);color:var(--text);font-family:var(--sans);font-size:14px;line-height:1.55;display:flex;flex-direction:column}
header{display:flex;align-items:center;gap:14px;padding:10px 16px;border-bottom:1px solid var(--border);flex-wrap:wrap}
.eyebrow{font-family:var(--mono);font-size:10px;font-weight:500;letter-spacing:.14em;text-transform:uppercase;color:var(--text-muted);white-space:nowrap}
#crumbs{display:flex;align-items:center;gap:6px;flex-wrap:wrap;font-family:var(--mono);font-size:12px;min-width:0}
#crumbs button{font-family:var(--mono);font-size:12px;color:var(--accent);background:none;border:none;padding:2px 2px;cursor:pointer}
#crumbs button:hover{color:var(--accent-bright);text-decoration:underline}
#crumbs .cur{color:var(--text-bright);font-weight:500}
#crumbs .sep{color:var(--crumb)}
#theme{margin-left:auto;font-family:var(--mono);font-size:11px;color:var(--text-secondary);background:var(--button);border:1px solid var(--border-soft);border-radius:4px;padding:4px 9px;cursor:pointer;white-space:nowrap}
#theme:hover{color:var(--text-bright);border-color:var(--border-strong)}
main{flex:1;display:flex;min-height:0}
#map{flex:1;position:relative;margin:12px;min-width:0}
.tile{position:absolute;background:var(--surface);border:1px solid var(--border-soft);border-radius:4px;overflow:hidden;cursor:pointer;padding:7px 9px;transition:background .12s,border-color .12s}
.tile:hover{background:var(--surface-raised);border-color:var(--border-strong)}
.tile.selected{border-color:var(--accent);box-shadow:inset 0 0 0 1px var(--accent)}
.tile .t{font-family:var(--mono);font-size:12px;font-weight:500;color:var(--text-bright);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.tile .m{font-family:var(--mono);font-size:10px;color:var(--text-muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.tile .b{position:absolute;right:7px;bottom:5px;font-family:var(--mono);font-size:9.5px;color:var(--accent)}
aside{width:420px;max-width:44vw;border-left:1px solid var(--border-soft);background:var(--card);overflow-y:auto;padding:18px 20px}
.kind{display:inline-block;font-family:var(--mono);font-size:9.5px;font-weight:500;letter-spacing:.12em;text-transform:uppercase;border:1px solid var(--border-strong);color:var(--text-secondary);border-radius:3px;padding:2px 7px;margin-bottom:10px}
.kind.k-function{color:var(--accent);border-color:var(--accent-dim)}
.kind.k-type{color:var(--review);border-color:var(--review-dim)}
.kind.k-area{color:var(--warn);border-color:var(--warn-dim)}
.kind.k-flow{color:var(--ok);border-color:var(--ok)}
.kind.k-overview{color:var(--text-heading);border-color:var(--border-strong)}
aside h2{font-family:var(--sans);font-size:16px;font-weight:600;color:var(--text-heading);margin:0 0 6px;letter-spacing:-.01em}
.sum{color:var(--text-secondary);margin:0 0 12px;font-size:13px}
.exp p{margin:0 0 10px;font-size:13.5px;color:var(--text)}
.exp code, .refs code{font-family:var(--mono);font-size:.88em;background:var(--surface);border:1px solid var(--border-soft);border-radius:3px;padding:0 4px;color:var(--text-session)}
.refs{margin:14px 0 0;padding:10px 0 0;border-top:1px solid var(--border-soft)}
.refs h3, .kids h3{font-family:var(--mono);font-size:10px;font-weight:500;text-transform:uppercase;letter-spacing:.12em;color:var(--text-muted);margin:0 0 6px}
.refs div{font-family:var(--mono);font-size:11.5px;color:var(--text-fact);padding:1px 0;word-break:break-all}
.refs div span{color:var(--text-muted)}
.kids{margin:14px 0 0;padding:10px 0 0;border-top:1px solid var(--border-soft)}
.kids button{display:block;width:100%;text-align:left;font-family:var(--mono);font-size:12px;color:var(--accent);background:none;border:none;padding:3px 0;cursor:pointer}
.kids button:hover{color:var(--accent-bright);text-decoration:underline}
.kids button span{color:var(--text-muted);font-size:10.5px}
footer{border-top:1px solid var(--border);padding:8px 16px;font-family:var(--mono);font-size:10.5px;color:var(--text-faint)}
.codes{margin:14px 0 0;padding:10px 0 0;border-top:1px solid var(--border-soft)}
.codeblk{margin:0 0 10px;border:1px solid var(--border-soft);border-radius:4px;background:var(--surface);overflow:hidden}
.codeblk .ch{display:flex;justify-content:space-between;align-items:center;gap:8px;padding:4px 8px;border-bottom:1px solid var(--border-soft);font-family:var(--mono);font-size:10.5px;color:var(--text-fact)}
.codeblk .ch span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.codeblk .ch a{color:var(--accent);text-decoration:none;margin-left:8px}
.codeblk .ch a:hover{color:var(--accent-bright);text-decoration:underline}
.codeblk pre{margin:0;padding:8px 10px;overflow:auto;max-height:340px;font-family:var(--mono);font-size:11px;line-height:1.5;color:var(--text-session)}
.codeblk .ln{color:var(--text-faint);user-select:none;display:inline-block;min-width:3.2em}
.codeblk .drift{color:var(--warn);font-family:var(--mono);font-size:10.5px;padding:4px 8px}
.conns{margin:14px 0 0;padding:10px 0 0;border-top:1px solid var(--border-soft)}
.conns .chip{display:inline-block;font-family:var(--mono);font-size:11px;color:var(--accent);border:1px solid var(--border-soft);border-radius:3px;padding:1px 7px;margin:0 6px 6px 0;cursor:pointer}
.conns .chip:hover{border-color:var(--accent);color:var(--accent-bright)}
.conns .more{color:var(--text-muted);font-family:var(--mono);font-size:10.5px}
.overlay{position:fixed;inset:0;background:var(--canvas);z-index:40;display:none;flex-direction:column}
.overlay.open{display:flex}
.overlay .obar{display:flex;align-items:center;gap:12px;padding:10px 18px;border-bottom:1px solid var(--border)}
.overlay .obar .t{font-family:var(--sans);font-size:15px;font-weight:600;color:var(--text-heading);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.overlay .obar button{margin-left:auto;font-family:var(--mono);font-size:11px;color:var(--text-secondary);background:var(--button);border:1px solid var(--border-soft);border-radius:4px;padding:4px 9px;cursor:pointer}
.overlay .obody{flex:1;overflow-y:auto;padding:22px 0}
.overlay .ocol{max-width:1020px;margin:0 auto;padding:0 24px}
.overlay .ocol .exp p{font-size:15px;line-height:1.65}
.overlay .codeblk pre{max-height:none;font-size:12.5px;line-height:1.55}
#navback,#navfwd{font-family:var(--mono);font-size:12px;color:var(--text-secondary);background:var(--button);border:1px solid var(--border-soft);border-radius:4px;padding:3px 9px;cursor:pointer}
#navback:hover,#navfwd:hover{color:var(--text-bright);border-color:var(--border-strong)}
.xbody{position:relative;display:flex;gap:26px;align-items:flex-start;max-width:1360px;margin:0 auto;padding:22px 24px 60px}
.xcol{width:250px;flex:none;display:flex;flex-direction:column;gap:12px}
.xcol .xlabel{font-family:var(--mono);font-size:10px;text-transform:uppercase;letter-spacing:.12em;color:var(--text-muted)}
.xcenter{flex:1;min-width:0}
.minicard{background:var(--card);border:1px solid var(--border-soft);border-radius:6px;padding:10px 12px;cursor:pointer}
.minicard:hover{border-color:var(--accent)}
.minicard h4{margin:2px 0 4px;font-family:var(--sans);font-size:13px;font-weight:600;color:var(--text-heading)}
.minicard p{margin:0;font-family:var(--sans);font-size:11.5px;color:var(--text-secondary);display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden}
.minicard .kind{margin-bottom:0;font-size:8.5px;padding:1px 5px}
#xedges{position:absolute;inset:0;pointer-events:none}
.conns h3[data-explore]{cursor:pointer}
.conns h3[data-explore]:hover{color:var(--accent)}
.story-col{max-width:920px;margin:0 auto;padding:8px 24px 80px}
.card{background:var(--card);border:1px solid var(--border-soft);border-radius:6px;padding:18px 20px;margin:0 0 14px}
.card.flash{border-color:var(--accent);box-shadow:0 0 0 1px var(--accent)}
.card .chead{display:flex;align-items:baseline;gap:10px;flex-wrap:wrap;margin-bottom:6px}
.card .chead h2{margin:0;font-family:var(--sans);font-size:15px;font-weight:600;color:var(--text-heading)}
.card .chead .cpos{margin-left:auto;font-family:var(--mono);font-size:10px;color:var(--text-faint)}
.card .codeblk pre{max-height:300px}
.scard{margin:34px 0 16px;padding:0 0 8px;border-bottom:1px solid var(--border-soft)}
.scard h2{margin:0 0 6px;font-family:var(--sans);font-size:19px;font-weight:600;color:var(--text-heading)}
.scard.file h2{font-size:16px}
.scard .sum{margin:0 0 8px}
.scard .exp p{font-size:13.5px}
.ego{margin:14px 0}
.ego svg{max-width:100%;height:auto;display:block}
.xref{color:var(--accent);cursor:pointer;border-bottom:1px dashed var(--accent-dim)}
.xref:hover{color:var(--accent-bright)}
#tourpos{font-family:var(--mono);font-size:11px;color:var(--text-secondary);white-space:nowrap}
@media (max-width:820px){main{flex-direction:column}aside{width:auto;max-width:none;border-left:none;border-top:1px solid var(--border-soft)}#map{min-height:340px}}
</style>
</head>
<body>
<header>
  <span class="eyebrow">ctx walkthrough</span>
  <nav id="crumbs" aria-label="Path"></nav>
  <button id="navback" aria-label="Back" title="back">‹</button>
  <button id="navfwd" aria-label="Forward" title="forward">›</button>
  <span id="tourpos" aria-live="polite"></span>
  <button id="theme" aria-label="Cycle color theme">theme: auto</button>
</header>
<main>
  <div id="map" role="tree" aria-label="Treemap"></div>
  <aside id="panel"></aside>
</main>
<div class="overlay" id="reader" role="dialog" aria-label="Reader">
  <div class="obar"><span class="kind" id="rkind"></span><span class="t" id="rtitle"></span><span id="rpos" style="font-family:var(--mono);font-size:11px;color:var(--text-secondary)"></span><button id="rclose">esc closes</button></div>
  <div class="obody"><div class="ocol" id="rbody"></div></div>
</div>
<div class="overlay" id="story" role="dialog" aria-label="Narration">
  <div class="obar"><span class="t" id="stitle">narration — reading order</span><button id="sclose">esc closes</button></div>
  <div class="obody" id="sbodywrap"><div class="story-col" id="sbody"></div></div>
</div>
<div class="overlay" id="explore" role="dialog" aria-label="Card explorer">
  <div class="obar"><span class="t">explorer — a card and its relationships</span><button id="xclose">esc closes</button></div>
  <div class="obody"><div class="xbody" id="xbody"><svg id="xedges"></svg></div></div>
</div>
<div class="overlay" id="graph" role="dialog" aria-label="File graph">
  <div class="obar"><span class="t">file connections — mechanical name-matches inside described spans</span><button id="gclose">esc closes</button></div>
  <div class="obody"><div class="ocol" id="gbody"></div></div>
</div>
<footer>click a tile to zoom in; Esc goes up · j/k walk the tour · Enter reader · n narration · e card explorer · g file graph · ‹/› or browser back-forward retrace your path · connections are name-matches, not call analysis · code read from the repo at render time</footer>
<script>
var DATA = __DATA__;
var HORIZON = __HORIZON__;
var REPOROOT = __REPOROOT__;
(function(){
  var byId = {};
  DATA.forEach(function(n){ byId[n.id] = n; });
  var ROOT = "__ROOT__";
  var focusId = ROOT, selectedId = ROOT;

  function esc(s){
    return String(s).replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;").replace(/"/g,"&quot;");
  }
  var NAMEIDX = {};
  DATA.forEach(function(n){
    if (n.symbol){ NAMEIDX[n.symbol.split(":").pop()] = n.id; }
  });
  DATA.forEach(function(n){
    var m = n.title && n.title.match(/^[A-Za-z_][A-Za-z0-9_]*$/);
    if (m && !NAMEIDX[n.title]) NAMEIDX[n.title] = n.id;
  });
  var USEDBY = {};
  DATA.forEach(function(n){
    (n.uses || []).forEach(function(t){ (USEDBY[t] = USEDBY[t] || []).push(n.id); });
  });
  function prose(s, selfId){
    var parts = String(s).split(/\\n\\s*\\n/);
    var html = "";
    for (var i=0;i<parts.length;i++){
      if (!parts[i].trim()) continue;
      var p = esc(parts[i].trim()).replace(/`([^`]+)`/g, function(_, c){
        var tid = NAMEIDX[c];
        if (tid && tid !== selfId) return "<code class=\\"xref\\" data-id=\\"" + tid + "\\">" + c + "</code>";
        return "<code>" + c + "</code>";
      });
      html += "<p>" + p + "</p>";
    }
    return html;
  }

  function squarify(items, x, y, w, h, out){
    if (!items.length) return;
    var total = 0;
    items.forEach(function(it){ total += it.weight; });
    if (total <= 0) return;
    var scale = (w * h) / total;
    var row = [], rest = items.slice(), rx = x, ry = y, rw = w, rh = h;
    function worst(row, side){
      var s = 0, min = Infinity, max = 0;
      row.forEach(function(it){ var a = it.weight * scale; s += a; if (a < min) min = a; if (a > max) max = a; });
      var s2 = s * s, side2 = side * side;
      return Math.max((side2 * max) / s2, s2 / (side2 * min));
    }
    function layoutRow(row){
      var s = 0;
      row.forEach(function(it){ s += it.weight * scale; });
      var horiz = rw >= rh;
      var side = horiz ? rh : rw;
      var thick = s / side;
      var off = 0;
      row.forEach(function(it){
        var len = (it.weight * scale) / thick;
        if (horiz) out.push({ id: it.id, x: rx, y: ry + off, w: thick, h: len });
        else out.push({ id: it.id, x: rx + off, y: ry, w: len, h: thick });
        off += len;
      });
      if (horiz){ rx += thick; rw -= thick; } else { ry += thick; rh -= thick; }
    }
    while (rest.length){
      var it = rest[0];
      var side = Math.min(rw, rh);
      if (!row.length || worst(row.concat([it]), side) <= worst(row, side)){
        row.push(rest.shift());
      } else {
        layoutRow(row); row = [];
      }
    }
    if (row.length) layoutRow(row);
  }

  function renderMap(){
    var map = document.getElementById("map");
    map.innerHTML = "";
    var focus = byId[focusId];
    var kids = focus.children.map(function(cid){ return { id: cid, weight: byId[cid].total }; });
    if (!kids.length){
      var note = document.createElement("div");
      note.style.cssText = "position:absolute;inset:0;display:flex;align-items:center;justify-content:center;color:var(--text-muted);font-family:var(--mono);font-size:12px";
      note.textContent = "leaf node — the panel on the right is the whole story";
      map.appendChild(note);
      return;
    }
    var W = map.clientWidth, H = map.clientHeight;
    var rects = [];
    squarify(kids.slice().sort(function(a,b){ return b.weight - a.weight; }), 0, 0, W, H, rects);
    var totalW = 0;
    kids.forEach(function(k){ totalW += k.weight; });
    rects.forEach(function(r){
      var n = byId[r.id];
      var el = document.createElement("div");
      el.className = "tile" + (r.id === selectedId ? " selected" : "");
      el.style.left = (r.x + 2) + "px";
      el.style.top = (r.y + 2) + "px";
      el.style.width = Math.max(2, r.w - 4) + "px";
      el.style.height = Math.max(2, r.h - 4) + "px";
      el.setAttribute("role", "treeitem");
      el.setAttribute("title", n.title);
      var pct = Math.round((n.total / totalW) * 100);
      var inner = "<div class=\\"t\\">" + esc(n.title) + "</div>" +
                  "<div class=\\"m\\">" + n.kind + " · " + pct + "% · ~" + n.total + " lines</div>";
      if (n.children.length) inner += "<div class=\\"b\\">" + n.children.length + " ▸</div>";
      el.innerHTML = inner;
      el.addEventListener("click", function(){
        if (n.children.length){ focusId = n.id; selectedId = n.id; }
        else { selectedId = n.id; }
        renderAll();
      });
      map.appendChild(el);
    });
  }

  function renderCrumbs(){
    var el = document.getElementById("crumbs");
    el.innerHTML = "";
    var path = [], cur = byId[focusId];
    while (cur){ path.unshift(cur); cur = cur.parent ? byId[cur.parent] : null; }
    path.forEach(function(n, i){
      if (i){ var s = document.createElement("span"); s.className = "sep"; s.textContent = "/"; el.appendChild(s); }
      if (i === path.length - 1){
        var c = document.createElement("span"); c.className = "cur"; c.textContent = n.title; el.appendChild(c);
      } else {
        var b = document.createElement("button"); b.textContent = n.title;
        b.addEventListener("click", function(){ focusId = n.id; selectedId = n.id; renderAll(); });
        el.appendChild(b);
      }
    });
  }

  function editorLinks(path, start){
    var abs = REPOROOT + "/" + path;
    return "<a href=\\"vscode://file" + abs + ":" + start + "\\">code</a>" +
           "<a href=\\"zed://file" + abs + ":" + start + "\\">zed</a>";
  }
  function refStart(r){
    var m = String(r.lines).match(/^(\\d+)/);
    return m ? m[1] : "1";
  }
  function codeBlocksHtml(n){
    if (!n.code || !n.code.length) return "";
    var html = "<div class=\\"codes\\"><h3>source</h3>";
    n.code.forEach(function(c){
      html += "<div class=\\"codeblk\\"><div class=\\"ch\\"><span>" + esc(c.path) + ":" + esc(c.lines) + "</span><span>" + editorLinks(c.path, c.start) + "</span></div>";
      if (c.drifted) html += "<div class=\\"drift\\">" + esc(c.drifted) + "</div>";
      if (c.text){
        var out = "";
        c.text.split("\\n").forEach(function(line, i){
          out += "<span class=\\"ln\\">" + (c.start + i) + "</span>" + esc(line) + "\\n";
        });
        html += "<pre>" + out + "</pre>";
      }
      html += "</div>";
    });
    return html + "</div>";
  }
  function chipList(ids, cap){
    var html = "";
    ids.slice(0, cap).forEach(function(id){
      var t = byId[id];
      if (t) html += "<span class=\\"chip\\" data-id=\\"" + id + "\\">" + esc(t.title) + "</span>";
    });
    if (ids.length > cap) html += "<span class=\\"more\\">+" + (ids.length - cap) + " more</span>";
    return html;
  }
  function connsHtml(n){
    var uses = n.uses || [], usedBy = USEDBY[n.id] || [];
    if (!uses.length && !usedBy.length) return "";
    var html = "<div class=\\"conns\\"><h3 data-explore=\\"1\\" title=\\"open card explorer\\">connections · name-matches ⤢</h3>";
    if (uses.length) html += "<div><span class=\\"more\\">uses → </span>" + chipList(uses, 14) + "</div>";
    if (usedBy.length) html += "<div><span class=\\"more\\">← used by </span>" + chipList(usedBy, 14) + "</div>";
    return html + "</div>";
  }
  function egoSvg(n){
    var ins = (USEDBY[n.id] || []).slice(0, 10), outs = (n.uses || []).slice(0, 10);
    if (!ins.length && !outs.length) return "";
    var rows = Math.max(ins.length, outs.length), H = rows * 30 + 40, W = 960, midY = H / 2;
    var s = "<div class=\\"ego\\"><svg viewBox=\\"0 0 " + W + " " + H + "\\" role=\\"img\\">";
    function nodeText(id, x, y, anchor){
      var t = byId[id];
      return "<text class=\\"gnode\\" data-id=\\"" + id + "\\" x=\\"" + x + "\\" y=\\"" + y + "\\" text-anchor=\\"" + anchor + "\\" font-size=\\"12\\" fill=\\"var(--accent)\\" style=\\"cursor:pointer\\">" + esc(t ? t.title : id) + "</text>";
    }
    ins.forEach(function(id, i){
      var y = 30 + i * 30;
      s += "<line x1=\\"268\\" y1=\\"" + (y - 4) + "\\" x2=\\"" + (W/2 - 90) + "\\" y2=\\"" + midY + "\\" stroke=\\"var(--border-strong)\\"/>" + nodeText(id, 260, y, "end");
    });
    outs.forEach(function(id, i){
      var y = 30 + i * 30;
      s += "<line x1=\\"" + (W/2 + 90) + "\\" y1=\\"" + midY + "\\" x2=\\"" + (W - 268) + "\\" y2=\\"" + (y - 4) + "\\" stroke=\\"var(--border-strong)\\"/>" + nodeText(id, W - 260, y, "start");
    });
    s += "<text x=\\"" + (W/2) + "\\" y=\\"" + (midY + 4) + "\\" text-anchor=\\"middle\\" font-size=\\"13\\" font-weight=\\"600\\" fill=\\"var(--text-heading)\\">" + esc(n.title) + "</text>";
    return s + "</svg></div>";
  }
  var readerOpen = false;
  function renderReader(){
    if (!readerOpen) return;
    var n = byId[selectedId];
    document.getElementById("rkind").textContent = n.kind;
    document.getElementById("rkind").className = "kind k-" + String(n.kind).toLowerCase().replace(/[^a-z-]/g, "");
    document.getElementById("rtitle").textContent = n.title;
    var i = ORDER.indexOf(selectedId);
    document.getElementById("rpos").textContent = i >= 0 ? (i + 1) + " / " + ORDER.length : "";
    document.getElementById("rbody").innerHTML =
      "<p class=\\"sum\\">" + esc(n.summary) + "</p>" +
      "<div class=\\"exp\\">" + prose(n.explanation, n.id) + "</div>" +
      egoSvg(n) + connsHtml(n) + codeBlocksHtml(n);
    document.getElementById("reader").scrollTop = 0;
  }
  function setReader(open){
    readerOpen = open;
    document.getElementById("reader").className = "overlay" + (open ? " open" : "");
    if (open) renderReader();
  }
  function fileOf(id){
    var cur = byId[id];
    while (cur && cur.kind !== "file" && cur.parent) cur = byId[cur.parent];
    return cur && cur.kind === "file" ? cur.id : null;
  }
  var storyOpen = false, storyBuilt = false;
  var SECTION_KINDS = { overview: 1, area: 1, file: 1, flow: 1 };
  function cardBody(n){
    return "<div class=\\"exp\\">" + prose(n.explanation, n.id) + "</div>" +
           (SECTION_KINDS[n.kind] ? "" : egoSvg(n)) + codeBlocksHtml(n) +
           (n.id === "__ROOT__" && HORIZON ? "<div class=\\"refs\\"><h3>horizon</h3><div class=\\"exp\\">" + prose(HORIZON, n.id) + "</div></div>" : "");
  }
  function buildStory(){
    var col = document.getElementById("sbody");
    var frag = document.createDocumentFragment();
    ORDER.forEach(function(id, idx){
      var n = byId[id];
      var el = document.createElement(SECTION_KINDS[n.kind] ? "section" : "article");
      el.className = SECTION_KINDS[n.kind] ? ("scard " + n.kind) : "card";
      el.setAttribute("data-card", id);
      var kindClass = String(n.kind).toLowerCase().replace(/[^a-z-]/g, "");
      el.innerHTML = "<div class=\\"chead\\"><span class=\\"kind k-" + kindClass + "\\">" + esc(n.kind) + "</span><h2>" + esc(n.title) + "</h2><span class=\\"cpos\\">" + (idx + 1) + " / " + ORDER.length + "</span></div>" +
                     "<p class=\\"sum\\">" + esc(n.summary) + "</p><div class=\\"cbody\\"></div>";
      frag.appendChild(el);
    });
    col.appendChild(frag);
    var io = new IntersectionObserver(function(entries){
      entries.forEach(function(en){
        if (!en.isIntersecting) return;
        var el = en.target, id = el.getAttribute("data-card");
        var body = el.querySelector(".cbody");
        if (body && !body.getAttribute("data-filled")){
          body.setAttribute("data-filled", "1");
          body.innerHTML = cardBody(byId[id]);
        }
        io.unobserve(el);
      });
    }, { root: document.getElementById("sbodywrap"), rootMargin: "1200px 0px" });
    col.querySelectorAll("[data-card]").forEach(function(el){ io.observe(el); });
    storyBuilt = true;
  }
  function scrollToCard(id){
    var el = document.querySelector("[data-card=\\"" + id + "\\"]");
    if (!el) return;
    var body = el.querySelector(".cbody");
    if (body && !body.getAttribute("data-filled")){
      body.setAttribute("data-filled", "1");
      body.innerHTML = cardBody(byId[id]);
    }
    el.scrollIntoView({ block: "start" });
    el.classList.add("flash");
    setTimeout(function(){ el.classList.remove("flash"); }, 900);
    selectedId = id;
  }
  function setStory(open){
    storyOpen = open;
    document.getElementById("story").className = "overlay" + (open ? " open" : "");
    if (open){
      if (!storyBuilt) buildStory();
      scrollToCard(selectedId);
    }
  }
  var exploreOpen = false;
  function miniCard(id){
    var t = byId[id];
    if (!t) return "";
    var kc = String(t.kind).toLowerCase().replace(/[^a-z-]/g, "");
    return "<div class=\\"minicard\\" data-id=\\"" + id + "\\"><span class=\\"kind k-" + kc + "\\">" + esc(t.kind) + "</span><h4>" + esc(t.title) + "</h4><p>" + esc(t.summary) + "</p></div>";
  }
  function renderExplore(){
    if (!exploreOpen) return;
    var n = byId[selectedId];
    var ins = (USEDBY[n.id] || []).slice(0, 8), outs = (n.uses || []).slice(0, 8);
    var insMore = (USEDBY[n.id] || []).length - ins.length, outsMore = (n.uses || []).length - outs.length;
    var kc = String(n.kind).toLowerCase().replace(/[^a-z-]/g, "");
    var center = "<div class=\\"card\\" id=\\"xcenter-card\\"><div class=\\"chead\\"><span class=\\"kind k-" + kc + "\\">" + esc(n.kind) + "</span><h2>" + esc(n.title) + "</h2></div>" +
      "<p class=\\"sum\\">" + esc(n.summary) + "</p><div class=\\"exp\\">" + prose(n.explanation, n.id) + "</div>" + codeBlocksHtml(n) + "</div>";
    var left = "<div class=\\"xlabel\\">← used by</div>" + ins.map(miniCard).join("") + (insMore > 0 ? "<div class=\\"xlabel\\">+" + insMore + " more</div>" : "");
    var right = "<div class=\\"xlabel\\">uses →</div>" + outs.map(miniCard).join("") + (outsMore > 0 ? "<div class=\\"xlabel\\">+" + outsMore + " more</div>" : "");
    document.getElementById("xbody").innerHTML =
      "<svg id=\\"xedges\\"></svg><div class=\\"xcol\\" id=\\"xleft\\">" + left + "</div><div class=\\"xcenter\\">" + center + "</div><div class=\\"xcol\\" id=\\"xright\\">" + right + "</div>";
    requestAnimationFrame(drawXEdges);
  }
  function drawXEdges(){
    var body = document.getElementById("xbody");
    var svg = document.getElementById("xedges");
    var center = document.getElementById("xcenter-card");
    if (!body || !svg || !center) return;
    var b = body.getBoundingClientRect(), c = center.getBoundingClientRect();
    svg.setAttribute("viewBox", "0 0 " + b.width + " " + b.height);
    svg.setAttribute("width", b.width); svg.setAttribute("height", b.height);
    var s = "";
    body.querySelectorAll("#xleft .minicard").forEach(function(el){
      var r = el.getBoundingClientRect();
      s += "<path d=\\"M " + (r.right - b.left) + " " + (r.top + r.height/2 - b.top) + " C " + (r.right - b.left + 40) + " " + (r.top + r.height/2 - b.top) + ", " + (c.left - b.left - 40) + " " + (c.top + 60 - b.top) + ", " + (c.left - b.left) + " " + (c.top + 60 - b.top) + "\\" fill=\\"none\\" stroke=\\"var(--accent-dim)\\" stroke-width=\\"1.4\\" opacity=\\"0.8\\"/>";
    });
    body.querySelectorAll("#xright .minicard").forEach(function(el){
      var r = el.getBoundingClientRect();
      s += "<path d=\\"M " + (c.right - b.left) + " " + (c.top + 60 - b.top) + " C " + (c.right - b.left + 40) + " " + (c.top + 60 - b.top) + ", " + (r.left - b.left - 40) + " " + (r.top + r.height/2 - b.top) + ", " + (r.left - b.left) + " " + (r.top + r.height/2 - b.top) + "\\" fill=\\"none\\" stroke=\\"var(--accent-dim)\\" stroke-width=\\"1.4\\" opacity=\\"0.8\\"/>";
    });
    svg.innerHTML = s;
  }
  function setExplore(open){
    exploreOpen = open;
    document.getElementById("explore").className = "overlay" + (open ? " open" : "");
    if (open) renderExplore();
  }
  window.addEventListener("resize", function(){ if (exploreOpen) drawXEdges(); });
  var navApplying = false;
  function nav(id){
    if (!byId[id]) return;
    if (location.hash !== "#" + id){ location.hash = id; }
    else applyNav(id);
  }
  function applyNav(id){
    navApplying = true;
    selectedId = id;
    if (exploreOpen){ renderExplore(); }
    else if (storyOpen){ scrollToCard(id); }
    else { jumpTo(id); }
    renderTourPos();
    navApplying = false;
  }
  window.addEventListener("hashchange", function(){
    var id = decodeURIComponent(location.hash.slice(1));
    if (byId[id]) applyNav(id);
  });
  var graphOpen = false, graphBuilt = false;
  function renderGraph(){
    var files = DATA.filter(function(n){ return n.kind === "file"; });
    var counts = {};
    DATA.forEach(function(n){
      var f = fileOf(n.id);
      (n.uses || []).forEach(function(t){
        var g = fileOf(t);
        if (f && g && f !== g){ var k = f + ">" + g; counts[k] = (counts[k] || 0) + 1; }
      });
    });
    var W = 980, H = 720, cx = W/2, cy = H/2, R = Math.min(cx, cy) - 130;
    var pos = {};
    files.forEach(function(f, i){
      var a = (i / files.length) * 2 * Math.PI - Math.PI/2;
      pos[f.id] = { x: cx + R * Math.cos(a), y: cy + R * Math.sin(a), a: a };
    });
    var s = "<svg viewBox=\\"0 0 " + W + " " + H + "\\" role=\\"img\\">";
    Object.keys(counts).sort().forEach(function(k){
      var ab = k.split(">"), p1 = pos[ab[0]], p2 = pos[ab[1]], c = counts[k];
      if (!p1 || !p2) return;
      var w = Math.min(6, 0.6 + Math.log(c + 1));
      s += "<path d=\\"M " + p1.x + " " + p1.y + " Q " + cx + " " + cy + " " + p2.x + " " + p2.y + "\\" fill=\\"none\\" stroke=\\"var(--accent-dim)\\" stroke-width=\\"" + w.toFixed(1) + "\\" opacity=\\"0.55\\"><title>" + esc(byId[ab[0]].title) + " → " + esc(byId[ab[1]].title) + ": " + c + " mentions</title></path>";
    });
    files.forEach(function(f){
      var p = pos[f.id], right = Math.cos(p.a) >= 0;
      s += "<circle cx=\\"" + p.x + "\\" cy=\\"" + p.y + "\\" r=\\"5\\" fill=\\"var(--accent)\\"/>";
      s += "<text class=\\"gnode\\" data-id=\\"" + f.id + "\\" x=\\"" + (p.x + (right ? 10 : -10)) + "\\" y=\\"" + (p.y + 4) + "\\" text-anchor=\\"" + (right ? "start" : "end") + "\\" font-size=\\"12\\" fill=\\"var(--text)\\" style=\\"cursor:pointer\\">" + esc(f.title) + " <tspan fill=\\"var(--text-muted)\\">~" + f.total + "</tspan></text>";
    });
    s += "</svg>";
    document.getElementById("gbody").innerHTML = s;
    graphBuilt = true;
  }
  function setGraph(open){
    graphOpen = open;
    document.getElementById("graph").className = "overlay" + (open ? " open" : "");
    if (open && !graphBuilt) renderGraph();
  }
  function renderPanel(){
    var n = byId[selectedId];
    var kindClass = String(n.kind).toLowerCase().replace(/[^a-z-]/g, "");
    var html = "<span class=\\"kind k-" + kindClass + "\\">" + esc(n.kind) + "</span>" +
               "<h2>" + esc(n.title) + "</h2>" +
               "<p class=\\"sum\\">" + esc(n.summary) + "</p>" +
               "<div class=\\"exp\\">" + prose(n.explanation, n.id) + "</div>";
    if (n.refs.length){
      html += "<div class=\\"refs\\"><h3>refs</h3>";
      n.refs.forEach(function(r){ html += "<div>" + esc(r.path) + "<span>:" + esc(r.lines) + "</span> " + editorLinks(r.path, refStart(r)) + "</div>"; });
      html += "</div>";
    }
    html += connsHtml(n);
    html += codeBlocksHtml(n);
    if (n.id === "__ROOT__" && HORIZON){
      html += "<div class=\\"refs\\"><h3>horizon</h3><div class=\\"exp\\">" + prose(HORIZON, n.id) + "</div></div>";
    }
    if (n.children.length){
      html += "<div class=\\"kids\\"><h3>inside</h3></div>";
    }
    var panel = document.getElementById("panel");
    panel.innerHTML = html;
    if (n.children.length){
      var kids = panel.querySelector(".kids");
      n.children.forEach(function(cid){
        var c = byId[cid];
        var b = document.createElement("button");
        b.innerHTML = esc(c.title) + " <span>· ~" + c.total + " lines</span>";
        b.addEventListener("click", function(){
          if (c.children.length){ focusId = c.id; }
          selectedId = c.id; renderAll();
        });
        kids.appendChild(b);
      });
    }
  }

  var ORDER = [];
  (function walk(id){
    ORDER.push(id);
    byId[id].children.slice().sort(function(a,b){ return byId[b].total - byId[a].total; }).forEach(walk);
  })(ROOT);
  function jumpTo(id){
    var n = byId[id];
    if (!n) return;
    selectedId = id;
    focusId = n.children.length ? id : (n.parent || id);
    renderAll();
  }
  function renderTourPos(){
    var el = document.getElementById("tourpos");
    var i = ORDER.indexOf(selectedId);
    el.textContent = i >= 0 ? (i + 1) + " / " + ORDER.length : "";
  }
  function renderAll(){ renderCrumbs(); renderMap(); renderPanel(); renderTourPos(); renderReader(); }

  function jumpDelegate(e){
    var t = e.target.closest ? e.target.closest(".xref, .chip, .gnode, .minicard") : null;
    if (t && t.getAttribute("data-id")){
      if (graphOpen) setGraph(false);
      nav(t.getAttribute("data-id"));
      return;
    }
    var x = e.target.closest ? e.target.closest("[data-explore]") : null;
    if (x){ setReader(false); setExplore(true); nav(selectedId); }
  }
  document.getElementById("panel").addEventListener("click", jumpDelegate);
  document.getElementById("reader").addEventListener("click", jumpDelegate);
  document.getElementById("graph").addEventListener("click", jumpDelegate);
  document.getElementById("story").addEventListener("click", jumpDelegate);
  document.getElementById("sclose").addEventListener("click", function(){ setStory(false); renderAll(); });
  document.getElementById("rclose").addEventListener("click", function(){ setReader(false); });
  document.getElementById("gclose").addEventListener("click", function(){ setGraph(false); });
  document.getElementById("explore").addEventListener("click", jumpDelegate);
  document.getElementById("xclose").addEventListener("click", function(){ setExplore(false); renderAll(); });
  document.getElementById("navback").addEventListener("click", function(){ history.back(); });
  document.getElementById("navfwd").addEventListener("click", function(){ history.forward(); });

  document.addEventListener("keydown", function(e){
    if (e.key === "e" && !graphOpen && !readerOpen){ setExplore(!exploreOpen); if (exploreOpen) renderExplore(); else renderAll(); e.preventDefault(); return; }
    if (exploreOpen){
      if (e.key === "Escape"){ setExplore(false); renderAll(); e.preventDefault(); }
      return;
    }
    if (e.key === "n" && !readerOpen && !graphOpen){ setStory(!storyOpen); if (!storyOpen) renderAll(); e.preventDefault(); return; }
    if (storyOpen){
      if (e.key === "Escape"){ setStory(false); renderAll(); e.preventDefault(); }
      return;
    }
    if (e.key === "Enter" && !graphOpen){ setReader(!readerOpen); e.preventDefault(); return; }
    if (e.key === "g"){ setGraph(!graphOpen); e.preventDefault(); return; }
    if ((e.key === "Escape" || e.key === "Backspace") && (readerOpen || graphOpen)){
      setReader(false); setGraph(false); e.preventDefault(); return;
    }
    if (e.key === "j" || e.key === "ArrowRight"){
      var i = ORDER.indexOf(selectedId);
      if (i < ORDER.length - 1){ jumpTo(ORDER[i + 1]); e.preventDefault(); }
      return;
    }
    if (e.key === "k" || e.key === "ArrowLeft"){
      var i2 = ORDER.indexOf(selectedId);
      if (i2 > 0){ jumpTo(ORDER[i2 - 1]); e.preventDefault(); }
      return;
    }
    if (e.key === "Escape" || e.key === "Backspace"){
      var f = byId[focusId];
      if (f.parent){ focusId = f.parent; selectedId = focusId; renderAll(); e.preventDefault(); }
    }
  });
  window.addEventListener("resize", renderMap);

  var btn = document.getElementById("theme");
  var KEY = "ctx-walkthrough-theme";
  var states = ["auto", "light", "dark"];
  var ti = states.indexOf((function(){ try { return localStorage.getItem(KEY); } catch (e) { return null; } })() || "auto");
  if (ti < 0) ti = 0;
  function applyTheme(){
    var s = states[ti];
    if (s === "auto") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.setAttribute("data-theme", s);
    btn.textContent = "theme: " + s;
    try { localStorage.setItem(KEY, s); } catch (e) {}
  }
  btn.addEventListener("click", function(){ ti = (ti + 1) % 3; applyTheme(); });
  applyTheme();

  renderAll();
})();
</script>
</body>
</html>
"""


def merge_inputs(skeleton_arg: str, batches_arg: str):
    skeleton = load_nodes(skeleton_arg)
    text = batches_arg
    if len(batches_arg) < 512 and os.path.exists(batches_arg) and not batches_arg.lstrip().startswith(("[", "{")):
        with open(batches_arg, encoding="utf-8") as fh:
            text = fh.read()
    try:
        batches = json.loads(text)
    except json.JSONDecodeError as e:
        fail(f"argv[2] is neither JSON nor a readable JSON file: {e}")
    flat = list(skeleton)
    if isinstance(batches, list):
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


CODE_KINDS = {"type", "function"}
MAX_EMBED_LINES = 90


def embed_code(nodes) -> None:
    """Attach the actual source lines each leaf node's refs cover (read at
    render time, deterministically, from the working tree). Whole-file spans
    on upper nodes are skipped; drifted spans are marked, never guessed."""
    cache = {}
    for n in nodes:
        if n.get("kind") not in CODE_KINDS:
            continue
        blocks = []
        for ref in n.get("refs", []):
            path = ref.get("path", "")
            if path not in cache:
                try:
                    with open(path, encoding="utf-8", errors="replace") as fh:
                        cache[path] = fh.read().splitlines()
                except OSError:
                    cache[path] = None
            lines = cache[path]
            span = str(ref.get("lines", "")).split(",")[0].strip()
            bits = span.split("-", 1)
            try:
                start = int(bits[0])
                end = int(bits[1]) if len(bits) > 1 else start
            except ValueError:
                continue
            if lines is None:
                blocks.append({"path": path, "lines": span, "start": start, "text": "", "drifted": "file unreadable at render time"})
                continue
            if start > len(lines):
                blocks.append({"path": path, "lines": span, "start": start, "text": "", "drifted": "span beyond current file — repo drifted since the run"})
                continue
            end = min(end, len(lines))
            chunk = lines[start - 1 : end]
            drift = ""
            if len(chunk) > MAX_EMBED_LINES:
                chunk = chunk[:MAX_EMBED_LINES]
                drift = f"trimmed to {MAX_EMBED_LINES} of {end - start + 1} lines"
            blocks.append({"path": path, "lines": span, "start": start, "text": "\n".join(chunk), "drifted": drift})
        if blocks:
            n["code"] = blocks


IDENT = __import__("re").compile(r"[A-Za-z_][A-Za-z0-9_]{2,}")
GENERIC_NAME_CAP = 6
MAX_USES = 40


def derive_edges(nodes) -> None:
    """Mechanical mention edges: identifier tokens inside a leaf node's
    embedded spans, matched against every other leaf's symbol name. Labeled
    as name-matches in the UI — never semantic call analysis. A name shared
    by more than GENERIC_NAME_CAP symbols is skipped as too generic."""
    name_to_ids = {}
    for n in nodes:
        sym = n.get("symbol")
        if not sym:
            continue
        name = sym.rsplit(":", 1)[-1]
        name_to_ids.setdefault(name, []).append(n["id"])
    name_to_ids = {k: v for k, v in name_to_ids.items() if len(v) <= GENERIC_NAME_CAP}
    for n in nodes:
        blocks = n.get("code") or []
        if not blocks:
            continue
        tokens = set()
        for b in blocks:
            tokens.update(IDENT.findall(b.get("text", "")))
        self_name = (n.get("symbol") or "").rsplit(":", 1)[-1]
        uses = set()
        for t in tokens:
            if t == self_name:
                continue
            for tid in name_to_ids.get(t, []):
                if tid != n["id"]:
                    uses.add(tid)
        if uses:
            n["uses"] = sorted(uses)[:MAX_USES]


def main() -> None:
    if len(sys.argv) < 5:
        fail("usage: render.py <skeleton> <batches> <horizon-text> <output-html-path>")
    nodes, root_id = normalize(merge_inputs(sys.argv[1], sys.argv[2]))
    embed_code(nodes)
    derive_edges(nodes)
    horizon = sys.argv[3].strip()
    out_path = sys.argv[4].strip()
    if not out_path:
        fail("empty output path")

    root = next(n for n in nodes if n["id"] == root_id)
    data_json = json.dumps(
        [
            {k: n[k] for k in ("id", "parent", "title", "kind", "summary", "explanation", "refs", "total", "children", "code", "uses", "symbol") if k in n and n[k] is not None}
            for n in nodes
        ],
        ensure_ascii=False,
    ).replace("</", "<\\/")

    html = (
        TEMPLATE.replace("__TITLE__", root["title"].replace("<", "").replace(">", "") + " — walkthrough")
        .replace("__DATA__", data_json)
        .replace("__HORIZON__", json.dumps(horizon, ensure_ascii=False).replace("</", "<\\/"))
        .replace("__REPOROOT__", json.dumps(os.path.abspath(os.getcwd())))
        .replace("__ROOT__", root_id)
    )

    parent = os.path.dirname(out_path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as fh:
        fh.write(html)
    print(f"wrote {out_path} ({len(nodes)} nodes, {len(html)} bytes)")


main()
