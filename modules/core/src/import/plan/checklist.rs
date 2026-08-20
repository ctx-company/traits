// Deterministic Markdown checklist reconciler (P405).
//
// Recognizes exactly two Markdown structures as checklists: a top-level
// contiguous run of task-list bullets (`- [ ] text` / `- [x] text`), and a
// heading whose text contains the word "checklist" followed by a bulleted
// list (checkbox optional). Everything else — ordinary prose, headings that
// don't mention "checklist", fenced code, frontmatter — is left untouched so
// recognition stays narrow and false-positive-resistant.
//
// Item identity is declared, never derived from text: a first derivation
// seeds ids by slugifying item text (with numeric suffixes on collision), and
// every later derivation reconciles against the caller-supplied prior items
// so a reworded criterion keeps its id instead of looking like a remove/add.

use std::collections::BTreeSet;

use crate::r#trait::{CHECKLIST_VARIANT, ChecklistItem, Resource, ResourceInputList, ResourceRoot};

/// A minimum-confidence Jaccard similarity between two items' normalized word
/// sets before a reworded item is allowed to reclaim a prior id. Chosen
/// conservatively: a majority of words must still overlap. Ties or scores
/// below this threshold are reported as an add/remove pair instead of a
/// guessed identity.
const REWORD_SIMILARITY_THRESHOLD: f64 = 0.6;

/// Maximum length of a seeded checklist item id before truncating at a
/// hyphen boundary, mirroring the practical length of other declared slugs.
const MAX_SEEDED_ID_LEN: usize = 60;

/// One Markdown checklist item as extracted from source text, before any id
/// has been assigned.
struct RawChecklistItem {
    text: String,
    detail: Option<String>,
}

/// The pure result of extracting checklist structures from Markdown text:
/// the raw items in source order, and the 0-indexed `[start, end)` line
/// ranges that carried them (and are therefore no longer prose).
struct RawChecklistExtraction {
    items: Vec<RawChecklistItem>,
    removed_ranges: Vec<(usize, usize)>,
}

/// A derived checklist resource plus the residual guidance text with the
/// recognized checklist spans removed.
pub struct ChecklistExtraction {
    pub resource: Resource,
    pub residual_text: String,
    /// Prompt-safety warnings from sanitizing each item's text/detail (see
    /// [`sanitize_prompt_text`]), collected here rather than by a caller
    /// running sanitization as a separate after-the-fact pass.
    pub warnings: Vec<String>,
}

/// Derive a typed `variant = "checklist"` resource from Markdown text,
/// reconciling against `prior_items` so an item that only changed wording
/// keeps its id. Returns `None` when no supported checklist structure is
/// recognized, leaving the existing path-backed/guidance handling in place.
///
/// Structure detection (task-list bullets, checklist headings, fences) runs
/// on the raw Markdown so a fenced-code task marker stays inside intact
/// fence delimiters. Every extracted item's text/detail is then sanitized
/// for prompt-safety *before* id reconciliation, so both the values compared
/// for identity and the values ultimately stored come from the same
/// canonical text — an item's identity can never depend on which raw
/// spelling (e.g. a backtick vs. a quote) happened to sanitize the same way.
pub fn derive_checklist_resource(
    resource_id: &str,
    markdown: &str,
    prior_items: &[ChecklistItem],
) -> Option<ChecklistExtraction> {
    let raw = extract_checklist_spans(markdown)?;
    let (sanitized_items, warnings) = sanitize_raw_items(raw.items);
    let items = reconcile_checklist_items(&sanitized_items, prior_items);
    let residual_text = build_residual_text(markdown, &raw.removed_ranges);

    Some(ChecklistExtraction {
        resource: Resource {
            id: resource_id.to_string(),
            path: None,
            digest: None,
            protected: None,
            root: ResourceRoot::default(),
            content: None,
            hint: None,
            variant: Some(CHECKLIST_VARIANT.to_string()),
            input: ResourceInputList::default(),
            items,
            trigger: None,
            render: None,
        },
        residual_text,
        warnings,
    })
}

/// Sanitize every raw item's text/detail for prompt-safety, in the same
/// canonical domain reconciliation and storage both use. Must run before
/// [`reconcile_checklist_items`] so identity matching never compares a raw
/// value against a previously-sanitized one.
fn sanitize_raw_items(items: Vec<RawChecklistItem>) -> (Vec<RawChecklistItem>, Vec<String>) {
    let mut warnings = Vec::new();
    let sanitized = items
        .into_iter()
        .map(|item| {
            let (text, text_warnings) = sanitize_prompt_text(&item.text);
            warnings.extend(text_warnings);
            let detail = item.detail.map(|detail| {
                let (detail, detail_warnings) = sanitize_prompt_text(&detail);
                warnings.extend(detail_warnings);
                detail
            });
            RawChecklistItem { text, detail }
        })
        .collect();
    (sanitized, warnings)
}

