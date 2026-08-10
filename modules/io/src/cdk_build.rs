//! CDK authoring module execution.
//!
//! This is intentionally an IO-boundary concern: authoring modules are host code
//! that emit draft JSON. Core synth remains pure and consumes only parsed JSON.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};

/// 2026-08-10: raised from 30s after the implement family's build crossed
/// the old ceiling (~34s measured) — a legitimately slow build, not a hang:
/// `normalize.ts` sorts canonical keys with `localeCompare` (an ICU call per
/// comparison), and cost scales with family size, so the 7-variant family's
/// de-abstraction pushed it over. The ceiling is a hang backstop, not a
/// performance budget; the comparator hotspot is tracked as its own task
/// (0152 — changing sort order drifts every canonical digest, so it cannot
/// be a drive-by fix).
pub const DEFAULT_BUILD_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdkSourceKind {
    TypeScript,
    JavaScriptModule,
}

impl CdkSourceKind {
    pub fn from_path(path: &Utf8Path) -> crate::Result<Self> {
        match path.extension() {
            Some("ts") => Ok(Self::TypeScript),
            Some("mjs") => Ok(Self::JavaScriptModule),
            _ => Err(crate::environment::Error::Process {
                command: None,
                path: Some(path.to_string()),
                exit_status: None,
                timed_out: false,
                message: "unsupported CDK source extension; expected .ts or .mjs".to_string(),
            }
            .into()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TypeScript => "typescript",
            Self::JavaScriptModule => "javascript-module",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdkBuildRequest {
    pub source_path: Utf8PathBuf,
    pub repo_root: Option<Utf8PathBuf>,
    pub timeout_ms: u64,
    pub capture_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdkBuildOutcome {
    pub source_kind: CdkSourceKind,
    pub argv: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u128,
}

#[derive(Debug)]
struct PipeCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

pub fn emit_draft_json(request: CdkBuildRequest) -> crate::Result<CdkBuildOutcome> {
    let source_path = request.source_path.canonicalize_utf8().map_err(|source| {
        crate::environment::Error::Filesystem {
            path: request.source_path.to_string(),
            source,
        }
    })?;
    validate_source_imports(&source_path)?;
    validate_define_trait_slug_literal(&source_path)?;
    run_node_module(
        request,
        NODE_EMIT_DRAFT_SCRIPT.as_str(),
        "@ctx-traits/cdk",
        "CDK build",
    )
}

/// P0107 — a dumb text scan (same posture as [`collect_structural_lints`]'s
/// mechanical checks) that every `defineTrait(...)` call in the package's
/// source files passes a quoted string literal as its first argument, not a
/// computed expression. Runtime (`defineTrait` itself, in
/// `packages/cdk/src/functional/trait.ts`) can only validate the resulting
/// *value* is slug-shaped — it cannot see whether the source expression was
/// a literal. This scan is what actually enforces literalness, the
/// precondition the future sandboxed supply-chain scanner depends on.
fn validate_define_trait_slug_literal(source_path: &Utf8Path) -> crate::Result<()> {
    let source_root = source_root_for_entry(source_path).to_path_buf();
    let source_root = canonicalize_path(&source_root);
    let mut files = Vec::new();
    collect_source_files(&source_root, &mut files).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: source_root.to_string(),
            source,
        }
    })?;
    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: file.to_string(),
                source,
            }
        })?;
        // The call-site scan and the argument-literalness check must both
        // read the same comment-stripped string: `find_call_sites` matches
        // against `strip_comments(text)`, whose byte offsets do not align
        // with the original `text` (block comments collapse to a single
        // space, line comments lose their content, and multibyte comment
        // content shifts byte lengths). Indexing the original text with
        // stripped offsets previously misfired on any preceding comment.
        let stripped = strip_comments(&text);
        for call_start in find_call_sites_in_stripped(&stripped, "defineTrait") {
            let after_paren = stripped[call_start..]
                .find('(')
                .map(|offset| call_start + offset + 1);
            let Some(argument_start) = after_paren else {
                continue;
            };
            let argument = stripped[argument_start..].trim_start();
            if !matches!(argument.chars().next(), Some('\'' | '"')) {
                return Err(crate::environment::Error::Process {
                    command: None,
                    path: Some(file.to_string()),
                    exit_status: None,
                    timed_out: false,
                    message: format!(
                        "defineTrait(...) in {file} must pass a quoted string literal as its slug argument, not a computed expression"
                    ),
                }
                .into());
            }
        }
    }
    Ok(())
}

