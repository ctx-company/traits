//! Shared prompt-composition chain for a resolved procedure frame.
//!
//! Extracted from `drive.rs` so live dispatch (CLI and MCP transports) and
//! `ctx traits preview` call one resolver/composer, never two. Every
//! function here is a pure read of a `LoadedTrait` + `Session` +
//! `SequenceFrame` triple — no harness dispatch, no session mutation.

use std::collections::BTreeMap;

use serde_json::Value;

/// Ceiling for one inlined input VALUE, matching the whole-prompt-body class
/// below rather than a stricter one.
///
/// This was 16 KiB, set when every slot value was a summary or a verdict of a
/// few paragraphs. The 2026-07-30 circulation redesign made the verdict a
/// cumulative step ledger, and the first live run under it deadlocked here:
/// a six-blocker verdict serialized to 16,434 bytes — 50 over — so the frame
/// dropped the ENTIRE value ("no per-step blocker list to execute against",
/// the worker reported, correctly making no blind edits). Since the ledger
/// only grows and the reviewer's self-read passes through the same gate,
/// every later round wasted identically: a hard loop deadlock from a limit
/// two documents were never told about.
///
/// One value can never exceed the whole body's own configurable ceiling
/// anyway (`[run] inline-prompt-bytes`, default 128 KiB) — that check, plus
/// the model's context, is the real budget authority. A tighter per-value
/// constant adds only the failure mode above.
const MAX_INLINE_VALUE_BYTES: usize = 128 * 1024;
/// Inherited by every resolved-frame-prompt caller when `[run]
/// inline-prompt-bytes` is absent (P489).
pub(crate) const DEFAULT_MAX_INLINE_PROMPT_BYTES: u64 = 128 * 1024;