/// Collect prior checklist items (by resource id) out of a canonical trait's
/// JSON representation, so import/refresh/check reconciliation all start
/// from the same typed-item identity regardless of which command loaded it.
pub fn extract_prior_checklists(
    canonical_json: &serde_json::Value,
) -> BTreeMap<String, Vec<ChecklistItem>> {
    let mut out = BTreeMap::new();
    let Some(resources) = canonical_json.get("resource").and_then(|v| v.as_array()) else {
        return out;
    };
    for entry in resources {
        let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if entry.get("variant").and_then(|v| v.as_str()) != Some(CHECKLIST_VARIANT) {
            continue;
        }
        let Some(raw_items) = entry.get("item").and_then(|v| v.as_array()) else {
            continue;
        };
        let items: Vec<ChecklistItem> = raw_items
            .iter()
            .filter_map(|v| serde_json::from_value::<ChecklistItem>(v.clone()).ok())
            .collect();
        if !items.is_empty() {
            out.insert(id.to_string(), items);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Reconciliation
// ---------------------------------------------------------------------------

fn reconcile_checklist_items(
    raw: &[RawChecklistItem],
    prior: &[ChecklistItem],
) -> Vec<ChecklistItem> {
    let mut unmatched: Vec<Option<&ChecklistItem>> = prior.iter().map(Some).collect();
    let mut reserved: BTreeSet<String> = prior.iter().map(|item| item.id.clone()).collect();
    let mut chosen: Vec<Option<usize>> = vec![None; raw.len()];

    // Phase 1: reserve every unique normalized-exact match across the whole
    // new item set before any fuzzy attempt runs, so a later exact candidate
    // is never pre-empted by an earlier item's reword-tolerant guess.
    for (ridx, raw_item) in raw.iter().enumerate() {
        let normalized_new_text = normalize_text(&raw_item.text);
        let found = unmatched.iter().position(|slot| {
            slot.is_some_and(|prior_item| normalize_text(&prior_item.text) == normalized_new_text)
        });
        if let Some(idx) = found {
            chosen[ridx] = Some(idx);
            unmatched[idx] = None;
        }
    }

    // Phase 2: conservative reword-tolerant fuzzy matching, only over items
    // and prior slots the exact pass left unmatched.
    for (ridx, raw_item) in raw.iter().enumerate() {
        if chosen[ridx].is_some() {
            continue;
        }
        let normalized_new_words = normalize_words(&raw_item.text);

        let mut best: Option<(usize, f64)> = None;
        let mut runner_up_score = 0.0f64;
        for (idx, slot) in unmatched.iter().enumerate() {
            let Some(prior_item) = slot else { continue };
            let score = word_similarity(&normalize_words(&prior_item.text), &normalized_new_words);
            if score < REWORD_SIMILARITY_THRESHOLD {
                continue;
            }
            match best {
                None => best = Some((idx, score)),
                Some((_, best_score)) if score > best_score => {
                    runner_up_score = best_score;
                    best = Some((idx, score));
                }
                Some((_, best_score)) => {
                    if score > runner_up_score {
                        runner_up_score = score;
                    }
                    let _ = best_score;
                }
            }
        }
        if let Some((idx, score)) = best
            && score > runner_up_score {
                chosen[ridx] = Some(idx);
                unmatched[idx] = None;
            }
    }

    // Phase 3: seed a fresh id for anything still unmatched.
    let mut result = Vec::with_capacity(raw.len());
    for (ridx, raw_item) in raw.iter().enumerate() {
        let id = if let Some(idx) = chosen[ridx] {
            prior[idx].id.clone()
        } else {
            let id = seed_item_id(&raw_item.text, &reserved);
            reserved.insert(id.clone());
            id
        };

        result.push(ChecklistItem {
            id,
            text: raw_item.text.clone(),
            detail: raw_item.detail.clone(),
            requires_evidence: None,
        });
    }

    result
}

fn seed_item_id(text: &str, reserved: &BTreeSet<String>) -> String {
    let base = crate::synth::slugify_trait_id(text).unwrap_or_else(|_| "item".to_string());
    let base = truncate_slug(&base, MAX_SEEDED_ID_LEN);
    if !reserved.contains(&base) {
        return base;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !reserved.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn truncate_slug(slug: &str, max_len: usize) -> String {
    if slug.len() <= max_len {
        return slug.to_string();
    }
    let cut = &slug[..max_len];
    match cut.rfind('-') {
        Some(idx) if idx > 0 => cut[..idx].to_string(),
        _ => cut.to_string(),
    }
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_words(text: &str) -> BTreeSet<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn word_similarity(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn heading_level(trimmed: &str) -> Option<usize> {
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if rest.is_empty() || rest.starts_with(' ') {
        Some(hashes)
    } else {
        None
    }
}

fn is_checklist_heading(text: &str) -> bool {
    text.split(|ch: char| !ch.is_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case("checklist"))
}

fn fence_marker(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// Extract the item text (and whether it carried a checkbox marker) from a
/// top-level `- `/`* ` bullet line. Returns `None` for a non-bullet line.
fn bullet_text(trimmed: &str) -> Option<(&str, bool)> {
    let rest = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))?;
    let rest = rest.trim_start();
    if let Some(after) = rest
        .strip_prefix("[ ] ")
        .or_else(|| rest.strip_prefix("[x] "))
        .or_else(|| rest.strip_prefix("[X] "))
    {
        Some((after.trim(), true))
    } else if rest == "[ ]" || rest == "[x]" || rest == "[X]" {
        Some(("", true))
    } else {
        Some((rest.trim(), false))
    }
}

fn append_detail(item: &mut RawChecklistItem, text: &str) {
    if text.is_empty() {
        return;
    }
    match &mut item.detail {
        Some(detail) => {
            detail.push(' ');
            detail.push_str(text);
        }
        None => item.detail = Some(text.to_string()),
    }
}

/// Skip a leading YAML frontmatter block (`---` ... `---`) so its contents —
/// which may themselves contain top-level `- ` list lines — are never
/// mistaken for a checklist. Returns the index of the first line to scan.
fn skip_frontmatter(lines: &[&str]) -> usize {
    if lines.first().map(|line| line.trim()) != Some("---") {
        return 0;
    }
    match lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
    {
        Some((end, _)) => end + 1,
        None => 0,
    }
}

fn extract_checklist_spans(markdown: &str) -> Option<RawChecklistExtraction> {
    let lines: Vec<&str> = markdown.lines().collect();
    let n = lines.len();
    let mut items = Vec::new();
    let mut removed = Vec::new();

    let mut in_fence = false;
    let mut current_fence_marker: &str = "";
    let mut i = skip_frontmatter(&lines);

    while i < n {
        let raw_line = lines[i];
        let trimmed = raw_line.trim();

        if let Some(marker) = fence_marker(trimmed) {
            if in_fence && marker == current_fence_marker {
                in_fence = false;
            } else if !in_fence {
                in_fence = true;
                current_fence_marker = marker;
            }
            i += 1;
            continue;
        }
        if in_fence {
            i += 1;
            continue;
        }

        if heading_level(trimmed).is_some() {
            let heading_text = trimmed.trim_start_matches('#').trim();
            if is_checklist_heading(heading_text) {
                let start = i;
                let (section_items, end, item_ranges) =
                    scan_checklist_heading_section(&lines, i + 1);
                if !section_items.is_empty() {
                    removed.push((start, start + 1));
                    removed.extend(item_ranges);
                    items.extend(section_items);
                }
                i = end;
                continue;
            }
            i += 1;
            continue;
        }

        if indent_of(raw_line) == 0
            && let Some((text, true)) = bullet_text(trimmed)
                && !text.is_empty() {
                    let start = i;
                    let (run_items, end) = scan_bare_task_list_run(&lines, i);
                    removed.push((start, end));
                    items.extend(run_items);
                    i = end;
                    continue;
                }

        i += 1;
    }

    if items.is_empty() {
        None
    } else {
        Some(RawChecklistExtraction {
            items,
            removed_ranges: removed,
        })
    }
}

/// Scan the body of a checklist-heading section (everything until the next
/// heading of any level, or end of document), collecting top-level bullets
/// (checkbox optional) as items and indented non-blank continuation lines as
/// detail on the preceding item. Nested fenced code is skipped.
///
/// Returns the items, the index of the first line past the section, and the
/// `[start, end)` line ranges actually consumed by an item (its bullet line
/// plus any indented detail lines immediately following it). Blank lines and
/// top-level non-bullet prose within the section are not part of any range,
/// so a caller building a removal list from these ranges leaves unrelated
/// prose and fenced content in the section untouched.
fn scan_checklist_heading_section(
    lines: &[&str],
    start: usize,
) -> (Vec<RawChecklistItem>, usize, Vec<(usize, usize)>) {
    let n = lines.len();
    let mut items: Vec<RawChecklistItem> = Vec::new();
    let mut item_ranges: Vec<(usize, usize)> = Vec::new();
    let mut current_range: Option<(usize, usize)> = None;
    let mut in_fence = false;
    let mut current_fence_marker: &str = "";
    let mut j = start;

    while j < n {
        let raw_line = lines[j];
        let trimmed = raw_line.trim();

        if trimmed.is_empty() {
            if let Some(range) = current_range.take() {
                item_ranges.push(range);
            }
            j += 1;
            continue;
        }

        if let Some(marker) = fence_marker(trimmed) {
            if let Some(range) = current_range.take() {
                item_ranges.push(range);
            }
            if in_fence && marker == current_fence_marker {
                in_fence = false;
            } else if !in_fence {
                in_fence = true;
                current_fence_marker = marker;
            }
            j += 1;
            continue;
        }
        if in_fence {
            j += 1;
            continue;
        }

        if heading_level(trimmed).is_some() {
            break;
        }

        if indent_of(raw_line) == 0 {
            if let Some((text, _)) = bullet_text(trimmed)
                && !text.is_empty() {
                    if let Some(range) = current_range.take() {
                        item_ranges.push(range);
                    }
                    items.push(RawChecklistItem {
                        text: text.to_string(),
                        detail: None,
                    });
                    current_range = Some((j, j + 1));
                    j += 1;
                    continue;
                }
            // Top-level non-bullet prose: not part of any item, and left
            // untouched by the caller's removal list.
            if let Some(range) = current_range.take() {
                item_ranges.push(range);
            }
        } else if let Some(last) = items.last_mut() {
            append_detail(last, trimmed);
            current_range = Some((
                current_range.map_or(j, |(range_start, _)| range_start),
                j + 1,
            ));
        }

        j += 1;
    }

    if let Some(range) = current_range.take() {
        item_ranges.push(range);
    }

    (items, j, item_ranges)
}

/// Scan a contiguous top-level run of task-list bullets (`- [ ]`/`- [x]`)
/// starting at `start` (already known to be one). The run ends at the first
/// blank line, non-task-list top-level bullet, or heading; indented non-blank
/// lines in between are detail on the preceding item.
fn scan_bare_task_list_run(lines: &[&str], start: usize) -> (Vec<RawChecklistItem>, usize) {
    let n = lines.len();
    let mut items = Vec::new();
    let mut j = start;

    loop {
        let raw_line = lines[j];
        let trimmed = raw_line.trim();
        match bullet_text(trimmed) {
            Some((text, true)) if !text.is_empty() => {
                items.push(RawChecklistItem {
                    text: text.to_string(),
                    detail: None,
                });
            }
            _ => unreachable!("scan_bare_task_list_run must start on a checkbox bullet"),
        }
        j += 1;

        loop {
            if j >= n {
                return (items, j);
            }
            let raw_line = lines[j];
            let trimmed = raw_line.trim();
            if trimmed.is_empty() || heading_level(trimmed).is_some() {
                return (items, j);
            }
            if indent_of(raw_line) == 0 {
                match bullet_text(trimmed) {
                    Some((text, true)) if !text.is_empty() => break,
                    _ => return (items, j),
                }
            }
            if let Some(last) = items.last_mut() {
                append_detail(last, trimmed);
            }
            j += 1;
        }
    }
}

fn build_residual_text(markdown: &str, removed: &[(usize, usize)]) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut sorted_ranges = removed.to_vec();
    sorted_ranges.sort_by_key(|(start, _)| *start);

    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut ranges = sorted_ranges.into_iter().peekable();
    for (idx, line) in lines.iter().enumerate() {
        while let Some(&(_, end)) = ranges.peek() {
            if idx >= end {
                ranges.next();
            } else {
                break;
            }
        }
        let in_removed = matches!(ranges.peek(), Some(&(start, end)) if idx >= start && idx < end);
        if !in_removed {
            kept.push(line);
        }
    }

    let mut collapsed: Vec<&str> = Vec::with_capacity(kept.len());
    let mut previous_blank = false;
    for line in kept {
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        collapsed.push(line);
        previous_blank = blank;
    }
    while collapsed.first().is_some_and(|line| line.trim().is_empty()) {
        collapsed.remove(0);
    }
    while collapsed.last().is_some_and(|line| line.trim().is_empty()) {
        collapsed.pop();
    }

    collapsed.join("\n")
}

