// Model-view sanitization.
// Model-view sanitization.

fn collect_exclusions(trait_ref: &Trait, exclusions: &mut Vec<Exclusion>) {
    if trait_ref.composition.is_some() {
        exclusions.push(Exclusion {
            field: "composition".to_string(),
            reason: "composition declarations are review metadata, not model-visible".to_string(),
        });
    }
    if !trait_ref.evals.is_empty() {
        exclusions.push(Exclusion {
            field: "evals".to_string(),
            reason: "eval declarations are product metadata, not model-visible".to_string(),
        });
    }
}

fn sanitize_model_values(
    values: &[String],
    field_prefix: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> Vec<String> {
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            sanitize_model_text(
                value,
                &format!("{field_prefix}[{i}]"),
                warnings,
                normalizations,
            )
        })
        .collect()
}

fn format_sanitized_refs(
    label: &str,
    refs: &[String],
    field_prefix: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let values = sanitize_model_values(refs, field_prefix, warnings, normalizations);
    if values.is_empty() {
        format!("{label}: none")
    } else {
        format!("{label}: {}", values.join(", "))
    }
}

fn comma_or_none_sanitized(
    values: &[String],
    field_prefix: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let sanitized = sanitize_model_values(values, field_prefix, warnings, normalizations);
    if sanitized.is_empty() {
        "none".to_string()
    } else {
        sanitized.join(", ")
    }
}

/// Sanitize hidden/deceptive text before it becomes model-visible output.
pub fn sanitize_model_text(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut current = remove_hidden_controls(value, field, warnings, normalizations);
    current = strip_html_comments(&current, field, warnings, normalizations);
    current = replace_hidden_css(&current, field, warnings, normalizations);
    current = replace_color_on_color_css(&current, field, warnings, normalizations);
    current = replace_deceptive_markdown_links(&current, field, warnings, normalizations);
    current = replace_data_urls(&current, field, warnings, normalizations);
    current = replace_base64_blobs(&current, field, warnings, normalizations);
    current = replace_shell_substitutions(&current, field, warnings, normalizations);
    current = redact_remaining_blocking_audit(&current, field, warnings, normalizations);
    current
}

fn remove_hidden_controls(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut output = String::new();
    let mut count = 0usize;
    for ch in value.chars() {
        if is_hidden_control(ch) {
            count += 1;
        } else {
            output.push(ch);
        }
    }
    if count > 0 {
        record_normalization(
            field,
            "hidden-control",
            NormalizationAction::Removed,
            count,
            "removed hidden Unicode control characters",
            warnings,
            normalizations,
        );
    }
    output
}

fn strip_html_comments(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut output = String::new();
    let mut rest = value;
    let mut count = 0usize;
    loop {
        let Some(start) = rest.find("<!--") else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..start]);
        let body = &rest[start + 4..];
        count += 1;
        let Some(end) = body.find("-->") else {
            break;
        };
        rest = &body[end + 3..];
    }
    if count > 0 {
        record_normalization(
            field,
            "html-comment",
            NormalizationAction::Removed,
            count,
            "removed HTML comments from model-visible text",
            warnings,
            normalizations,
        );
    }
    output
}

fn replace_hidden_css(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let patterns = [
        "display:none",
        "display: none",
        "visibility:hidden",
        "visibility: hidden",
        "opacity:0",
        "opacity: 0",
        "left:-999",
        "left: -999",
        "left:-1000",
        "left: -1000",
        "text-indent:-999",
        "text-indent: -999",
        "text-indent:-1000",
        "text-indent: -1000",
        "font-size:0",
        "font-size: 0",
        "font-size:1px",
        "font-size: 1px",
        "font-size:2px",
        "font-size: 2px",
        "font-size:3px",
        "font-size: 3px",
    ];
    replace_patterns(
        value,
        field,
        &patterns,
        "hidden-css",
        "[hidden-css-removed]",
        warnings,
        normalizations,
    )
}

fn replace_data_urls(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let lower = value.to_ascii_lowercase();
    let mut output = String::new();
    let mut last = 0usize;
    let mut search = 0usize;
    let mut count = 0usize;

    while let Some(relative_start) = lower[search..].find("data:") {
        let start = search + relative_start;
        let end = data_url_end(value, start);
        output.push_str(&value[last..start]);
        output.push_str("[data-url-removed]");
        count += 1;
        last = end;
        search = end;
    }
    output.push_str(&value[last..]);

    if count > 0 {
        record_normalization(
            field,
            "data-url",
            NormalizationAction::Replaced,
            count,
            "replaced inline encoded content in model-visible text",
            warnings,
            normalizations,
        );
    }
    output
}

