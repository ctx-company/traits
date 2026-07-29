//! Static hidden-content and deceptive-content audit.
//!
//! Scans Markdown and text resources for hidden or deceptive content that
//! could compromise reviewer trust. Detection is deterministic and produces
//! structured findings with severity, code, message, trait ID, path, and
//! remediation guidance.
//!
//! This module is pure — it scans text content and returns findings. It does
//! not execute scripts, call external tools, or modify files.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Finding types
// ---------------------------------------------------------------------------

/// The severity of an audit finding.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Severity {
    /// Hard hidden-content risk that could compromise reviewer trust.
    Critical,
    /// Warning about suspicious content.
    Warning,
    /// Advisory note about potential quality issues warranting reviewer attention.
    #[serde(alias = "info")]
    Advisory,
}

/// The code identifying the type of audit finding.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Code {
    /// HTML comment that could hide instructions from human reviewers.
    HtmlComment,
    /// Raw HTML or CSS that could hide content.
    RawHtml,
    /// Inline CSS likely renders foreground and background as the same color.
    ColorOnColor,
    /// Zero-width or invisible Unicode characters.
    ZeroWidthChar,
    /// Bidi (bidirectional) Unicode control characters.
    BidiControl,
    /// Suspicious homoglyph or confusable character.
    Homoglyph,
    /// Deceptive link (mismatched display text and URL).
    DeceptiveLink,
    /// Base64-encoded blob that could hide content.
    Base64Blob,
    /// Data URL with embedded content.
    DataUrl,
    /// Shell substitution pattern in content.
    ShellSubstitution,
    /// Renderer-owned markup smuggled inside an untrusted trait body.
    ForgedRenderTag,
}

impl Code {
    /// Stable kebab-case finding code.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Human-readable description of this finding code.
    pub fn description(&self) -> &'static str {
        match self {
            Self::HtmlComment => "HTML comment may hide instructions from reviewers",
            Self::RawHtml => "raw HTML or CSS may hide content from plain-text review",
            Self::ColorOnColor => {
                "inline CSS may render foreground and background as the same color"
            }
            Self::ZeroWidthChar => "zero-width or invisible Unicode character detected",
            Self::BidiControl => "bidi control character may reverse or obscure text display",
            Self::Homoglyph => "homoglyph or confusable character detected",
            Self::DeceptiveLink => "link display text does not match target URL",
            Self::Base64Blob => "base64-encoded blob may hide content",
            Self::DataUrl => "data URL embeds content inline",
            Self::ShellSubstitution => "shell substitution pattern detected in content",
            Self::ForgedRenderTag => "renderer-owned tag found inside an untrusted trait body",
        }
    }
}