/// Resolve the effective inline-prompt-body ceiling: the `[run]
/// inline-prompt-bytes` override when configured, otherwise
/// [`DEFAULT_MAX_INLINE_PROMPT_BYTES`]. Reads config fresh (mirrors the
/// ad hoc `resolve_runtime_config(Utf8Path::new("."))` pattern used at other
/// command entry points) rather than requiring every prompt-composition
/// caller to thread a config object through.
fn effective_max_inline_prompt_bytes() -> usize {
    ctx_traits_io::harness_config::resolve_runtime_config(camino::Utf8Path::new("."))
        .ok()
        .and_then(|runtime| runtime.run)
        .and_then(|run| run.inline_prompt_bytes)
        .unwrap_or(DEFAULT_MAX_INLINE_PROMPT_BYTES) as usize
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedFramePrompt {
    pub(crate) prompt_section: String,
    pub(crate) input_section: String,
}

/// One declared-but-unaccepted input, carrying both the human-readable reason
/// (`resolved_input_section`'s "not inlined (pending: ...)" line) and the raw
/// `ref_text` (`frame_contract_section`'s schema lookup) from a single source
/// so the resolved-values view and the input-schema view can never disagree
/// about which refs are pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingInput {
    pub(crate) ref_text: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestedSlotKey {
    pub(crate) ref_text: String,
    pub(crate) property: String,
    pub(crate) operation: ctx_traits_core::r#trait::procedure::WriteOperation,
    pub(crate) schema_ref: Option<String>,
}

/// Compose the MCP onboarding prompt for a frame. Takes the session spelling,
/// session-store value, role, and harness id explicitly rather than a full
/// `DriveInputs`, so preview (which has no drive-loop budget/profile state)
/// can call it with the same session provenance a live MCP dispatch used.
pub(crate) fn mcp_frame_prompt(
    session: &str,
    session_store: Option<&str>,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    context: &ResolvedFramePrompt,
    role: &str,
    harness_id: &str,
) -> String {
    format!(
        "Serve this ctx.traits frame via MCP.\nAgent role: {role}\nHarness id: {harness_id}\nRun session: {}\nSession store: {}\n\nRequired steps:\n1. Call ctx_traits_run_next with agent={role}, session={}, and the session-store above when present.\n2. Use the authoritative frame refs/digests from ctx, and use the resolved content below for the actual goal, inputs, and instructions.\n3. Complete only the returned frame.\n4. Submit with ctx_traits_run_set or ctx_traits_run_call, including agent={role} and harness={harness_id}.\n5. Stop after the submit succeeds; do not continue the procedure loop.\n\nFrame title: {}\n\n{}\n\nResolved prompt instructions:\n{}\nResolved input values:\n{}\n",
        session,
        session_store.unwrap_or(""),
        session,
        frame.title,
        frame_summary_text(frame),
        context.prompt_section,
        context.input_section
    )
}

pub(crate) fn frame_prompt(
    context: &ResolvedFramePrompt,
    contract: &str,
    schema: &Value,
    correction: Option<&str>,
) -> String {
    let correction = correction
        .map(|text| {
            format!(
                "  <correction>\n{}\n  </correction>\n",
                indent_block(text, 4)
            )
        })
        .unwrap_or_default();
    // P561/P562 (2026-07-28): the frame is two envelopes, mirroring the
    // behavioral render P493 landed for the trait itself. What is GONE from
    // the old shape: the `Frame title:`/`Step [run N / source M]`/`Loop … 2/10`
    // header, the `Guard all[...] => false` evaluation dump, and the
    // `Available inputs:` ref list that the resolved-values section then
    // repeated in full — 5.7k chars of runtime bookkeeping a model cannot act
    // on. Digests are gone from the model's view too: the ledger holds
    // provenance, the model does not need it.
    format!(
        "{}<input>\n  <prompt>\n{}\n  </prompt>\n\n  <data>\n{}  </data>\n{correction}</input>\n\n{}",
        // P563: the seat's identity and its one-step boundary lead the frame
        // as a single line. The standing cross-frame discipline (submit through
        // the ctx channel, do not invent loop control) lives in the system
        // prompt — it is stable for a whole run and paying for it per frame is
        // the cache-hostile half of the split.
        contract,
        indent_block(&context.prompt_section, 4),
        context.input_section,
        requested_output_contract_section(schema)
    )
}

/// Indent every line of `text` by `spaces`, leaving blank lines empty so an
/// envelope stays readable without trailing whitespace.
fn indent_block(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render only the requested-output schema and response instruction. This is
/// intentionally separate from the full per-step contract: a resumed reshape
/// already has its input contract in conversation context and needs only the
/// output shape it must correct.
pub(crate) fn requested_output_contract_section(schema: &Value) -> String {
    let (sketch, named) = output_format_sketch(schema);
    let schema_section = if named.is_empty() {
        String::new()
    } else {
        format!("\n  <schema>\n{}\n  </schema>\n", indent_block(&named, 4))
    };
    format!(
        "<output>\n  <format>\n{}\n  </format>\n{schema_section}\n  <response>\n    Return ONLY one JSON object matching <format> — no prose before or after it, no code fences, no extra top-level fields. String-typed fields are single strings, never arrays.\n  </response>\n</output>\n",
        indent_block(&sketch, 4)
    )
}

/// The key skeleton a model reads first: one line per requested output naming
/// its type, with object/array shapes named rather than expanded. The full
/// JSON Schema stays the validation authority and rides in `<schema>`; this is
/// a sketch, never a second dialect, and the runtime never validates against
/// it.
fn output_format_sketch(schema: &Value) -> (String, String) {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return ("{}".to_string(), String::new());
    };
    let mut named: Vec<String> = Vec::new();
    let fields = properties
        .iter()
        .map(|(name, spec)| {
            // An object or array-of-object output is named after its own key
            // and DEFINED once in <schema>; scalars and enums sit inline. So a
            // step producing only scalars emits no <schema> at all, and a step
            // producing one typed verdict emits exactly that one definition —
            // never the whole request wrapper.
            let rendered = match spec.get("type").and_then(Value::as_str) {
                Some("object") => {
                    named.push(format!("<{name}>{spec}</{name}>"));
                    name.clone()
                }
                Some("array")
                    if spec
                        .get("items")
                        .and_then(|items| items.get("type"))
                        .and_then(Value::as_str)
                        == Some("object") =>
                {
                    if let Some(items) = spec.get("items") {
                        named.push(format!("<{name}>{items}</{name}>"));
                    }
                    format!("[{name}]")
                }
                _ => sketch_type(spec),
            };
            format!("  \"{name}\": {rendered}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    (format!("{{\n{fields}\n}}"), named.join("\n"))
}

fn sketch_type(spec: &Value) -> String {
    match spec.get("type").and_then(Value::as_str) {
        Some("array") => {
            let item = spec
                .get("items")
                .map(sketch_type)
                .unwrap_or_else(|| "any".to_string());
            format!("[{item}]")
        }
        Some("object") => "object".to_string(),
        Some(other) => {
            if let Some(values) = spec.get("enum").and_then(Value::as_array) {
                let allowed = values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" | ");
                if !allowed.is_empty() {
                    return allowed;
                }
            }
            other.to_string()
        }
        None => "any".to_string(),
    }
}

/// Human-owned ask presentation deliberately excludes the agent response
/// contract and transport selection while retaining the shared interpolation.
pub(crate) fn human_frame_prompt(
    session_id: &str,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    context: &ResolvedFramePrompt,
) -> String {
    let output = frame
        .requested_outputs
        .first()
        .map(|output| output.slot_ref.to_string())
        .unwrap_or_else(|| "slot:<answer>".to_string());
    format!(
        "Human question:\n{}\n{}\nSubmit the answer with:\nctx traits session frame set --session {} --key {} --value <value>\n",
        context.prompt_section, context.input_section, session_id, output
    )
}

/// The interpolated question text alone — no `source:`/`ref:`/`digest:`
/// metadata lines and no `---BEGIN CTX_TRAITS_PROMPT…---` delimiters. Used by
/// dashboard surfaces (row summary, answer modal) where the metadata wrapper
/// pushes the actual question out of view; falls back to a short metadata
/// line only when the body itself could not be resolved.
pub(crate) fn resolved_human_question_body(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> crate::Result<String> {
    let Some(evidence) = frame.prompt.as_ref() else {
        return Ok("(no prompt declared for this frame)".to_string());
    };
    match resolved_prompt_body(loaded, session, frame, evidence)? {
        Ok(text) => {
            let text = resolve_resource_tokens(loaded, session, frame, text, &[])?;
            let text = resolve_input_value_tokens(session, frame, text);
            let max_inline_prompt_bytes = effective_max_inline_prompt_bytes();
            if text.len() > max_inline_prompt_bytes {
                return Err(crate::Error::Command {
                    message: format!(
                        "resolved prompt body is {} bytes, over inline limit {}",
                        text.len(),
                        max_inline_prompt_bytes
                    ),
                });
            }
            Ok(text)
        }
        Err(reason) => Ok(format!(
            "(question body not inlined: {reason}; source: {}, ref: {}, digest: {})",
            evidence.source,
            evidence.prompt_ref.as_deref().unwrap_or("inline"),
            evidence.digest
        )),
    }
}

fn frame_summary_text(frame: &ctx_traits_core::procedure::runtime::SequenceFrame) -> String {
    frame
        .frame_text
        .lines()
        .filter(|line| !line.trim_start().starts_with("Prompt digest:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `pending_inputs` carries declared-but-unaccepted input descriptions (e.g.
/// a no-session static preview's "produced by step X" lines) into the SAME
/// resolved input section that ends up in the composed prompt, rather than a
/// side-channel the model never sees. Pass `&[]` for every live dispatch
/// caller (CLI/MCP transports) so their composed prompt text and goldens stay
/// byte-identical — only the no-session static preview path has anything to
/// report here.
pub(crate) fn resolved_frame_prompt(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    pending_inputs: &[PendingInput],
) -> crate::Result<ResolvedFramePrompt> {
    Ok(ResolvedFramePrompt {
        prompt_section: resolved_prompt_section(loaded, session, frame, pending_inputs)?,
        input_section: resolved_input_section(loaded, session, frame, pending_inputs)?,
    })
}

fn resolved_input_section(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    pending_inputs: &[PendingInput],
) -> crate::Result<String> {
    if frame.available_inputs.is_empty()
        && frame.resource_evidence.is_empty()
        && pending_inputs.is_empty()
    {
        return Ok(String::new());
    }

    let mut section = String::new();
    for input in &frame.available_inputs {
        // One element per input, named for the ref's own id. No schema
        // attribute (P562's rule: input carries the value, only output
        // describes shapes) and no digest — provenance lives in the ledger.
        let id = element_id(&input.ref_text);
        match accepted_input_value(session, input) {
            Some(value) => match inline_value(&value.value, input.schema_ref.as_deref())? {
                Ok(text) => section.push_str(&data_element(&id, &input.ref_text, "INPUT", &text)),
                Err(reason) => {
                    section.push_str(&format!("    <{id} unavailable=\"{reason}\" />\n"));
                }
            },
            None => {
                let reason = accepted_input_miss_reason(session, input);
                section.push_str(&format!("    <{id} unavailable=\"{reason}\" />\n"));
            }
        }
    }
    for resource in &frame.resource_evidence {
        let id = element_id(resource.resource_ref.as_str());
        match resolved_resource_presentation_verified(
            loaded,
            session,
            frame,
            resource.resource_ref.as_str(),
        )? {
            Some(ResourcePresentation::Inline { content, hint }) => {
                let hint = hint
                    .as_deref()
                    .map(|hint| format!(" hint=\"{hint}\""))
                    .unwrap_or_default();
                section.push_str(&format!(
                    "    <{id}{hint}>\n{}    </{id}>\n",
                    delimited_block("RESOURCE", resource.resource_ref.as_str(), &content)
                ));
            }
            Some(ResourcePresentation::InlineUnavailable { reason }) => {
                section.push_str(&format!("    <{id} unavailable=\"{reason}\" />\n"));
            }
            Some(ResourcePresentation::File(resolved)) => {
                // A path, not a body: the model opens it with its own tools.
                let hint = resolved
                    .hint
                    .as_deref()
                    .map(|hint| format!(" hint=\"{hint}\""))
                    .unwrap_or_default();
                match resolved.status {
                    ctx_traits_io::resource::PresentationStatus::Available => {
                        section
                            .push_str(&format!("    <{id} path=\"{}\"{hint} />\n", resolved.path));
                    }
                    _ => {
                        section.push_str(&format!(
                            "    <{id} path=\"{}\" unavailable=\"declared resource file is not readable\" />\n",
                            resolved.path
                        ));
                    }
                }
            }
            None => {}
        }
    }

    for pending in pending_inputs {
        // Static-preview only: a declared input with no accepted value yet.
        // A live frame never renders these — an absent OPTIONAL input simply
        // has no element at all, which is what makes `slot.optional()` usable
        // in prompt text: a `[ref]` to a missing element is a reference to
        // something not present, not an unresolved token.
        let id = element_id(&pending.ref_text);
        section.push_str(&format!("    <{id} pending=\"{}\" />\n", pending.reason));
    }
    Ok(section)
}

/// A resolved resource presentation: its path, an optional disclosure hint,
/// and whether that path is safe to present as an openable file reference.
#[derive(Debug, Clone)]
struct ResolvedResourcePresentation {
    path: String,
    hint: Option<String>,
    status: ctx_traits_io::resource::PresentationStatus,
}

/// How a declared resource reaches the model in a frame.
///
/// Reference-mode resources are served as openable paths and never pasted —
/// the bytes stay on disk, outside the prompt, and the digest is the
/// contract. Inline-mode resources (a `content` body, a checklist body, or a
/// `path` resource that declares `render = "inline"`) have no path to serve:
/// the only way to deliver one is to include it, exactly as the static model
/// view already does. Returning `None` for a declared resource silently
/// dropped it and reported it as undeclared; `InlineUnavailable` is the
/// distinct "this resource is inline-only but its body could not be safely
/// read" outcome, which must never fall back to presenting a reference path
/// that does not exist for it.
#[derive(Debug, Clone)]
enum ResourcePresentation {
    File(ResolvedResourcePresentation),
    Inline {
        content: String,
        hint: Option<String>,
    },
    InlineUnavailable {
        reason: String,
    },
}

/// Resolve a declared resource's frame-presentation path against its
/// declared root (package or invocation repository), and disclose when a
/// repo-root path is gitignored. Resolution is a presentation step for CLI
/// frames: references are served as paths, never as pasted content, and this
/// never touches persisted `ResourceEvidence`.
///
/// Root selection and the safe, non-symlink-following path resolution are
/// delegated entirely to `ctx_traits_io::resource`, the same path used by
/// digesting and audit, so frame presentation cannot show a path that the
/// no-follow containment checks would have rejected. The returned `status`
/// tells the caller whether the path may be presented as openable; a
/// `Missing`/`Symlink` status must never be offered as a file reference.
fn absolute_resource_path(
    package_root: &camino::Utf8Path,
    resource: &ctx_traits_core::r#trait::Resource,
    relative: &str,
) -> crate::Result<ResolvedResourcePresentation> {
    let roots = ctx_traits_io::resource::resolve_resource_roots(
        package_root,
        std::slice::from_ref(resource),
    )?;

    // A protected (pinned) resource is verified against its declared digest
    // immediately before this path is handed back for opening; a mismatch or
    // unavailable file fails the whole frame closed rather than degrading to
    // an unverified "not available" text line.
    if resource.is_protected() {
        let path = match ctx_traits_io::resource::verify_protected_resource(&roots, resource)? {
            ctx_traits_io::resource::ProtectionVerification::Verified { path } => path,
            ctx_traits_io::resource::ProtectionVerification::Unprotected => {
                unreachable!("is_protected() checked above")
            }
            ctx_traits_io::resource::ProtectionVerification::Failed(failure) => {
                return Err(crate::Error::Command {
                    message: failure.to_string(),
                });
            }
        };
        let disclosure = repo_root_disclosure(&roots, resource, relative)?;
        return Ok(ResolvedResourcePresentation {
            path: path.to_string(),
            hint: disclosure,
            status: ctx_traits_io::resource::PresentationStatus::Available,
        });
    }

    let presented = ctx_traits_io::resource::presentation_path(&roots, resource, relative)?;
    let disclosure = repo_root_disclosure(&roots, resource, relative)?;

    Ok(ResolvedResourcePresentation {
        path: presented.path.to_string(),
        hint: disclosure,
        status: presented.status,
    })
}

/// Disclose when a repo-root resource's declared path is gitignored (and so
/// may be absent from a clean checkout/worktree). `None` for a package-root
/// resource, which carries no such caveat.
fn repo_root_disclosure(
    roots: &ctx_traits_io::resource::ResourceRoots,
    resource: &ctx_traits_core::r#trait::Resource,
    relative: &str,
) -> crate::Result<Option<String>> {
    match resource.effective_root() {
        ctx_traits_core::r#trait::ResourceRoot::Package => Ok(None),
        ctx_traits_core::r#trait::ResourceRoot::Repo => {
            let repo_root = roots.invocation_repo_root.as_deref().expect(
                "resolve_resource_roots discovers the invocation repo for root=repo resources",
            );
            Ok(
                match ctx_traits_io::repository::check_ignored(repo_root, relative)? {
                    ctx_traits_io::repository::IgnoreStatus::Ignored => {
                        Some("gitignored, may be absent from a clean checkout/worktree".to_string())
                    }
                    ctx_traits_io::repository::IgnoreStatus::NotIgnored => None,
                },
            )
        }
    }
}

/// Resolve an unqualified local resource or a qualified local-dependency
/// resource to its filesystem path for frame presentation.
fn resolved_resource_presentation(
    loaded: &ctx_traits_io::run::LoadedTrait,
    reference: &str,
) -> crate::Result<Option<ResourcePresentation>> {
    let Some(path) = reference.strip_prefix("resource:") else {
        return Ok(None);
    };
    let Some((alias, resource_id)) = path.split_once('/') else {
        let Some(resource) = loaded
            .trait_ref
            .resources
            .iter()
            .find(|resource| resource.id == path)
        else {
            return Ok(None);
        };
        let Some(resource_path) = resource.path.as_deref() else {
            return Ok(inline_resource_presentation(resource));
        };
        return Ok(Some(match resource.effective_render() {
            ctx_traits_core::r#trait::ResourceRender::Inline => {
                inline_path_presentation(&loaded.trait_root, resource)?
            }
            ctx_traits_core::r#trait::ResourceRender::Reference => {
                let mut resolved =
                    absolute_resource_path(&loaded.trait_root, resource, resource_path)?;
                resolved.hint = merge_hint_disclosure(resource.hint.clone(), resolved.hint);
                ResourcePresentation::File(resolved)
            }
        }));
    };
    if resource_id.contains('/') {
        return Ok(None);
    }
    let Some(dependency) = loaded
        .trait_ref
        .dependencies
        .iter()
        .find(|dependency| dependency.alias == alias)
    else {
        return Ok(None);
    };
    let Some(ctx_traits_core::manifest::TraitSource::Local { path }) = dependency.source.as_ref()
    else {
        return Ok(None);
    };
    let root = loaded.trait_root.join(path);
    let manifest = ctx_traits_io::layout::resolve_package_manifest(&root).unwrap_or_else(|| {
        root.join(ctx_traits_io::layout::GENERATED)
            .join(ctx_traits_io::layout::TRAIT_MANIFEST)
    });
    let text = ctx_traits_io::read::read_text(&manifest)?;
    let (trait_ref, decode_warnings) = ctx_traits_core::encoding::decode_trait_with_warnings(
        ctx_traits_core::encoding::Encoding::Toml,
        &text,
    )?;
    ctx_traits_io::decode_diagnostics::print_decode_warnings(manifest.as_str(), &decode_warnings);
    if trait_ref.id.as_str() != dependency.id || trait_ref.version.as_str() != dependency.version {
        return Err(crate::Error::Command {
            message: format!("local dependency {alias:?} does not match its declared id/version"),
        });
    }
    let Some(resource) = trait_ref
        .resources
        .iter()
        .find(|resource| resource.id == resource_id)
    else {
        return Ok(None);
    };
    let Some(resource_path) = resource.path.as_deref() else {
        return Ok(inline_resource_presentation(resource));
    };
    // Dependency-qualified repo-root resources reuse the same invocation
    // repo; package-root dependency resources stay jailed per dependency
    // package.
    Ok(Some(match resource.effective_render() {
        ctx_traits_core::r#trait::ResourceRender::Inline => {
            inline_path_presentation(&root, resource)?
        }
        ctx_traits_core::r#trait::ResourceRender::Reference => {
            let mut resolved = absolute_resource_path(&root, resource, resource_path)?;
            resolved.hint = merge_hint_disclosure(resource.hint.clone(), resolved.hint);
            ResourcePresentation::File(resolved)
        }
    }))
}

/// Resolve a declared resource's presentation the way both the resolved
/// input section and prompt-token substitution must see it: inline content
/// is only ever returned once its bytes are verified against the frame's
/// accepted `ResourceEvidence` digest. This is the single evidence-aware
/// presentation operation — every caller that can put resource bytes in
/// front of the model routes through it, so there is exactly one place a
/// digest check can be skipped or an unverified reread introduced.
fn resolved_resource_presentation_verified(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    reference: &str,
) -> crate::Result<Option<ResourcePresentation>> {
    let Some(presentation) = resolved_resource_presentation(loaded, reference)? else {
        return Ok(None);
    };
    Ok(Some(match presentation {
        ResourcePresentation::Inline { content, hint } => {
            match verified_inline_content(session, frame, reference, &content) {
                Ok(()) => ResourcePresentation::Inline { content, hint },
                Err(reason) => ResourcePresentation::InlineUnavailable { reason },
            }
        }
        other => other,
    }))
}

/// Compare inline resource bytes against the frame's accepted
/// `ResourceEvidence` digest for `reference`. An accepted digest must be
/// present and must match, or the bytes are refused — a missing accepted
/// digest is not a free pass to present unverified content.
fn verified_inline_content(
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    reference: &str,
    content: &str,
) -> Result<(), String> {
    let content_digest = ctx_traits_core::digest::Digest::source(content);
    let accepted = resource_evidence(session, frame, reference)
        .and_then(|evidence| evidence.digest.as_deref());
    match accepted {
        Some(accepted) if accepted == content_digest.as_str() => Ok(()),
        Some(accepted) => Err(format!(
            "inline content digest {} does not match accepted evidence digest {accepted}; refusing to present unverified bytes",
            content_digest.as_str(),
        )),
        None => Err(
            "no accepted resource evidence digest for this resource; refusing to present unverified bytes"
                .to_string(),
        ),
    }
}

/// Present a `path`-backed resource declared `render = "inline"` by reading
/// its body through the shared no-follow text reader and embedding it,
/// rather than serving a path. Missing, symlinked, special, binary, or
/// non-UTF-8 files stay unavailable with an explicit reason — presenting a
/// reference path for a resource declared inline-only would name a file the
/// model was never told to open.
fn inline_path_presentation(
    package_root: &camino::Utf8Path,
    resource: &ctx_traits_core::r#trait::Resource,
) -> crate::Result<ResourcePresentation> {
    let roots = ctx_traits_io::resource::resolve_resource_roots(
        package_root,
        std::slice::from_ref(resource),
    )?;
    // A protected resource's bytes are re-verified against its declared pin
    // immediately before this body is embedded — the softer `InlineUnavailable`
    // outcomes below are for unprotected resources only; a pinned resource
    // that mismatches or is unreadable fails the whole frame closed.
    if resource.is_protected() {
        match ctx_traits_io::resource::verify_protected_resource(&roots, resource)? {
            ctx_traits_io::resource::ProtectionVerification::Verified { .. } => {}
            ctx_traits_io::resource::ProtectionVerification::Unprotected => {
                unreachable!("is_protected() checked above")
            }
            ctx_traits_io::resource::ProtectionVerification::Failed(failure) => {
                return Err(crate::Error::Command {
                    message: failure.to_string(),
                });
            }
        }
    }
    let outcome = ctx_traits_io::resource::read_text_resource(&roots, resource)?;
    if let Some(text) = outcome.text {
        return Ok(ResourcePresentation::Inline {
            content: text,
            hint: resource.hint.clone(),
        });
    }
    let reason = match outcome.skipped {
        Some(ctx_traits_io::resource::ResourceTextSkipReason::UnsupportedExtension) => {
            "declared resource file has an extension not supported for inline text reading"
        }
        Some(ctx_traits_io::resource::ResourceTextSkipReason::Missing) => {
            "declared resource file does not exist"
        }
        Some(ctx_traits_io::resource::ResourceTextSkipReason::Symlink) => {
            "a symlink was detected and rejected"
        }
        Some(ctx_traits_io::resource::ResourceTextSkipReason::SpecialFile) => {
            "the path is not a regular file"
        }
        Some(ctx_traits_io::resource::ResourceTextSkipReason::Binary) => {
            "content is binary or not valid UTF-8"
        }
        Some(ctx_traits_io::resource::ResourceTextSkipReason::RepoRootReference) | None => {
            "resource text is unavailable"
        }
    };
    Ok(ResourcePresentation::InlineUnavailable {
        reason: reason.to_string(),
    })
}

/// Present a path-less resource from its inline body. `validate_resources`
/// enforces exactly one of `path`/`content`, so a resource with neither is
/// unreachable for a decoded manifest; it stays `None` rather than asserting.
fn inline_resource_presentation(
    resource: &ctx_traits_core::r#trait::Resource,
) -> Option<ResourcePresentation> {
    // A checklist's body is its typed items; the presented text is rendered
    // from them in core so a frame and a static render cannot disagree.
    if resource.is_checklist() {
        return Some(ResourcePresentation::Inline {
            content: ctx_traits_core::r#trait::checklist::render_items(resource),
            hint: resource.hint.clone(),
        });
    }
    resource
        .content
        .as_deref()
        .map(|content| ResourcePresentation::Inline {
            content: content.to_string(),
            hint: resource.hint.clone(),
        })
}

fn merge_hint_disclosure(hint: Option<String>, disclosure: Option<String>) -> Option<String> {
    match (hint, disclosure) {
        (Some(hint), Some(disclosure)) => Some(format!("{hint} ({disclosure})")),
        (Some(hint), None) => Some(hint),
        (None, Some(disclosure)) => Some(disclosure),
        (None, None) => None,
    }
}

/// Replace `{resource:<id>}` interpolation tokens with absolute resolved
/// paths. Runs after prompt digest verification: the digest covers the
/// canonical prompt text; token resolution is frame presentation.
///
/// A resource ref listed in `pending_inputs` is declared-but-unaccepted for
/// this frame (only the no-session static preview path has anything here —
/// live dispatch always passes `&[]`) and must not have its path substituted:
/// the pending line in `resolved_input_section` carries no path or gitignore
/// disclosure, so splicing the resolved path into the prompt body here would
/// deliver a repo-root path with the required disclosure silently dropped.
fn resolve_resource_tokens(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    text: String,
    pending_inputs: &[PendingInput],
) -> crate::Result<String> {
    let mut resolved = text;
    let mut references = loaded
        .trait_ref
        .resources
        .iter()
        .map(|resource| format!("resource:{}", resource.id))
        .collect::<Vec<_>>();
    for dependency in &loaded.trait_ref.dependencies {
        if let Some(ctx_traits_core::manifest::TraitSource::Local { path }) =
            dependency.source.as_ref()
        {
            let root = loaded.trait_root.join(path);
            let manifest =
                ctx_traits_io::layout::resolve_package_manifest(&root).unwrap_or_else(|| {
                    root.join(ctx_traits_io::layout::GENERATED)
                        .join(ctx_traits_io::layout::TRAIT_MANIFEST)
                });
            let text = ctx_traits_io::read::read_text(&manifest)?;
            let (trait_ref, decode_warnings) =
                ctx_traits_core::encoding::decode_trait_with_warnings(
                    ctx_traits_core::encoding::Encoding::Toml,
                    &text,
                )?;
            ctx_traits_io::decode_diagnostics::print_decode_warnings(
                manifest.as_str(),
                &decode_warnings,
            );
            if trait_ref.id.as_str() != dependency.id
                || trait_ref.version.as_str() != dependency.version
            {
                return Err(crate::Error::Command {
                    message: format!(
                        "local dependency {:?} does not match its declared id/version",
                        dependency.alias
                    ),
                });
            }
            references.extend(
                trait_ref
                    .resources
                    .iter()
                    .map(|resource| format!("resource:{}/{}", dependency.alias, resource.id)),
            );
        }
    }
    for reference in references {
        let token = format!("{{{reference}}}");
        if resolved.contains(&token) {
            let is_pending = pending_inputs
                .iter()
                .any(|pending| pending.ref_text == reference);
            if !is_pending {
                match resolved_resource_presentation_verified(loaded, session, frame, &reference)? {
                    // A file resource resolves to a path the model opens itself.
                    Some(ResourcePresentation::File(presentation))
                        if presentation.status
                            == ctx_traits_io::resource::PresentationStatus::Available =>
                    {
                        resolved = resolved.replace(&token, &presentation.path);
                    }
                    // An inline resource has no path to name, so the token
                    // carries the body itself — the same rule input-value
                    // tokens follow: a prompt that names an input receives it.
                    Some(ResourcePresentation::Inline { content, .. }) => {
                        resolved = resolved.replace(&token, &content);
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(resolved)
}

/// Replace `{port:<id>}`/`{slot:<id>}` tokens with the complete accepted scalar
/// value. A prompt that explicitly references an input must receive it even
/// when the digest-carrying input record is too large to inline separately.
/// Runs after prompt digest verification and after resource-token resolution,
/// so a substituted value cannot inject tokens that would then re-resolve.
fn resolve_input_value_tokens(
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    text: String,
) -> String {
    let mut values = BTreeMap::new();
    for input in &frame.available_inputs {
        let Some(value) = accepted_input_value(session, input) else {
            continue;
        };
        let Some(scalar) = value.value.as_str() else {
            continue;
        };
        values.insert(input.ref_text.as_str(), scalar);
    }
    let mut resolved = String::with_capacity(text.len());
    let mut remaining = text.as_str();
    while let Some(start) = remaining.find('{') {
        let token_start = start + 1;
        let Some(end) = remaining[token_start..].find('}') else {
            resolved.push_str(remaining);
            return resolved;
        };
        let token_end = token_start + end;
        resolved.push_str(&remaining[..start]);
        if let Some(value) = values.get(&remaining[token_start..token_end]) {
            resolved.push_str(value);
        } else {
            resolved.push_str(&remaining[start..=token_end]);
        }
        remaining = &remaining[token_end + 1..];
    }
    resolved.push_str(remaining);
    resolved
}

fn accepted_input_value<'a>(
    session: &'a ctx_traits_core::procedure::session::Session,
    input: &ctx_traits_core::procedure::runtime::FrameInput,
) -> Option<&'a ctx_traits_core::procedure::runtime::Value> {
    accepted_session_values(session).find(|value| {
        value.ref_text == input.ref_text
            && value.value_digest == input.value_digest
            && value.acceptance == ctx_traits_core::procedure::runtime::AcceptanceStatus::Accepted
    })
}

fn accepted_input_miss_reason(
    session: &ctx_traits_core::procedure::session::Session,
    input: &ctx_traits_core::procedure::runtime::FrameInput,
) -> String {
    match accepted_session_values(session).find(|value| value.ref_text == input.ref_text) {
        Some(value) if value.value_digest != input.value_digest => format!(
            "accepted ledger value digest mismatch: frame={} ledger={}",
            input.value_digest, value.value_digest
        ),
        Some(value)
            if value.acceptance
                != ctx_traits_core::procedure::runtime::AcceptanceStatus::Accepted =>
        {
            format!("ledger value is {:?}, not accepted", value.acceptance)
        }
        Some(_) => "accepted ledger value was not usable".to_string(),
        None => "no accepted ledger value matched this ref".to_string(),
    }
}

fn accepted_session_values(
    session: &ctx_traits_core::procedure::session::Session,
) -> impl Iterator<Item = &ctx_traits_core::procedure::runtime::Value> {
    session
        .accepted_port_values
        .iter()
        .chain(session.accepted_slot_values.iter())
}

fn inline_value(value: &Value, schema_ref: Option<&str>) -> crate::Result<Result<String, String>> {
    let schema_ref = schema_ref.unwrap_or("schema:any");
    let rendered = match value {
        Value::String(text) if matches!(schema_ref, "schema:text" | "schema:any") => {
            if text.contains('\0') {
                return Ok(Err("string contains NUL bytes".to_string()));
            }
            text.clone()
        }
        Value::Bool(_) | Value::Number(_) | Value::Null => value.to_string(),
        _ => serde_json::to_string_pretty(value).map_err(|err| crate::Error::Command {
            message: format!("failed to serialize runtime value for drive prompt: {err}"),
        })?,
    };
    if rendered.len() > MAX_INLINE_VALUE_BYTES {
        return Ok(Err(format!(
            "serialized value is {} bytes, over inline limit {}",
            rendered.len(),
            MAX_INLINE_VALUE_BYTES
        )));
    }
    Ok(Ok(rendered))
}

fn resolved_prompt_section(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    pending_inputs: &[PendingInput],
) -> crate::Result<String> {
    let Some(evidence) = frame.prompt.as_ref() else {
        return Ok("none\n".to_string());
    };

    // P561: the prompt's source/ref/digest header is gone from the model's
    // view. It is provenance — the ledger records it, and the model can act on
    // none of it, so the section opens empty.
    let mut section = String::new();
    let _ = &evidence.source;
    match resolved_prompt_body(loaded, session, frame, evidence)? {
        Ok(text) => {
            let text = resolve_resource_tokens(loaded, session, frame, text, pending_inputs)?;
            let text = resolve_input_value_tokens(session, frame, text);
            let max_inline_prompt_bytes = effective_max_inline_prompt_bytes();
            if text.len() > max_inline_prompt_bytes {
                return Err(crate::Error::Command {
                    message: format!(
                        "resolved prompt body is {} bytes, over inline limit {}",
                        text.len(),
                        max_inline_prompt_bytes
                    ),
                });
            }
            // The authored prompt text, bare. It is trait-authored (trusted)
            // and already sits inside <prompt>, so it needs no second wrapper.
            section.push_str(&text);
        }
        Err(reason) => {
            section.push_str(&format!("(prompt body unavailable: {reason})\n"));
        }
    }
    Ok(section)
}

fn resolved_prompt_body(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    evidence: &ctx_traits_core::procedure::runtime::PromptEvidence,
) -> crate::Result<Result<String, String>> {
    if evidence.source == "inline" {
        let Some(item) = sequence_item_for_frame(&loaded.trait_ref, frame) else {
            return Ok(Err(
                "inline prompt item could not be located in loaded trait".to_string(),
            ));
        };
        return checked_prompt_text(&item.prompt, &evidence.digest);
    }

    let Some(prompt_ref) = evidence.prompt_ref.as_deref() else {
        return Ok(Err("prompt evidence has no prompt ref".to_string()));
    };
    let Some(prompt_id) = local_ref_id(prompt_ref, "prompt") else {
        return Ok(Err(format!(
            "prompt ref {prompt_ref:?} is dependency-qualified or unsupported"
        )));
    };
    let Some(prompt) = loaded.trait_ref.prompts.get(prompt_id) else {
        return Ok(Err(format!("prompt {prompt_ref:?} is not declared")));
    };
    if let Some(text) = prompt.text.as_deref() {
        return checked_prompt_text(text, &evidence.digest);
    }
    if let Some(source) = prompt.source.as_deref() {
        return resolved_resource_prompt_body(loaded, session, frame, source);
    }
    Ok(Err(format!("prompt {prompt_ref:?} has no text or source")))
}

fn checked_prompt_text(
    text: &str,
    expected_digest: &ctx_traits_core::digest::Digest,
) -> crate::Result<Result<String, String>> {
    let actual = ctx_traits_core::digest::Digest::source(text);
    if &actual != expected_digest {
        return Ok(Err(format!(
            "prompt digest mismatch: frame={} loaded={}",
            expected_digest, actual
        )));
    }
    let max_inline_prompt_bytes = effective_max_inline_prompt_bytes();
    if text.len() > max_inline_prompt_bytes {
        return Ok(Err(format!(
            "prompt body is {} bytes, over inline limit {}",
            text.len(),
            max_inline_prompt_bytes
        )));
    }
    Ok(Ok(text.to_string()))
}

fn resolved_resource_prompt_body(
    loaded: &ctx_traits_io::run::LoadedTrait,
    session: &ctx_traits_core::procedure::session::Session,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
    source: &str,
) -> crate::Result<Result<String, String>> {
    let Some(resource_id) = local_ref_id(source, "resource") else {
        return Ok(Err(format!(
            "prompt source {source:?} is dependency-qualified or unsupported"
        )));
    };
    let Some(resource) = loaded
        .trait_ref
        .resources
        .iter()
        .find(|resource| resource.id == resource_id)
    else {
        return Ok(Err(format!("resource {source:?} is not declared")));
    };
    let Some(evidence) = resource_evidence(session, frame, source) else {
        return Ok(Err(format!(
            "resource evidence for {source:?} is unavailable"
        )));
    };
    if !evidence.available {
        return Ok(Err(format!(
            "resource evidence unavailable: {}",
            evidence.reason
        )));
    }
    if evidence.is_binary {
        return Ok(Err("resource evidence marks the body as binary".to_string()));
    }
    let max_inline_prompt_bytes = effective_max_inline_prompt_bytes();
    if evidence.byte_size as usize > max_inline_prompt_bytes {
        return Ok(Err(format!(
            "resource body is {} bytes, over inline limit {}",
            evidence.byte_size, max_inline_prompt_bytes
        )));
    }
    let Some(expected_digest) = evidence.digest.as_ref() else {
        return Ok(Err("resource evidence has no digest".to_string()));
    };
    if let Some(content) = resource.content.as_deref() {
        return checked_prompt_text(content, expected_digest);
    }
    let roots = ctx_traits_io::resource::resolve_resource_roots(
        &loaded.trait_root,
        &loaded.trait_ref.resources,
    )?;
    let outcome = ctx_traits_io::resource::read_text_resource(&roots, resource)?;
    let Some(text) = outcome.text else {
        return Ok(Err(format!(
            "resource text was skipped: {:?}",
            outcome.skipped
        )));
    };
    let actual = ctx_traits_core::digest::Digest::from_bytes(text.as_bytes());
    if &actual != expected_digest {
        return Ok(Err(format!(
            "resource digest mismatch: frame={} loaded={}",
            expected_digest, actual
        )));
    }
    Ok(Ok(text))
}

fn sequence_item_for_frame<'a>(
    trait_ref: &'a ctx_traits_core::Trait,
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> Option<&'a ctx_traits_core::r#trait::procedure::SequenceItem> {
    if let Some(item_id) = frame.item_id.as_deref() {
        if let Some(item) = trait_ref.procedure.as_ref().and_then(|procedure| {
            procedure
                .sequence
                .iter()
                .find(|item| item.id.as_deref() == Some(item_id))
        }) {
            return Some(item);
        }
        for (_, sequence) in trait_ref.sequences.iter() {
            if let Some(item) = sequence
                .sequence
                .iter()
                .find(|item| item.id.as_deref() == Some(item_id))
            {
                return Some(item);
            }
        }
    }
    frame.sequence_index.and_then(|index| {
        trait_ref
            .procedure
            .as_ref()
            .and_then(|procedure| procedure.sequence.get(index))
    })
}

fn resource_evidence<'a>(
    session: &'a ctx_traits_core::procedure::session::Session,
    frame: &'a ctx_traits_core::procedure::runtime::SequenceFrame,
    ref_text: &str,
) -> Option<&'a ctx_traits_core::procedure::runtime::ResourceEvidence> {
    frame
        .resource_evidence
        .iter()
        .chain(session.resource_evidence.iter())
        .find(|evidence| evidence.resource_ref.as_str() == ref_text)
}

fn local_ref_id<'a>(ref_text: &'a str, kind: &str) -> Option<&'a str> {
    let path = ref_text.strip_prefix(&format!("{kind}:"))?;
    if path.is_empty() || path.contains('/') {
        None
    } else {
        Some(path)
    }
}