/// Byte offsets, in `stripped` (an already comment-stripped source string —
/// see [`strip_comments`]), of every whole-word occurrence of `name` that is
/// immediately followed (ignoring whitespace) by `(` — a plain identifier
/// scan, not a parser, matching [`relative_specifiers`]'s posture. Callers
/// must index the SAME `stripped` string with the returned offsets; the
/// original unstripped text has different byte offsets and must never be
/// indexed with them.
fn find_call_sites_in_stripped(stripped: &str, name: &str) -> Vec<usize> {
    let mut sites = Vec::new();
    let bytes = stripped.as_bytes();
    let mut cursor = 0;
    while let Some(relative) = stripped[cursor..].find(name) {
        let start = cursor + relative;
        let end = start + name.len();
        let preceded_ok = start == 0
            || !matches!(bytes[start - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.');
        let followed_ok = !matches!(
            bytes.get(end),
            Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
        );
        if preceded_ok && followed_ok {
            let rest = stripped[end..].trim_start();
            if rest.starts_with('(') {
                sites.push(start);
            }
        }
        cursor = end;
    }
    sites
}

/// Shared node-invoking runner behind every CDK-style build path (draft
/// synthesis, P457's `config build`): spawn `node --input-type=module --eval
/// <script> <source_path>`, capture with timeout/limit, and classify
/// failures (timeout, capture-limit truncation, nonzero exit, an unresolved
/// bare-specifier import of `unresolved_package_hint`). Callers that need
/// the CDK trait-package source-root import boundary
/// ([`validate_source_imports`]) enforce it themselves before calling this —
/// it deliberately does not apply here, since a config author legitimately
/// imports shared local modules from anywhere in the repo.
pub fn run_node_module(
    request: CdkBuildRequest,
    script: &'static str,
    unresolved_package_hint: &str,
    label: &str,
) -> crate::Result<CdkBuildOutcome> {
    let source_path = request.source_path.canonicalize_utf8().map_err(|source| {
        crate::environment::Error::Filesystem {
            path: request.source_path.to_string(),
            source,
        }
    })?;
    let source_kind = CdkSourceKind::from_path(&source_path)?;
    let argv = argv_for_source(source_kind, &source_path, script);
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    // Anchors bare-specifier resolution (e.g. `@ctx-traits/cdk`,
    // `@ctx-traits/config`) to the repo's `node_modules` regardless of the
    // invoking process's actual cwd — Node resolves `--eval` bare imports by
    // walking up from the child's cwd. An empty `repo_root` (a relative
    // source path with no ancestor directory component) means "the
    // process's own cwd"; `Command::current_dir` errors on an empty path, so
    // normalize it to `.` rather than skip it.
    if let Some(repo_root) = &request.repo_root {
        let base = if repo_root.as_str().is_empty() {
            Utf8Path::new(".")
        } else {
            repo_root.as_path()
        };
        // Prefer ctx's own `.ctx/node_modules` when it exists, falling back to
        // the repository root.
        //
        // Node walks UP from the child's cwd, so anchoring inside `.ctx`
        // reaches `.ctx/node_modules` first and then the repo root anyway —
        // a project that installed the authoring packages itself keeps
        // resolving exactly as before, and one that let `ctx traits init`
        // install them no longer needs a package.json of its own at the root.
        // Without this, authoring in a repository with no JavaScript project
        // fails with `Cannot find package '@ctx-traits/cdk'`, and the remedy
        // the product printed was "run pnpm install" — this repository's own
        // development setup offered as user-facing advice.
        let ctx_root = base.join(".ctx");
        let owned_current_dir;
        let current_dir = if ctx_root.join("node_modules").is_dir() {
            owned_current_dir = ctx_root;
            owned_current_dir.as_path()
        } else {
            base
        };
        command.current_dir(current_dir);
    }
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(Stdio::null());

    let outcome = run_captured(
        source_kind,
        command,
        argv,
        request.timeout_ms,
        request.capture_limit,
    )?;
    let command_text = outcome.argv.join(" ");
    if outcome.timed_out {
        return Err(crate::environment::Error::Process {
            command: Some(command_text),
            path: Some(source_path.to_string()),
            exit_status: outcome.exit_code,
            timed_out: true,
            message: format!("{label} timed out after {} ms", request.timeout_ms),
        }
        .into());
    }
    if outcome.stdout_truncated || outcome.stderr_truncated {
        return Err(crate::environment::Error::Process {
            command: Some(command_text),
            path: Some(source_path.to_string()),
            exit_status: outcome.exit_code,
            timed_out: false,
            message: format!("{label} output exceeded capture limit"),
        }
        .into());
    }
    if outcome.exit_code != Some(0) {
        let stderr = outcome.stderr.trim();
        return Err(crate::environment::Error::Process {
            command: Some(command_text),
            path: Some(source_path.to_string()),
            exit_status: outcome.exit_code,
            timed_out: false,
            message: if is_unresolved_package_error(stderr, unresolved_package_hint) {
                format!(
                    // Names a command that works in a repository with no
                    // JavaScript project of its own, which is the situation
                    // this error actually describes. The previous text said
                    // "run pnpm install", which assumes a pnpm workspace the
                    // author may not have and never will.
                    "cannot resolve {unresolved_package_hint} — run `ctx traits init` to install the authoring packages into .ctx/node_modules, or add {unresolved_package_hint} to this project yourself"
                )
            } else if stderr.is_empty() {
                format!("{label} command exited nonzero")
            } else {
                format!("{label} command exited nonzero: {stderr}")
            },
        }
        .into());
    }

    Ok(outcome)
}

/// Reject relative authoring imports that escape the package's source root —
/// EXCEPT into a sibling package's own `source/` directory under the same
/// trait-family parent (e.g. `.ctx/traits/implement-quick/source/index.ts`
/// importing `../../implement-default/source/shared.ts`): an intentional,
/// pre-existing pattern (P363) several `implement-*` packages depend on to
/// share one procedure-building helper module rather than duplicate it.
/// Anything escaping to a location that is NOT another package's `source/`
/// directory (a stray file at the trait-family root, an unrelated repo path,
/// …) is still rejected exactly as before. Bare package imports are
/// intentionally left to Node's normal resolver.
pub fn validate_source_imports(source_path: &Utf8Path) -> crate::Result<()> {
    let source_root = source_root_for_entry(source_path).to_path_buf();
    let source_root = canonicalize_path(&source_root);
    let mut files = Vec::new();
    collect_source_files(&source_root, &mut files).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: source_root.to_string(),
            source,
        }
    })?;
    for importer in files {
        let text = std::fs::read_to_string(&importer).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: importer.to_string(),
                source,
            }
        })?;
        for specifier in relative_specifiers(&text) {
            let target = normalize_path(&importer, &specifier);
            if target.strip_prefix(&source_root).is_err()
                && !escapes_into_sibling_package_source(&source_root, &target)
            {
                return Err(crate::environment::Error::Process {
                    command: None,
                    path: Some(importer.to_string()),
                    exit_status: None,
                    timed_out: false,
                    message: format!(
                        "CDK source-relative import/export {specifier:?} in {importer} resolves outside source root to {target}"
                    ),
                }
                .into());
            }
        }
    }
    Ok(())
}

