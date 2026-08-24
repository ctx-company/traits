//! Process-boundary smoke proof for the private center sentinel.

#[test]
fn center_sentinel_is_not_a_supported_clap_command() {
    // The sentinel is consumed before Clap by the binary entry point. Keeping
    // it absent from help prevents it becoming a supported user command.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ctx"))
        .arg("--help")
        .output()
        .expect("run ctx help");
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("__ctx-center"));
}