/// Values shorter than this are inlined bare inside their element; longer ones
/// keep the content-derived delimiter. A six-character `"revise"` does not need
/// a forty-character wrapper, and injection lives in the prose blobs
/// (`work-summary`, `draft`, `review-diff`), not in scalars.
const BARE_VALUE_BYTES: usize = 200;

/// The element name for a ref: its bare id, so `slot:review-verdict` renders as
/// `<review-verdict>`. Slot and port ids are already kebab-case slugs, which is
/// a valid element name; anything else falls back to the full ref with the
/// separator replaced so the element stays well-formed.
fn element_id(ref_text: &str) -> String {
    ref_text
        .split_once(':')
        .map_or_else(|| ref_text.to_string(), |(_, id)| id.to_string())
        .replace(['/', ':'], "-")
}

/// One `<data>` child. Short values sit bare inside the element; long ones keep
/// [`delimited_block`]'s content-derived marker, whose suffix is the first 12
/// hex chars of the value's OWN digest — forging it needs a preimage, not a
/// guess, which a fixed `</tag>` boundary could never offer. That is why the
/// delimiter survives the move to XML rather than being replaced by it.
fn data_element(id: &str, label: &str, kind: &str, body: &str) -> String {
    if body.len() <= BARE_VALUE_BYTES && !body.contains('\n') {
        return format!("    <{id}>{body}</{id}>\n");
    }
    format!(
        "    <{id}>\n{}    </{id}>\n",
        delimited_block(kind, label, body)
    )
}

