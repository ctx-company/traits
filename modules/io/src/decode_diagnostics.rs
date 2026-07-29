//! Shared presentation for canonical-decode deprecation warnings.
//!
//! `ctx_traits_core::encoding::decode_trait_with_warnings` tolerates retired
//! top-level `status`/`trust` keys on a canonical trait document for one
//! transition period, returning one warning string per deprecated field found
//! instead of hard-failing. Every native (CLI/IO) call site that decodes a
//! trait document this way needs to surface those warnings to the user in
//! the same `ctx traits: <label>: <warning>` shape; this is the single place
//! that does it, so the presentation format changes in exactly one place
//! instead of N duplicated print loops.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// P244 fix (`inline-pane-stderr-interleave`): while a ratatui inline
/// viewport (`Viewport::Inline`) owns the terminal, ANY raw stderr write
/// scrolls the user's real screen out from under the pane without ratatui
/// knowing — the alternate screen used to absorb this silently, but an
/// inline viewport desyncs permanently for the rest of the run once that
/// happens. `load_trait_for_session`/`resolved_frame_prompt` on the drive
/// loop's per-frame path both reach [`print_decode_warnings`] for any trait
/// or dependency manifest that decodes with warnings, so this capture
/// toggle is the routed sink the CLI's inline pane installs for its
/// lifetime instead of patching those two call sites individually.
static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Starts routing every subsequent [`print_decode_warnings`] line into an
/// in-memory buffer instead of `stderr`, until [`end_capture`] is called.
/// Callers own pairing this with a matching `end_capture` — this module has
/// no lifecycle of its own to enforce it.
pub fn begin_capture() {
    CAPTURING.store(true, Ordering::SeqCst);
}

/// Stops capturing and returns every line captured since the matching
/// [`begin_capture`], in order. Idempotent when capture was already off (an
/// empty vec) — safe to call defensively from a restore path that cannot
/// know whether a matching commit already drained the buffer.
pub fn end_capture() -> Vec<String> {
    CAPTURING.store(false, Ordering::SeqCst);
    let mut captured = CAPTURED.lock().unwrap_or_else(|poison| poison.into_inner());
    std::mem::take(&mut *captured)
}

/// Print one `ctx traits: <label>: <warning>` line per decode warning, in
/// the order given — or, while [`begin_capture`] is active, buffer it for
/// [`end_capture`] to return instead. `label` identifies what was decoded (a
/// manifest path, a built-in package id, a source description, ...).
pub fn print_decode_warnings(label: &str, warnings: &[String]) {
    for warning in warnings {
        let line = format!("ctx traits: {label}: {warning}");
        if CAPTURING.load(Ordering::SeqCst) {
            let mut captured = CAPTURED.lock().unwrap_or_else(|poison| poison.into_inner());
            captured.push(line);
        } else {
            eprintln!("{line}");
        }
    }
}
