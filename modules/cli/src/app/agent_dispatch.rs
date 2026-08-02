//! Shared conventions for a standing agent's one-shot CLI harness call.
//!
//! `master`/`narrator` (in `drive.rs`) and `merger` (in `merge.rs`) are all
//! self-described standing agents: none inherit the `[agent]` catch-all
//! defaults, and each dispatches through the same CLI transport, env-remove,
//! and argv-building conventions. This module holds that shared surface so
//! neither caller re-implements it.

use camino::Utf8Path;

use ctx_traits_io::harness_config::{
    HarnessCliConvention, HarnessDefinition, HarnessRegistry, RunAssignmentMode, RunTransport,
};

/// Environment variables removed before spawning a harness subprocess, so a
/// host-level provider credential never silently overrides the harness's own
/// configured auth.
pub(crate) fn harness_env_remove(harness: &HarnessDefinition) -> Vec<String> {
    if harness.kind() == "claude-code" {
        vec!["ANTHROPIC_API_KEY".to_string()]
    } else {
        Vec::new()
    }
}

/// Build argv for a standing agent's one-shot call: the harness binary, a
/// base argv selected by `use_narrator_argv`, then model/reasoning-effort/
/// system-prompt flags and any extra args from the resolved assignment.
///
/// `use_narrator_argv` selects between two distinct base argvs and must never
/// be `true` for a merger call: `cli.narrator_argv` (falling back to
/// `cli.argv` when unconfigured) is the narrator's deliberately tiny,
/// tool-less one-shot convention, while `cli.argv` is the normal tool-capable
/// convention a standing merger needs in order to actually edit conflicted
/// files.
pub(crate) fn standing_agent_argv(
    harness: &HarnessDefinition,
    cli: &HarnessCliConvention,
    use_narrator_argv: bool,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    system_prompt: &str,
    extra_args: &[String],
) -> Vec<String> {
    let mut argv = Vec::new();
    argv.push(harness.bin().to_string());
    if use_narrator_argv {
        argv.extend(
            cli.narrator_argv
                .clone()
                .unwrap_or_else(|| cli.argv.clone()),
        );
    } else {
        argv.extend(cli.argv.clone());
    }
    if let (Some(flag), Some(model)) = (cli.model_flag.as_ref(), model) {
        argv.push(flag.clone());
        argv.push(model.to_string());
    }
    append_reasoning_effort(&mut argv, harness, cli, reasoning_effort);
    if let Some(flag) = cli.system_prompt_flag.as_ref() {
        argv.push(flag.clone());
        argv.push(system_prompt.to_string());
    }
    argv.extend(extra_args.to_vec());
    argv
}

/// Append a resolved reasoning effort using the harness's declared mapping.
/// Codex uses its generic config flag rather than a standalone flag.
pub(crate) fn append_reasoning_effort(
    argv: &mut Vec<String>,
    harness: &HarnessDefinition,
    cli: &HarnessCliConvention,
    reasoning_effort: Option<&str>,
) {
    let (Some(flag), Some(effort)) = (cli.reasoning_effort_flag.as_ref(), reasoning_effort) else {
        return;
    };
    argv.push(flag.clone());
    if harness.kind() == "codex" && flag == "--config" {
        argv.push(format!("model_reasoning_effort=\"{effort}\""));
    } else {
        argv.push(effort.to_string());
    }
}

/// Pin a server-anchored CLI harness to the requested execution directory.
/// Process cwd alone is insufficient for harnesses such as OpenCode.
pub(crate) fn append_exec_dir(
    argv: &mut Vec<String>,
    cli: &HarnessCliConvention,
    exec_dir: Option<&Utf8Path>,
) {
    if let (Some(flag), Some(dir)) = (cli.dir_flag.as_ref(), exec_dir) {
        argv.push(flag.clone());
        argv.push(dir.to_string());
    }
}