fn delimited_block(kind: &str, label: &str, body: &str) -> String {
    let digest = ctx_traits_core::digest::Digest::source(&format!("{kind}\n{label}\n{body}"));
    let suffix = digest
        .as_str()
        .trim_start_matches("sha256:")
        .chars()
        .take(12)
        .collect::<String>();
    let marker = format!("CTX_TRAITS_{kind}_{suffix}");
    format!("---BEGIN {marker} {label}---\n{body}\n---END {marker}---\n")
}

pub(crate) fn requested_output_schema(
    requested: &[RequestedSlotKey],
    loaded: &ctx_traits_io::run::LoadedTrait,
) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for output in requested {
        required.push(Value::String(output.property.clone()));
        properties.insert(
            output.property.clone(),
            requested_output_json_schema(output, loaded),
        );
    }
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
    })
}

fn requested_output_json_schema(
    output: &RequestedSlotKey,
    loaded: &ctx_traits_io::run::LoadedTrait,
) -> Value {
    let schema = json_schema_for_ref(output.schema_ref.as_deref(), loaded, 0);
    if output.operation == ctx_traits_core::r#trait::procedure::WriteOperation::Merge {
        merge_delta_json_schema(schema)
    } else {
        schema
    }
}

fn merge_delta_json_schema(mut schema: Value) -> Value {
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    object.remove("required");
    if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
        for property in properties.values_mut() {
            let nested = std::mem::take(property);
            *property = merge_delta_json_schema(nested);
        }
    }
    schema
}

