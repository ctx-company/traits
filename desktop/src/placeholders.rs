//! Named seams for actions a screen wires up before v1 gives them real
//! behaviour. A caller reaches through this module rather than writing the
//! label as a literal, so the one place that needs to change when a
//! placeholder gains behaviour is this file, not every call site.

/// A labelled action with no bound behaviour yet — the no-op is the absence
/// of a handler at the call site, not a branch in this module.
pub struct Placeholder {
    pub label: &'static str,
}

/// `screens/sessions.md:32-34` tags this leaf `placeholder:` — "`watch raw`
/// opens the raw view — v1 may no-op".
pub const WATCH_RAW: Placeholder = Placeholder { label: "watch raw" };
