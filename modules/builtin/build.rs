use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;

struct SectionDef {
    name: &'static str,
    constant: &'static str,
}

const SECTIONS: &[SectionDef] = &[
    SectionDef {
        name: "intent",
        constant: "INTENT",
    },
    SectionDef {
        name: "behavior.tone",
        constant: "BEHAVIOR_TONE",
    },
    SectionDef {
        name: "behavior.method",
        constant: "BEHAVIOR_METHOD",
    },
    SectionDef {
        name: "behavior.format",
        constant: "BEHAVIOR_FORMAT",
    },
    SectionDef {
        name: "behavior.verbosity",
        constant: "BEHAVIOR_VERBOSITY",
    },
    SectionDef {
        name: "behavior.directness",
        constant: "BEHAVIOR_DIRECTNESS",
    },
    SectionDef {
        name: "behavior.scope-control",
        constant: "BEHAVIOR_SCOPE_CONTROL",
    },
    SectionDef {
        name: "behavior.initiative",
        constant: "BEHAVIOR_INITIATIVE",
    },
    SectionDef {
        name: "behavior.uncertainty",
        constant: "BEHAVIOR_UNCERTAINTY",
    },
];

type Entry = (String, String, String, Option<String>);

const VOCABULARY_DIR: &str = "vocabulary";