const MAX_SCHEMA_REF_DEPTH: usize = 4;

/// Resolve a schema ref into full JSON Schema for frame prompts. Custom
/// `schema:<id>` refs resolve against the trait's `[[schema]]` declarations —
/// an unresolved custom ref used to render as an empty `{}`, which told the
/// model nothing about field types and let it invent shapes (observed live:
/// `feedback` arrays against a string field, burning correction retries).
fn json_schema_for_ref(
    schema_ref: Option<&str>,
    loaded: &ctx_traits_io::run::LoadedTrait,
    depth: usize,
) -> Value {
    let reference = schema_ref.unwrap_or("schema:any");
    // List wrapper: `[schema:x]` is an array of x.
    if let Some(inner) = reference
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return serde_json::json!({
            "type": "array",
            "items": json_schema_for_ref(Some(inner.trim()), loaded, depth),
        });
    }
    match reference {
        "schema:text" => serde_json::json!({ "type": "string" }),
        "schema:number" => serde_json::json!({ "type": "number" }),
        "schema:integer" => serde_json::json!({ "type": "integer" }),
        "schema:boolean" => serde_json::json!({ "type": "boolean" }),
        other => {
            let Some(id) = other.strip_prefix("schema:") else {
                return serde_json::json!({});
            };
            if depth >= MAX_SCHEMA_REF_DEPTH {
                return serde_json::json!({});
            }
            match loaded
                .trait_ref
                .schemas
                .iter()
                .find(|declared| declared.id == id)
            {
                Some(declared) => declared_json_schema(declared, loaded, depth),
                None => serde_json::json!({}),
            }
        }
    }
}

