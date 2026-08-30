//! Bundled IBM Plex faces: one registration path, one verification.
//!
//! `0256.1` established no asset mechanism (no `AssetSource`, no
//! `desktop/assets/` directory), so this is the first, not a second.
//! gpui's `TextSystem::add_fonts` consumes bytes directly; introducing an
//! `AssetSource` to hand it two static blobs would be the extra mechanism,
//! not the reuse.
//!
//! Faces are vendored from the IBM Plex GitHub release `v6.4.2`
//! (`github.com/IBM/plex`, commit `242c4cccd37e87985a5337815c99b960ef13c65c`)
//! — see `desktop/assets/fonts/README.md` for the exact upstream paths and
//! per-file sha256.

use gpui::{App, font};
use std::borrow::Cow;

use crate::tokens::{FONT_MONO, FONT_SANS};

const SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");

/// A family name that cannot exist. Used by [`verify_family_resolves`] as a
/// negative control: a family that fell through to gpui's fallback stack
/// would otherwise resolve successfully, indistinguishable from a bundled
/// face.
const ABSENT_SENTINEL: &str = "ctx-desktop-absent-family-sentinel";

/// Two family names guaranteed to exist on no host, real or bundled. Used
/// by the font-proof binary's negative control
/// (`src/bin/font_proof.rs --skip-registration`) to prove that
/// [`verify_family_resolves`] actually rejects a missing family —
/// deterministically, regardless of what a given machine has installed.
/// Unlike checking `FONT_SANS`/`FONT_MONO` without registering them first,
/// these names can never legitimately be satisfied by a host's own font
/// collection, so the control cannot be weakened by system state.
pub const GUARANTEED_ABSENT_FAMILY_A: &str = "ctx-desktop-guaranteed-absent-family-a";
pub const GUARANTEED_ABSENT_FAMILY_B: &str = "ctx-desktop-guaranteed-absent-family-b";

/// The one registration path. Both `main.rs` and the font-proof binary
/// (`src/bin/font_proof.rs`) call exactly this — the proof must exercise
/// production registration, not a copy of it.
pub fn register(cx: &App) -> Result<(), String> {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(SANS_REGULAR),
            Cow::Borrowed(MONO_REGULAR),
        ])
        .map_err(|error| error.to_string())
}

/// The shared membership-and-discrimination check: `family` must appear in
/// the text system's known font names, must not resolve to the same
/// `FontId` as an absent sentinel (which would mean it fell through to
/// gpui's fallback stack instead of a real face), and must not resolve to
/// the same `FontId` as `other_family` (which would mean a duplicated or
/// wrong-name-table vendored file). `TextSystem::resolve_font` silently
/// walks a fallback stack, so "it resolved to *something*" is not
/// evidence — this is the actual discriminator.
///
/// Used both by [`verify_bundled`] (against the real bundled families) and
/// by the font-proof binary's negative control (against
/// [`GUARANTEED_ABSENT_FAMILY_A`]/`_B`, which can never be satisfied by any
/// host's own collection) — the same logic proves both that the bundled
/// faces are real and that the check can detect a missing family.
pub fn verify_family_resolves(
    cx: &App,
    family: &'static str,
    other_family: &'static str,
) -> Result<(), String> {
    let text_system = cx.text_system();
    let known_names = text_system.all_font_names();
    if !known_names.iter().any(|name| name == family) {
        return Err(format!(
            "{family} did not register as a font family (known families: {known_names:?})"
        ));
    }

    let sentinel_id = text_system.resolve_font(&font(ABSENT_SENTINEL));
    let family_id = text_system.resolve_font(&font(family));
    let other_id = text_system.resolve_font(&font(other_family));

    if family_id == sentinel_id {
        return Err(format!(
            "{family} resolved to the same FontId as an absent sentinel family — it fell through to a fallback rather than the bundled face"
        ));
    }
    if family_id == other_id {
        return Err(format!(
            "{family} and {other_family} resolved to the same FontId — check for a duplicated or wrong-name-table vendored file"
        ));
    }

    Ok(())
}

/// Every family the repainted call sites request must resolve to a
/// *bundled* face, not a fallback.
pub fn verify_bundled(cx: &App) -> Result<(), String> {
    verify_family_resolves(cx, FONT_SANS, FONT_MONO)?;
    verify_family_resolves(cx, FONT_MONO, FONT_SANS)?;
    Ok(())
}