/// True when `target` resolves to `<trait-family-root>/<other-package>/source/...`,
/// where `<trait-family-root>` is `source_root`'s own package directory's
/// parent (i.e. `source_root` itself is `<trait-family-root>/<this-package>/source`).
fn escapes_into_sibling_package_source(source_root: &Utf8Path, target: &Utf8Path) -> bool {
    let Some(package_dir) = source_root.parent() else {
        return false;
    };
    let Some(family_root) = package_dir.parent() else {
        return false;
    };
    let Ok(relative) = target.strip_prefix(family_root) else {
        return false;
    };
    let mut components = relative.components();
    let Some(sibling_package) = components.next() else {
        return false;
    };
    let Some(second) = components.next() else {
        return false;
    };
    second.as_str() == "source"
        && sibling_package.as_str() != package_dir.file_name().unwrap_or_default()
}

/// P533 — the doctrine's three mechanical rules, one lint code each. Advisory
/// only: emitted alongside `cdk-orphan-declaration` as a `CdkAuthoringDiagnostic`
/// warning by the CLI layer, never blocking a build. Detection is deliberately
/// dumb text scanning (reusing [`relative_specifiers`] for the import graph and
/// a plain quoted/template-literal length scan) — no TS parser, same posture
/// as `builtin_templates.rs::instantiate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralLint {
    pub code: &'static str,
    pub file: Utf8PathBuf,
    pub message: String,
}

/// A string/template literal at or past this many characters reads as a
/// prompt body rather than incidental text (an id, a short description).
/// Calibrated against every current first-party builtin template and
/// `refactor-direct`: all pass at this threshold, and the known
/// everything-inline offenders warn.
const PROMPT_BODY_THRESHOLD: usize = 700;

/// Generic, non-domain module basenames: a module named one of these earns
/// its filename with a domain word instead, per doctrine. `shared` is
/// deliberately included — it is the exact legacy pattern the P530/P531
/// folds retire.
const GENERIC_MODULE_NAMES: &[&str] = &[
    "utils", "util", "helpers", "helper", "common", "misc", "lib", "shared",
];

