//! Native == WASM ABI parity (P240): byte-compare `validate_json`,
//! `normalize_json`, `audit_json`, and `render_json` against a response
//! built directly from the corresponding `ctx-traits-core` operations. Any
//! mismatch is a correctness bug, not a warning — this test has no
//! tolerance/threshold, only equality.
//!
//! P235 (marketplace corpus), P236 (labeled audit fixture set), and P237
//! (comparison-arm run) are not landed in this checkout, so this test
//! consumes the deterministic fixtures already committed to the repository
//! for the equivalent purpose today: the `ctx.traits` builtin trait
//! documents (`modules/core/builtins/**/generated/index.toml`) for
//! validate/normalize/render, and the existing marketplace-audit self-test
//! fixtures (`scripts/goldens/fixtures/audit*`, `.fixtures/mattpocock`) for
//! audit. When P235/P236/P237 land their own committed fixtures at their
//! eventual repository paths, extend `canonical_trait_fixtures` /
//! `audit_text_fixtures` below to include them — the table-driven checks
//! themselves do not need to change.

use std::path::{Path, PathBuf};

use ctx_traits_core::Trait;
use ctx_traits_core::digest::{Digest, canonical_digest, canonical_json};
use ctx_traits_core::encoding::{self, Encoding};
use ctx_traits_core::render::{ExtendedRenderProfile, plan_render};
use ctx_traits_core::response::{Envelope, JsonAbiErrorCodes};
use ctx_traits_wasm_core::abi::{
    ABI_SCHEMA_VERSION, AuditRequest, NormalizeRequest, NormalizeResponse, RenderRequest,
    TraitDocument, ValidateRequest, ValidateResponse, audit_json, normalize_json, render_json,
    validate_json,
};

const TEST_ERROR_CODES: JsonAbiErrorCodes = JsonAbiErrorCodes {
    decode_code: "test.decode",
    decode_message: "native parity test could not decode a fixture-derived request",
    serialize_code: "test.serialize",
    serialize_message: "native parity test could not serialize its expected envelope",
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Canonical trait documents already committed as `ctx.traits` builtins.
/// Stable sorted order so failures are reproducible across runs.
fn canonical_trait_fixtures() -> Vec<PathBuf> {
    let root = repo_root().join("modules/core/builtins");
    let mut paths = Vec::new();
    collect_generated_index_toml(&root, &mut paths);
    paths.sort();
    assert!(
        !paths.is_empty(),
        "expected committed builtin trait fixtures under {}",
        root.display()
    );
    paths
}

fn collect_generated_index_toml(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("reading fixture dir {}: {error}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| {
                panic!("reading fixture dir entry in {}: {error}", dir.display())
            })
            .path();
        if path.is_dir() {
            collect_generated_index_toml(&path, out);
        } else if path.file_name().and_then(|name| name.to_str()) == Some("index.toml")
            && path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                == Some("generated")
        {
            out.push(path);
        }
    }
}

/// Committed first-party text used as the hidden-content audit corpus.
///
/// The corpus only has to be REAL committed text that both engines must agree
/// on — it is a parity fixture, not a golden. It has been repointed twice as
/// its sources were deleted (the third-party `.fixtures/` samples in ab4855e,
/// then `scripts/goldens` and `.configs/` in P569), so it now reads trait
/// resources that ship with the repository's own packages and have no reason
/// to disappear.
fn audit_text_fixtures() -> Vec<(PathBuf, &'static str)> {
    let root = repo_root();
    let fixtures = vec![
        (
            root.join(".ctx/traits/packages/engineering-standards/resources/coding-standards.md"),
            "engineering-standards",
        ),
        (
            root.join(".ctx/traits/packages/deep-research/resources/research-standards.md"),
            "deep-research",
        ),
        (
            root.join(".ctx/traits/packages/deep-research/resources/quality-rubric.md"),
            "deep-research-rubric",
        ),
    ];
    for (path, _) in &fixtures {
        assert!(
            path.exists(),
            "missing committed audit fixture {}",
            path.display()
        );
    }
    fixtures
}

fn read_fixture(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading fixture {}: {error}", path.display()))
}

fn trait_document(text: String) -> TraitDocument {
    TraitDocument {
        encoding: Encoding::Toml,
        text,
        source_digest: None,
        status: None,
        trust: None,
        trust_digest: None,
    }
}

fn decode_fixture(path: &Path, text: &str) -> Trait {
    encoding::decode_trait(Encoding::Toml, text)
        .unwrap_or_else(|error| panic!("decoding fixture {}: {error}", path.display()))
}

