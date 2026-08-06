//! Markdown → [`TaskDocument`] import.
//!
//! Extracts the H1 line (`key`, `title`), the `**Status:**` header line
//! (`status`, `relations.depends-on`, `raised`), and an optional
//! `**Wall:**` label (`wall`, 0063.1) out of a board markdown file into the
//! typed envelope. Everything else — every other line of the file — is
//! opaque prose, split by [`split_sections`] into `content`/`scope`/
//! `validation` (0063.1): sections headed `## Decisions`/`## Scope` move to
//! `scope`, `## Watch`/`## Done when` move to `validation`, the remainder
//! (including the H1/status/wall lines' surrounding text) stays in
//! `content`. Checkbox lists convert to `[[steps]]` only under an explicit
//! `## Steps` heading; every other list (numbered steps, prose bullets) is
//! left untouched inside its prose field.

use super::{Relations, Step, TaskDocument, TaskStatus};

const STATUS_LABEL: &str = "**Status:**";
const DEPENDS_ON_LABEL: &str = "**Depends on:**";
const RAISED_LABEL: &str = "**Raised:**";
const WALL_LABEL: &str = "**Wall:**";
const STEPS_HEADING: &str = "## Steps";
const SCOPE_HEADINGS: [&str; 2] = ["Decisions", "Scope"];
const VALIDATION_HEADINGS: [&str; 2] = ["Watch", "Done when"];

/// Markdown-import specific failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no H1 heading (`# NNNN — Title`) found in the markdown source")]
    MissingHeading,
    #[error("H1 heading {0:?} has no leading task number")]
    MissingKey(String),
    #[error("no `**Status:**` header line found in the markdown source")]
    MissingStatusLine,
}

/// Import a markdown board file into a [`TaskDocument`] with
/// [`super::SCHEMA_VERSION`]. `raised` and `relations.depends-on` are only
/// populated when the `**Status:**` header line carries the corresponding
/// `**Raised:**` / `**Depends on:**` segment.
pub fn import(text: &str) -> Result<TaskDocument, Error> {
    let heading_index = text
        .lines()
        .position(|line| line.trim_start().starts_with("# "))
        .ok_or(Error::MissingHeading)?;
    let heading_line = text.lines().nth(heading_index).expect("index in bounds");
    let (key, title) = parse_heading(heading_line)?;

    let status_index = text
        .lines()
        .position(|line| line.contains(STATUS_LABEL))
        .ok_or(Error::MissingStatusLine)?;
    let status_line = text.lines().nth(status_index).expect("index in bounds");
    let (status, depends_on, raised) = parse_status_line(status_line);

    let wall_index = text
        .lines()
        .position(|line| line.trim_start().starts_with(WALL_LABEL));
    let wall = wall_index.and_then(|i| wall_label(text.lines().nth(i).expect("index in bounds")));
    // A wall line is only removed from prose when it actually declared a
    // wall id — a bare or unparseable label line stays in content.
    let removed_wall_index = wall.is_some().then_some(wall_index).flatten();

    // Everything else, in original order, is opaque prose — the H1,
    // status, and (if present) wall lines are the only ones ever removed.
    let raw_content = text
        .lines()
        .enumerate()
        .filter(|(i, _)| {
            *i != heading_index && *i != status_index && Some(*i) != removed_wall_index
        })
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n");
    // Preserve the trailing newline convention of the source file.
    let raw_content = if text.ends_with('\n') && !raw_content.is_empty() {
        format!("{raw_content}\n")
    } else {
        raw_content
    };
    let steps = extract_steps(&raw_content);
    let (content, scope, validation) = split_sections(&raw_content);

    Ok(TaskDocument {
        schema_version: super::SCHEMA_VERSION.to_string(),
        key,
        title,
        status,
        raised,
        closed: None,
        wall,
        origin: None,
        content,
        scope,
        validation,
        relations: Relations {
            depends_on,
            parent: None,
        },
        steps,
        checks: Vec::new(),
        auto_close: None,
        closure: None,
    })
}