/// Append the generated per-kind confinement overrides after configured argv,
/// so a configured `--yolo` or conflicting sandbox flag cannot win.
pub(crate) fn append_confinement(
    argv: &mut Vec<String>,
    harness: &HarnessDefinition,
    payloads: Option<&ctx_traits_io::confinement::ConfinementPayloads>,
) {
    let Some(payloads) = payloads else {
        return;
    };
    match harness.kind() {
        "claude-code" => {
            argv.push("--settings".to_string());
            argv.push(payloads.claude_code.to_string());
        }
        "codex" => {
            // The confinement plan is the only authority for Codex's sandbox,
            // working directory, and writable roots. Its bypass and short
            // forms have stronger semantics than argv order.
            let mut normalized = Vec::with_capacity(argv.len());
            let mut args = std::mem::take(argv).into_iter();
            while let Some(arg) = args.next() {
                if matches!(
                    arg.as_str(),
                    "--yolo"
                        | "--dangerously-bypass-approvals-and-sandbox"
                        | "-s"
                        | "--sandbox"
                        | "--add-dir"
                        | "-C"
                        | "--cd"
                ) || arg.starts_with("--sandbox=")
                    || arg.starts_with("--add-dir=")
                    || arg.starts_with("--cd=")
                    || (arg.starts_with("-s") && arg.len() > 2)
                    || (arg.starts_with("-C") && arg.len() > 2)
                {
                    if matches!(
                        arg.as_str(),
                        "-s" | "--sandbox" | "--add-dir" | "-C" | "--cd"
                    ) {
                        let _ = args.next();
                    }
                    continue;
                }
                normalized.push(arg);
            }
            *argv = normalized;
            // `codex exec` does not accept the interactive
            // `--ask-for-approval` option. Its documented config override is
            // valid on `exec` and preserves the same non-interactive policy.
            argv.extend([
                "--config".to_string(),
                "approval_policy=\"never\"".to_string(),
            ]);
            if let Some(sandbox) = payloads
                .codex
                .get("sandbox")
                .and_then(serde_json::Value::as_str)
            {
                argv.extend(["--sandbox".to_string(), sandbox.to_string()]);
            }
            for directory in payloads
                .codex
                .get("add-directory")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                argv.extend(["--add-dir".to_string(), directory.to_string()]);
            }
        }
        _ => {}
    }
}

/// Append a resumed harness conversation id to argv, on both the cold and
/// warm dispatch channels (P516) — a no-op whenever no id is observed or the
/// convention declares neither `session-flag` nor `resume-flag`; that silent
/// case is a reported cold start, not a hidden one (see the drive loop's
/// `harness declares no session-flag or resume-flag` capability report).
pub(crate) fn append_session_resume(
    argv: &mut Vec<String>,
    cli: &HarnessCliConvention,
    session_id: Option<&String>,
) {
    if let Some(session_id) = session_id
        && let Some(flag) = cli.session_flag.as_ref().or(cli.resume_flag.as_ref())
    {
        argv.push(flag.clone());
        argv.push(session_id.clone());
    }
}

/// Validate that a resolved standing-agent assignment is dispatchable over
/// CLI: harness mode (not attach), CLI transport, a known registered harness
/// that declares CLI, a CLI model flag if a model was resolved, and a CLI
/// reasoning-effort flag if a reasoning effort was resolved. Returns the
/// resolved harness/cli convention so callers do not repeat the same
/// registry lookup after validating.
pub(crate) fn validate_cli_standing_agent(
    registry: &HarnessRegistry,
    mode: RunAssignmentMode,
    harness_id: &str,
    transport: RunTransport,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    label: &str,
) -> crate::Result<(HarnessDefinition, HarnessCliConvention)> {
    if mode != RunAssignmentMode::Harness {
        return Err(crate::Error::Command {
            message: format!("{label} requires harness mode; attach mode cannot dispatch"),
        });
    }
    if transport != RunTransport::Cli {
        return Err(crate::Error::Command {
            message: format!("{label} requires transport cli"),
        });
    }
    let harness = registry
        .harness
        .get(harness_id)
        .ok_or_else(|| crate::Error::Command {
            message: format!("{label} references unknown harness {harness_id:?}"),
        })?;
    if !harness.transports.contains(&RunTransport::Cli) {
        return Err(crate::Error::Command {
            message: format!("{label} harness {harness_id:?} does not declare transport cli"),
        });
    }
    let cli = harness.cli.as_ref().ok_or_else(|| crate::Error::Command {
        message: format!("{label} harness {harness_id:?} has no cli configuration"),
    })?;
    if model.is_some() && cli.model_flag.is_none() {
        return Err(crate::Error::Command {
            message: format!(
                "{label} harness {harness_id:?} has a resolved model but no CLI model flag"
            ),
        });
    }
    if reasoning_effort.is_some() && cli.reasoning_effort_flag.is_none() {
        return Err(crate::Error::Command {
            message: format!(
                "{label} harness {harness_id:?} has a resolved reasoning effort but no CLI reasoning-effort flag"
            ),
        });
    }
    Ok((harness.clone(), cli.clone()))
}