fn native_envelope<T: serde::Serialize>(value: T) -> String {
    ctx_traits_core::response::envelope_to_json(Envelope::ok(value), TEST_ERROR_CODES)
}

#[test]
fn validate_json_matches_native() {
    for path in canonical_trait_fixtures() {
        let text = read_fixture(&path);
        let trait_ref = decode_fixture(&path, &text);

        let request = ValidateRequest {
            trait_document: trait_document(text),
        };
        let abi_response = validate_json(&serde_json::to_string(&request).expect("encode request"));

        let digest = canonical_digest(&trait_ref)
            .unwrap_or_else(|error| panic!("canonical digest for {}: {error}", path.display()));
        let native = native_envelope(ValidateResponse {
            schema_version: ABI_SCHEMA_VERSION.to_string(),
            valid: true,
            trait_id: trait_ref.id.as_str().to_string(),
            trait_version: trait_ref.version.as_str().to_string(),
            canonical_digest: digest.as_str().to_string(),
        });

        assert_eq!(
            abi_response,
            native,
            "validate_json native/ABI parity mismatch for {}",
            path.display()
        );
    }
}

#[test]
fn normalize_json_matches_native() {
    let output_formats = [Encoding::Toml, Encoding::Json, Encoding::Yaml];

    for path in canonical_trait_fixtures() {
        let text = read_fixture(&path);
        for &output_format in &output_formats {
            let trait_ref = decode_fixture(&path, &text);

            let request = NormalizeRequest {
                trait_document: trait_document(text.clone()),
                output_format,
            };
            let abi_response =
                normalize_json(&serde_json::to_string(&request).expect("encode request"));

            let canonical_json_text = canonical_json(&trait_ref)
                .unwrap_or_else(|error| panic!("canonical json for {}: {error}", path.display()));
            let canonical_digest_value = canonical_digest(&trait_ref)
                .unwrap_or_else(|error| panic!("canonical digest for {}: {error}", path.display()));
            let output_text = encoding::encode(output_format, &trait_ref).unwrap_or_else(|error| {
                panic!(
                    "encoding {} as {:?}: {error}",
                    path.display(),
                    output_format
                )
            });
            let native = native_envelope(NormalizeResponse {
                schema_version: ABI_SCHEMA_VERSION.to_string(),
                canonical: trait_ref,
                canonical_json: canonical_json_text,
                canonical_digest: canonical_digest_value.as_str().to_string(),
                output_text,
                output_format,
            });

            assert_eq!(
                abi_response,
                native,
                "normalize_json native/ABI parity mismatch for {} -> {:?}",
                path.display(),
                output_format
            );
        }
    }
}

#[test]
fn audit_json_matches_native() {
    for (path, trait_id) in audit_text_fixtures() {
        let text = read_fixture(&path);

        let request = AuditRequest {
            text: text.clone(),
            trait_id: trait_id.to_string(),
            path: Some(path.display().to_string()),
        };
        let abi_response = audit_json(&serde_json::to_string(&request).expect("encode request"));

        let native = native_envelope(ctx_traits_core::audit::scan_hidden_content(
            &text,
            trait_id,
            Some(path.display().to_string()).as_deref(),
        ));

        assert_eq!(
            abi_response,
            native,
            "audit_json native/ABI parity mismatch for {}",
            path.display()
        );
    }
}

#[test]
fn render_json_matches_native() {
    let profiles = [
        ExtendedRenderProfile::AgentSkills,
        ExtendedRenderProfile::Pi,
        ExtendedRenderProfile::Opencode,
        ExtendedRenderProfile::ClaudeCode,
        ExtendedRenderProfile::Codex,
        ExtendedRenderProfile::Copilot,
        ExtendedRenderProfile::MarkdownOnly,
    ];

    for path in canonical_trait_fixtures() {
        let text = read_fixture(&path);
        let source_digest = Digest::source(&text);

        for &profile in &profiles {
            let trait_ref = decode_fixture(&path, &text);

            let request = RenderRequest {
                trait_document: trait_document(text.clone()),
                profile,
                source_digest: Some(source_digest.as_str().to_string()),
            };
            let abi_response =
                render_json(&serde_json::to_string(&request).expect("encode request"));

            let native = native_envelope(plan_render(&trait_ref, profile, source_digest.as_str()));

            assert_eq!(
                abi_response,
                native,
                "render_json native/ABI parity mismatch for {} -> {:?}",
                path.display(),
                profile
            );
        }
    }
}
