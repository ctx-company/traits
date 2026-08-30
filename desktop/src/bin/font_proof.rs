//! Real-platform font-resolution proof for `0265.1`'s bundled IBM Plex
//! faces.
//!
//! `#[gpui::test]` cannot prove anything here: `TestPlatform` installs
//! `NoopTextSystem`, whose `add_fonts` is a no-op returning `Ok` and whose
//! `font_id` returns `Ok(FontId(1))` for every descriptor — a
//! `TestAppContext` font check would pass even with nothing vendored at
//! all. This binary runs against the real platform text system via
//! `gpui::Application::headless()` instead, driven as a subprocess by
//! `desktop/tests/bundled_fonts.rs` so the proof still lands inside
//! `cargo test`.
//!
//! `Application::run` never returns on macOS in headless mode — it blocks
//! forever in `CFRunLoopRun()` after invoking the closure — so the
//! assertions below terminate the process directly rather than returning
//! from the closure.
//!
//! Two modes, selected by argv:
//! - default: register the bundled faces, then require both to resolve to
//!   a bundled `FontId` (not a fallback's). Exit 0 on success.
//! - `--skip-registration`: run the *same* shared verification
//!   (`fonts::verify_family_resolves`) against two guaranteed-absent family
//!   names and require it to fail — the forced-negative control that
//!   proves the check's discrimination logic actually rejects a missing
//!   family. This uses names no host, real or bundled, can ever satisfy
//!   (`GUARANTEED_ABSENT_FAMILY_A`/`_B`), so unlike checking
//!   `FONT_SANS`/`FONT_MONO` unregistered, a machine that happens to have
//!   IBM Plex installed system-wide cannot weaken or short-circuit the
//!   result. Exit 0 only if the check reports a rejection.

use ctx_traits_desktop::fonts::{self, GUARANTEED_ABSENT_FAMILY_A, GUARANTEED_ABSENT_FAMILY_B};
use gpui::{App, Application};

fn main() {
    let skip_registration = std::env::args().any(|arg| arg == "--skip-registration");

    Application::headless().run(move |cx: &mut App| {
        if skip_registration {
            match fonts::verify_family_resolves(
                cx,
                GUARANTEED_ABSENT_FAMILY_A,
                GUARANTEED_ABSENT_FAMILY_B,
            ) {
                Err(reason) => {
                    println!(
                        "ok: verify_family_resolves correctly rejected a guaranteed-absent family: {reason}"
                    );
                    std::process::exit(0);
                }
                Ok(()) => {
                    eprintln!(
                        "fail: the font check is vacuous — it accepted a guaranteed-absent family"
                    );
                    std::process::exit(1);
                }
            }
        }

        if let Err(reason) = fonts::register(cx) {
            eprintln!("fail: register bundled IBM Plex faces: {reason}");
            std::process::exit(1);
        }
        match fonts::verify_bundled(cx) {
            Ok(()) => {
                println!("ok: bundled IBM Plex faces resolve to bundled faces, not a fallback");
                std::process::exit(0);
            }
            Err(reason) => {
                eprintln!("fail: bundled faces did not resolve as bundled: {reason}");
                std::process::exit(1);
            }
        }
    });
}
