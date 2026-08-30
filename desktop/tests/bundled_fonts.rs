//! Drives `ctx-desktop-font-proof` (`src/bin/font_proof.rs`) in both modes
//! so the real-platform font-resolution proof lands inside plain
//! `cargo test` / `just desktop-test`.
//!
//! This has to be a subprocess rather than a `#[gpui::test]`: gpui's own
//! test harness installs `NoopTextSystem`, whose `add_fonts`/`font_id`
//! never touch a real font at all, so a `TestAppContext` version of this
//! check would be vacuous.

use std::process::Command;

fn run_proof(extra_args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ctx-desktop-font-proof"))
        .args(extra_args)
        .output()
        .expect("spawn ctx-desktop-font-proof")
}

#[test]
fn bundled_faces_resolve_and_the_check_is_not_vacuous() {
    let registered = run_proof(&[]);
    assert!(
        registered.status.success(),
        "font proof (default mode) must exit 0 — stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&registered.stdout),
        String::from_utf8_lossy(&registered.stderr),
    );

    let skipped = run_proof(&["--skip-registration"]);
    let skipped_stdout = String::from_utf8_lossy(&skipped.stdout);
    assert!(
        skipped.status.success(),
        "font proof (--skip-registration) must exit 0 by observing verify_family_resolves \
         reject a guaranteed-absent family — stdout: {skipped_stdout}\nstderr: {}",
        String::from_utf8_lossy(&skipped.stderr),
    );
    assert!(
        skipped_stdout.contains("correctly rejected a guaranteed-absent family"),
        "font proof (--skip-registration) must record an actual rejected substitution, not \
         just an exit code — stdout: {skipped_stdout}",
    );
    assert!(
        !skipped_stdout.contains("weakened control"),
        "font proof (--skip-registration) must never report a weakened, host-dependent \
         control — stdout: {skipped_stdout}",
    );
}