/// Split opaque header-convention prose into the three typed prose fields
/// per 0063.1: every line lands in exactly one of `content`/`scope`/
/// `validation` — a top-level (`## `) heading line whose trimmed text is
/// `## Decisions`/`## Scope` switches the running section to `scope`,
/// `## Watch`/`## Done when` switches it to `validation`, any other
/// heading switches back to `content`. The heading line itself stays
/// inside the section it opens, so the field boundary is exact and the
/// three outputs' line multiset always equals `content`'s — the same
/// algorithm the 0.1 -> 0.2 board migration uses, so importer and migrator
/// can never diverge.
pub fn split_sections(content: &str) -> (String, String, String) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        Content,
        Scope,
        Validation,
    }

    let mut buckets: (Vec<&str>, Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new(), Vec::new());
    let mut current = Section::Content;
    for line in content.lines() {
        if let Some(heading) = top_level_heading(line) {
            current = if SCOPE_HEADINGS.contains(&heading) {
                Section::Scope
            } else if VALIDATION_HEADINGS.contains(&heading) {
                Section::Validation
            } else {
                Section::Content
            };
        }
        match current {
            Section::Content => buckets.0.push(line),
            Section::Scope => buckets.1.push(line),
            Section::Validation => buckets.2.push(line),
        }
    }

    let join = |lines: &[&str]| -> String {
        if lines.is_empty() {
            return String::new();
        }
        let mut joined = lines.join("\n");
        if content.ends_with('\n') {
            joined.push('\n');
        }
        joined
    };
    (join(&buckets.0), join(&buckets.1), join(&buckets.2))
}

/// The trimmed text of `line` if it is a top-level (`## `, not `### `)
/// markdown heading, e.g. `"Decisions"` for `"## Decisions"` or
/// `"## Watch "` (trailing whitespace tolerated).
fn top_level_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed.strip_prefix("## ").map(str::trim)
}

/// Parse `# NNNN — Title` (also tolerating a plain hyphen or colon
/// separator) into `(key, title)`.
fn parse_heading(line: &str) -> Result<(String, String), Error> {
    let rest = line.trim_start().trim_start_matches('#').trim();
    // The key is a run of digits, optionally followed by `.digits` split
    // suffixes (`0078.1`, `0010.5`) — the board's own scheme for a slice
    // split off an oversized task. A dot NOT followed by a digit (the
    // heading's own `NNNN — Title` separator can itself be a bare hyphen
    // adjacent to digits in rare titles) ends the key.
    let digits_end = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let mut key_end = digits_end(rest);
    while rest[key_end..].starts_with('.')
        && rest[key_end + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    {
        key_end += 1 + digits_end(&rest[key_end + 1..]);
    }
    let key = &rest[..key_end];
    if key.is_empty() {
        return Err(Error::MissingKey(line.to_string()));
    }
    let title = rest[key_end..]
        .trim()
        .trim_start_matches(['—', '-', ':'])
        .trim()
        .to_string();
    Ok((key.to_string(), title))
}

/// Split the `**Status:**` header line on the `·` segment separator and
/// extract the status text, `**Depends on:**` task numbers, and
/// `**Raised:**` date.
fn parse_status_line(line: &str) -> (Option<TaskStatus>, Vec<String>, Option<String>) {
    let mut status = None;
    let mut depends_on = Vec::new();
    let mut raised = None;
    for segment in line.split('·') {
        let segment = segment.trim();
        if let Some(rest) = segment.strip_prefix(STATUS_LABEL) {
            status = Some(classify_status(rest.trim()));
        } else if let Some(rest) = segment.strip_prefix(DEPENDS_ON_LABEL) {
            depends_on = extract_task_numbers(rest);
        } else if let Some(rest) = segment.strip_prefix(RAISED_LABEL) {
            raised = extract_date(rest);
        }
    }
    (status, depends_on, raised)
}

/// Best-effort keyword classification of free-text status prose into the
/// closed set. Defaults to `Ready` whenever no closing keyword matches —
/// deliberately conservative, since the alternative is guessing a task is
/// finished or dead. Cancellation keywords are checked first so a status
/// like "superseded — implemented by run X" is never mistaken for `Done`.
fn classify_status(raw: &str) -> TaskStatus {
    let lower = raw.to_ascii_lowercase();
    let has_word = |word: &str| contains_word(&lower, word);
    if has_word("cancelled")
        || has_word("canceled")
        || has_word("superseded")
        || has_word("retired")
        || has_word("abandoned")
    {
        TaskStatus::Cancelled
    } else if has_word("implemented") || has_word("done") || has_word("landed") {
        TaskStatus::Done
    } else {
        TaskStatus::Ready
    }
}

/// Whether `haystack` (already lowercased) contains `word` as a standalone
/// token, not immediately preceded or followed by an ASCII alphanumeric.
fn contains_word(haystack: &str, word: &str) -> bool {
    let is_word_char = |c: char| c.is_ascii_alphanumeric();
    let mut start = 0;
    while let Some(offset) = haystack[start..].find(word) {
        let match_start = start + offset;
        let match_end = match_start + word.len();
        let before_ok = haystack[..match_start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after_ok = haystack[match_end..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        start = match_start + 1;
    }
    false
}

/// Every standalone 4-digit task number mentioned in `text`, in order of
/// first appearance, deduplicated.
fn extract_task_numbers(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let candidate = &text[start..i];
            let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric();
            let after_ok = i == bytes.len() || !bytes[i].is_ascii_alphanumeric();
            if candidate.len() == 4
                && before_ok
                && after_ok
                && !found.contains(&candidate.to_string())
            {
                found.push(candidate.to_string());
            }
        } else {
            i += 1;
        }
    }
    found
}

/// The id following a `**Wall:**` label declared at the start of `line`
/// (after trimming leading whitespace), if present — the first
/// whitespace-delimited token after the label. A mid-sentence mention of
/// the label is not a declaration and yields `None`.
fn wall_label(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(WALL_LABEL)?;
    let id = rest.split_whitespace().next()?;
    let id = id.trim_end_matches(['.', ',']);
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// The first `YYYY-MM-DD` token in `text`, if any.
fn extract_date(text: &str) -> Option<String> {
    let text = text.trim();
    let candidate = text.split_whitespace().next()?;
    let bytes = candidate.as_bytes();
    let is_date = candidate.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && candidate[0..4].bytes().all(|b| b.is_ascii_digit())
        && candidate[5..7].bytes().all(|b| b.is_ascii_digit())
        && candidate[8..10].bytes().all(|b| b.is_ascii_digit());
    is_date.then(|| candidate.trim_end_matches(['.', ',']).to_string())
}

/// Convert checkbox list items under an explicit `## Steps` heading (up to
/// the next heading of level 1 or 2) into `[[steps]]`. Any other heading's
/// checkboxes, or numbered/prose lists, are left untouched inside
/// `content`. A checkbox's indented continuation lines become that step's
/// `content`.
fn extract_steps(content: &str) -> Vec<Step> {
    let Some(heading_start) = content.find(STEPS_HEADING) else {
        return Vec::new();
    };
    let section_start = heading_start + STEPS_HEADING.len();
    let all_lines: Vec<&str> = content[section_start..].lines().collect();
    let section_end = all_lines
        .iter()
        .position(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("# ") || trimmed.starts_with("## ")
        })
        .unwrap_or(all_lines.len());
    let lines = &all_lines[..section_end];

    let mut steps = Vec::new();
    let mut index = 0usize;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((done, title)) = parse_checkbox(line) {
            index += 1;
            let mut body_lines = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && is_continuation(lines[j]) {
                body_lines.push(lines[j].trim_start());
                j += 1;
            }
            steps.push(Step {
                id: format!("s{index}"),
                title,
                done,
                content: body_lines.join("\n"),
            });
            i = j;
        } else {
            i += 1;
        }
    }
    steps
}