fn data_url_end(value: &str, start: usize) -> usize {
    for (offset, ch) in value[start..].char_indices() {
        if offset > 0 && is_data_url_delimiter(ch) {
            return start + offset;
        }
    }
    value.len()
}

fn is_data_url_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, ')' | ']' | '}' | '"' | '\'' | '<' | '>')
}

fn replace_base64_blobs(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut count = 0usize;
    let mut output = String::new();
    let mut last = 0usize;
    let mut run_start = None;
    let mut run_len = 0usize;

    for (index, ch) in value.char_indices() {
        if is_base64_char(ch) {
            if run_start.is_none() {
                run_start = Some(index);
                run_len = 0;
            }
            run_len += 1;
            continue;
        }

        if let Some(start) = run_start
            && run_len > 80 {
                output.push_str(&value[last..start]);
                output.push_str("[base64-blob-removed]");
                count += 1;
                last = index;
            }
        run_start = None;
        run_len = 0;
    }

    if let Some(start) = run_start
        && run_len > 80 {
            output.push_str(&value[last..start]);
            output.push_str("[base64-blob-removed]");
            count += 1;
            last = value.len();
        }

    if count > 0 {
        record_normalization(
            field,
            "base64-blob",
            NormalizationAction::Replaced,
            count,
            "replaced base64-like blobs in model-visible text",
            warnings,
            normalizations,
        );
    }
    output.push_str(&value[last..]);
    output
}

fn is_base64_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=')
}

fn replace_shell_substitutions(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut output = value.to_string();
    let mut count = 0usize;
    if output.contains("$(") {
        count += output.matches("$(").count();
        output = output.replace("$(", "$ (");
    }
    if output.contains('`') {
        count += output.matches('`').count();
        output = output.replace('`', "'");
    }
    if count > 0 {
        record_normalization(
            field,
            "shell-substitution",
            NormalizationAction::Replaced,
            count,
            "replaced shell-substitution markers in model-visible text",
            warnings,
            normalizations,
        );
    }
    output
}

fn replace_patterns(
    value: &str,
    field: &str,
    patterns: &[&str],
    code: &str,
    replacement: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut output = value.to_string();
    let mut count = 0usize;
    for pattern in patterns {
        loop {
            let lower = output.to_ascii_lowercase();
            let Some(start) = lower.find(pattern) else {
                break;
            };
            let end = start + pattern.len();
            output.replace_range(start..end, replacement);
            count += 1;
        }
    }
    if count > 0 {
        record_normalization(
            field,
            code,
            NormalizationAction::Replaced,
            count,
            "replaced hidden/deceptive pattern in model-visible text",
            warnings,
            normalizations,
        );
    }
    output
}

fn record_normalization(
    source: &str,
    code: &str,
    action: NormalizationAction,
    count: usize,
    message: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) {
    warnings.push(format!("{source}: {message} ({count})"));
    normalizations.push(Normalization {
        source: source.to_string(),
        code: code.to_string(),
        action,
        count,
        message: message.to_string(),
    });
}

fn replace_color_on_color_css(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let lower = value.to_ascii_lowercase();
    let mut count = 0usize;
    let mut output = String::new();
    let mut last = 0usize;

    let mut search_from = 0usize;
    while search_from < lower.len() {
        let Some(rel_start) = lower[search_from..].find("color") else {
            break;
        };
        let color_start = search_from + rel_start;

        let Some(color_val) = extract_css_value(&lower, color_start, "color") else {
            search_from = color_start + 5;
            continue;
        };

        let bg_start_offset = find_property_after(&lower, color_start, "background-color")
            .or_else(|| find_property_after(&lower, color_start, "background"));
        let Some(bg_start) = bg_start_offset else {
            search_from = color_start + 5;
            continue;
        };
        let Some(bg_val) = extract_css_value(&lower, bg_start, property_name_at(&lower, bg_start))
        else {
            search_from = bg_start + 10;
            continue;
        };

        if color_val == bg_val && !color_val.is_empty() {
            let span_start = find_tag_start(value, color_start);
            let span_end = find_tag_end(value, color_start);
            output.push_str(&value[last..span_start]);
            output.push_str("[color-on-color-removed]");
            count += 1;
            last = span_end;
            search_from = span_end;
        } else {
            search_from = bg_start + 10;
        }
    }
    output.push_str(&value[last..]);

    if count > 0 {
        record_normalization(
            field,
            "color-on-color",
            NormalizationAction::Replaced,
            count,
            "replaced same-color foreground/background CSS in model-visible text",
            warnings,
            normalizations,
        );
    }
    output
}

