//! Prompt definitions: reusable prompt bodies with optional input/output
//! contracts.
//!
//! `[prompt.<id>]` sections define reusable prompt text that sequence items
//! reference via `prompt = "prompt:<id>"`. The TOML table key is the prompt ID.
//! Prompt text may interpolate typed refs such as `{port:user-prompt}` and
//! `{slot:note-risk}` only. No arbitrary template code, shell interpolation,
//! or dynamic execution is permitted in MVP.
//!
//! Prompt `input` declares the refs the prompt expects (`port:*`, `slot:*`,
//! `resource:*`). Prompt `output` declares the `slot:*` refs the prompt
//! produces. Interpolated refs in prompt text are implicitly required inputs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::reference::{Kind, Reference};

crate::shared::string_list_wrapper! {
    /// Normalized list of contract ref strings for prompt input/output fields.
    /// Accepts scalar-or-array at the decode boundary but always stores a
    /// `Vec<String>` — source scalar-vs-array shape is not preserved as semantic
    /// state. This type is distinct from [`TargetList`](crate::shared::TargetList)
    /// so render target lists and protocol contract refs cannot drift together.
    #[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
    pub struct ContractRefList
}

/// A `[prompt.<id>]` body: inline text or resource source plus contracts.
///
/// The prompt ID is the map key in `Trait.prompts`, not a field on this
/// struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Prompt {
    /// The prompt body text. May contain typed ref interpolations like
    /// `{port:user-prompt}` or `{slot:scope}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Resource-backed prompt body source (`resource:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Required input refs for this prompt (e.g. `port:user-prompt`,
    /// `slot:scope`, `resource:checklist`). Accepts scalar-or-array,
    /// normalized to a flat list.
    #[serde(default, skip_serializing_if = "ContractRefList::is_empty")]
    pub input: ContractRefList,

    /// Output contract refs — local `slot:<id>` refs only. Accepts
    /// scalar-or-array, normalized to a flat list.
    #[serde(default, skip_serializing_if = "ContractRefList::is_empty")]
    pub output: ContractRefList,

    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Duplicate-aware prompt map
// ---------------------------------------------------------------------------

/// A map of prompt IDs to prompt bodies that rejects duplicate keys at decode
/// time rather than silently overwriting.
///
/// This wraps [`BTreeMap<String, Prompt>`] with a custom `Deserialize` that
/// detects duplicate keys and returns an error before collapsing them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct PromptMap(BTreeMap<String, Prompt>);

impl PromptMap {
    pub fn get(&self, key: &str) -> Option<&Prompt> {
        self.0.get(key)
    }

    pub fn keys(&self) -> std::collections::btree_map::Keys<'_, String, Prompt> {
        self.0.keys()
    }

    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, String, Prompt> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<'de> Deserialize<'de> for PromptMap {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PromptMapVisitor;

        impl<'de> Visitor<'de> for PromptMapVisitor {
            type Value = PromptMap;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a prompt object/map keyed by prompt id")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut map = BTreeMap::new();
                while let Some((id, body)) = access.next_entry::<String, Prompt>()? {
                    if map.contains_key(&id) {
                        return Err(de::Error::custom(format!(
                            "duplicate prompt key at prompt.{id}"
                        )));
                    }
                    map.insert(id, body);
                }
                Ok(PromptMap(map))
            }
        }

        deserializer.deserialize_map(PromptMapVisitor)
    }
}

// ---------------------------------------------------------------------------
// Interpolation scanning
// ---------------------------------------------------------------------------

/// An interpolation found in prompt text: a `{ref}` expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpolation {
    /// The raw text between braces (e.g. `port:user-prompt`).
    pub ref_text: String,
    /// Character offset of the opening `{`.
    ///
    /// Crate-internal: lets runtime command-argv resolution substitute
    /// exactly the spans this scanner recognizes, instead of re-parsing the
    /// brace grammar, so validation and substitution can never disagree on
    /// what counts as an interpolation.
    pub(crate) start: usize,
    /// Character offset one past the closing `}`.
    pub(crate) end: usize,
}