/// Collect the doctrine's three mechanical lints for the package rooted at
/// `source_path`'s `source/` directory. A single-file package is exempt from
/// all three by construction — the never-oversplit clause working as
/// intended — so this returns empty whenever the package has one source
/// module.
pub fn collect_structural_lints(source_path: &Utf8Path) -> crate::Result<Vec<StructuralLint>> {
    let source_root = source_root_for_entry(source_path).to_path_buf();
    let source_root = canonicalize_path(&source_root);
    let mut files = Vec::new();
    collect_source_files(&source_root, &mut files).map_err(|source| {
        crate::environment::Error::Filesystem {
            path: source_root.to_string(),
            source,
        }
    })?;
    if files.len() <= 1 {
        return Ok(Vec::new());
    }

    let mut import_counts: std::collections::HashMap<Utf8PathBuf, usize> =
        std::collections::HashMap::new();
    let mut file_texts = Vec::with_capacity(files.len());
    for file in &files {
        let text = std::fs::read_to_string(file).map_err(|source| {
            crate::environment::Error::Filesystem {
                path: file.to_string(),
                source,
            }
        })?;
        for specifier in relative_specifiers(&text) {
            let target = normalize_path(file, &specifier);
            *import_counts.entry(target).or_insert(0) += 1;
        }
        file_texts.push((file.clone(), text));
    }

    let mut lints = Vec::new();
    for (file, text) in &file_texts {
        let is_index = file.parent() == Some(source_root.as_path())
            && matches!(file.file_stem(), Some("index"));
        let is_data = matches!(file.file_stem(), Some("data"));
        let max_literal_len = max_literal_len(text);

        if is_index && max_literal_len > PROMPT_BODY_THRESHOLD {
            lints.push(StructuralLint {
                code: "cdk-index-defines",
                file: file.clone(),
                message: format!(
                    "index.ts contains a {max_literal_len}-character literal; in a split \
                     package index.ts declares and composes, bodies belong in data.ts or a \
                     sequence/*.ts step module (escape: single-file packages are exempt)"
                ),
            });
        } else if !is_index && !is_data && max_literal_len > PROMPT_BODY_THRESHOLD {
            lints.push(StructuralLint {
                code: "cdk-inline-prompt-body",
                file: file.clone(),
                message: format!(
                    "contains a {max_literal_len}-character literal past the {PROMPT_BODY_THRESHOLD}-character \
                     threshold; move it to data.ts, or keep it step-local under the threshold \
                     (escape: the threshold itself — bodies below it never warn)"
                ),
            });
        }

        if let Some(basename) = file
            .file_stem()
            .filter(|name| GENERIC_MODULE_NAMES.contains(name))
        {
            let import_count = import_counts.get(file).copied().unwrap_or(0);
            if import_count == 1 {
                lints.push(StructuralLint {
                    code: "cdk-generic-module-name",
                    file: file.clone(),
                    message: format!(
                        "{basename}.ts is imported by exactly one other module and named for a \
                         primitive, not a domain concept; inline it, or rename it to a domain \
                         word (escape: rename — import count no longer matters once the name \
                         carries meaning)"
                    ),
                });
            }
        }
    }
    lints.sort_by(|left, right| {
        (left.file.as_str(), left.code).cmp(&(right.file.as_str(), right.code))
    });
    Ok(lints)
}

/// Longest quoted or template-literal span in `source`, ignoring comments.
/// Deliberately naive: any string/template literal counts, not just
/// "top-level" ones — the doctrine's own escapes (single-file, rename,
/// threshold) are what keep this from fighting legitimate style, not
/// precision in what counts as a literal.
fn max_literal_len(source: &str) -> usize {
    let stripped = strip_comments(source);
    let chars: Vec<char> = stripped.chars().collect();
    let mut index = 0;
    let mut max_len = 0;
    while index < chars.len() {
        if matches!(chars[index], '\'' | '"' | '`') {
            let delimiter = chars[index];
            index += 1;
            let start = index;
            while index < chars.len() {
                if chars[index] == '\\' {
                    index = (index + 2).min(chars.len());
                } else if chars[index] == delimiter {
                    break;
                } else {
                    index += 1;
                }
            }
            max_len = max_len.max(index - start);
            index += 1;
        } else {
            index += 1;
        }
    }
    max_len
}

/// Return the package source root used by both the import boundary and
/// authoring-diagnostic filtering.
pub fn source_root_for_entry(source_path: &Utf8Path) -> &Utf8Path {
    source_path
        .parent()
        .filter(|parent| parent.file_name() == Some("source"))
        .unwrap_or_else(|| source_path.parent().unwrap_or(source_path))
}

fn collect_source_files(root: &Utf8Path, files: &mut Vec<Utf8PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("non-UTF-8 path {}", path.display()),
            )
        })?;
        if path.is_dir() {
            collect_source_files(&path, files)?;
        } else if matches!(path.extension(), Some("ts" | "mjs" | "js")) {
            files.push(path);
        }
    }
    files.sort();
    Ok(())
}

