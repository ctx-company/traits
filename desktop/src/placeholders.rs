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

/// `screens/sessions.md:19-20` tags the current frame row's description
/// `placeholder:` — "data: frame name; description placeholder: until
/// frames carry intent text". Neutral phrasing, not the export's example
/// sentence, and not fabricated run-specific content.
pub const CURRENT_FRAME_INTENT: Placeholder = Placeholder {
    label: "working on this frame",
};

/// `screens/sessions.md:22-24` tags this leaf `data: loop marker exists;
/// copy phrasing placeholder:`. The round(s) are data and pass through;
/// only the sentence around them is the placeholder. Neutral phrasing,
/// never the export's example sentence.
pub fn loop_round_narration(rounds: &str) -> String {
    format!("round {rounds} — the loop runs again")
}
