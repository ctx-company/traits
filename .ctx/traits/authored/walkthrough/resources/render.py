#!/usr/bin/env python3
"""Deterministic walkthrough renderer.

argv[1] — the skeleton node list: JSON text (the runtime substitutes the
          typed slot value), or, as a hedge, a path to a JSON file.
argv[2] — the node batches (list of lists of nodes, appended per frame),
          same JSON-or-path convention. Flattened in order and deduped by
          id, last occurrence winning (append-only revisions).
argv[3] — repo-relative output path for the self-contained HTML.

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
:root{
  --ground:#f5f7f7; --panel:#fdfefe; --ink:#1d2528; --dim:#68787d; --rule:#d3dcde;
  --accent:#2e7d92; --ok:#2f7d46; --warn:#a87616; --bad:#bb4a3c;
  --tile:#eef3f4; --tile-hover:#e2ecee;
  --mono:ui-monospace,"SF Mono",Menlo,Consolas,"Cascadia Mono",monospace;
  --serif:"Charter","Iowan Old Style",Georgia,serif;
}
@media (prefers-color-scheme: dark){
  :root{
    --ground:#0f1516; --panel:#151d1f; --ink:#d9e4e6; --dim:#7e9096; --rule:#2a393d;
    --accent:#5fb3c6; --ok:#5cb87c; --warn:#d3a145; --bad:#e0705f;
    --tile:#182225; --tile-hover:#1e2c30;
  }
}
:root[data-theme="light"]{
  --ground:#f5f7f7; --panel:#fdfefe; --ink:#1d2528; --dim:#68787d; --rule:#d3dcde;
  --accent:#2e7d92; --ok:#2f7d46; --warn:#a87616; --bad:#bb4a3c;
  --tile:#eef3f4; --tile-hover:#e2ecee;
}
:root[data-theme="dark"]{
  --ground:#0f1516; --panel:#151d1f; --ink:#d9e4e6; --dim:#7e9096; --rule:#2a393d;
  --accent:#5fb3c6; --ok:#5cb87c; --warn:#d3a145; --bad:#e0705f;
  --tile:#182225; --tile-hover:#1e2c30;
}
*{box-sizing:border-box}
html,body{margin:0;padding:0;height:100%}
body{background:var(--ground);color:var(--ink);font-family:var(--serif);font-size:15px;line-height:1.5;display:flex;flex-direction:column}
header{display:flex;align-items:center;gap:14px;padding:12px 18px;border-bottom:1px solid var(--rule);flex-wrap:wrap}
.eyebrow{font-family:var(--mono);font-size:10.5px;letter-spacing:.14em;text-transform:uppercase;color:var(--dim);white-space:nowrap}
#crumbs{display:flex;align-items:center;gap:6px;flex-wrap:wrap;font-family:var(--mono);font-size:12px;min-width:0}
#crumbs button{font-family:var(--mono);font-size:12px;color:var(--accent);background:none;border:none;padding:2px 2px;cursor:pointer}
#crumbs button:hover{text-decoration:underline}
#crumbs .cur{color:var(--ink);font-weight:600}
#crumbs .sep{color:var(--dim)}
#theme{margin-left:auto;font-family:var(--mono);font-size:11px;color:var(--dim);background:var(--panel);border:1px solid var(--rule);border-radius:3px;padding:4px 8px;cursor:pointer;white-space:nowrap}
#theme:hover{color:var(--ink)}
main{flex:1;display:flex;min-height:0}
#map{flex:1;position:relative;margin:14px;min-width:0}
.tile{position:absolute;background:var(--tile);border:1px solid var(--rule);border-radius:3px;overflow:hidden;cursor:pointer;padding:6px 8px;transition:background .12s}
.tile:hover{background:var(--tile-hover);border-color:var(--accent)}
.tile.selected{border-color:var(--accent);box-shadow:inset 0 0 0 1px var(--accent)}
.tile .t{font-family:var(--mono);font-size:12px;font-weight:600;color:var(--ink);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.tile .m{font-family:var(--mono);font-size:10px;color:var(--dim);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.tile .b{position:absolute;right:6px;bottom:4px;font-family:var(--mono);font-size:9.5px;color:var(--accent)}
aside{width:400px;max-width:44vw;border-left:1px solid var(--rule);background:var(--panel);overflow-y:auto;padding:18px 20px}
.kind{display:inline-block;font-family:var(--mono);font-size:10px;letter-spacing:.1em;text-transform:uppercase;border:1px solid var(--accent);color:var(--accent);border-radius:3px;padding:2px 7px;margin-bottom:8px}
aside h2{font-family:var(--mono);font-size:16px;margin:0 0 6px;letter-spacing:-.01em}
.sum{color:var(--dim);font-style:italic;margin:0 0 12px;font-size:13.5px}
.exp p{margin:0 0 10px;font-size:14px}
.exp code, .refs code{font-family:var(--mono);font-size:.88em;background:var(--ground);border:1px solid var(--rule);border-radius:3px;padding:0 4px}
.refs{margin:14px 0 0;padding:10px 0 0;border-top:1px solid var(--rule)}
.refs h3, .kids h3{font-family:var(--mono);font-size:10.5px;text-transform:uppercase;letter-spacing:.1em;color:var(--dim);margin:0 0 6px}
.refs div{font-family:var(--mono);font-size:11.5px;color:var(--ink);padding:1px 0;word-break:break-all}
.refs div span{color:var(--dim)}
.kids{margin:14px 0 0;padding:10px 0 0;border-top:1px solid var(--rule)}
.kids button{display:block;width:100%;text-align:left;font-family:var(--mono);font-size:12px;color:var(--accent);background:none;border:none;padding:3px 0;cursor:pointer}
.kids button:hover{text-decoration:underline}
.kids button span{color:var(--dim);font-size:10.5px}
footer{border-top:1px solid var(--rule);padding:8px 18px;font-family:var(--mono);font-size:10.5px;color:var(--dim)}
@media (max-width:820px){main{flex-direction:column}aside{width:auto;max-width:none;border-left:none;border-top:1px solid var(--rule)}#map{min-height:340px}}
</style>
</head>
<body>
<header>
  <span class="eyebrow">ctx walkthrough</span>
  <nav id="crumbs" aria-label="Path"></nav>
  <button id="theme" aria-label="Cycle color theme">theme: auto</button>
</header>
<main>
  <div id="map" role="tree" aria-label="Treemap"></div>
  <aside id="panel"></aside>
</main>
<footer>click a tile to zoom into it; click a leaf to read it; Esc or Backspace goes up one level · tile area = lines of code the node covers</footer>
<script>
var DATA = __DATA__;
(function(){
  var byId = {};
  DATA.forEach(function(n){ byId[n.id] = n; });
  var ROOT = "__ROOT__";
  var focusId = ROOT, selectedId = ROOT;

  function esc(s){
    return String(s).replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;").replace(/"/g,"&quot;");
  }
  function prose(s){
    var parts = String(s).split(/\\n\\s*\\n/);
    var html = "";
    for (var i=0;i<parts.length;i++){
      if (!parts[i].trim()) continue;
      var p = esc(parts[i].trim()).replace(/`([^`]+)`/g, function(_, c){ return "<code>" + c + "</code>"; });
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
      note.style.cssText = "position:absolute;inset:0;display:flex;align-items:center;justify-content:center;color:var(--dim);font-family:var(--mono);font-size:12px";
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

  function renderPanel(){
    var n = byId[selectedId];
    var html = "<span class=\\"kind\\">" + n.kind + "</span>" +
               "<h2>" + esc(n.title) + "</h2>" +
               "<p class=\\"sum\\">" + esc(n.summary) + "</p>" +
               "<div class=\\"exp\\">" + prose(n.explanation) + "</div>";
    if (n.refs.length){
      html += "<div class=\\"refs\\"><h3>code</h3>";
      n.refs.forEach(function(r){ html += "<div>" + esc(r.path) + "<span>:" + esc(r.lines) + "</span></div>"; });
      html += "</div>";
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

  function renderAll(){ renderCrumbs(); renderMap(); renderPanel(); }

  document.addEventListener("keydown", function(e){
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


def main() -> None:
    if len(sys.argv) < 4:
        fail("usage: render.py <skeleton-json-or-path> <batches-json-or-path> <output-html-path>")
    nodes, root_id = normalize(merge_inputs(sys.argv[1], sys.argv[2]))
    out_path = sys.argv[3].strip()
    if not out_path:
        fail("empty output path")

    root = next(n for n in nodes if n["id"] == root_id)
    data_json = json.dumps(
        [
            {k: n[k] for k in ("id", "parent", "title", "kind", "summary", "explanation", "refs", "total", "children")}
            for n in nodes
        ],
        ensure_ascii=False,
    ).replace("</", "<\\/")

    html = (
        TEMPLATE.replace("__TITLE__", root["title"].replace("<", "").replace(">", "") + " — walkthrough")
        .replace("__DATA__", data_json)
        .replace("__ROOT__", root_id)
    )

    parent = os.path.dirname(out_path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as fh:
        fh.write(html)
    print(f"wrote {out_path} ({len(nodes)} nodes, {len(html)} bytes)")


main()