fn relative_specifiers(source: &str) -> Vec<String> {
    let source = strip_comments(source);
    let mut result = Vec::new();
    let mut cursor = 0;
    while cursor < source.len() {
        let Some((keyword, start)) = next_keyword(&source, cursor) else {
            break;
        };
        let end = start + keyword.len();
        let tail: String = source[end..].chars().take(512).collect();
        let trimmed_tail = tail.trim_start();
        let candidate = if keyword == "import"
            && (trimmed_tail.starts_with('(')
                || trimmed_tail.starts_with('\'')
                || trimmed_tail.starts_with('"')
                || trimmed_tail.starts_with('`'))
        {
            quoted_after(&tail)
        } else if keyword == "import" || keyword == "export" {
            tail.find("from")
                .and_then(|index| quoted_after(&tail[index + 4..]))
        } else {
            None
        };
        if let Some(value) =
            candidate.filter(|value| value.starts_with("./") || value.starts_with("../"))
        {
            result.push(value);
        }
        cursor = end;
    }
    result
}

fn next_keyword(source: &str, cursor: usize) -> Option<(&'static str, usize)> {
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    let start = chars.iter().position(|(index, _)| *index >= cursor)?;
    let mut index = start;
    while index < chars.len() {
        let (byte_index, character) = chars[index];
        if matches!(character, '\'' | '"' | '`') {
            let delimiter = character;
            index += 1;
            while index < chars.len() {
                if chars[index].1 == '\\' {
                    index = (index + 2).min(chars.len());
                } else if chars[index].1 == delimiter {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
            continue;
        }
        let matched = ["import", "export"].into_iter().find(|keyword| {
            let keyword_chars = keyword.chars().count();
            source[byte_index..].starts_with(keyword)
                && !chars
                    .get(index.wrapping_sub(1))
                    .is_some_and(|(_, previous)| {
                        previous.is_ascii_alphanumeric() || *previous == '_'
                    })
                && !chars
                    .get(index + keyword_chars)
                    .is_some_and(|(_, next)| next.is_ascii_alphanumeric() || *next == '_')
        });
        if let Some(keyword) = matched {
            return Some((keyword, byte_index));
        }
        index += 1;
    }
    None
}

fn quoted_after(text: &str) -> Option<String> {
    let (quote, delimiter) = text
        .char_indices()
        .find(|(_, character)| matches!(character, '\'' | '"' | '`'))?;
    let content_start = quote + delimiter.len_utf8();
    let end = text[content_start..].find(delimiter)? + content_start;
    Some(text[content_start..end].to_string())
}

fn strip_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let mut index = 0;
    let mut quote = None;
    while index < chars.len() {
        let byte = chars[index];
        if let Some(delimiter) = quote {
            output.push(byte);
            if byte == '\\' && index + 1 < chars.len() {
                index += 1;
                output.push(chars[index]);
            } else if byte == delimiter {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, '\'' | '"' | '`') {
            quote = Some(byte);
            output.push(byte);
            index += 1;
        } else if byte == '/' && chars.get(index + 1) == Some(&'/') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            output.push('\n');
        } else if byte == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            index = (index + 2).min(chars.len());
            output.push(' ');
        } else {
            output.push(byte);
            index += 1;
        }
    }
    output
}

fn normalize_path(importer: &Utf8Path, specifier: &str) -> Utf8PathBuf {
    let mut path = importer.parent().unwrap_or(importer).join(specifier);
    let mut normalized = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            camino::Utf8Component::CurDir => {}
            camino::Utf8Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_str()),
        }
    }
    path = normalized;
    canonicalize_path(&path)
}

fn canonicalize_path(path: &Utf8Path) -> Utf8PathBuf {
    std::fs::canonicalize(path)
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .unwrap_or_else(|| path.to_path_buf())
}

fn argv_for_source(
    source_kind: CdkSourceKind,
    source_path: &Utf8Path,
    script: &str,
) -> Vec<String> {
    match source_kind {
        CdkSourceKind::TypeScript | CdkSourceKind::JavaScriptModule => vec![
            "node".to_string(),
            "--input-type=module".to_string(),
            "--eval".to_string(),
            script.to_string(),
            source_path.to_string(),
        ],
    }
}

/// Narrowly classify Node's bare-specifier resolution failure for
/// `package` so callers can surface an actionable diagnostic instead of a
/// raw Node stack trace. Any other failure (a bug in the authored module, a
/// different missing dependency, ...) falls through to the raw stderr.
fn is_unresolved_package_error(stderr: &str, package: &str) -> bool {
    stderr.contains(&format!("Cannot find package '{package}'"))
}

