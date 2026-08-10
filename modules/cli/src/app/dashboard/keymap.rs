//! Task 0148: single source of truth for the dashboard's per-screen key
//! bindings. `handle_key`'s screen arms remain the actual dispatch (moving
//! them behind this table too would mean a second, larger rewrite for no
//! behavior change — see the task's own "keep the map lookup thin" risk
//! note); this module exists so [`super::footer_line`] can no longer drift
//! from what a screen actually binds, since both now read the same
//! `BINDINGS` slice instead of a hand-maintained `format!` string per
//! screen.
//!
//! Case discipline (0148 Phase 2): lowercase is a row verb (acts on the
//! selected item), uppercase is a screen/bulk verb (acts on the whole
//! screen or a marked set). `sync` (whole task board) and `split` (selected
//! task only) were bound backwards — `s` sync / `S` split — until this
//! task; see the 0148 landing evidence for the full inventory and the
//! rebind list.

use super::Screen;
use crate::app::tui_kit::Binding;

/// Bindings shared by every screen, rendered after the screen-specific ones.
pub(super) const GLOBAL_TAIL: &[Binding] = &[
    Binding {
        key: "r",
        hint: "reload",
    },
    Binding {
        key: "Tab/1-5",
        hint: "screens",
    },
    Binding {
        key: "q",
        hint: "quit",
    },
];

const SESSIONS: &[Binding] = &[
    Binding {
        key: "Enter",
        hint: "attach/expand",
    },
    Binding {
        key: "space",
        hint: "expand",
    },
    Binding {
        key: "a",
        hint: "answer",
    },
    Binding {
        key: "s",
        hint: "resume",
    },
    Binding {
        key: "S",
        hint: "story",
    },
    Binding {
        key: "x",
        hint: "stop",
    },
    Binding {
        key: "d",
        hint: "delete",
    },
    Binding {
        key: "n",
        hint: "spawn",
    },
];

const TRAITS: &[Binding] = &[
    Binding {
        key: "a",
        hint: "approve",
    },
    Binding {
        key: "b",
        hint: "block",
    },
    Binding {
        key: "e",
        hint: "edit source",
    },
    Binding {
        key: "x",
        hint: "explain",
    },
];

const MERGES: &[Binding] = &[
    Binding {
        key: "m",
        hint: "merge",
    },
    Binding {
        key: "d",
        hint: "deep-merge",
    },
    Binding {
        key: "p",
        hint: "print path",
    },
    Binding {
        key: "x",
        hint: "drop",
    },
];

// `o` (toggle orphaned) and, on SESSIONS/MERGES, `v` (toggle repo scope) are
// left out of these tables: their hint text is state-dependent
// (show/hide, this-repo/all-repos) rather than a fixed label, so
// `footer_line` renders them itself, after the map-derived part.
const TRUST: &[Binding] = &[
    Binding {
        key: "a",
        hint: "approve",
    },
    Binding {
        key: "b",
        hint: "block",
    },
    Binding {
        key: "space",
        hint: "mark",
    },
    Binding {
        key: "A",
        hint: "approve family/marked",
    },
];

/// Task 0148 rebind: `sync` acts on the whole board (screen-level, so
/// uppercase); `split` acts on the selected task only (row-level, so
/// lowercase). Previously `s`=sync / `S`=split, backwards under the case
/// convention. Old -> new: sync `s`->`S`, split `S`->`s`.
const TASKS: &[Binding] = &[
    Binding {
        key: "space",
        hint: "expand",
    },
    Binding {
        key: "s",
        hint: "split",
    },
    Binding {
        key: "S",
        hint: "sync",
    },
    Binding {
        key: "R",
        hint: "reconcile",
    },
    Binding {
        key: "a",
        hint: "archive",
    },
    Binding {
        key: "e",
        hint: "edit",
    },
    Binding {
        key: "y",
        hint: "mark done",
    },
    Binding {
        key: "d",
        hint: "dispatch",
    },
];

/// The screen-specific bindings shown in that screen's footer, in the order
/// they should render — `GLOBAL_TAIL` (reload/screens/quit) always follows.
pub(super) fn bindings(screen: Screen) -> &'static [Binding] {
    match screen {
        Screen::Sessions => SESSIONS,
        Screen::Traits => TRAITS,
        Screen::Merges => MERGES,
        Screen::Trust => TRUST,
        Screen::Tasks => TASKS,
    }
}

/// Renders a screen's own bindings (not `GLOBAL_TAIL`) as `"key hint  key
/// hint  ..."`, the shape every footer format string already used.
pub(super) fn hint_line(screen: Screen) -> String {
    crate::app::tui_kit::render_hint_line(bindings(screen).iter().copied())
}

/// Renders `GLOBAL_TAIL` the same way, for the fixed suffix every screen's
/// footer shares.
pub(super) fn global_tail_hint() -> String {
    crate::app::tui_kit::render_hint_line(GLOBAL_TAIL.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0148 self-consistency proof (c): no screen binds the same key twice
    /// between its own table and the global tail every screen appends.
    #[test]
    fn no_duplicate_key_within_a_context() {
        for screen in Screen::all() {
            let mut keys: Vec<&str> = bindings(screen).iter().map(|b| b.key).collect();
            keys.extend(GLOBAL_TAIL.iter().map(|b| b.key));
            let mut seen = std::collections::HashSet::new();
            for key in &keys {
                assert!(
                    seen.insert(*key),
                    "{screen:?} binds {key} more than once: {keys:?}"
                );
            }
        }
    }

    /// 0148 self-consistency proof (a): every bound key's hint actually
    /// appears in that screen's rendered footer text.
    #[test]
    fn every_binding_appears_in_its_own_hint_line() {
        for screen in Screen::all() {
            let rendered = hint_line(screen);
            for binding in bindings(screen) {
                let expected = format!("{} {}", binding.key, binding.hint);
                assert!(
                    rendered.contains(&expected),
                    "{screen:?} hint line missing {expected:?}: {rendered}"
                );
            }
        }
    }
}