/// Optional stdout/tick observers for [`run_one_shot`] — bundled (with the
/// rest of the call in [`RunOneShotRequest`]) so the function stays under the
/// arity lint. `ctx traits merge`'s merger dispatch is the only caller that
/// populates either (P463: a transcript stdout observer, and a tick observer
/// for audible progress on a long deep-merge call); every other
/// `run_one_shot` caller passes `Default::default()`.
#[derive(Default)]
pub(crate) struct RunOneShotObservers {
    pub(crate) stdout_observer: Option<ctx_traits_io::harness::OutputObserver>,
    pub(crate) tick_observer: Option<ctx_traits_io::harness::TickObserver>,
}

pub(crate) struct RunOneShotRequest<'a> {
    pub(crate) harness: &'a HarnessDefinition,
    pub(crate) argv: Vec<String>,
    pub(crate) prompt: String,
    pub(crate) prompt_via_stdin: bool,
    pub(crate) timeout_ms: u64,
    pub(crate) exec_dir: Option<&'a Utf8Path>,
    pub(crate) env_overlay: &'a std::collections::BTreeMap<String, String>,
    pub(crate) observers: RunOneShotObservers,
    /// P480: this worktree's generated OS-level spawn sandbox, `None` for a
    /// non-worktree call, `sandbox = false`, or an unsupported platform.
    pub(crate) sandbox: Option<ctx_traits_io::confinement::SpawnSandbox>,
}

