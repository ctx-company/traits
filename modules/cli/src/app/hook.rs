//! P499/P501: `ctx traits internal hook` — the claude-code and codex hook adapter.
//!
//! Reads a hook payload as JSON on stdin and writes
//! `{"hookSpecificOutput":{"hookEventName":…,"additionalContext":…}}` on
//! stdout, or (with `--settings`) prints the host's hooks config snippet.
//! Runs in-process against the P498 planner (`context_cache`) — no `ctx`
//! re-spawn, no shell-out (D1).
//!
//! D2: the hook never fails the user's session. `run` is the single error
//! boundary — every internal failure (payload not JSON, unknown event, cwd
//! gone, no repo/traits, resolver or render error) becomes one diagnostic
//! line on stderr, no stdout, exit 0. Consequence: this module never calls
//! `println!` for anything but [`print_hook_output`]'s one JSON line, and
//! never calls `print_json_report`/`emit_human`/`Panel` — stdout is a wire,
//! not a report.
//!
//! P501: codex rides this same handler unchanged in substance — its
//! `SessionStart` source enum (`startup | resume | clear | compact`, no
//! `fork`) and its `UserPromptSubmit`/`SessionStart` output wire are
//! field-for-field claude-code's, verified against the codex binary
//! (0.145.0-alpha.30). The harness is now a `--host` flag
//! ([`crate::app::surface::cli::HookHost`]); [`ctx_traits_io::context_ledger::HostKey`]
//! already namespaces by `(harness, host-session)`, so no downstream
//! planner/ledger code changes for a second harness.

use std::io::Read;

use ctx_traits_core::response::CommandOutput;

use crate::app::context_cache::{
    PlanFromTaskInputs, PlannedRow, commit_rows, plan_from_ledger, plan_from_task,
};
use crate::app::surface::cli::HookHost;

/// The `additionalContext` character cap (P499 §4.4). Measured in
/// `chars().count()`, not bytes. Verified against claude-code only; kept as
/// a single shared constant for codex too — known-safe on one harness,
/// conservative on the other, and not worth a per-harness table for one
/// unverified value.
const CONTEXT_CAP_CHARS: usize = 10_000;

const PREAMBLE: &str = "Active ctx.traits behavior traits for this session:";

/// Deserialized with plain serde structs, no `deny_unknown_fields` — the
/// harness adds fields over time and only these five are read.
#[derive(serde::Deserialize)]
struct HookPayload {
    hook_event_name: String,
    session_id: String,
    cwd: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

pub(crate) fn handle_hook(host: HookHost, settings: bool) -> crate::Result<CommandOutput<()>> {
    if settings {
        emit_settings(host)?;
        return Ok(CommandOutput::new(()));
    }

    if let Err(error) = run(host) {
        eprintln!("ctx traits internal hook: {error}");
    }
    Ok(CommandOutput::new(()))
}

fn run(host: HookHost) -> crate::Result<()> {
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|source| crate::Error::Command {
            message: format!("cannot read hook payload from stdin: {source}"),
        })?;

    let payload: HookPayload =
        serde_json::from_str(&raw).map_err(|source| crate::Error::Command {
            message: format!("hook payload is not valid JSON: {source}"),
        })?;

    // §4.2: the payload's `cwd` is authoritative — the ledger's repo key,
    // project trait discovery, and config layering are all CWD-derived, so
    // one chdir keeps them consistent on this single-shot process.
    std::env::set_current_dir(&payload.cwd).map_err(|source| crate::Error::Command {
        message: format!("cannot chdir to hook cwd {}: {source}", payload.cwd),
    })?;

    let host_key = ctx_traits_io::context_ledger::HostKey::new(host.as_str(), &payload.session_id)?;