fn run_captured(
    source_kind: CdkSourceKind,
    mut command: Command,
    argv: Vec<String>,
    timeout_ms: u64,
    capture_limit: usize,
) -> crate::Result<CdkBuildOutcome> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(crate::Error::Usage {
            message: "CDK build argv is empty".to_string(),
        });
    }

    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|source| crate::environment::Error::Filesystem {
            path: argv.join(" "),
            source,
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: argv.join(" "),
            source: std::io::Error::other("failed to open CDK build stdout"),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| crate::environment::Error::Filesystem {
            path: argv.join(" "),
            source: std::io::Error::other("failed to open CDK build stderr"),
        })?;
    let limit = if capture_limit == 0 {
        crate::harness::DEFAULT_CAPTURE_LIMIT
    } else {
        capture_limit
    };
    let stdout_handle = spawn_capture(stdout, limit);
    let stderr_handle = spawn_capture(stderr, limit);

    let timeout = Duration::from_millis(if timeout_ms == 0 {
        DEFAULT_BUILD_TIMEOUT_MS
    } else {
        timeout_ms
    });
    let mut timed_out = false;
    loop {
        match child
            .try_wait()
            .map_err(|source| crate::environment::Error::Filesystem {
                path: argv.join(" "),
                source,
            })? {
            Some(_) => break,
            None if started.elapsed() >= timeout => {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }

    let status = child
        .wait()
        .map_err(|source| crate::environment::Error::Filesystem {
            path: argv.join(" "),
            source,
        })?;
    let command_text = argv.join(" ");
    let stdout = stdout_handle
        .join()
        .map_err(|_| crate::environment::Error::Process {
            command: Some(command_text.clone()),
            path: None,
            exit_status: status.code(),
            timed_out: false,
            message: "CDK build stdout capture thread panicked".to_string(),
        })?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| crate::environment::Error::Process {
            command: Some(command_text),
            path: None,
            exit_status: status.code(),
            timed_out: false,
            message: "CDK build stderr capture thread panicked".to_string(),
        })?;

    Ok(CdkBuildOutcome {
        source_kind,
        argv,
        stdout: String::from_utf8_lossy(&stdout.bytes).to_string(),
        stderr: String::from_utf8_lossy(&stderr.bytes).to_string(),
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        exit_code: status.code(),
        timed_out,
        duration_ms: started.elapsed().as_millis(),
    })
}

fn spawn_capture<R>(mut reader: R, limit: usize) -> std::thread::JoinHandle<PipeCapture>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let remaining = limit.saturating_sub(bytes.len());
                    if read > remaining {
                        bytes.extend_from_slice(&buffer[..remaining]);
                        truncated = true;
                    } else if remaining > 0 {
                        bytes.extend_from_slice(&buffer[..read]);
                    } else {
                        truncated = true;
                    }
                }
                Err(_) => break,
            }
        }
        PipeCapture { bytes, truncated }
    })
}

/// Shared resolve-hook prelude embedded in both [`NODE_EMIT_DRAFT_SCRIPT`]
/// and [`NODE_EMIT_CONFIG_SCRIPT`]: the ownership-propagation invariant that
/// a file reached only through a bare-specifier resolution
/// (`@ctx-traits/config`, not `./x` or `../x` or an already-absolute `file:`
/// URL) is dependency-owned, not author-owned. The flag propagates to
/// whatever a dependency-origin file itself imports, so a package's own
/// internal relative imports (its `dist/index.js` requiring its own
/// `dist/generated.js`) stay excluded too. Exists once so the two emit
/// scripts cannot diverge on this semantics silently.
const RESOLVE_TRACKER_PRELUDE: &str = r#"
const dependencyUrls = new Set();
function isBareSpecifier(specifier) {
  return !specifier.startsWith('.') && !specifier.startsWith('/') && !specifier.startsWith('file:');
}
"#;