/// Run one non-streaming, one-shot CLI harness call and return its outcome.
/// Shared by the narrator's out-of-loop calls and `ctx traits merge`'s
/// merger dispatch — neither needs the drive loop's retry/warm-session
/// machinery, just a single bounded call.
pub(crate) fn run_one_shot(
    request: RunOneShotRequest<'_>,
) -> crate::Result<ctx_traits_io::harness::HarnessRunOutcome> {
    let prompt_delivery = if request.prompt_via_stdin {
        ctx_traits_io::harness::PromptDelivery::Stdin
    } else {
        ctx_traits_io::harness::PromptDelivery::Arg
    };
    ctx_traits_io::harness::run(ctx_traits_io::harness::HarnessRunRequest {
        argv: request.argv,
        env_overlay: request.env_overlay.clone(),
        env_remove: harness_env_remove(request.harness),
        prompt: request.prompt,
        prompt_delivery,
        timeout_ms: request.timeout_ms,
        idle_timeout_ms: None,
        capture_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
        stream: false,
        stdout_observer: request.observers.stdout_observer,
        tick_observer: request.observers.tick_observer,
        exec_dir: request.exec_dir.map(Utf8Path::to_path_buf),
        sandbox: request.sandbox,
    })
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{append_confinement, append_exec_dir, standing_agent_argv};
    use ctx_traits_io::confinement::ConfinementPayloads;
    use ctx_traits_io::harness_config::{
        HarnessCliConvention, HarnessDefinition, HarnessRegistry, built_in_harness_definition,
    };

    fn harness(kind: &str) -> HarnessDefinition {
        HarnessDefinition {
            bin: Some(kind.to_string()),
            kind: Some(kind.to_string()),
            transports: vec![],
            version_probe: vec![],
            cli: None,
            mcp: None,
        }
    }

    fn payloads() -> ConfinementPayloads {
        ConfinementPayloads {
            claude_code: serde_json::json!({"sandbox": {"enabled": true}}),
            opencode: serde_json::json!({}),
            codex: serde_json::json!({"sandbox": "workspace-write", "add-directory": ["/tmp/worktree"]}),
            spawn_sandbox: None,
            sandbox_requested: true,
        }
    }

    #[test]
    fn appends_settings_flag_for_claude_code_when_confinement_present() {
        let mut argv = vec!["claude".to_string(), "-p".to_string()];
        let payloads = payloads();

        append_confinement(&mut argv, &harness("claude-code"), Some(&payloads));

        assert_eq!(
            argv,
            [
                "claude",
                "-p",
                "--settings",
                &payloads.claude_code.to_string()
            ]
        );
    }

    #[test]
    fn leaves_argv_unchanged_without_confinement_payload() {
        let mut argv = vec!["claude".to_string(), "-p".to_string()];
        append_confinement(&mut argv, &harness("claude-code"), None);
        assert_eq!(argv, ["claude", "-p"]);
    }

    #[test]
    fn leaves_argv_unchanged_for_a_non_claude_code_harness() {
        let mut argv = vec!["opencode".to_string(), "run".to_string()];
        let payloads = payloads();
        append_confinement(&mut argv, &harness("opencode"), Some(&payloads));
        assert_eq!(argv, ["opencode", "run"]);
    }

    #[test]
    fn appends_codex_overrides_last() {
        let mut argv = vec![
            "codex".to_string(),
            "--yolo".to_string(),
            "--sandbox=danger-full-access".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
            "-s".to_string(),
            "danger-full-access".to_string(),
            "--add-dir".to_string(),
            "/main".to_string(),
            "--add-dir=/other".to_string(),
            "-C".to_string(),
            "/main".to_string(),
            "--cd=/other".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
        ];
        let payloads = payloads();
        append_confinement(&mut argv, &harness("codex"), Some(&payloads));
        assert_eq!(
            argv,
            [
                "codex",
                "--config",
                "approval_policy=\"never\"",
                "--sandbox",
                "workspace-write",
                "--add-dir",
                "/tmp/worktree"
            ]
        );
    }

    #[test]
    fn standing_agent_codex_argv_retains_subagent_controls() {
        let harness = built_in_harness_definition("codex", &HarnessRegistry::default());
        let cli = harness.cli.as_ref().expect("Codex has a CLI convention");

        let argv = standing_agent_argv(&harness, cli, false, None, Some("high"), "system", &[]);

        assert_eq!(
            argv,
            [
                "codex",
                "exec",
                "--json",
                "--config",
                "approval_policy=\"never\"",
                "--config",
                "agents.enabled=false",
                "--config",
                "features.multi_agent_v2=false",
                "--config",
                "model_reasoning_effort=\"high\"",
            ]
        );
    }

    fn cli(dir_flag: Option<&str>) -> HarnessCliConvention {
        HarnessCliConvention {
            argv: Vec::new(),
            narrator_argv: None,
            warm_argv: None,
            json_schema_flag: None,
            model_flag: None,
            reasoning_effort_flag: None,
            system_prompt_flag: None,
            resume_flag: None,
            session_flag: None,
            dir_flag: dir_flag.map(str::to_string),
            prompt_via: None,
            stream: Some(false),
            output: Some("opencode-json".to_string()),
        }
    }

    #[test]
    fn appends_declared_execution_directory_flag() {
        let mut argv = vec!["opencode".to_string(), "run".to_string()];

        append_exec_dir(
            &mut argv,
            &cli(Some("--dir")),
            Some(camino::Utf8Path::new("/tmp/worktree")),
        );

        assert_eq!(argv, ["opencode", "run", "--dir", "/tmp/worktree"]);
    }

    #[test]
    fn leaves_argv_unchanged_without_directory_convention() {
        let mut argv = vec!["claude".to_string(), "-p".to_string()];

        append_exec_dir(
            &mut argv,
            &cli(None),
            Some(camino::Utf8Path::new("/tmp/worktree")),
        );

        assert_eq!(argv, ["claude", "-p"]);
    }
}