/// Single-pass scanner for prompt text interpolation.
///
/// Owns the character buffer, cursor, collected interpolations, and
/// diagnostics. The grammar is intentionally tiny: literal text plus
/// typed-ref interpolation expressions of the exact form `{<body>}`.
///
/// Character positions in diagnostics are Unicode scalar offsets (character
/// positions, not byte offsets).
struct InterpolationScanner {
    chars: Vec<char>,
    cursor: usize,
    interpolations: Vec<Interpolation>,
    diagnostics: Vec<String>,
}

impl InterpolationScanner {
    fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            cursor: 0,
            interpolations: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> (Vec<Interpolation>, Vec<String>) {
        while self.cursor < self.chars.len() {
            let c = self.chars[self.cursor];

            if c == '`' {
                self.scan_backtick();
            } else if c == '$' {
                self.scan_dollar();
            } else if c == '{' {
                self.scan_opening_brace();
            } else if c == '}' {
                self.diagnostics.push(format!(
                    "unmatched closing brace at position {}",
                    self.cursor
                ));
                self.advance();
            } else {
                self.advance();
            }
        }
        (self.interpolations, self.diagnostics)
    }

    // --- Cursor helpers ---

    /// Whether the cursor is at a two-character marker.
    fn starts_with_two(&self, first: char, second: char) -> bool {
        self.chars.get(self.cursor) == Some(&first)
            && self.chars.get(self.cursor + 1) == Some(&second)
    }

    fn advance(&mut self) {
        self.cursor += 1;
    }

    /// Advance cursor past `count` characters.
    fn advance_n(&mut self, count: usize) {
        self.cursor += count;
    }

    /// Find the next occurrence of `target` after the cursor, returning its
    /// index. Does not move the cursor.
    fn find_after(&self, target: char) -> Option<usize> {
        self.chars[self.cursor..].iter().position(|&c| c == target)
    }

    /// Find the next occurrence of a two-character marker after the cursor.
    fn find_pair_after(&self, a: char, b: char) -> Option<usize> {
        self.chars[self.cursor..]
            .windows(2)
            .position(|w| w == [a, b])
    }

    /// Collect characters from `start` to `end` (exclusive) as a `String`.
    fn slice(&self, start: usize, end: usize) -> String {
        self.chars[start..end].iter().collect()
    }

    // --- Rejection helpers ---

    /// Scan a backtick substitution `` `...` ``.
    fn scan_backtick(&mut self) {
        let pos = self.cursor;
        self.advance(); // past opening `
        if let Some(offset) = self.find_after('`') {
            self.advance_n(offset + 1);
        } else {
            self.cursor = self.chars.len();
        }
        self.diagnostics.push(format!(
            "unsupported backtick substitution at position {pos}"
        ));
    }

    /// Scan `${...}` or `$()` forms.
    fn scan_dollar(&mut self) {
        let pos = self.cursor;

        if self.starts_with_two('$', '{') {
            self.advance_n(2); // past ${
            if let Some(offset) = self.find_after('}') {
                self.advance_n(offset + 1);
            } else {
                self.cursor = self.chars.len();
            }
            self.diagnostics.push(format!(
                "unsupported template expression: ${{...}} at position {pos}"
            ));
            return;
        }

        if self.starts_with_two('$', '(') {
            self.advance_n(2); // past $(
            if let Some(offset) = self.find_after(')') {
                self.advance_n(offset + 1);
            } else {
                self.cursor = self.chars.len();
            }
            self.diagnostics.push(format!(
                "unsupported shell substitution: $() at position {pos}"
            ));
            return;
        }

        // Lone $ — just a literal character
        self.advance();
    }

    /// Scan an opening brace: dispatch to double-brace, template-tag, or
    /// single-brace interpolation.
    fn scan_opening_brace(&mut self) {
        if self.starts_with_two('{', '{') {
            self.scan_double_brace();
        } else if self.starts_with_two('{', '%') {
            self.scan_template_tag();
        } else {
            self.scan_braced_interpolation();
        }
    }

    /// Scan `{{...}}` double-brace expression.
    fn scan_double_brace(&mut self) {
        let pos = self.cursor;
        self.advance_n(2); // past {{
        if let Some(offset) = self.find_pair_after('}', '}') {
            self.advance_n(offset + 2);
        } else {
            self.cursor = self.chars.len();
        }
        self.diagnostics.push(format!(
            "unsupported double-brace template at position {pos}"
        ));
    }