fn declared_json_schema(
    declared: &ctx_traits_core::r#trait::schema::Schema,
    loaded: &ctx_traits_io::run::LoadedTrait,
    depth: usize,
) -> Value {
    let mut root = if let Some(fields) = &declared.fields {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        for (name, field) in fields {
            let mut property = json_schema_for_ref(Some(field.schema.as_str()), loaded, depth + 1);
            if let Some(object) = property.as_object_mut() {
                if let Some(allowed) = &field.allowed {
                    object.insert("enum".to_string(), Value::Array(allowed.clone()));
                }
                if let Some(description) = &field.description {
                    object.insert(
                        "description".to_string(),
                        Value::String(description.clone()),
                    );
                }
            }
            if field.required {
                required.push(Value::String(name.clone()));
            }
            properties.insert(name.clone(), property);
        }
        let mut object = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": Value::Object(properties),
        });
        if !required.is_empty()
            && let Some(map) = object.as_object_mut()
        {
            map.insert("required".to_string(), Value::Array(required));
        }
        object
    } else if let Some(allowed) = &declared.allowed {
        serde_json::json!({ "enum": allowed })
    } else {
        // Resource-backed or otherwise opaque declarations stay unconstrained.
        serde_json::json!({})
    };
    if let Some(description) = &declared.description
        && let Some(map) = root.as_object_mut()
    {
        map.entry("description".to_string())
            .or_insert_with(|| Value::String(description.clone()));
    }
    root
}

