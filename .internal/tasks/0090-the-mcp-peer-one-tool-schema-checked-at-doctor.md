# 0090 — The `mcp` peer: one tool per peer, its schema checked at `doctor`

**Status:** ready to implement · **Depends on:** 0087 · **Raised:** 2026-08-04 (owner design session; decisions in this file are the contract — they were settled deliberately, do not re-open them without a concrete contradiction)

Fourth slice of the peer arc, and the one that costs least for what it returns. Docs lookup, web
search, a file corpus, an internal index — all of it arrives as configuration, because an MCP tool is
already a typed callable with a published schema. There is no search backend to design.

## Decisions

- **A peer is one callable, not a server.** `server` plus `tool`. Three tools from one server are
  three peers. Naming only the server would leave the binding incomplete — the tools behind it have
  different schemas — and would force the trait to name a tool, which is exactly the coupling the
  role indirection exists to prevent.
- **`doctor` checks both schemas.** It reads the tool's published input and output schema and matches
  them against what the trait's `consult` declares. A missing tool, an unreachable server, a renamed
  argument or a reply shape that no longer fits fails there. This is the strongest static check in the
  whole arc and it is free, because MCP tools publish JSON Schema.
- **`args` is a field rename map, not a template.** `args = { query = "question" }` maps request
  fields to tool arguments by name; unmapped fields match by name. 0073's rule holds — no half-language
  with no types and no errors.
- **Server connection config is declared once and referenced**, not inlined per peer. 0092 needs the
  same servers for ambient tool grants, and two places declaring the same stdio command with different
  environments is a bug waiting for a Friday.
- **The host owns the server process**, as it owns every other effect: started, bounded, and torn down
  outside the sandbox with the host's environment. A run never launches an MCP server.
- **Answers by default, tells off.** A side-effecting tool is still modelled as a consult that returns
  a result — there is no useful `tell` against a tool that reports what it did.

## Scope

The `mcp` peer variant; shared server declarations referenced by peers; stdio and HTTP transports as
the server config allows; tool discovery and both-direction schema matching in `resolve()`; the `args`
rename map; host-owned server lifecycle with bounds from 0084; `--dry-run` printing the resolved
server, tool and arguments without calling.

## Watch

- **Tool output is untrusted input** and is validated against the declared reply schema before it
  becomes a slot value (0088). A published schema is a claim, not a guarantee.
- **Schema drift is a live hazard, not just a `doctor` one.** A server upgraded between `doctor` and a
  run changes the contract underneath it. The reply check catches it, but the message must name the
  server and tool or it reads as a trait bug.
- A slow or dead server parks the run as a fault, not as an open question (`deferred` stays false).
  Getting that default wrong makes an outage look like it is waiting for a human.
- Third-party servers carry their own auth and reach their own network. Their credentials are env
  references resolved in the host and never printed (0069), and their reachability is a `doctor` line.
- Do not let this grow into ambient tool access. A peer is called at a declared point in a procedure;
  granting a model a tool to use at will is 0092 and has a completely different determinism story.

## Done when

A `[[peer]]` binds one MCP tool on a declared server; `doctor` matches the tool's published input and
output schemas against the trait's declared request and reply and fails on a mismatch, a missing tool
or an unreachable server; a consult calls it and lands a validated typed reply in the ledger; server
declarations are shared rather than inlined; the host owns the process lifecycle; and `--dry-run`
prints the resolved call without making it.