    /// Scan `{% ... %}` template tag.
    fn scan_template_tag(&mut self) {
        let pos = self.cursor;
        self.advance_n(2); // past {%
        if let Some(offset) = self.find_pair_after('%', '}') {
            self.advance_n(offset + 2);
        } else {
            self.cursor = self.chars.len();
        }
        self.diagnostics.push(format!(
            "unsupported template tag: {{% ... %}} at position {pos}"
        ));
    }

    /// Scan a single-brace `{body}` interpolation.
    fn scan_braced_interpolation(&mut self) {
        let pos = self.cursor;
        let body_start = self.cursor + 1;

        match self.find_after('}') {
            None => {
                self.diagnostics
                    .push(format!("unmatched opening brace at position {pos}"));
                self.cursor = self.chars.len();
            }
            Some(rel_end) => {
                let body_end = self.cursor + rel_end;
                let body = self.slice(body_start, body_end);

                if body.is_empty() {
                    self.diagnostics
                        .push(format!("empty interpolation braces at position {pos}"));
                } else if body.contains('{') {
                    self.diagnostics
                        .push(format!("nested brace in interpolation at position {pos}"));
                } else {
                    self.interpolations.push(Interpolation {
                        ref_text: body,
                        start: pos,
                        end: body_end + 1,
                    });
                }

                self.advance_n(rel_end + 1); // past `{...}`
            }
        }
    }
}

/// Scan prompt text for `{...}` interpolation expressions and detect
/// unsupported template/shell-like forms.
///
/// Returns `(valid_interpolations, diagnostics)`. Valid interpolations are
/// `{ref}` expressions with non-empty content and no nested braces.
/// Diagnostics are produced for:
/// - Empty braces `{}`
/// - Unmatched opening `{` without closing `}`
/// - Unmatched closing `}` without opening
/// - Nested braces inside an interpolation body (e.g. `{input:{user-prompt}}`)
/// - Shell-like forms: `${...}`, `{{...}}`, `{% ... %}`, `$()`
/// - Backtick command substitution
///
/// Character positions in diagnostics are Unicode scalar offsets.
pub fn scan_interpolations(text: &str) -> (Vec<Interpolation>, Vec<String>) {
    InterpolationScanner::new(text).run()
}

/// Canonical single-token text for a resolved interpolation value: text
/// as-is, number/boolean canonical, and every non-scalar as compact JSON.
/// Shared by every interpolation site (command argv, `[sink.*]` templates)
/// so they render an accepted value identically.
pub fn render_interpolation_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(boolean) => Some(boolean.to_string()),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(value).ok()
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// How a sequence item's `prompt` field should be resolved.
///
/// This enum is the single source of truth for prompt classification across
/// sequence-item validation, procedure wiring, and dry planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptClassification {
    /// Inline prompt text that may contain `{typed-ref}` interpolations.
    Inline,
    /// A local `prompt:<id>` reference (unqualified).
    LocalPromptRef(Reference),
    /// A dependency-qualified `prompt:<dep>/<id>` reference.
    DependencyPromptRef(Reference),
}

/// Classify a sequence item's `prompt` field.
///
/// Rules (in order):
/// 1. If the entire string parses as a typed ref with kind `prompt`:
///    - Unqualified → [`PromptClassification::LocalPromptRef`]
///    - Qualified → [`PromptClassification::DependencyPromptRef`]
/// 2. If the entire string parses as a typed ref of any **other** kind →
///    `Err` (wrong kind for a prompt ref).
/// 3. If the string starts with a known ref-kind prefix followed by `:`
///    but does not parse as a valid typed ref → `Err` (malformed ref attempt).
/// 4. Otherwise → [`PromptClassification::Inline`] (plain prose, possibly
///    with interpolations).
pub(crate) fn classify_prompt(text: &str) -> Result<PromptClassification, String> {
    if let Ok(r) = Reference::parse(text) {
        return if r.kind() == Kind::Prompt {
            if r.is_qualified() {
                Ok(PromptClassification::DependencyPromptRef(r))
            } else {
                Ok(PromptClassification::LocalPromptRef(r))
            }
        } else {
            Err(format!(
                "sequence item prompt is a typed ref of kind {:?}; expected kind \"prompt\" or inline text",
                r.kind()
            ))
        };
    }

    if let Some((prefix, _rest)) = text.split_once(':')
        && Kind::parse(prefix).is_some()
    {
        return Err(format!(
            "sequence item prompt starts with ref-kind prefix {prefix:?} but is not a valid typed ref"
        ));
    }

    Ok(PromptClassification::Inline)
}