fn main() {
    println!("cargo:rerun-if-changed={VOCABULARY_DIR}");
    let mut paths: Vec<_> = fs::read_dir(VOCABULARY_DIR)
        .expect("read vocabulary")
        .map(|entry| entry.expect("read vocabulary entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();

    let mut input = String::new();
    for path in &paths {
        println!("cargo:rerun-if-changed={}", path.display());
        input.push_str(&fs::read_to_string(path).expect("read vocabulary file"));
        input.push('\n');
    }

    let definitions = parse(&input);
    let output = env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(Path::new(&output).join("builtins.rs"), definitions).expect("write builtins.rs");

    generate_builtin_trait_packages(&output);
    generate_builtin_templates(&output);
}

/// Exact set of first-party meta-trait packages embedded into the binary.
/// Each entry names only the relative-to-package-root paths eligible for
/// embedding: the package manifest, the compiled trait manifest, and any
/// resources the compiled manifest declares are discovered dynamically by
/// parsing `generated/index.toml`.
const BUILTIN_TRAIT_PACKAGE_IDS: &[&str] = &["generate", "refine", "critique", "explain", "import"];

const BUILTIN_TRAIT_PACKAGES_DIR: &str = "traits";

/// Shared packages the meta-traits depend on but which are not themselves
/// runnable: no procedure, nothing to select or dispatch. They are embedded
/// and published to the runtime store exactly like a trait package — the
/// store is what makes a sibling `../spec` dependency resolve — but they are
/// kept out of the runnable catalog, so `list`, query selection, and
/// `run-info` never offer a package that cannot run.
const BUILTIN_SHARED_PACKAGE_IDS: &[&str] = &["spec"];

const BUILTIN_SHARED_PACKAGES_DIR: &str = "shared";

fn generate_builtin_trait_packages(out_dir: &str) {
    // Feature-gate both the filesystem reads and the generated table so a
    // build without `builtin-trait-packages` neither hashes nor embeds any
    // package bytes.
    if env::var("CARGO_FEATURE_TRAIT_PACKAGES").is_err() {
        fs::write(
            Path::new(out_dir).join("builtin_trait_packages.rs"),
            "pub static BUILTIN_TRAIT_PACKAGES: &[BuiltinTraitPackage] = &[];\n",
        )
        .expect("write builtin_trait_packages.rs");
        return;
    }

    println!("cargo:rerun-if-changed={BUILTIN_TRAIT_PACKAGES_DIR}");
    println!("cargo:rerun-if-changed={BUILTIN_SHARED_PACKAGES_DIR}");

    let mut generated =
        String::from("pub static BUILTIN_TRAIT_PACKAGES: &[BuiltinTraitPackage] = &[\n");
    let embedded = BUILTIN_TRAIT_PACKAGE_IDS
        .iter()
        .map(|id| (*id, BUILTIN_TRAIT_PACKAGES_DIR, true))
        .chain(
            BUILTIN_SHARED_PACKAGE_IDS
                .iter()
                .map(|id| (*id, BUILTIN_SHARED_PACKAGES_DIR, false)),
        );
    for (id, packages_dir, runnable) in embedded {
        let package_dir = Path::new(packages_dir).join(id);
        let manifest_rel = "trait.toml";
        let index_rel = "generated/index.toml";

        let index_abs = package_dir.join(index_rel);
        println!("cargo:rerun-if-changed={}", index_abs.display());
        let index_text = fs::read_to_string(&index_abs)
            .unwrap_or_else(|e| panic!("read {}: {e}", index_abs.display()));
        let manifest_abs = package_dir.join(manifest_rel);
        println!("cargo:rerun-if-changed={}", manifest_abs.display());
        if !manifest_abs.is_file() {
            panic!(
                "missing {} for built-in trait package {id:?}",
                manifest_abs.display()
            );
        }

        let mut relative_paths = vec![manifest_rel.to_string(), index_rel.to_string()];
        relative_paths.extend(declared_resource_paths(&index_text, id));

        generated.push_str(&format!(
            "    BuiltinTraitPackage {{ id: {id:?}, runnable: {runnable}, files: &[\n"
        ));
        for relative_path in &relative_paths {
            let file_abs = package_dir.join(relative_path);
            println!("cargo:rerun-if-changed={}", file_abs.display());
            let bytes =
                fs::read(&file_abs).unwrap_or_else(|e| panic!("read {}: {e}", file_abs.display()));
            panic_on_embedded_absolute_home_path(&bytes, &file_abs);
            let digest = sha256_hex_digest(&bytes);
            generated.push_str(&format!(
                "        BuiltinTraitFile {{ relative_path: {relative_path:?}, bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{packages_dir}/{id}/{relative_path}\")), digest: \"sha256:{digest}\" }},\n"
            ));
        }
        generated.push_str("    ] },\n");
    }
    generated.push_str("];\n");

    fs::write(
        Path::new(out_dir).join("builtin_trait_packages.rs"),
        generated,
    )
    .expect("write builtin_trait_packages.rs");
}

/// Exact set of first-party teaching templates consumed by `ctx traits create`.
/// Deliberately kept separate from [`BUILTIN_TRAIT_PACKAGE_IDS`]: templates
/// are authoring inputs for scaffolding a draft package, never resolvable
/// runtime traits, so they never enter the built-in trait catalog or the
/// runtime built-in store. Listed in the fixed, sorted order `ctx traits
/// new` reports them in.
const BUILTIN_TEMPLATE_IDS: &[&str] = &["implement", "research", "review"];

const BUILTIN_TEMPLATES_DIR: &str = "templates";

/// Embed each template's authoring inputs (`trait.toml`, every
/// `source/**/*.ts` file) as UTF-8 static strings. Only authoring files are
/// embedded — no `generated/` or `trait.lock`, since `ctx traits create` always rebuilds and
/// re-locks a freshly instantiated package rather than trusting a stale
/// committed artifact. Feature-gated identically to
/// [`generate_builtin_trait_packages`] so templates ship in native CLI
/// binaries but never enter WASM artifacts.
///
/// `source/index.ts` is always embedded as the anchored, rewritable entry
/// file (P271's two anchors — `trait(<id>` and `name: <name>` — live only
/// there). Every other `source/**/*.ts` file (P533's doctrine layout —
/// `data.ts`, `schema.ts`, `agent.ts`, `sequence/<concept>.ts`) is embedded
/// verbatim as an extra file, passed through byte-identical by `instantiate`.
fn generate_builtin_templates(out_dir: &str) {
    if env::var("CARGO_FEATURE_TRAIT_PACKAGES").is_err() {
        fs::write(
            Path::new(out_dir).join("builtin_templates.rs"),
            "pub static BUILTIN_TEMPLATES: &[BuiltinTemplate] = &[];\n",
        )
        .expect("write builtin_templates.rs");
        return;
    }

    println!("cargo:rerun-if-changed={BUILTIN_TEMPLATES_DIR}");

    let mut generated = String::from("pub static BUILTIN_TEMPLATES: &[BuiltinTemplate] = &[\n");
    for id in BUILTIN_TEMPLATE_IDS {
        let package_dir = Path::new(BUILTIN_TEMPLATES_DIR).join(id);
        let manifest_abs = package_dir.join("trait.toml");
        let source_abs = package_dir.join("source/index.ts");
        println!("cargo:rerun-if-changed={}", manifest_abs.display());
        println!("cargo:rerun-if-changed={}", source_abs.display());
        if !manifest_abs.is_file() {
            panic!("missing {} for template {id:?}", manifest_abs.display());
        }
        if !source_abs.is_file() {
            panic!("missing {} for template {id:?}", source_abs.display());
        }
        // Fail fast on invalid UTF-8 rather than embedding non-UTF-8 bytes
        // include_str! would reject anyway, with a clearer message.
        fs::read_to_string(&manifest_abs)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_abs.display()));
        fs::read_to_string(&source_abs)
            .unwrap_or_else(|e| panic!("read {}: {e}", source_abs.display()));

        let mut extra_relative_paths = Vec::new();
        collect_extra_template_source_files(
            &package_dir.join("source"),
            Path::new(""),
            &mut extra_relative_paths,
        );
        extra_relative_paths.sort();

        let mut extra_files_literal = String::from("&[");
        for relative in &extra_relative_paths {
            let file_abs = package_dir.join("source").join(relative);
            println!("cargo:rerun-if-changed={}", file_abs.display());
            fs::read_to_string(&file_abs)
                .unwrap_or_else(|e| panic!("read {}: {e}", file_abs.display()));
            // The embedded key must use stable forward slashes regardless of
            // the building platform; the concat!/include_str! literal below
            // reuses the same forward-slash form, which rustc accepts as a
            // path on every supported host.
            let relative_key: String = relative
                .components()
                .map(|component| component.as_os_str().to_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("/");
            extra_files_literal.push_str(&format!(
                "({relative_key:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{BUILTIN_TEMPLATES_DIR}/{id}/source/{relative_key}\"))),"
            ));
        }
        extra_files_literal.push(']');

        generated.push_str(&format!(
            "    BuiltinTemplate {{ id: {id:?}, trait_toml: include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{BUILTIN_TEMPLATES_DIR}/{id}/trait.toml\")), source_ts: include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{BUILTIN_TEMPLATES_DIR}/{id}/source/index.ts\")), extra_source_files: {extra_files_literal} }},\n"
        ));
    }
    generated.push_str("];\n");

    fs::write(Path::new(out_dir).join("builtin_templates.rs"), generated)
        .expect("write builtin_templates.rs");
}

/// Recursively collect every `source/**/*.ts` file under `source_root` other
/// than `index.ts` at the root, returned as paths relative to `source_root`
/// with forward-slash separators (stable across the building platform).
fn collect_extra_template_source_files(
    source_root: &Path,
    relative_dir: &Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    let dir = source_root.join(relative_dir);
    let entries = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read template source dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("read dir entry in {}: {e}", dir.display()));
        let file_type = entry
            .file_type()
            .unwrap_or_else(|e| panic!("read file type in {}: {e}", dir.display()));
        let relative_path = relative_dir.join(entry.file_name());
        if file_type.is_dir() {
            collect_extra_template_source_files(source_root, &relative_path, out);
        } else if relative_path.extension().and_then(|ext| ext.to_str()) == Some("ts")
            && relative_path != Path::new("index.ts")
        {
            out.push(relative_path);
        }
    }
}