/// A single audit finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Finding {
    /// The severity of the finding.
    pub severity: Severity,
    /// The code identifying the finding type.
    pub code: Code,
    /// Human-readable message.
    pub message: String,
    /// The trait ID being audited.
    pub trait_id: String,
    /// The source path of the content, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Byte offset in the content, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<usize>,
    /// Line number (1-based), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// Remediation guidance.
    pub remediation: String,
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// Scan text content for hidden or deceptive content.
///
/// Produces findings sorted deterministically by
/// `(severity, code, path, line, byte_offset, message)`.
/// Advisory-severity findings are kept distinct from hard hidden-content
/// risks (Critical/Warning).
pub fn scan_hidden_content(text: &str, trait_id: &str, path: Option<&str>) -> Vec<Finding> {
    let mut findings = Vec::new();

    let mut byte_start = 0usize;
    for (line_num, raw_line) in text.split_inclusive('\n').enumerate() {
        let line = raw_line
            .strip_suffix('\n')
            .map_or(raw_line, |without_newline| without_newline);
        let line_no = line_num + 1;
        let path_str = path.map(|p| p.to_string());

        // HTML comments: <!-- ... -->
        scan_html_comments(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Raw HTML tags (display:none, visibility:hidden, etc.)
        scan_raw_html(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Zero-width and invisible characters
        scan_zero_width_chars(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Bidi control characters
        scan_bidi_controls(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Homoglyph/confusable characters
        scan_homoglyphs(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Base64 blobs (long base64-like strings)
        scan_base64_blobs(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Data URLs
        scan_data_urls(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Shell substitution patterns
        scan_shell_substitution(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        // Deceptive links [text](url) where text != url domain
        scan_deceptive_links(
            line,
            trait_id,
            &path_str,
            line_no,
            byte_start,
            &mut findings,
        );

        byte_start += raw_line.len();
    }

    findings.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(a.code.cmp(&b.code))
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then(a.byte_offset.cmp(&b.byte_offset))
            .then(a.message.cmp(&b.message))
    });

    findings
}

/// Scan one untrusted trait body for renderer-owned markup (e.g.
/// `<intent>` or `</resource>`). A closing tag could escape its element and
/// forge a sibling element; an opening tag remains visible but is still
/// reviewer-relevant author-controlled markup. This is a scoped, Advisory-only
/// companion to [`scan_hidden_content`]: it is called by the tag emitter on
/// each untrusted body it escapes. Closing tags are escaped by the emitter;
/// all renderer-tag markup is reported for review. Advisory severity is
/// required — Warning or Critical would trigger blocking redaction of the
/// whole body, which is not the specified remedy for a forged closer.
pub fn scan_forged_render_tags(text: &str, trait_id: &str, path: Option<&str>) -> Vec<Finding> {
    let mut findings = Vec::new();
    let path_str = path.map(|p| p.to_string());
    for tag in crate::model_view::RENDER_TAGS {
        let opener = format!("<{tag}");
        for (offset, _) in text.match_indices(&opener) {
            let tag_end = offset + opener.len();
            if !text
                .as_bytes()
                .get(tag_end)
                .is_some_and(|byte| matches!(*byte, b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r'))
            {
                continue;
            }
            let line_no = text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1;
            let markup = text[offset..]
                .find('>')
                .map(|end| &text[offset..=offset + end])
                .unwrap_or(&text[offset..]);
            add_finding_with_offset(
                &mut findings,
                FindingInput {
                    severity: Severity::Advisory,
                    code: Code::ForgedRenderTag,
                    trait_id,
                    path: &path_str,
                    line: line_no,
                    byte_offset: Some(offset),
                    message: format!(
                        "renderer-owned tag `{markup}` found in an untrusted body at line {line_no}; closing tags are escaped in the emitted render"
                    ),
                    remediation: "the emitter escapes renderer-owned closing tags automatically; remove renderer markup from author-controlled content",
                },
            );
        }
        let closer = format!("</{tag}>");
        for (offset, _) in text.match_indices(&closer) {
            let line_no = text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1;
            add_finding_with_offset(
                &mut findings,
                FindingInput {
                    severity: Severity::Advisory,
                    code: Code::ForgedRenderTag,
                    trait_id,
                    path: &path_str,
                    line: line_no,
                    byte_offset: Some(offset),
                    message: format!(
                        "renderer-owned closing tag `{closer}` found in an untrusted body at line {line_no}; escaped in the emitted render"
                    ),
                    remediation: "the emitter escapes this occurrence automatically; no author action required",
                },
            );
        }
    }
    findings
}

struct FindingInput<'a> {
    severity: Severity,
    code: Code,
    trait_id: &'a str,
    path: &'a Option<String>,
    line: usize,
    byte_offset: Option<usize>,
    message: String,
    remediation: &'a str,
}

fn add_finding_with_offset(findings: &mut Vec<Finding>, input: FindingInput<'_>) {
    findings.push(Finding {
        severity: input.severity,
        code: input.code,
        message: input.message,
        trait_id: input.trait_id.to_string(),
        path: input.path.clone(),
        byte_offset: input.byte_offset,
        line: Some(input.line),
        remediation: input.remediation.to_string(),
    });
}

fn scan_html_comments(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    if let Some(offset) = line.find("<!--") {
        add_finding_with_offset(
            findings,
            FindingInput {
                severity: Severity::Critical,
                code: Code::HtmlComment,
                trait_id,
                path,
                line: line_no,
                byte_offset: Some(byte_start + offset),
                message: format!(
                    "HTML comment at line {line_no} may hide instructions from reviewers"
                ),
                remediation: "remove HTML comments or move their content to visible documentation",
            },
        );
    }
}

fn scan_raw_html(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    let lower = line.to_ascii_lowercase();
    if let Some((offset, reason)) = hidden_css_match(&lower) {
        add_finding_with_offset(
            findings,
            FindingInput {
                severity: Severity::Critical,
                code: Code::RawHtml,
                trait_id,
                path,
                line: line_no,
                byte_offset: Some(byte_start + offset),
                message: format!(
                    "hidden HTML/CSS at line {line_no} may conceal content ({reason})"
                ),
                remediation: "remove hidden HTML/CSS or make the content visible",
            },
        );
    }

    if let Some(offset) = color_on_color_match(&lower) {
        add_finding_with_offset(
            findings,
            FindingInput {
                severity: Severity::Warning,
                code: Code::ColorOnColor,
                trait_id,
                path,
                line: line_no,
                byte_offset: Some(byte_start + offset),
                message: format!("foreground and background colors may match at line {line_no}"),
                remediation: "avoid same-color foreground/background inline styles",
            },
        );
    }
}

fn hidden_css_match(line: &str) -> Option<(usize, &'static str)> {
    let patterns = [
        ("display:none", "display:none"),
        ("display: none", "display:none"),
        ("visibility:hidden", "visibility:hidden"),
        ("visibility: hidden", "visibility:hidden"),
        ("opacity:0", "opacity:0"),
        ("opacity: 0", "opacity:0"),
        ("left:-999", "large negative left positioning"),
        ("left: -999", "large negative left positioning"),
        ("left:-1000", "large negative left positioning"),
        ("left: -1000", "large negative left positioning"),
        ("text-indent:-999", "large negative text-indent"),
        ("text-indent: -999", "large negative text-indent"),
        ("text-indent:-1000", "large negative text-indent"),
        ("text-indent: -1000", "large negative text-indent"),
        ("font-size:0", "tiny font-size"),
        ("font-size: 0", "tiny font-size"),
        ("font-size:1px", "tiny font-size"),
        ("font-size: 1px", "tiny font-size"),
        ("font-size:2px", "tiny font-size"),
        ("font-size: 2px", "tiny font-size"),
        ("font-size:3px", "tiny font-size"),
        ("font-size: 3px", "tiny font-size"),
    ];

    patterns
        .iter()
        .filter_map(|(pattern, reason)| line.find(pattern).map(|offset| (offset, *reason)))
        .min_by_key(|(offset, _)| *offset)
}

fn color_on_color_match(line: &str) -> Option<usize> {
    let color = css_property_value(line, "color")?;
    let background = css_property_value(line, "background-color")
        .or_else(|| css_property_value(line, "background"))?;

    if color.1 == background.1 {
        Some(color.0.min(background.0))
    } else {
        None
    }
}

fn css_property_value(line: &str, property: &str) -> Option<(usize, String)> {
    let property_start = line.find(property)?;
    let after_property = &line[property_start + property.len()..];
    let colon_offset = after_property.find(':')?;
    let value_start = property_start + property.len() + colon_offset + 1;
    let value = line[value_start..]
        .split([';', '"', '\'', '>'])
        .next()
        .map(str::trim)
        .map(|value| value.trim_end_matches(" !important"))
        .filter(|value| !value.is_empty())?;

    Some((property_start, value.to_string()))
}

fn scan_zero_width_chars(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    for (col, ch) in line.char_indices() {
        let code = ch as u32;
        // Zero-width space (U+200B), zero-width non-joiner (U+200C),
        // zero-width joiner (U+200D), word joiner (U+2060),
        // zero-width no-break space / BOM (U+FEFF)
        if matches!(code, 0x200B | 0x200C | 0x200D | 0x2060 | 0xFEFF) {
            add_finding_with_offset(
                findings,
                FindingInput {
                    severity: Severity::Critical,
                    code: Code::ZeroWidthChar,
                    trait_id,
                    path,
                    line: line_no,
                    byte_offset: Some(byte_start + col),
                    message: format!(
                        "zero-width character U+{code:04X} at line {line_no} column {col}"
                    ),
                    remediation: "remove invisible characters from the content",
                },
            );
        }
    }
}

fn scan_bidi_controls(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    for (col, ch) in line.char_indices() {
        let code = ch as u32;
        // LRE (U+202A), RLE (U+202B), PDF (U+202C), LRO (U+202D), RLO (U+202E),
        // LRI (U+2066), RLI (U+2067), FSI (U+2068), PDI (U+2069)
        if matches!(
            code,
            0x202A | 0x202B | 0x202C | 0x202D | 0x202E | 0x2066 | 0x2067 | 0x2068 | 0x2069
        ) {
            add_finding_with_offset(
                findings,
                FindingInput {
                    severity: Severity::Critical,
                    code: Code::BidiControl,
                    trait_id,
                    path,
                    line: line_no,
                    byte_offset: Some(byte_start + col),
                    message: format!(
                        "bidi control character U+{code:04X} at line {line_no} column {col} may obscure text display"
                    ),
                    remediation: "remove bidi control characters from the content",
                },
            );
        }
    }
}

fn scan_homoglyphs(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    for (col, ch) in line.char_indices() {
        let code = ch as u32;
        // Greek and Cyrillic ranges contain many Latin-looking confusables.
        // This is intentionally advisory: legitimate non-Latin content may be
        // reviewed and allowed, but reviewers should see the mismatch.
        if matches!(code, 0x0370..=0x052F) {
            add_finding_with_offset(
                findings,
                FindingInput {
                    severity: Severity::Advisory,
                    code: Code::Homoglyph,
                    trait_id,
                    path,
                    line: line_no,
                    byte_offset: Some(byte_start + col),
                    message: format!(
                        "possible homoglyph character U+{code:04X} at line {line_no} column {col}"
                    ),
                    remediation: "verify the character is intentional and not a Latin-looking confusable",
                },
            );
        }
    }
}

fn scan_base64_blobs(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    // Look for base64-like strings longer than 80 chars
    let trimmed = line.trim();
    if trimmed.len() > 80
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
    {
        let offset = line.find(trimmed).map(|offset| byte_start + offset);
        add_finding_with_offset(
            findings,
            FindingInput {
                severity: Severity::Warning,
                code: Code::Base64Blob,
                trait_id,
                path,
                line: line_no,
                byte_offset: offset,
                message: format!(
                    "base64-like blob at line {line_no} ({} chars) may hide content",
                    trimmed.len()
                ),
                remediation: "replace embedded base64 with a resource reference",
            },
        );
    }
}

fn scan_data_urls(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    if line.contains("data:") && (line.contains("base64,") || line.contains("text/html")) {
        let offset = line.find("data:").map(|offset| byte_start + offset);
        add_finding_with_offset(
            findings,
            FindingInput {
                severity: Severity::Warning,
                code: Code::DataUrl,
                trait_id,
                path,
                line: line_no,
                byte_offset: offset,
                message: format!("data URL at line {line_no} embeds content inline"),
                remediation: "replace inline data URLs with explicit resource references",
            },
        );
    }
}

fn scan_shell_substitution(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    // Look for $(...) or `...` in non-code-block lines
    if line.trim_start().starts_with("```") {
        return;
    }
    let dollar_paren = line.find("$(");
    let backtick = line.find('`');
    if dollar_paren.is_none() && backtick.is_none() {
        return;
    }
    // `$(...)` is genuine substitution syntax and stays a warning. A bare
    // backtick is overwhelmingly Markdown inline code — plainly visible to
    // any reviewer, so it cannot hide content; advisory keeps it surfaced
    // without blocking the trust gates.
    let (severity, message, remediation) = if dollar_paren.is_some() {
        (
            Severity::Warning,
            format!("shell substitution pattern at line {line_no}"),
            "verify shell patterns are not attempting execution",
        )
    } else {
        (
            Severity::Advisory,
            format!("backtick substitution marker at line {line_no} (likely inline code)"),
            "backticks usually mark inline code; verify none are intended for shell execution",
        )
    };
    let offset = dollar_paren.or(backtick).map(|offset| byte_start + offset);
    add_finding_with_offset(
        findings,
        FindingInput {
            severity,
            code: Code::ShellSubstitution,
            trait_id,
            path,
            line: line_no,
            byte_offset: offset,
            message,
            remediation,
        },
    );
}

fn scan_deceptive_links(
    line: &str,
    trait_id: &str,
    path: &Option<String>,
    line_no: usize,
    byte_start: usize,
    findings: &mut Vec<Finding>,
) {
    // Markdown links: [display](url) where display looks like a URL but differs.
    // Iterate all `[` positions so we detect deceptive links even when preceded
    // by other bracket-containing text (e.g. sanitizer placeholders).
    let mut search_from = 0;
    while let Some(rel_start) = line[search_from..].find('[') {
        let start = search_from + rel_start;
        if let Some(mid) = line[start..].find("](") {
            if let Some(end) = line[start + mid + 2..].find(')') {
                let display = &line[start + 1..start + mid];
                // Skip cross-bracket matches: display must not contain `[`.
                if !display.contains('[') && display.starts_with("http") {
                    let url = &line[start + mid + 2..start + mid + 2 + end];
                    if url.starts_with("http") && display != url {
                        add_finding_with_offset(
                            findings,
                            FindingInput {
                                severity: Severity::Critical,
                                code: Code::DeceptiveLink,
                                trait_id,
                                path,
                                line: line_no,
                                byte_offset: Some(byte_start + start),
                                message: format!(
                                    "deceptive link at line {line_no}: display {display:?} does not match URL {url:?}"
                                ),
                                remediation: "fix the link so display text matches the target URL",
                            },
                        );
                    }
                }
            }
        }
        search_from = start + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{Code, Severity, scan_forged_render_tags, scan_hidden_content};

    #[test]
    fn reports_deceptive_link_with_stable_location() {
        let findings = scan_hidden_content(
            "[https://trusted.example](https://attacker.example)",
            "example",
            Some("SKILL.md"),
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, Code::DeceptiveLink);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert_eq!(findings[0].line, Some(1));
        assert_eq!(findings[0].byte_offset, Some(0));
    }

    #[test]
    fn ignores_matching_link_text() {
        assert!(
            scan_hidden_content(
                "[https://trusted.example](https://trusted.example)",
                "example",
                None,
            )
            .is_empty()
        );
    }

    #[test]
    fn forged_renderer_openers_cross_line_endings_without_prefix_false_positives() {
        let text = "<intent\n group=\"require\">one</intent>\r\n<intent\r\n group=\"avoid\">two</intent>\n<intentional>safe</intentional>\n<div>safe</div>";
        let findings = scan_forged_render_tags(text, "example", None);

        let opening_findings: Vec<_> = findings
            .iter()
            .filter(|finding| finding.message.starts_with("renderer-owned tag `<intent"))
            .collect();
        assert_eq!(opening_findings.len(), 2);
        assert_eq!(opening_findings[0].line, Some(1));
        assert_eq!(opening_findings[0].byte_offset, Some(0));
        assert_eq!(opening_findings[1].line, Some(3));
        assert_eq!(
            opening_findings[1].byte_offset,
            Some(text.find("<intent\r\n").expect("second opener is present"))
        );
        assert!(
            findings
                .iter()
                .all(|finding| !finding.message.contains("intentional"))
        );
        assert!(
            findings
                .iter()
                .all(|finding| !finding.message.contains("<div>"))
        );
    }
}