/// Kinds that are valid in prompt input contracts.
const VALID_INPUT_KINDS: &[Kind] = &[Kind::Port, Kind::Slot, Kind::Resource, Kind::Setting];

/// Kinds that are valid in prompt output contracts.
const VALID_OUTPUT_KINDS: &[Kind] = &[Kind::Slot];

/// Check whether a ref kind is valid for prompt input contracts.
pub fn is_valid_input_kind(kind: Kind) -> bool {
    VALID_INPUT_KINDS.contains(&kind)
}

/// Check whether a ref kind is valid for prompt output contracts.
pub fn is_valid_output_kind(kind: Kind) -> bool {
    VALID_OUTPUT_KINDS.contains(&kind)
}

/// Validate prompt declarations from a [`PromptMap`].
///
/// Checks for:
/// - Invalid prompt IDs (map keys must be valid kebab-case slugs)
/// - Unsupported template/shell-like expressions or malformed interpolations
/// - Input/output contract refs that don't parse as typed refs or have
///   invalid kinds
///
/// Returns the first error found, or `Ok(())` if all prompts are valid.
/// Diagnostics use field-specific paths such as `prompt.<id>.text` or
/// `prompt.<id>.input[0]`.
pub fn validate_prompts(
    prompts: &PromptMap,
    settings: &[crate::r#trait::Setting],
) -> crate::Result<()> {
    for (id, body) in prompts.iter() {
        let id_path = format!("prompt.{id}");
        crate::shared::validate_slug_shape(id, &id_path)?;

        match (&body.text, &body.source) {
            (Some(text), None) => validate_prompt_text(text, &id_path, settings)?,
            (None, Some(source)) => validate_prompt_source(source, &id_path)?,
            (Some(_), Some(_)) => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: id_path,
                    message: "prompt must declare either text or source, not both".to_string(),
                }
                .into());
            }
            (None, None) => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: id_path,
                    message: "prompt must declare text or source".to_string(),
                }
                .into());
            }
        }

        // Validate input contract refs: syntax + kind
        for (j, ref_text) in body.input.iter().enumerate() {
            let parsed =
                Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                    field_path: format!("{id_path}.input[{j}]"),
                    message: format!("invalid typed ref {:?}", ref_text),
                })?;
            if !is_valid_input_kind(parsed.kind()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{id_path}.input[{j}]"),
                    message: format!(
                        "prompt input ref kind {:?} not allowed; expected port, slot, setting, or resource",
                        parsed.kind()
                    ),
                }
                .into());
            }
            if parsed.kind() == Kind::Setting {
                validate_setting_ref_exists(
                    &parsed,
                    ref_text,
                    settings,
                    &format!("{id_path}.input[{j}]"),
                )?;
            }
        }

        // Validate output contract refs: syntax, kind, and local-only.
        for (j, ref_text) in body.output.iter().enumerate() {
            let parsed =
                Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                    field_path: format!("{id_path}.output[{j}]"),
                    message: format!("invalid typed ref {:?}", ref_text),
                })?;
            if !is_valid_output_kind(parsed.kind()) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{id_path}.output[{j}]"),
                    message: format!(
                        "prompt output ref kind {:?} not allowed; expected slot",
                        parsed.kind()
                    ),
                }
                .into());
            }
            if parsed.is_qualified() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{id_path}.output[{j}]"),
                    message: "prompt output must be a local unqualified slot ref".to_string(),
                }
                .into());
            }
        }
    }

    Ok(())
}