    match payload.hook_event_name.as_str() {
        "UserPromptSubmit" => {
            let task = payload.prompt.as_deref().unwrap_or("");
            let rows = plan_from_task(PlanFromTaskInputs {
                host_key: &host_key,
                task,
                trait_files: &[],
                repo_root: None,
                files: &[],
                mode: None,
                languages: &[],
                budget: None,
                json: false,
            })?;
            emit_selected_rows(&payload.hook_event_name, &host_key, rows)?;
        }
        "SessionStart" => handle_session_start(&payload, &host_key)?,
        other => {
            eprintln!("ctx traits internal hook: unrecognized hook_event_name {other:?}, no-op");
        }
    }

    Ok(())
}

fn handle_session_start(
    payload: &HookPayload,
    host_key: &ctx_traits_io::context_ledger::HostKey,
) -> crate::Result<()> {
    match payload.source.as_deref() {
        Some(reason @ ("startup" | "clear" | "fork")) => {
            // D3: there is no unconditional-activation predicate in the
            // trait model today, so "the always-on set" is the same planner
            // call with empty signals — zero extra machinery, and it starts
            // injecting the day an unconditional predicate exists.
            ctx_traits_io::context_ledger::clear(host_key, reason)?;
            let rows = plan_from_task(PlanFromTaskInputs {
                host_key,
                task: "",
                trait_files: &[],
                repo_root: None,
                files: &[],
                mode: None,
                languages: &[],
                budget: None,
                json: false,
            })?;
            emit_selected_rows(&payload.hook_event_name, host_key, rows)?;
        }
        Some("compact") => {
            // D4: read the ledger, re-render exactly those trait ids
            // (tolerant of a trait that now refuses trust, D6), clear, then
            // upsert fresh entries for only what was actually emitted.
            let rows = plan_from_ledger(host_key, &[], None)?;
            let (emitted, context) = select_within_cap(&rows);
            ctx_traits_io::context_ledger::clear(host_key, "compact")?;
            if !emitted.is_empty() {
                commit_rows(host_key, &emitted)?;
                print_hook_output(&payload.hook_event_name, &context);
            }
        }
        Some("resume") => {
            // No ledger touch, no output.
        }
        other => {
            eprintln!(
                "ctx traits internal hook: unrecognized SessionStart source {other:?}, no-op"
            );
        }
    }
    Ok(())
}

/// Filter `rows` to `inject`/`reinject`, fit them under the cap, commit only
/// the emitted subset, and emit stdout if anything survived.
fn emit_selected_rows(
    event_name: &str,
    host_key: &ctx_traits_io::context_ledger::HostKey,
    rows: Vec<PlannedRow>,
) -> crate::Result<()> {
    let to_inject: Vec<PlannedRow> = rows
        .into_iter()
        .filter(|row| {
            matches!(
                row.action,
                ctx_traits_core::context::ledger::Action::Inject
                    | ctx_traits_core::context::ledger::Action::Reinject
            )
        })
        .collect();

    let (emitted, context) = select_within_cap(&to_inject);
    if emitted.is_empty() {
        return Ok(());
    }
    commit_rows(host_key, &emitted)?;
    print_hook_output(event_name, &context);
    Ok(())
}

/// Greedily fill `rows` (already in resolver order) into one
/// `additionalContext` string under [`CONTEXT_CAP_CHARS`] (P499 §4.4): never
/// truncates an individual trait's rendered text, and stops before the
/// first trait that would cross the cap rather than skipping ahead to a
/// smaller one — every trait from that point on is named on stderr as
/// omitted, never silently dropped.
fn select_within_cap(rows: &[PlannedRow]) -> (Vec<&PlannedRow>, String) {
    let mut emitted: Vec<&PlannedRow> = Vec::new();
    let mut context = String::from(PREAMBLE);
    let mut total = PREAMBLE.chars().count();
    let mut stopped = false;

    for row in rows {
        if stopped {
            report_omitted(row);
            continue;
        }
        let block_len = row.text.chars().count();
        if total + 2 + block_len > CONTEXT_CAP_CHARS {
            stopped = true;
            report_omitted(row);
            continue;
        }
        total += 2 + block_len;
        context.push_str("\n\n");
        context.push_str(&row.text);
        emitted.push(row);
    }

    (emitted, context)
}

