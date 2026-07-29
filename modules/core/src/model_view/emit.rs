// Model-view tag emitters (Render v2).
//
// The single renderer-owned tag table, plus the imperative-body element and
// envelope constructors. Shared by the emitter, the delimiter-integrity
// escape, and the forged-render-tag audit — nothing else may hardcode a
// second tag list.

/// The complete set of renderer-owned tag names used across both the
/// behavior and authoring envelopes. A closing tag for any of these found
/// inside an untrusted body is escaped by [`body_integrity`] and flagged by
/// `crate::audit::scan_forged_render_tags`.
pub static RENDER_TAGS: &[&str] = &[
    "trait",
    "summary",
    "intent",
    "behavior",
    "resource",
    "agent",
    "prompt",
    "procedure",
    "activation",
    "port",
    "signal",
    "scenario",
    "relations",
];

/// Render one imperative-body element with a two-space indent inside its
/// envelope: `<tag attr="value" ...>\n  body\n</tag>`.
pub fn element(tag: &str, attrs: &[(&str, &str)], body: &str) -> String {
    let attr_text = attrs
        .iter()
        .map(|(name, value)| format!(" {name}=\"{value}\""))
        .collect::<String>();
    if body.is_empty() {
        return format!("<{tag}{attr_text}></{tag}>");
    }
    let indented = body
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("<{tag}{attr_text}>\n{indented}\n</{tag}>")
}

/// Wrap the selected sections' already-rendered content in the `<trait>`
/// envelope, in section order, skipping empty sections.
pub fn envelope(attrs: &[(&str, &str)], sections: &[&Section]) -> String {
    let body = sections
        .iter()
        .map(|section| section.content.as_str())
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    element("trait", attrs, &body)
}

/// Rule 6 delimiter integrity: escape the `<` of any renderer-owned closing
/// tag found inside an untrusted body so it cannot terminate its enclosing
/// element early or forge a sibling element. Every escape is recorded as a
/// visible `Normalization` (`code = "render-tag-escape"`). Called only from
/// [`leaf_element`], which pairs the escape with the reviewer-visible
/// `ForgedRenderTag` finding — the escape (a safe rewrite) and the finding (a
/// reviewer-visible flag) are two halves of the same remedy.
///
/// Scoped to renderer-owned closers only: a generic `<div>` or `<T>` inside a
/// code exemplar is left untouched, since only a real renderer tag name can
/// forge a real element.
pub fn body_integrity(
    body: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut output = body.to_string();
    let mut total = 0usize;
    for tag in RENDER_TAGS {
        let closer = format!("</{tag}>");
        let escaped = format!("&lt;/{tag}>");
        let count = output.matches(closer.as_str()).count();
        if count > 0 {
            total += count;
            output = output.replace(closer.as_str(), &escaped);
        }
    }
    if total > 0 {
        record_normalization(
            field,
            "render-tag-escape",
            NormalizationAction::Replaced,
            total,
            "escaped renderer-owned closing tag(s) found in an untrusted body",
            warnings,
            normalizations,
        );
    }
    output
}

/// The single choke point through which any element carrying an untrusted
/// (author-controlled) body becomes an element: it scans the pre-escape body
/// for a forged renderer tag (recording an Advisory `ForgedRenderTag` finding
/// via `crate::audit::scan_forged_render_tags`), escapes it (`body_integrity`,
/// which records the paired `Normalization`), and only then wraps the result
/// in `tag`. This is the ONLY path that may turn author-controlled text into
/// an element — `element()`/`envelope()` stay reserved for renderer-assembled
/// container bodies (the envelope body and joined leaf-element output), which
/// legitimately contain real closing tags and must never be escaped.
#[allow(clippy::too_many_arguments)]
pub fn leaf_element(
    tag: &str,
    attrs: &[(&str, &str)],
    body: &str,
    field: &str,
    trait_id: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
    findings: &mut Vec<Finding>,
) -> String {
    findings.extend(crate::audit::scan_forged_render_tags(body, trait_id, None));
    let escaped = body_integrity(body, field, warnings, normalizations);
    element(tag, attrs, &escaped)
}