/// Cross-check local resource-backed prompt sources after resource IDs are known.
pub fn validate_prompt_resource_sources(
    prompts: &PromptMap,
    resource_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    for (id, body) in prompts.iter() {
        let Some(source) = body.source.as_deref() else {
            continue;
        };
        let id_path = format!("prompt.{id}");
        let parsed =
            Reference::parse(source).map_err(|_| crate::manifest::Error::InvalidField {
                field_path: format!("{id_path}.source"),
                message: format!("invalid prompt source ref {source:?}"),
            })?;
        if parsed.kind() != Kind::Resource || parsed.is_qualified() {
            continue;
        }
        if !resource_ids.contains(parsed.id()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{id_path}.source"),
                message: format!(
                    "prompt source references undeclared resource {:?}",
                    parsed.id()
                ),
            }
            .into());
        }
    }
    Ok(())
}

/// Resolve a `setting:<id>` ref against declared settings, failing with the
/// resolved id when unnamed — mirrors `procedure::validate::resolve_setting_ref`
/// (task Watch item: an unknown id names itself the same way at every
/// reference site). Kept local since prompt declaration validation runs
/// before a full `Trait` is available, only a settings slice.
pub(crate) fn validate_setting_ref_exists(
    parsed: &Reference,
    ref_text: &str,
    settings: &[crate::r#trait::Setting],
    field_path: &str,
) -> crate::Result<()> {
    if parsed.is_qualified() || !settings.iter().any(|s| s.id == parsed.id()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!("unresolved setting ref {ref_text:?} names no declared setting"),
        }
        .into());
    }
    Ok(())
}

fn validate_prompt_text(
    text: &str,
    id_path: &str,
    settings: &[crate::r#trait::Setting],
) -> crate::Result<()> {
    if text.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{id_path}.text"),
            message: "must not be empty".to_string(),
        }
        .into());
    }

    let (interps, template_diags) = scan_interpolations(text);
    if let Some(d) = template_diags.first() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{id_path}.text"),
            message: d.clone(),
        }
        .into());
    }
    for interp in &interps {
        let parsed = Reference::parse(&interp.ref_text).map_err(|_| {
            crate::manifest::Error::InvalidField {
                field_path: format!("{id_path}.text"),
                message: format!(
                    "interpolation {{{}}} is not a valid typed ref",
                    interp.ref_text
                ),
            }
        })?;
        if !VALID_INPUT_KINDS.contains(&parsed.kind()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{id_path}.text"),
                message: format!(
                    "interpolation {{{}}} kind {:?} not allowed; expected port, slot, setting, or resource",
                    interp.ref_text,
                    parsed.kind()
                ),
            }
            .into());
        }
        if parsed.kind() == Kind::Setting {
            validate_setting_ref_exists(
                &parsed,
                &interp.ref_text,
                settings,
                &format!("{id_path}.text"),
            )?;
        }
    }
    Ok(())
}

fn validate_prompt_source(source: &str, id_path: &str) -> crate::Result<()> {
    let parsed = Reference::parse(source).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: format!("{id_path}.source"),
        message: format!("invalid prompt source ref {source:?}"),
    })?;
    if parsed.kind() != Kind::Resource {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{id_path}.source"),
            message: format!(
                "prompt source ref kind {:?} not allowed; expected resource",
                parsed.kind()
            ),
        }
        .into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Interpolation ref extraction
// ---------------------------------------------------------------------------

/// Extract all interpolation refs from a prompt's text as parsed typed refs.
///
/// Returns `(valid_refs, invalid_texts, template_diagnostics)`. If
/// `template_diagnostics` is non-empty, the prompt text contains unsupported
/// expressions and should have been rejected by [`validate_prompts`] before
/// calling this function. Sequence-item input satisfaction checks are deferred
/// to procedure planning.
pub fn prompt_interpolation_refs(prompt: &Prompt) -> (Vec<Reference>, Vec<String>, Vec<String>) {
    let (interps, diags) = prompt
        .text
        .as_deref()
        .map(scan_interpolations)
        .unwrap_or_default();
    let mut valid = Vec::new();
    let mut invalid = Vec::new();

    for interp in interps {
        match Reference::parse(&interp.ref_text) {
            Ok(r) => valid.push(r),
            Err(_) => invalid.push(interp.ref_text),
        }
    }

    (valid, invalid, diags)
}
