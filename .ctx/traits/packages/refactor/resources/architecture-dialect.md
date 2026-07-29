# Architecture Dialect

The umbrella test for every rule here is the deep-module principle (John
Ousterhout, popularized for agent workflows by the deep-module-refactor skill):
a good module has a **small interface hiding a deep implementation**. If an
interface is nearly as wide as the code behind it, the module is shallow and
the boundary is in the wrong place. Everything below is that one idea applied
three ways. The notation is pseudocode — `T = A | B` declares a type with
variants, `f(x) -> y` declares an operation — the rules apply in any language.
The names (`Interface`, `Service`, `Request`, `Response`, `Command`, `Event`,
`Context`) are the house dialect: use them literally.

## 1. Boundaries — contracts first, implementations by context

Every capability is a contract named `Interface`, implemented by one or more
`Service`s, namespaced by responsibility:

```
store::Interface = {
  put(record) -> ok | store::Error
  get(id) -> record? | store::Error
}

blob::Service     : store::Interface   // one context
dynamodb::Service : store::Interface   // another context — callers never know
```

Why this shape is load-bearing:

- **Testability.** Anything that depends on `store::Interface` can be tested
  against a mock `Service`, no infrastructure. When a unit is hard to test,
  the first question is "which Interface is missing?", not "how do I set up
  the environment?"
- **Reusability.** One contract, context-picked implementations. Caller code
  does not change when the context does.
- **Surface-agnostic layers.** A capability speaks its own request/response
  vocabulary, never a transport's:

  ```
  api::Interface = { handle(api::Request) -> api::Response }
  ```

  An HTTP handler, a lambda wrapper, and a CLI are then thin adapters that
  normalize into `api::Request` and render `api::Response`. The core never
  learns which surface called it.
- **Small packages of responsibility.** Split contracts by what they are
  allowed to do: `control::Interface` performs mutations, `view::Interface`
  only reads, `trace::Interface` only traces. A holder of `view::Interface`
  provably cannot mutate — reasoning gets cheaper.

## 2. Data flow — typed events and commands, normalized once

Effects are typed events, handled in one flow layer instead of ad-hoc calls
across the system:

```
entity::Event = UserUpdated(id) | SessionClosed(id) | ...
flow::Service = { handle(entity::Event) -> effects }
```

Requests to change the system are typed commands, normalized exactly once at
the edge:

```
entity::Command = InitSession(init::Request) | CloseSession(id) | ...
// CLI arguments, HTTP bodies, queue messages all normalize INTO Command;
// handlers match on Command variants and never re-parse raw input.
```

The payoff is one narrow river of data: surface -> Command -> service ->
Event -> flow. When you ask "what can change X?", the answer is one type's
variant list.

## 3. Entity containment — everything about a thing lives with the thing

All of an entity's vocabulary belongs to the entity's namespace: its
`Command`, its `Event`, its store contract, its api types, its queries, its
invariants. The rule is about ownership, not file layout: wherever the code
lives, the entity's types and operations are reachable through the entity's
own namespace, and nothing outside it manipulates the entity's internals
directly.

The test: to understand or change the entity, you look in one place. If its
commands live with a CLI, its events with a runtime, and its storage calls
are inlined at call sites, the entity has leaked and every change is a
scavenger hunt.

## 4. Naming & placement — the path is part of the name

A full path is one name read left to right; every segment must add
information. Redundancy anywhere in the chain is noise — and placement is
part of naming: where a thing lives says what owns it.

- **Never re-state the chain.** A name never repeats a segment of its own
  path: `some::entity::State`, not `some::entity::EntityState` — the path
  already said entity.
- **Lift the namesake.** When a module has a main entity, the parent
  re-exports it so consumers write the short spelling: `some::entity::Entity`
  is used as `some::Entity`. Full paths are for the module's internals.
- **Modules state responsibility; methods state the operation within it.**
  `entity::presentation::Service = { render(view) -> output }` — never an
  agent-noun echo module (`presentation::presenter`) and never a method that
  re-states its module (`presentation::Service.present()`). If a method adds
  nothing the module didn't already say, the operation is misnamed or
  misplaced.
- **Contracts live with the concept they describe, not with their
  consumers.** An interface or type belongs to the domain module that owns
  the concept; other modules implement or use it from there:

  ```
  message::transport::Interface     // owned by the message domain
  display::feed::Service            // a consumer: imports or implements it
  ```

  A domain contract declared inside a consumer's namespace has leaked —
  move it home and import it.
- **Every nesting level must add a distinct concept.** A child module that
  is a synonym or rephrasing of its parent collapses into it:
  `parent::synonym::Interface` becomes `parent::Interface`. Depth is
  justified only by new meaning, never by ceremony.

## How to apply during refactoring

1. Name the responsibility, then the contract (`<responsibility>::Interface`),
   then the context implementations (`<context>::Service`).
2. Keep interfaces small — 1 to 3 entry points is the target; more is a sign
   the boundary is shallow or in the wrong place.
3. Never widen a contract for one caller's convenience — give the caller a
   `Request` object instead (see the smell catalog).
4. Prefer moving code INTO the entity's namespace over importing the entity's
   internals elsewhere.
5. Prefer transformations that delete. When your change supersedes code, the
   same change removes it — nothing survives beside its replacement. Every
   net addition must name what it buys (a boundary, a type, a proof).