fn node_emit_draft_script() -> String {
    const HEAD: &str = r#"
import { pathToFileURL } from 'node:url';
import { registerHooks } from 'node:module';

const sourcePath = process.argv[process.argv.length - 1];
// P0107 package-level provenance: every bare-specifier import resolved from
// an author-owned file (not from inside an already-dependency-owned file) is
// this trait's own package dependency edge — the import graph half of
// "package-level, import graph + lockfile" provenance.
const packageDependencies = new Set();
"#;
    const TAIL: &str = r#"
registerHooks({
  resolve(specifier, context, nextResolve) {
    const result = nextResolve(specifier, context);
    const parentIsDependency = context.parentURL ? dependencyUrls.has(context.parentURL) : false;
    if (isBareSpecifier(specifier)) {
      if (!parentIsDependency) packageDependencies.add(specifier);
      dependencyUrls.add(result.url);
    } else if (parentIsDependency) {
      dependencyUrls.add(result.url);
    }
    return result;
  },
});
const module = await import(pathToFileURL(sourcePath).href);
let draft;
for (const name of ['default', 'draft', 'traitDraft', 'TRAIT']) {
  if (Object.prototype.hasOwnProperty.call(module, name)) {
    draft = module[name];
    break;
  }
}
if (draft === undefined) {
  console.error('CDK module must export default, draft, traitDraft, or TRAIT');
  process.exit(1);
}
let envelope;
const cdk = await import('@ctx-traits/cdk');
if (typeof cdk.isTraitFamilyHandle === 'function' && cdk.isTraitFamilyHandle(draft)) {
  envelope = await cdk.resolveTraitFamily(draft);
} else if (typeof draft === 'function' && typeof cdk.evaluateTraitFunction === 'function') {
  envelope = cdk.evaluateTraitFunction(draft);
  // A hook-style FAMILY function (defineTrait + useVariant bindings)
  // evaluates to a family handle, not a draft envelope — resolve it through
  // the same flattening path an object-style family module takes.
  if (typeof cdk.isTraitFamilyHandle === 'function' && cdk.isTraitFamilyHandle(envelope)) {
    envelope = await cdk.resolveTraitFamily(envelope);
  }
} else if (typeof cdk.toDraftJsonWithSourceMap === 'function') {
  envelope = cdk.toDraftJsonWithSourceMap(draft);
}
if (envelope === undefined) {
  envelope = { draft, __map: {}, authoredDeclarations: [] };
}
envelope.packageDependencies = Array.from(packageDependencies).sort();
process.stdout.write(`${JSON.stringify(envelope, null, 2)}\n`);
"#;
    format!("{HEAD}{RESOLVE_TRACKER_PRELUDE}{TAIL}")
}

/// P457 `config build`: import the module, take its default export, and
/// emit `{ config, sources }` where `sources` is every `file:` URL Node
/// actually resolved while loading the module graph — captured via
/// `node:module`'s `registerHooks({ load })`, registered before the dynamic
/// import so it sees the full transitive graph (including dynamic imports
/// and re-exports a static specifier scan would miss).
fn node_emit_config_script() -> String {
    const HEAD: &str = r#"
import { pathToFileURL } from 'node:url';
import { registerHooks } from 'node:module';

const sourcePath = process.argv[process.argv.length - 1];
const sources = new Set();
"#;
    const TAIL: &str = r#"
registerHooks({
  resolve(specifier, context, nextResolve) {
    const result = nextResolve(specifier, context);
    const parentIsDependency = context.parentURL ? dependencyUrls.has(context.parentURL) : false;
    if (isBareSpecifier(specifier) || parentIsDependency) {
      dependencyUrls.add(result.url);
    }
    return result;
  },
  load(url, context, nextLoad) {
    if (url.startsWith('file:') && !dependencyUrls.has(url)) {
      sources.add(url);
    }
    return nextLoad(url, context);
  },
});
const module = await import(pathToFileURL(sourcePath).href);
const config = module.default;
if (config === undefined) {
  console.error('config module must export a default value (defineConfig(...))');
  process.exit(1);
}
process.stdout.write(`${JSON.stringify({ config, sources: Array.from(sources) }, null, 2)}\n`);
"#;
    format!("{HEAD}{RESOLVE_TRACKER_PRELUDE}{TAIL}")
}

static NODE_EMIT_DRAFT_SCRIPT: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(node_emit_draft_script);

pub static NODE_EMIT_CONFIG_SCRIPT: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(node_emit_config_script);

