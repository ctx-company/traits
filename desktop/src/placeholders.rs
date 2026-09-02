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

/// Narration around the served Tasks-board counts. The numbers themselves are
/// supplied by the board projection, never inferred by a view.
pub fn board_summary(open: usize, states: usize, live: usize, waiting: usize) -> String {
    format!(
        "{open} {} across {states} {} — {live} live, {waiting} waiting",
        if open == 1 { "open task" } else { "open tasks" },
        if states == 1 { "state" } else { "states" },
    )
}

/// Neutral narration around served authored-row and provenance facts.
pub fn traits_summary(authored: usize, pinned: bool) -> String {
    format!(
        "{authored} {} — {}",
        if authored == 1 {
            "authored trait"
        } else {
            "authored traits"
        },
        if pinned {
            "library pinned"
        } else {
            "provenance unavailable"
        },
    )
}

/// The Traits screen's deliberately inert authoring action.
pub const AUTHOR_TRAIT: Placeholder = Placeholder {
    label: "author trait",
};

/// Neutral narration around the Config screen's served counts.
pub fn config_summary(seats: usize, engines: usize, trust: usize) -> String {
    format!(
        "{seats} {} · {engines} {} · {trust} trusted {}",
        if seats == 1 { "seat" } else { "seats" },
        if engines == 1 { "engine" } else { "engines" },
        if trust == 1 { "member" } else { "members" },
    )
}

/// The Config screen's deliberately inert source-editing action.
pub const EDIT_RUNTIME_TOML: Placeholder = Placeholder {
    label: "edit runtime.toml",
};

/// `screens/config.md:31` leaves the selected seat's explanatory prose as
/// unserved data. This is data-shaped text, not an inert action placeholder.
pub const SEAT_PROSE: &str = "configuration resolved for this physical seat";

/// `grammar.md:98` sets this value; the center serves no owner identity
/// today, so it resolves here rather than as a literal at the rail's call
/// site. A plain `&str`, not a [`Placeholder`]: that struct is documented
/// as a labelled *action* with no bound behaviour, and this is unserved
/// data, not an action.
pub const OWNER_HANDLE: &str = "Oskar Cieslik";

/// Placeholder-served content for one Merges row. `screens/merges.md:23-29`
/// authorizes the static rows; headings and state roles remain interface and
/// presentation concerns outside this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeRowContent {
    pub title: &'static str,
    pub description: &'static str,
    pub state_detail: Option<&'static str>,
    pub meta: &'static str,
}

/// `screens/merges.md:23-29` placeholder-served Landing content.
pub const MERGE_LANDING_ROWS: &[MergeRowContent] = &[MergeRowContent {
    title: "doctor schema guard",
    description: "run-1a2b3c → main · guarded-change · closes 0257",
    state_detail: Some("deep merge"),
    meta: "gates green · +27 −1",
}];

/// `screens/merges.md:23-29` placeholder-served Awaiting approval content.
pub const MERGE_AWAITING_ROWS: &[MergeRowContent] = &[
    MergeRowContent {
        title: "0252.1 — one output grammar",
        description: "run-99ffe1 → main · every command ends in a panel",
        state_detail: None,
        meta: "+214 −180 · gates green",
    },
    MergeRowContent {
        title: "0253.1 — signals carry payloads",
        description: "run-77cd10 → main · schema'd emissions",
        state_detail: None,
        meta: "+96 −12 · security signed",
    },
];

/// `screens/merges.md:23-29` placeholder-served Landed content. The export's
/// relative-time prose is corrected to run-to-target form; `landed` is Ok.
pub const MERGE_LANDED_ROWS: &[MergeRowContent] = &[
    MergeRowContent {
        title: "0243.3 — a closed task never re-derives to blocked",
        description: "run-7e90ad11 → main · merge complete",
        state_detail: None,
        meta: "7e90ad11",
    },
    MergeRowContent {
        title: "0253.2 — agent intent authoring",
        description: "run-a0ba4f87 → main · schema 0.6",
        state_detail: None,
        meta: "a0ba4f87",
    },
];

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

    #[test]
    fn merge_rows_keep_the_run_to_target_shape() {
        for row in MERGE_LANDING_ROWS
            .iter()
            .chain(MERGE_AWAITING_ROWS)
            .chain(MERGE_LANDED_ROWS)
        {
            assert!(row.description.contains(" → main · "));
            assert!(!row.description.ends_with('·'));
        }
    }
}