fn extract_css_value(lower: &str, start: usize, property: &str) -> Option<String> {
    let after = &lower[start + property.len()..];
    let colon = after.find(':')?;
    let value_start = start + property.len() + colon + 1;
    lower[value_start..]
        .split([';', '"', '\'', '>'])
        .next()
        .map(str::trim)
        .map(|v| v.trim_end_matches(" !important"))
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
}

fn find_property_after(lower: &str, after: usize, property: &str) -> Option<usize> {
    lower[after..].find(property).map(|rel| after + rel)
}

fn property_name_at(lower: &str, start: usize) -> &'static str {
    if lower[start..].starts_with("background-color") {
        "background-color"
    } else {
        "background"
    }
}

fn find_tag_start(value: &str, pos: usize) -> usize {
    let bytes = value.as_bytes();
    let mut i = pos;
    while i > 0 && bytes[i] != b'<' {
        i -= 1;
    }
    i
}

fn find_tag_end(value: &str, pos: usize) -> usize {
    let bytes = value.as_bytes();
    let mut i = pos;
    while i < bytes.len() && bytes[i] != b'>' {
        i += 1;
    }
    if i < bytes.len() { i + 1 } else { bytes.len() }
}

fn replace_deceptive_markdown_links(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let mut output = String::new();
    let mut rest = value;
    let mut count = 0usize;

    loop {
        let Some(bracket_start) = rest.find('[') else {
            output.push_str(rest);
            break;
        };
        let after_bracket = &rest[bracket_start + 1..];
        let Some(display_end_rel) = after_bracket.find("](") else {
            output.push_str(&rest[..bracket_start + 1]);
            rest = after_bracket;
            continue;
        };
        let display = &after_bracket[..display_end_rel];

        // If the display text contains another opening bracket, we matched
        // across multiple bracket groups (e.g. a sanitizer placeholder like
        // [color-on-color-removed] followed by a real Markdown link). Skip
        // just this opening bracket so the next iteration finds the correct
        // link boundary.
        if display.contains('[') {
            output.push_str(&rest[..bracket_start + 1]);
            rest = after_bracket;
            continue;
        }

        let url_start_rel = display_end_rel + 2;
        let after_paren = &after_bracket[url_start_rel..];
        let Some(url_end_rel) = after_paren.find(')') else {
            output.push_str(&rest[..bracket_start + 1]);
            rest = after_bracket;
            continue;
        };
        let url = &after_paren[..url_end_rel];

        if display.starts_with("http") && url.starts_with("http") && display != url {
            output.push_str(&rest[..bracket_start]);
            output.push_str("[deceptive-link-removed]");
            count += 1;
            rest = &after_paren[url_end_rel + 1..];
        } else {
            let abs_display_end = bracket_start + 1 + display_end_rel;
            output.push_str(&rest[..abs_display_end + 2]);
            rest = &after_paren[url_end_rel + 1..];
        }
    }

    if count > 0 {
        record_normalization(
            field,
            "deceptive-link",
            NormalizationAction::Replaced,
            count,
            "replaced deceptive Markdown links in model-visible text",
            warnings,
            normalizations,
        );
    }
    output
}

fn redact_remaining_blocking_audit(
    value: &str,
    field: &str,
    warnings: &mut Vec<String>,
    normalizations: &mut Vec<Normalization>,
) -> String {
    let findings = scan_hidden_content(value, "model-view-sanitizer", Some(field));
    let blocking_count = findings
        .iter()
        .filter(|f| !matches!(f.severity, Severity::Advisory))
        .count();

    if blocking_count == 0 {
        return value.to_string();
    }

    warnings.push(format!(
        "{field}: redacted remaining hidden/deceptive content after field sanitation ({blocking_count})"
    ));
    normalizations.push(Normalization {
        source: field.to_string(),
        code: "blocking-audit-redaction".to_string(),
        action: NormalizationAction::Replaced,
        count: blocking_count,
        message: "redacted remaining hidden/deceptive content after field sanitation".to_string(),
    });
    "[redacted-hidden-or-deceptive-content]".to_string()
}

fn is_hidden_control(ch: char) -> bool {
    matches!(
        ch as u32,
        0x200B
            | 0x200C
            | 0x200D
            | 0x2060
            | 0xFEFF
            | 0x202A
            | 0x202B
            | 0x202C
            | 0x202D
            | 0x202E
            | 0x2066
            | 0x2067
            | 0x2068
            | 0x2069
    )
}