/// Parse the declared `[[resource]].path` entries out of a compiled trait
/// manifest. Only these declared resources are embedded; authoring files
/// like `source/index.ts` and `generated/index.map` never are.
fn declared_resource_paths(index_toml: &str, id: &str) -> Vec<String> {
    // toml 1.x: `FromStr` on `Value` parses a VALUE, not a document, so a
    // normal manifest fails with "unexpected content, expected nothing".
    // `from_str` is the document entry point.
    let document: toml::Value = toml::from_str(index_toml)
        .unwrap_or_else(|e| panic!("parse generated/index.toml for {id:?}: {e}"));
    let Some(resources) = document.get("resource").and_then(toml::Value::as_array) else {
        return Vec::new();
    };
    resources
        .iter()
        .map(|resource| {
            resource
                .get("path")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| panic!("resource entry missing string `path` for {id:?}"))
                .to_string()
        })
        .collect()
}

/// Refuses to embed a resource body carrying an authoring-machine absolute
/// home path (`/Users/<name>/...` or `/home/<name>/...`), which would ship
/// the owner's identity in the binary. P490.
fn panic_on_embedded_absolute_home_path(bytes: &[u8], file_abs: &Path) {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return;
    };
    for prefix in ["/Users/", "/home/"] {
        if text.contains(prefix) {
            panic!(
                "{} embeds an absolute host home path ({prefix}...); built-in package bytes must never carry the authoring machine's identity",
                file_abs.display()
            );
        }
    }
}

fn sha256_hex_digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn parse(input: &str) -> String {
    let mut current = None;
    let mut seen = BTreeSet::new();
    let mut sections: Vec<(&'static str, Vec<Entry>)> = Vec::new();

    for (line_number, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = &line[1..line.len() - 1];
            let section = SECTIONS.iter().find(|s| s.name == name).unwrap_or_else(|| {
                panic!(
                    "builtins.toml:{}: unknown section [{}]",
                    line_number + 1,
                    name
                )
            });
            if !seen.insert(section.name) {
                panic!(
                    "builtins.toml:{}: duplicate section [{}]",
                    line_number + 1,
                    name
                );
            }
            sections.push((section.name, Vec::new()));
            current = Some(sections.len() - 1);
            continue;
        }
        let Some(index) = current else {
            panic!("builtins.toml:{}: entry outside a section", line_number + 1);
        };
        let Some((slug, value)) = line.split_once(" = ") else {
            panic!(
                "builtins.toml:{}: expected `slug = \"description\"` or `slug = {{ ... }}`",
                line_number + 1
            );
        };
        if !is_slug(slug) {
            panic!("builtins.toml:{}: invalid slug", line_number + 1);
        }
        if sections[index].1.iter().any(|(s, _, _, _)| s == slug) {
            panic!("builtins.toml:{}: duplicate slug", line_number + 1);
        }
        let (summary, description, directive) = parse_value(value, line_number + 1);
        sections[index]
            .1
            .push((slug.to_string(), summary, description, directive));
    }

    for section in SECTIONS {
        let found = sections
            .iter()
            .find(|(name, _)| *name == section.name)
            .unwrap_or_else(|| {
                panic!("builtins.toml: missing required section [{}]", section.name)
            });
        if found.1.is_empty() {
            panic!(
                "builtins.toml: section [{}] must contain entries",
                section.name
            );
        }
    }

    let mut generated = String::new();
    for section in SECTIONS {
        let entries = sections
            .iter()
            .find(|(name, _)| *name == section.name)
            .map(|(_, e)| e.as_slice())
            .unwrap();
        generated.push_str(&emit_static(section.constant, entries));
    }
    generated
}