#[cfg(test)]
mod structural_lint_tests {
    use super::*;

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-structural-lint-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    fn write(root: &Utf8Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap().as_std_path()).expect("create parent dir");
        std::fs::write(path.as_std_path(), contents).expect("write fixture file");
    }

    #[test]
    fn single_file_package_is_exempt() {
        let root = scratch_dir("single-file");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            &format!("const body = `{}`;\n", "x".repeat(2000)),
        );
        let lints = collect_structural_lints(&source.join("index.ts")).expect("collect lints");
        assert!(lints.is_empty());
    }

    #[test]
    fn index_defining_a_large_body_warns() {
        let root = scratch_dir("index-defines");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            &format!(
                "import {{ port }} from \"./data.ts\";\nconst body = `{}`;\n",
                "x".repeat(2000)
            ),
        );
        write(&source, "data.ts", "export const port = {};\n");
        let lints = collect_structural_lints(&source.join("index.ts")).expect("collect lints");
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].code, "cdk-index-defines");
        assert!(lints[0].file.as_str().ends_with("index.ts"));
    }

    #[test]
    fn generic_module_name_imported_once_warns() {
        let root = scratch_dir("generic-name");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "import { helper } from \"./shared.ts\";\nexport default helper;\n",
        );
        write(&source, "shared.ts", "export const helper = 1;\n");
        let lints = collect_structural_lints(&source.join("index.ts")).expect("collect lints");
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].code, "cdk-generic-module-name");
        assert!(lints[0].file.as_str().ends_with("shared.ts"));
    }

    #[test]
    fn generic_module_name_imported_by_two_modules_is_silent() {
        let root = scratch_dir("generic-name-shared-twice");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "import { helper } from \"./shared.ts\";\nimport { other } from \"./other.ts\";\nexport default { helper, other };\n",
        );
        write(&source, "shared.ts", "export const helper = 1;\n");
        write(
            &source,
            "other.ts",
            "import { helper } from \"./shared.ts\";\nexport const other = helper;\n",
        );
        let lints = collect_structural_lints(&source.join("index.ts")).expect("collect lints");
        assert!(
            lints
                .iter()
                .all(|lint| lint.code != "cdk-generic-module-name")
        );
    }

    #[test]
    fn inline_prompt_body_outside_data_warns() {
        let root = scratch_dir("inline-prompt-body");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "import step from \"./sequence/step.ts\";\nexport default step;\n",
        );
        write(&source, "data.ts", "export const port = {};\n");
        write(
            &source,
            "sequence/step.ts",
            &format!(
                "const prompt = `{}`;\nexport default prompt;\n",
                "x".repeat(2000)
            ),
        );
        let lints = collect_structural_lints(&source.join("index.ts")).expect("collect lints");
        assert_eq!(lints.len(), 1);
        assert_eq!(lints[0].code, "cdk-inline-prompt-body");
        assert!(lints[0].file.as_str().ends_with("step.ts"));
    }

    #[test]
    fn large_body_in_data_ts_is_silent() {
        let root = scratch_dir("large-body-in-data");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "import { body } from \"./data.ts\";\nexport default body;\n",
        );
        write(
            &source,
            "data.ts",
            &format!("export const body = `{}`;\n", "x".repeat(2000)),
        );
        let lints = collect_structural_lints(&source.join("index.ts")).expect("collect lints");
        assert!(lints.is_empty());
    }

    #[test]
    fn conformant_multi_file_package_is_silent() {
        let root = scratch_dir("conformant");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "import { port } from \"./data.ts\";\nimport annotate from \"./sequence/annotation.ts\";\nexport default { port, annotate };\n",
        );
        write(
            &source,
            "data.ts",
            "export const port = { id: \"work-report\" };\n",
        );
        write(
            &source,
            "sequence/annotation.ts",
            "export default { title: \"Collect annotations\" };\n",
        );
        let lints = collect_structural_lints(&source.join("index.ts")).expect("collect lints");
        assert!(lints.is_empty());
    }
}

#[cfg(test)]
mod define_trait_slug_literal_tests {
    use super::*;

    fn scratch_dir(name: &str) -> Utf8PathBuf {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-define-trait-slug-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir
    }

    fn write(root: &Utf8Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap().as_std_path()).expect("create parent dir");
        std::fs::write(path.as_std_path(), contents).expect("write fixture file");
    }

    #[test]
    fn literal_slug_preceded_by_line_comment_with_paren_passes() {
        let root = scratch_dir("line-comment-paren");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "// helper (see docs)\ndefineTrait(\"my-trait\", {});\n",
        );
        validate_define_trait_slug_literal(&source.join("index.ts"))
            .expect("literal slug must pass despite a preceding comment containing '('");
    }

    #[test]
    fn literal_slug_preceded_by_em_dash_comment_passes() {
        let root = scratch_dir("em-dash-comment");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "// house style — see docs\ndefineTrait(\"my-trait\", {});\n",
        );
        validate_define_trait_slug_literal(&source.join("index.ts"))
            .expect("literal slug must pass despite a preceding multibyte comment");
    }

    #[test]
    fn computed_slug_is_rejected() {
        let root = scratch_dir("computed-slug");
        let source = root.join("source");
        write(&source, "index.ts", "defineTrait(SLUG, {});\n");
        let error = validate_define_trait_slug_literal(&source.join("index.ts"))
            .expect_err("computed slug must be rejected");
        assert!(error.to_string().contains("quoted string literal"));
    }

    #[test]
    fn computed_slug_preceded_by_block_comment_with_paren_and_quote_is_rejected() {
        let root = scratch_dir("block-comment-paren-quote");
        let source = root.join("source");
        write(
            &source,
            "index.ts",
            "/* (\"a\") */\ndefineTrait(SLUG, {});\n",
        );
        let error = validate_define_trait_slug_literal(&source.join("index.ts"))
            .expect_err("computed slug preceded by a comment must still be rejected");
        assert!(error.to_string().contains("quoted string literal"));
    }
}
