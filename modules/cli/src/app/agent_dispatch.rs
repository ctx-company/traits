//! Shared conventions for a standing agent's one-shot CLI harness call.
//!
//! `master`/`narrator` (in `drive.rs`) and `merger` (in `merge.rs`) are all
//! self-described standing agents: none inherit the `[agent]` catch-all
//! defaults, and each dispatches through the same CLI transport, env-remove,
//! and argv-building conventions. This module holds that shared surface so
//! neither caller re-implements it.

use camino::Utf8Path;

use ctx_traits_io::harness_config::{
    HarnessCliConvention, HarnessDefinition, HarnessRegistry, ProfileAssignment, ProviderWire,
    RunAssignmentMode, RunTransport,
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

/// Build the strictly tool-less one-shot argv used by the live guide. Unlike
/// narrator's historical convenience path, this never falls back to `argv`:
/// an omitted `narrator-argv` means the harness has not declared a safe
/// tool-less convention.
pub(crate) fn tool_less_standing_agent_argv(
    harness: &HarnessDefinition,
    cli: &HarnessCliConvention,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    system_prompt: &str,
) -> crate::Result<Vec<String>> {
    if cli.narrator_argv.as_ref().is_none_or(Vec::is_empty) {
        return Err(crate::Error::Command {
            message: "guide requires an explicit non-empty cli.narrator-argv tool-less convention"
                .to_string(),
        });
    }
    let mut argv = Vec::new();
    argv.push(harness.bin().to_string());
    argv.extend(cli.narrator_argv.clone().unwrap_or_default());
    if let (Some(flag), Some(model)) = (cli.model_flag.as_ref(), model) {
        argv.push(flag.clone());
        argv.push(model.to_string());
    }
    append_reasoning_effort(&mut argv, harness, cli, reasoning_effort);
    if let Some(flag) = cli.system_prompt_flag.as_ref() {
        argv.push(flag.clone());
        argv.push(system_prompt.to_string());
    }
    Ok(argv)
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

// ---------------------------------------------------------------------------
// 0079: `transport = "api"` resolution and dispatch
// ---------------------------------------------------------------------------

/// One resolved `transport = "api"` seat's request-shaped fields, with every
/// default (timeout, retries) already applied.
#[derive(Clone)]
pub(crate) struct ApiSeatRequest {
    pub(crate) base_url: String,
    pub(crate) wire: ProviderWire,
    pub(crate) model: String,
    /// The resolved credential VALUE. Held only for the duration of the one
    /// call this is built for — never logged, serialized, or echoed.
    pub(crate) api_key: String,
    pub(crate) connect_timeout_ms: u64,
    pub(crate) read_timeout_ms: u64,
    pub(crate) retries: u32,
}

/// Whether a resolved standing-agent assignment can dispatch over the native
/// api provider client, and if not, why. Config resolution already rejects
/// `transport = "api"` on the worker/driver seat and requires `base-url`/
/// `model` when it is declared, so this only has to resolve the credential
/// — a run must never fail because a status line had no key (0030's rule):
/// `Unavailable` is the caller's signal to degrade to the seat's harness
/// declaration, or skip with a `doctor`-visible "unavailable" if none.
pub(crate) enum ApiSeatResolution {
    /// The seat did not declare `transport = "api"` — every existing config
    /// takes this path unchanged.
    NotConfigured,
    Unavailable {
        reason: String,
    },
    Ready(ApiSeatRequest),
}

pub(crate) fn resolve_api_seat(assignment: &ProfileAssignment) -> ApiSeatResolution {
    if assignment.transport != Some(RunTransport::Api) {
        return ApiSeatResolution::NotConfigured;
    }
    let (Some(base_url), Some(model)) = (assignment.api.base_url.clone(), assignment.model.clone())
    else {
        // Defense in depth: `validate_api_transport` already requires both
        // whenever `transport = "api"` is declared, so this path is not the
        // primary guard.
        return ApiSeatResolution::Unavailable {
            reason: "transport = \"api\" is missing base-url or model".to_string(),
        };
    };
    let wire = assignment.api.wire.unwrap_or(ProviderWire::OpenaiCompat);
    let resolved_key = assignment
        .api
        .api_key_env
        .as_deref()
        .and_then(ctx_traits_io::env_reference::resolve_env_var_reference);
    let Some(api_key) = resolved_key else {
        let name = assignment
            .api
            .api_key_env
            .as_deref()
            .unwrap_or("(none declared)");
        return ApiSeatResolution::Unavailable {
            reason: format!("api-key-env {name:?} does not resolve"),
        };
    };
    ApiSeatResolution::Ready(ApiSeatRequest {
        base_url,
        wire,
        model,
        api_key,
        connect_timeout_ms: assignment
            .api
            .connect_timeout_ms
            .unwrap_or(ctx_traits_io::provider_client::DEFAULT_CONNECT_TIMEOUT_MS),
        read_timeout_ms: assignment
            .api
            .read_timeout_ms
            .unwrap_or(ctx_traits_io::provider_client::DEFAULT_READ_TIMEOUT_MS),
        retries: assignment
            .api
            .retries
            .unwrap_or(ctx_traits_io::provider_client::DEFAULT_RETRIES),
    })
}

/// Resolution-owned precedence between a seat's `transport = "api"`
/// declaration and its harness declaration, so `doctor` and every dispatch
/// call site agree by construction instead of each call site improvising its
/// own fallback order (0079 risk: "fallback ordering ambiguity"). Api wins
/// whenever its key resolves; a declared harness is the degrade target
/// whenever it does not (0030's "never fail" rule); `Unavailable` only when
/// neither applies.
pub(crate) enum SeatDispatch {
    /// The seat did not declare `transport = "api"` at all — ordinary Cli
    /// resolution proceeds exactly as it did before 0079.
    NotConfigured,
    Api(ApiSeatRequest),
    /// Degrade to the seat's own harness declaration over the Cli transport.
    Harness,
    Unavailable {
        reason: String,
    },
}

pub(crate) fn resolve_seat_dispatch(assignment: &ProfileAssignment) -> SeatDispatch {
    match resolve_api_seat(assignment) {
        ApiSeatResolution::NotConfigured => SeatDispatch::NotConfigured,
        ApiSeatResolution::Ready(request) => SeatDispatch::Api(request),
        ApiSeatResolution::Unavailable { reason } => {
            if assignment.harness.is_some() {
                SeatDispatch::Harness
            } else {
                SeatDispatch::Unavailable { reason }
            }
        }
    }
}

/// One blocking round trip through the resolved api seat. Callers own the
/// system/user prompt shape and the fallback-to-harness degrade on `Err`.
pub(crate) fn dispatch_api_seat(
    request: &ApiSeatRequest,
    system: Option<&str>,
    user: &str,
    max_tokens: u32,
) -> Result<ctx_traits_io::provider_client::ProviderResponse, ctx_traits_io::provider_client::Error>
{
    ctx_traits_io::provider_client::dispatch(&ctx_traits_io::provider_client::ProviderRequest {
        base_url: &request.base_url,
        wire: request.wire,
        model: &request.model,
        api_key: &request.api_key,
        system,
        user,
        max_tokens,
        connect_timeout_ms: request.connect_timeout_ms,
        read_timeout_ms: request.read_timeout_ms,
        retries: request.retries,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ApiSeatResolution, SeatDispatch, append_confinement, append_exec_dir, dispatch_api_seat,
        resolve_api_seat, resolve_seat_dispatch, standing_agent_argv,
        tool_less_standing_agent_argv,
    };
    use ctx_traits_io::confinement::ConfinementPayloads;
    use ctx_traits_io::harness_config::ProviderWire;
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
            billing: None,
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

    #[test]
    fn guide_tool_less_refuses_normal_cli_fallback() {
        let harness = harness("custom");
        let cli = HarnessCliConvention {
            argv: vec!["run-with-tools".to_string()],
            narrator_argv: None,
            warm_argv: None,
            json_schema_flag: None,
            model_flag: None,
            reasoning_effort_flag: None,
            system_prompt_flag: None,
            resume_flag: None,
            session_flag: None,
            dir_flag: None,
            prompt_via: None,
            stream: None,
            output: None,
        };
        assert!(tool_less_standing_agent_argv(&harness, &cli, None, None, "system").is_err());
    }

    #[test]
    fn guide_tool_less_keeps_only_declared_safe_and_typed_arguments() {
        let harness = harness("custom");
        let cli = HarnessCliConvention {
            argv: vec!["unsafe".to_string()],
            narrator_argv: Some(vec!["safe".to_string()]),
            model_flag: Some("--model".to_string()),
            reasoning_effort_flag: Some("--reasoning".to_string()),
            system_prompt_flag: Some("--system".to_string()),
            warm_argv: None,
            json_schema_flag: None,
            resume_flag: None,
            session_flag: None,
            dir_flag: None,
            prompt_via: None,
            stream: None,
            output: None,
        };
        assert_eq!(
            tool_less_standing_agent_argv(&harness, &cli, Some("m"), Some("low"), "system")
                .unwrap(),
            [
                "custom",
                "safe",
                "--model",
                "m",
                "--reasoning",
                "low",
                "--system",
                "system"
            ]
        );
    }

    #[test]
    fn guide_tool_less_never_forwards_untrusted_assignment_arguments() {
        let harness = harness("custom");
        let cli = HarnessCliConvention {
            argv: vec!["unsafe".to_string(), "--enable-tools".to_string()],
            narrator_argv: Some(vec!["safe".to_string()]),
            model_flag: None,
            reasoning_effort_flag: None,
            system_prompt_flag: None,
            warm_argv: None,
            json_schema_flag: None,
            resume_flag: None,
            session_flag: None,
            dir_flag: None,
            prompt_via: None,
            stream: None,
            output: None,
        };
        let argv = tool_less_standing_agent_argv(&harness, &cli, None, None, "system").unwrap();
        assert_eq!(argv, ["custom", "safe"]);
        assert!(!argv.iter().any(|arg| arg == "--enable-tools"));
    }

    #[test]
    fn guide_tool_less_rejects_resolved_assignment_extra_args() {
        // Assignment validation rejects this before dispatch. The argv builder
        // has no extra-argument parameter, which is the second line of
        // defense for a resolved guide assignment.
        let assignment: ctx_traits_io::harness_config::ProfileAssignment =
            toml::from_str("extra-args = [\"--enable-tools\"]").unwrap();
        assert!(!assignment.extra_args.is_empty());
        let harness = harness("custom");
        let cli = HarnessCliConvention {
            argv: vec!["unsafe".to_string()],
            narrator_argv: Some(vec!["safe".to_string()]),
            model_flag: None,
            reasoning_effort_flag: None,
            system_prompt_flag: None,
            warm_argv: None,
            json_schema_flag: None,
            resume_flag: None,
            session_flag: None,
            dir_flag: None,
            prompt_via: None,
            stream: None,
            output: None,
        };
        let argv = tool_less_standing_agent_argv(&harness, &cli, None, None, "system").unwrap();
        assert_eq!(argv, ["custom", "safe"]);
        assert!(!argv.iter().any(|arg| assignment.extra_args.contains(arg)));
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

    fn api_assignment() -> ctx_traits_io::harness_config::ProfileAssignment {
        let mut assignment: ctx_traits_io::harness_config::ProfileAssignment = toml::from_str(
            "transport = \"api\"\nmodel = \"gpt-4o-mini\"\nbase-url = \"https://example.invalid\"\n",
        )
        .expect("api assignment decodes");
        assignment.api.api_key_env =
            Some(ctx_traits_io::env_reference::TESTHOOK_API_TRANSPORT_MISSING_KEY.to_string());
        assignment
    }

    #[test]
    fn resolve_api_seat_is_not_configured_for_a_cli_transport_seat() {
        let assignment = ctx_traits_io::harness_config::ProfileAssignment::default();
        assert!(matches!(
            resolve_api_seat(&assignment),
            ApiSeatResolution::NotConfigured
        ));
    }

    #[test]
    fn resolve_api_seat_degrades_when_key_env_does_not_resolve() {
        // Missing env var → run must never fail because a status line had
        // no key (0030's rule) — the caller degrades to its harness
        // declaration on `Unavailable`, never a hard error here.
        assert!(
            std::env::var(ctx_traits_io::env_reference::TESTHOOK_API_TRANSPORT_MISSING_KEY)
                .is_err()
        );
        let assignment = api_assignment();
        match resolve_api_seat(&assignment) {
            ApiSeatResolution::Unavailable { reason } => {
                assert!(
                    reason
                        .contains(ctx_traits_io::env_reference::TESTHOOK_API_TRANSPORT_MISSING_KEY)
                );
            }
            _ => panic!("expected Unavailable for an unresolved key reference"),
        }
    }

    #[test]
    fn resolve_api_seat_applies_default_timeouts_and_retries_when_undeclared() {
        // Reuses an env var already present in every test process (PATH) to
        // exercise the `Ready` branch without `std::env::set_var`, which is
        // `unsafe` on this edition and process-wide/thread-unsafe to boot.
        let mut assignment = api_assignment();
        assignment.api.api_key_env = Some("PATH".to_string());
        match resolve_api_seat(&assignment) {
            ApiSeatResolution::Ready(request) => {
                assert_eq!(request.base_url, "https://example.invalid");
                assert_eq!(request.model, "gpt-4o-mini");
                assert_eq!(request.wire, ProviderWire::OpenaiCompat);
                assert_eq!(
                    request.connect_timeout_ms,
                    ctx_traits_io::provider_client::DEFAULT_CONNECT_TIMEOUT_MS
                );
                assert_eq!(
                    request.read_timeout_ms,
                    ctx_traits_io::provider_client::DEFAULT_READ_TIMEOUT_MS
                );
                assert_eq!(
                    request.retries,
                    ctx_traits_io::provider_client::DEFAULT_RETRIES
                );
                assert!(!request.api_key.is_empty());
            }
            _ => panic!("expected Ready when the key reference resolves"),
        }
    }

    #[test]
    fn seat_dispatch_degrades_to_the_declared_harness_when_the_key_env_does_not_resolve() {
        // 0079 blocker missing-key-fallback-never-reaches-harness: an
        // unresolved api key with a declared harness must resolve to the
        // harness dispatch, never `Unavailable`.
        let mut assignment = api_assignment();
        assignment.harness = Some("claude-code".to_string());
        assert!(matches!(
            resolve_seat_dispatch(&assignment),
            SeatDispatch::Harness
        ));
    }

    #[test]
    fn seat_dispatch_is_unavailable_when_the_key_env_does_not_resolve_and_no_harness_is_declared() {
        let assignment = api_assignment();
        assert!(assignment.harness.is_none());
        match resolve_seat_dispatch(&assignment) {
            SeatDispatch::Unavailable { .. } => {}
            _ => panic!("expected Unavailable when neither api nor a harness is dispatchable"),
        }
    }

    #[test]
    fn seat_dispatch_prefers_api_over_a_declared_harness_when_the_key_resolves() {
        let mut assignment = api_assignment();
        assignment.harness = Some("claude-code".to_string());
        assignment.api.api_key_env = Some("PATH".to_string());
        assert!(matches!(
            resolve_seat_dispatch(&assignment),
            SeatDispatch::Api(_)
        ));
    }

    #[test]
    fn seat_dispatch_is_not_configured_for_a_cli_transport_seat() {
        let assignment = ctx_traits_io::harness_config::ProfileAssignment::default();
        assert!(matches!(
            resolve_seat_dispatch(&assignment),
            SeatDispatch::NotConfigured
        ));
    }

    /// Live end-to-end probe of the narrator seat, exercising the EXACT
    /// production seam a session title / narration turn uses: resolve the
    /// machine's real runtime config, take the narrator assignment, classify
    /// it through [`resolve_seat_dispatch`], and — for an api seat — perform
    /// one real [`dispatch_api_seat`] round trip, asserting non-empty
    /// narration text.
    ///
    /// `#[ignore]`: this reads the invoking machine's own config, needs the
    /// declared api-key env and the network, and spends one (flash-tier)
    /// model call — a diagnostic, not a gate test. Run it explicitly:
    ///
    /// ```text
    /// cargo test -p ctx-traits-cli --lib narrator_seat_narrates_live -- --ignored --nocapture
    /// ```
    ///
    /// Every stage prints what it resolved (key env NAME only, never the
    /// value), so a failure names the broken link instead of just failing.
    #[test]
    #[ignore = "live probe: reads real config, needs network + api key, spends one model call"]
    fn narrator_seat_narrates_live() {
        let mut profile = ctx_traits_io::harness_config::resolve_runtime_assignments(&[])
            .expect("runtime config resolves");
        let assignment = profile
            .resolved_narrator_assignment()
            .expect("narrator assignment resolves")
            .expect("a narrator seat is configured (none found in any config tier)");
        eprintln!(
            "narrator seat: transport={:?} harness={:?} model={:?} base-url={:?} wire={:?} api-key-env={:?}",
            assignment.transport,
            assignment.harness,
            assignment.model,
            assignment.api.base_url,
            assignment.api.wire,
            assignment.api.api_key_env,
        );
        let request = match resolve_seat_dispatch(&assignment) {
            SeatDispatch::Api(request) => request,
            SeatDispatch::Harness => panic!(
                "seat degrades to its harness: api-key-env {:?} did not resolve in this environment",
                assignment.api.api_key_env
            ),
            SeatDispatch::NotConfigured => panic!(
                "narrator seat does not declare transport = \"api\" — nothing to probe on the native path"
            ),
            SeatDispatch::Unavailable { reason } => {
                panic!("narrator seat unavailable: {reason}")
            }
        };
        eprintln!(
            "dispatching: model={} base-url={} wire={:?} connect-timeout={}ms read-timeout={}ms retries={}",
            request.model,
            request.base_url,
            request.wire,
            request.connect_timeout_ms,
            request.read_timeout_ms,
            request.retries,
        );
        let response = dispatch_api_seat(
            &request,
            Some("You are a run narrator. Reply with exactly one short plain sentence."),
            "Narrate this event: the run's first step completed successfully.",
            128,
        )
        .expect("live narration round trip succeeds");
        eprintln!(
            "narration: {:?} (input-tokens={:?} output-tokens={:?})",
            response.text, response.input_tokens, response.output_tokens,
        );
        assert!(
            !response.text.trim().is_empty(),
            "provider returned an empty narration"
        );
    }
}
