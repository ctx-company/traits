# Smell Catalog

Ten detectable smells. For each: how to find it, the before -> after shape,
and when to leave it alone. Notation is pseudocode (`T = A | B` for types,
`f(x) -> y` for operations) — apply in any language. A finding that cites
none of these (and no deep-module violation from the dialect doc) is taste,
not a defect — do not raise it.

## S1 — Hardcoded mechanism where a type should be

**Detect:** dispatch by comparing raw strings that name commands, states, or
kinds; output assembled by hand as arrays of formatted lines.

```
// before
if command == "init-session" { ... } else if command == "close" { ... }
lines.append(format(label, value))

// after
Command = InitSession(request) | Close(id)
match command { InitSession(request) -> ..., Close(id) -> ... }
panel.row(label, value)   // a builder owns layout; callers state intent
```

**Leave alone:** one-off parsing at the true system edge whose only job is to
normalize into the typed form.

## S2 — Over-specific helper that should be a utility

**Detect:** operation names that encode one caller's story into a general
operation — the name mentions the caller's domain even though the logic
does not need it.

```
// before — the operation was never about configs:
find_root_from_config_file(config_path) -> root
// after — general utility; the caller passes whatever field it has:
find_root(start_path) -> root
```

**Leave alone:** helpers whose logic genuinely depends on the specific domain
(renaming those "generic" only hides a real coupling).

## S3 — Stringly-typed fields

**Detect:** string (or list-of-string) fields whose values have internal
structure the code later re-parses.

```
// before
entity_refs: [string]           // "slot:draft", "port:phase", ...
// after
entities: [entity::Reference]
entity::Reference = VariantA(identifier) | VariantB(identifier)
```

Parse once at the edge; everything downstream matches on variants.
**Leave alone:** genuinely opaque text (user prose, foreign identifiers you
never interpret).

## S4 — Parameter bloat

**Detect:** operations taking more than 3-4 arguments, or growing a new
argument per feature.

```
// before
dispatch(role, harness, prompt, timeout, warm, trace) -> outcome
// after
dispatch(request: dispatch::Request) -> outcome   // named fields, defaults where sane
```

Name the request/options type after the operation (`dispatch::Request`,
`some_function::Options`) so it lives next to its only consumer.
**Leave alone:** 2-3 argument operations; do not ceremonialize small things.

## S5 — Re-threading values instead of carrying a Context

**Detect:** the same service or value constructed at multiple call sites, or
threaded untouched through many layers.

```
// before
svc = entity::Service(config)    // ...appearing again and again at call sites
// after — build once at the composition root, carry down:
Context = { entity_svc: entity::Service, trace: trace::Interface }
handle(ctx: Context, command) -> response
```

**Leave alone:** values used by exactly one layer — a Context is not a junk
drawer, and operations should still declare what they use.

## S6 — Text-based errors

**Detect:** error variants that carry only a formatted message; message
formatting at the error-creation site.

```
// before
Error::Action { message: "directory X is not valid" }
// after — typed, matchable, module-owned, rendered later:
action::Error = DirectoryInvalid(path) | NotFound(id)
```

Typed errors keep the decision (what happened) separate from the rendering
(what to tell which surface).
**Leave alone:** the final human-facing rendering layer, which legitimately
formats.

## S7 — `Kind` indirection instead of matching on the type

**Detect:** a parallel `*Kind` type shadowing a real type, compared via
`.kind()`.

```
// before
if error.kind() == ErrorKind::NotFound { ... }
// after
error is Error::NotFound       // or a semantic helper: error.is_not_found()
```

If the variants carry no data and exist only to be compared, fold them into
the carrying type.
**Leave alone:** mirroring a foreign API's kind type at the boundary that
wraps it.

## S8 — Surface-blind returns

**Detect:** business logic returning display-ready strings ("message to
user"), forcing every new surface to re-parse or duplicate.

```
// before
advance(...) -> "session started: ..."
// after
advance(...) -> advance::Response
advance::Response = Started(session_id) | Blocked(reason)
```

Each surface (CLI, TUI, JSON, machine protocol) renders the Response its own
way.
**Leave alone:** the presentation layer itself.

## S9 — Accretion: adding beside instead of replacing

**Detect:** a change introduces a new mechanism while its predecessor
survives — helper beside helper, a wrapper over a thing instead of a change
to the thing, near-duplicate blocks, "compat"/legacy paths with no remaining
consumer, dead code left behind after a migration.

```
// before — both alive after the "refactor":
old_way(x) -> y     // still exported, still called in two places
new_way(x) -> y     // the improvement, added beside it

// after — replacement consumes its predecessor, same change:
new_way(x) -> y     // callers migrated, old_way deleted
```

Deletion is the cheapest, lowest-risk refactor and the most under-produced
one. When your change supersedes code, the same change removes it.
**Leave alone:** genuine deprecation windows with external consumers — but
then the deprecation is DECLARED (marked, dated), never silent survival.

## S10 — Namespace stutter & misplacement

**Detect:** names repeating a segment of their own path; agent-noun modules
echoing a responsibility module; methods re-stating their module; namesakes
never lifted (every consumer spells the full chain); nesting levels that are
synonyms of their parent; domain contracts declared inside a consumer's
namespace instead of the module that owns the concept.

```
// before
some::entity::EntityState                    // chain already says entity
entity::presentation::presenter.present(x)   // echo module + echo method
consumer::domain_thing::Interface            // contract living with a consumer
parent::synonym::Interface                   // nesting restates the parent

// after
some::entity::State
entity::presentation::Service.render(x) -> output
domain_thing::Interface                      // lives with its concept; consumers import
parent::Interface                            // synonym level collapsed
some::Entity                                 // namesake lifted at the parent
```

The path is part of the name: every segment must add information, and where
a thing lives says what owns it.
**Leave alone:** re-stating that resolves a genuine ambiguity at a public
boundary (two lifted namesakes colliding at one parent — the full path IS
the disambiguation), and mirrors of a foreign API's naming.

## Severity discipline

- A smell instance is a finding only when fixing it materially improves
  testability, reuse, or reasoning — cite which, and cite the smell id.
- Behavior-preserving is the default: byte-stable outputs stay byte-stable;
  public serialized shapes never change under a refactor unless the frame
  explicitly says so.
- Never fix a smell by widening an interface. If the fix makes the boundary
  wider, the boundary is wrong (see the dialect doc).
- A refactor's natural direction is negative: same behavior, less code. Net
  growth is a cost that must buy something named — a boundary, a type, a
  proof. Report the net line delta as evidence and justify growth; never
  chase the number as a target (that trades comments, error handling, and
  clarity for line count, which is worse than accretion).