fn report_omitted(row: &PlannedRow) {
    eprintln!(
        "ctx traits internal hook: omitting {} from additionalContext ({CONTEXT_CAP_CHARS}-char cap)",
        row.trait_id
    );
}

#[derive(serde::Serialize)]
struct HookSpecificOutput<'a> {
    #[serde(rename = "hookEventName")]
    hook_event_name: &'a str,
    #[serde(rename = "additionalContext")]
    additional_context: &'a str,
}

#[derive(serde::Serialize)]
struct HookOutput<'a> {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificOutput<'a>,
}

/// The only stdout write on the payload-handling path (D2): one JSON line.
fn print_hook_output(event_name: &str, additional_context: &str) {
    let output = HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: event_name,
            additional_context,
        },
    };
    match serde_json::to_string(&output) {
        Ok(json) => println!("{json}"),
        Err(error) => eprintln!("ctx traits internal hook: cannot serialize hook output: {error}"),
    }
}

/// `ctx traits internal hook --host <host> --settings` (§4.5/P501 §3.1.B): prints the
/// host's hooks config snippet, naming the running binary's absolute path
/// as the command and an explicit timeout comfortably under the harness's
/// 30s budget. No matcher — the handler dispatches on `source` itself.
/// Stdout stays a pure JSON wire (hook.rs's stdout doctrine); any advisory
/// note goes to stderr, keeping `ctx traits internal hook --host codex --settings >
/// ~/.codex/hooks.json` safe to pipe.
fn emit_settings(host: HookHost) -> crate::Result<()> {
    let exe = std::env::current_exe().map_err(|source| crate::Error::Command {
        message: format!("cannot resolve running binary path: {source}"),
    })?;
    let command = format!(
        "{} traits hook --host {}",
        exe.to_string_lossy(),
        host.as_str()
    );
    let snippet = match host {
        HookHost::ClaudeCode => serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [
                    {
                        "hooks": [
                            { "type": "command", "command": command, "timeout": 20 }
                        ]
                    }
                ],
                "SessionStart": [
                    {
                        "hooks": [
                            { "type": "command", "command": command, "timeout": 20 }
                        ]
                    }
                ]
            }
        }),
        HookHost::Codex => {
            eprintln!(
                "ctx traits internal hook: codex gates hooks on a content hash of the configured \
                 command string (`trusted_hash`); this snippet's command differs from any \
                 previously-trusted one, so codex will re-prompt for hook trust once after \
                 install. Pin `ctx` at a stable path (e.g. via `install-bin`, not a copy over \
                 the running binary) so the command string — and its hash — stops moving on \
                 later upgrades."
            );
            // Field names re-verified directly against the codex binary's string table
            // (0.145.0-alpha.30): the per-handler tagged union is `{"type": "command",
            // "command": ..., "timeout": ...}` — NOT `timeout_ms` (a `HookHandlerConfig
            // ::Command` struct with fields `command`/`commandWindows`/`timeout`/`async`
            // /`statusMessage`; `timeout` pairs with the introspection struct's
            // `timeoutSec`, i.e. seconds, not milliseconds). The top-level event keys
            // (`session_start`, `user_prompt_submit`) are confirmed by a literal
            // `hooks.json` + snake_case-event-name string cluster in the same binary.
            serde_json::json!({
                "hooks": {
                    "session_start": [
                        {
                            "hooks": [
                                { "type": "command", "command": command, "timeout": 20 }
                            ]
                        }
                    ],
                    "user_prompt_submit": [
                        {
                            "hooks": [
                                { "type": "command", "command": command, "timeout": 20 }
                            ]
                        }
                    ]
                }
            })
        }
    };
    let json = serde_json::to_string_pretty(&snippet).map_err(|source| crate::Error::Command {
        message: format!("cannot serialize settings snippet: {source}"),
    })?;
    println!("{json}");
    Ok(())
}