/// The explicit per-frame contract: who the agent is, what input types it
/// receives, and (rendered separately) the output schema it must return —
/// stated directly so no frame relies on the model inferring its role.
///
/// `pending_inputs` is the same [`PendingInput`] view `resolved_input_section`
/// renders into `Resolved input values` — a pending ref still has a declared
/// schema even though no value is inlined yet, so it belongs in this section
/// too; passing the same slice both places is what keeps the two sections
/// from disagreeing about which refs are pending. Live dispatch passes `&[]`,
/// same as `resolved_frame_prompt`, so its section stays byte-identical.
pub(crate) fn frame_contract_section(
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> String {
    let role = frame
        .assigned_agent
        .as_ref()
        .map_or(ctx_traits_io::harness_config::DEFAULT_SEAT, |agent| {
            agent.role.as_str()
        });
    // P560 (2026-07-28): the input JSON Schema enumeration is GONE. It listed
    // the full nested schema — descriptions included — of every available and
    // pending input on every frame: 4,725 chars on a measured worker frame,
    // describing the types of values the model has already been handed and
    // will never emit. Nothing consumed it. A description that genuinely
    // matters to a step belongs in that step's authored prompt text, not
    // auto-dumped for every input on every round.
    let section = format!(
        "You are agent:{role}, serving one step: \"{}\". Do only this step's work.\n\n",
        frame.title
    );
    section
}

pub(crate) fn requested_outputs(
    frame: &ctx_traits_core::procedure::runtime::SequenceFrame,
) -> crate::Result<Vec<RequestedSlotKey>> {
    let outputs = frame
        .requested_outputs
        .iter()
        .map(|output| {
            let property = output_property_name(&output.slot_ref)?;
            Ok(RequestedSlotKey {
                ref_text: output.slot_ref.to_string(),
                property,
                operation: output.operation.clone(),
                schema_ref: output.schema_ref.as_ref().map(ToString::to_string),
            })
        })
        .collect::<crate::Result<Vec<_>>>()?;
    let mut seen = BTreeMap::<&str, &str>::new();
    for output in &outputs {
        if let Some(existing) = seen.insert(output.property.as_str(), output.ref_text.as_str()) {
            return Err(crate::Error::Command {
                message: format!(
                    "driver generated duplicate JSON property {:?} for outputs {existing:?} and {:?}",
                    output.property, output.ref_text
                ),
            });
        }
    }
    Ok(outputs)
}

fn output_property_name(ref_text: &str) -> crate::Result<String> {
    let raw = ref_text.split_once(':').map_or(ref_text, |(_, path)| path);
    let property = if is_schema_property_name(raw) {
        raw.to_string()
    } else {
        let digest = ctx_traits_core::digest::Digest::source(ref_text)
            .as_str()
            .trim_start_matches("sha256:")
            .chars()
            .take(12)
            .collect::<String>();
        let mut sanitized = raw
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
                    ch
                } else if ch == '/' {
                    '.'
                } else {
                    '-'
                }
            })
            .collect::<String>();
        if sanitized.is_empty() {
            sanitized.push_str("value");
        }
        let suffix = format!("-{digest}");
        let max_base = 64_usize.saturating_sub(suffix.len());
        if sanitized.len() > max_base {
            sanitized.truncate(max_base);
        }
        format!("{sanitized}{suffix}")
    };
    if !is_schema_property_name(&property) || property.contains(':') {
        return Err(crate::Error::Command {
            message: format!(
                "driver generated invalid JSON schema property {:?} for requested output {:?}",
                property, ref_text
            ),
        });
    }
    Ok(property)
}

fn is_schema_property_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}