fn parse_value(value: &str, line_number: usize) -> (String, String, Option<String>) {
    let value = value.trim();
    if value.starts_with('{') && value.ends_with('}') {
        parse_inline_table(&value[1..value.len() - 1], line_number)
    } else if value.starts_with('"') && value.ends_with('"') && value.len() >= 3 {
        let text = &value[1..value.len() - 1];
        validate_emittable(text, line_number);
        (text.to_string(), text.to_string(), None)
    } else {
        panic!(
            "builtins.toml:{}: expected `\"description\"` or `{{ summary = \"...\", description = \"...\" }}`",
            line_number
        );
    }
}

fn parse_inline_table(inner: &str, line_number: usize) -> (String, String, Option<String>) {
    let mut summary = None;
    let mut description = None;
    let mut directive = None;
    let bytes = inner.as_bytes();
    let mut pos = 0;
    loop {
        while pos < bytes.len() && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let key_start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_alphabetic() {
            pos += 1;
        }
        let key = &inner[key_start..pos];
        while pos < bytes.len() && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos >= bytes.len() || bytes[pos] != b'=' {
            panic!(
                "builtins.toml:{}: expected `=` in inline table",
                line_number
            );
        }
        pos += 1;
        while pos < bytes.len() && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos >= bytes.len() || bytes[pos] != b'"' {
            panic!(
                "builtins.toml:{}: expected quoted string in inline table",
                line_number
            );
        }
        pos += 1;
        let val_start = pos;
        while pos < bytes.len() && bytes[pos] != b'"' {
            pos += 1;
        }
        if pos >= bytes.len() {
            panic!(
                "builtins.toml:{}: unterminated string in inline table",
                line_number
            );
        }
        let text = &inner[val_start..pos];
        validate_emittable(text, line_number);
        pos += 1;
        match key {
            "summary" => {
                if summary.replace(text.to_string()).is_some() {
                    panic!("builtins.toml:{}: duplicate field `summary`", line_number);
                }
            }
            "description" => {
                if description.replace(text.to_string()).is_some() {
                    panic!(
                        "builtins.toml:{}: duplicate field `description`",
                        line_number
                    );
                }
            }
            "directive" => {
                if directive.replace(text.to_string()).is_some() {
                    panic!("builtins.toml:{}: duplicate field `directive`", line_number);
                }
            }
            _ => panic!("builtins.toml:{}: unknown field `{}`", line_number, key),
        }
        while pos < bytes.len() && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos < bytes.len() {
            if bytes[pos] != b',' {
                panic!(
                    "builtins.toml:{}: expected `,` between inline-table entries",
                    line_number
                );
            }
            pos += 1;
        }
    }
    let summary =
        summary.unwrap_or_else(|| panic!("builtins.toml:{}: missing summary", line_number));
    let description =
        description.unwrap_or_else(|| panic!("builtins.toml:{}: missing description", line_number));
    (summary, description, directive)
}

fn validate_emittable(text: &str, line_number: usize) {
    if text.contains(['"', '\\', '\n', '\r']) {
        panic!(
            "builtins.toml:{}: value cannot be emitted safely",
            line_number
        );
    }
}

fn emit_static(constant: &str, entries: &[Entry]) -> String {
    let mut out = format!("pub static {constant}: &[BuiltinDefinition] = &[\n");
    for (slug, summary, description, directive) in entries {
        let directive_literal = match directive {
            Some(text) => format!("Some(\"{text}\")"),
            None => "None".to_string(),
        };
        out.push_str(&format!(
            "    BuiltinDefinition {{ slug: \"{slug}\", summary: \"{summary}\", description: \"{description}\", directive: {directive_literal} }},\n"
        ));
    }
    out.push_str("];\n");
    out
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
