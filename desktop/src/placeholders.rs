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

/// `screens/sessions.md:38-39` tags the preview `in progress` block's
/// narrated line `placeholder:` — the title and marker are data, the
/// sentence is not. Neutral phrasing, never the export's example sentence,
/// never run-specific content.
pub const NOW_NARRATION: Placeholder = Placeholder {
    label: "working on this frame now",
};

/// `screens/sessions.md:14-18` tags the header summary's prose phrasing
/// `placeholder:` — the run description and the frame counter are data and
/// pass through; only the sentence around them is the placeholder. Neutral
/// phrasing, never the export's example sentence.
pub fn sessions_summary(description: Option<&str>, counter: Option<&str>) -> String {
    let base = match description {
        Some(description) => description.to_string(),
        None => "no run description yet".to_string(),
    };
    match counter {
        Some(counter) => format!("{base} \u{b7} {counter}"),
        None => base,
    }
}

/// `grammar.md:98` sets this value; the center serves no owner identity
/// today, so it resolves here rather than as a literal at the rail's call
/// site. A plain `&str`, not a [`Placeholder`]: that struct is documented
/// as a labelled *action* with no bound behaviour, and this is unserved
/// data, not an action.
pub const OWNER_HANDLE: &str = "Oskar Cieslik";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_summary_never_returns_an_empty_string() {
        assert_eq!(sessions_summary(None, None), "no run description yet");
    }

    #[test]
    fn sessions_summary_description_alone_has_no_separator() {
        let summary = sessions_summary(Some("review the plan"), None);
        assert_eq!(summary, "review the plan");
        assert!(!summary.contains(" \u{b7} "));
    }

    #[test]
    fn sessions_summary_appends_counter_after_a_single_dot() {
        let summary = sessions_summary(Some("review the plan"), Some("frame 2 of 5"));
        assert_eq!(summary, "review the plan \u{b7} frame 2 of 5");
        assert_eq!(summary.matches('\u{b7}').count(), 1);
    }

    #[test]
    fn sessions_summary_counter_with_no_description_appends_once() {
        let summary = sessions_summary(None, Some("frame 2 of 5"));
        assert_eq!(summary, "no run description yet \u{b7} frame 2 of 5");
        assert_eq!(summary.matches('\u{b7}').count(), 1);
    }
}