/// Whether `line` is a top-level checkbox list item (`- [ ]` / `- [x]`),
/// returning `(done, title)`.
fn parse_checkbox(line: &str) -> Option<(bool, String)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        // Indented past the list's own level — a continuation of a prior
        // item, not a new checkbox.
        return None;
    }
    let rest = trimmed.strip_prefix("- [")?;
    let (marker, rest) = rest.split_at(1);
    let rest = rest.strip_prefix("] ").or_else(|| rest.strip_prefix(']'))?;
    let done = matches!(marker, "x" | "X");
    if !matches!(marker, "x" | "X" | " ") {
        return None;
    }
    Some((done, rest.trim().to_string()))
}

/// A non-checkbox, indented line following a checkbox item — treated as
/// that step's continuation body.
fn is_continuation(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    let indent = line.len() - line.trim_start().len();
    indent > 0 && parse_checkbox(line).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_heading_and_status_line() {
        let text = "# 0050 — Root-cause the thing\n\n**Status:** ready to implement · **Depends on:** 0049 · **Raised:** 2026-08-01\n\nBody paragraph one.\n\nBody paragraph two.\n";
        let document = import(text).expect("import");
        assert_eq!(document.key, "0050");
        assert_eq!(document.title, "Root-cause the thing");
        assert_eq!(document.status, Some(TaskStatus::Ready));
        assert_eq!(document.relations.depends_on, vec!["0049".to_string()]);
        assert_eq!(document.raised.as_deref(), Some("2026-08-01"));
        assert!(!document.content.contains("**Status:**"));
        assert!(!document.content.contains("# 0050"));
        assert!(document.content.contains("Body paragraph one."));
        assert!(document.content.contains("Body paragraph two."));
    }

    #[test]
    fn dotted_split_task_numbers_keep_their_suffix_as_the_key() {
        let text = "# 0078.1 — Land the harvested retry core\n\n**Status:** done\n\nbody\n";
        let document = import(text).expect("import");
        assert_eq!(document.key, "0078.1");
        assert_eq!(document.title, "Land the harvested retry core");
    }

    #[test]
    fn classifies_done_and_cancelled_statuses() {
        assert_eq!(classify_status("implemented 2026-08-05"), TaskStatus::Done);
        assert_eq!(classify_status("DONE 2026-07-29"), TaskStatus::Done);
        assert_eq!(
            classify_status("superseded — implemented and reviewer-approved"),
            TaskStatus::Cancelled
        );
        assert_eq!(
            classify_status("ready to implement (owner-run: needs network)"),
            TaskStatus::Ready
        );
    }

    #[test]
    fn extracts_multiple_depends_on_numbers() {
        let (_, depends_on, _) =
            parse_status_line("**Status:** ready · **Depends on:** 0060, 0061, 0063");
        assert_eq!(
            depends_on,
            vec!["0060".to_string(), "0061".to_string(), "0063".to_string()]
        );
    }

    #[test]
    fn missing_heading_is_an_error() {
        let text = "no heading here\n**Status:** ready\n";
        assert!(import(text).is_err());
    }

    #[test]
    fn missing_status_line_is_an_error() {
        let text = "# 0001 — Title\n\nbody\n";
        assert!(import(text).is_err());
    }

    #[test]
    fn checkbox_list_under_steps_heading_becomes_typed_steps() {
        let text = "# 0001 — Title\n\n**Status:** ready\n\n## Steps\n\n- [x] First step\n  continuation line\n- [ ] Second step\n\n## Done when\n\nprose\n";
        let document = import(text).expect("import");
        assert_eq!(document.steps.len(), 2);
        assert_eq!(document.steps[0].id, "s1");
        assert!(document.steps[0].done);
        assert_eq!(document.steps[0].content, "continuation line");
        assert!(!document.steps[1].done);
        assert!(document.content.contains("## Steps"));
        assert!(document.validation.contains("## Done when"));
    }

    #[test]
    fn checkbox_list_outside_steps_heading_is_left_as_prose() {
        let text = "# 0001 — Title\n\n**Status:** ready\n\n## Contract\n\n- [x] not a step\n";
        let document = import(text).expect("import");
        assert!(document.steps.is_empty());
        assert!(document.content.contains("- [x] not a step"));
    }

    #[test]
    fn imports_wall_label_into_typed_field_and_removes_it_from_content() {
        let text = "# 0001 — Title\n\n**Status:** ready\n\n**Wall:** wall-42\n\nbody\n";
        let document = import(text).expect("import");
        assert_eq!(document.wall.as_deref(), Some("wall-42"));
        assert!(!document.content.contains("**Wall:**"));
    }

    #[test]
    fn prose_mention_of_wall_label_is_not_a_declaration() {
        let text = "# 0001 — Title\n\n**Status:** ready\n\nscrapes the `**Wall:** <id>` label out of the prose\n";
        let document = import(text).expect("import");
        assert_eq!(document.wall, None);
        assert!(
            document
                .content
                .contains("scrapes the `**Wall:** <id>` label out of the prose")
        );
    }

    #[test]
    fn imports_headers_split_into_scope_and_validation() {
        let text = "# 0001 — Title\n\n**Status:** ready\n\nintro\n\n## Decisions\n\n- a ruling\n\n## Scope\n\nwhat's in\n\n## Watch\n\nwatch this\n\n## Done when\n\nit's done\n\n## Approach\n\nhow\n";
        let document = import(text).expect("import");
        assert!(document.content.contains("intro"));
        assert!(document.content.contains("## Approach"));
        assert!(!document.content.contains("## Decisions"));
        assert!(!document.content.contains("## Watch"));
        assert!(document.scope.contains("## Decisions"));
        assert!(document.scope.contains("- a ruling"));
        assert!(document.scope.contains("## Scope"));
        assert!(document.scope.contains("what's in"));
        assert!(document.validation.contains("## Watch"));
        assert!(document.validation.contains("watch this"));
        assert!(document.validation.contains("## Done when"));
        assert!(document.validation.contains("it's done"));
    }

    #[test]
    fn split_sections_partitions_every_line_exactly_once() {
        let content = "pre\n## Decisions\nd1\n## Watch\nw1\n## Other\no1\n";
        let (content_out, scope_out, validation_out) = split_sections(content);
        let mut recombined: Vec<&str> = content_out
            .lines()
            .chain(scope_out.lines())
            .chain(validation_out.lines())
            .collect();
        let mut original: Vec<&str> = content.lines().collect();
        recombined.sort_unstable();
        original.sort_unstable();
        assert_eq!(recombined, original);
        assert!(scope_out.contains("## Decisions"));
        assert!(scope_out.contains("d1"));
        assert!(validation_out.contains("## Watch"));
        assert!(validation_out.contains("w1"));
        assert!(content_out.contains("pre"));
        assert!(content_out.contains("## Other"));
        assert!(content_out.contains("o1"));
    }

    #[test]
    fn numbered_lists_are_never_converted_to_steps() {
        let text = "# 0001 — Title\n\n**Status:** ready\n\n## Steps\n\n1. First\n2. Second\n";
        let document = import(text).expect("import");
        assert!(document.steps.is_empty());
    }
}
