//! SDK generation and source-drift checks at the CLI filesystem boundary.

mod canonical;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ctx_traits_core::response::CommandOutput;
use serde_json::Value;

const CDK_STRUCTURAL_KINDS: &[&str] = &[
    "procedure",
    "sequence-step",
    "sequence-linear",
    "template",
    "schema-field",
    "guard",
    "ref",
    "behavior",
    "output-sink",
    "instruction-output",
    "output-template",
];

struct Builtin {
    slug: String,
    summary: String,
    description: String,
}

struct AgentTemplate {
    name: String,
    description: String,
    summary: String,
    model_tier: Option<String>,
}

struct SchemaWrapper {
    name: String,
    prefix: String,
    separator: String,
    suffix: String,
    type_template: String,
}

enum WriteOperation {
    Unit(String),
    Field(String),
}

struct Model {
    agent_templates: Vec<AgentTemplate>,
    ref_kinds: BTreeSet<String>,
    resource_triggers: Vec<String>,
    resource_roots: Vec<String>,
    resource_renders: Vec<String>,
    statuses: Vec<String>,
    trust_levels: Vec<String>,
    schema_builtins: Vec<String>,
    schema_wrappers: Vec<SchemaWrapper>,
    write_operations: Vec<WriteOperation>,
    synth_formats: Vec<String>,
    runtime_surfaces: Vec<String>,
    runtime_operations: Vec<String>,
    intent: Vec<Builtin>,
    tone: Vec<Builtin>,
    method: Vec<Builtin>,
    verbosity: Vec<Builtin>,
    format: Vec<Builtin>,
    directness: Vec<Builtin>,
    scope_control: Vec<Builtin>,
    initiative: Vec<Builtin>,
    uncertainty: Vec<Builtin>,
}

pub fn handle(check: bool) -> crate::Result<CommandOutput<()>> {
    let root = std::env::current_dir().map_err(|error| crate::Error::Command {
        message: format!("resolve workspace root: {error}"),
    })?;
    let source_root = root.join("packages/cdk/src");
    if !source_root.is_dir() {
        return Err(crate::Error::Command {
            message: "expected packages/cdk/src below the workspace root".to_string(),
        });
    }

    let model = Model::load()?;
    let generated = source_root.join("generated.ts");
    let trait_schema = ctx_traits_core::schema::trait_schema()?;
    let output = format!("{}{}", render(&model)?, canonical::render(&trait_schema)?);
    if check {
        let current = fs::read_to_string(&generated).map_err(|error| crate::Error::Command {
            message: format!("read {}: {error}", generated.display()),
        })?;
        if current != output {
            return Err(crate::Error::Command {
                message: "stale generated SDK file: packages/cdk/src/generated.ts".to_string(),
            });
        }
        validate_sources(&source_root, &model)?;
    } else {
        let current = fs::read_to_string(&generated).ok();
        if current.as_deref() != Some(&output) {
            fs::write(&generated, output).map_err(|error| crate::Error::Command {
                message: format!("write {}: {error}", generated.display()),
            })?;
        }
    }

    write_config_generated(&root, check)?;

    Ok(CommandOutput::new(()))
}

/// P457: `packages/config/src/generated.ts`, the Rust-config-derived
/// TypeScript type mirror. Shares this command's write-if-different and
/// `--check` staleness comparison with the CDK generated file above, rather
/// than a second `sdk-generate`-style verb.
fn write_config_generated(root: &Path, check: bool) -> crate::Result<()> {
    let source_root = root.join("packages/config/src");
    if !source_root.is_dir() {
        return Err(crate::Error::Command {
            message: "expected packages/config/src below the workspace root".to_string(),
        });
    }
    let generated = source_root.join("generated.ts");
    let output = crate::app::config_types::render()?;
    if check {
        let current = fs::read_to_string(&generated).map_err(|error| crate::Error::Command {
            message: format!("read {}: {error}", generated.display()),
        })?;
        if current != output {
            return Err(crate::Error::Command {
                message: "stale generated SDK file: packages/config/src/generated.ts".to_string(),
            });
        }
    } else {
        let current = fs::read_to_string(&generated).ok();
        if current.as_deref() != Some(&output) {
            fs::write(&generated, output).map_err(|error| crate::Error::Command {
                message: format!("write {}: {error}", generated.display()),
            })?;
        }
    }
    Ok(())
}

impl Model {
    fn load() -> crate::Result<Self> {
        let schema = ctx_traits_core::schema::sdk_types_schema()?;
        let exports = schema
            .get("x-ctx-sdk")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_model("missing x-ctx-sdk object"))?;
        let ref_kinds = strings(exports.get("ref-kinds"), "ref-kinds")?
            .into_iter()
            .collect();
        let builtins = exports
            .get("builtins")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_model("missing builtins object"))?;
        let schema_forms = exports
            .get("schema-forms")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_model("missing schema-forms object"))?;
        let behavior = builtins
            .get("behavior")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid_model("missing builtins.behavior object"))?;
        Ok(Self {
            agent_templates: agent_templates(exports.get("agent-templates"))?,
            ref_kinds,
            resource_triggers: strings(exports.get("resource-triggers"), "resource-triggers")?,
            resource_roots: strings(exports.get("resource-roots"), "resource-roots")?,
            resource_renders: strings(exports.get("resource-renders"), "resource-renders")?,
            statuses: strings(exports.get("statuses"), "statuses")?,
            trust_levels: strings(exports.get("trust-levels"), "trust-levels")?,
            schema_builtins: strings(schema_forms.get("builtins"), "schema-forms.builtins")?,
            schema_wrappers: schema_wrappers(schema_forms.get("wrappers"))?,
            write_operations: write_operations(exports.get("write-operations"))?,
            synth_formats: strings(exports.get("synth-formats"), "synth-formats")?,
            runtime_surfaces: strings(exports.get("runtime-surfaces"), "runtime-surfaces")?,
            runtime_operations: strings(exports.get("runtime-operations"), "runtime-operations")?,
            intent: builtins_for(builtins.get("intent"), "intent")?,
            tone: builtins_for(behavior.get("tone"), "behavior.tone")?,
            method: builtins_for(behavior.get("method"), "behavior.method")?,
            verbosity: builtins_for(behavior.get("verbosity"), "behavior.verbosity")?,
            format: builtins_for(behavior.get("format"), "behavior.format")?,
            directness: builtins_for(behavior.get("directness"), "behavior.directness")?,
            scope_control: builtins_for(behavior.get("scope-control"), "behavior.scope-control")?,
            initiative: builtins_for(behavior.get("initiative"), "behavior.initiative")?,
            uncertainty: builtins_for(behavior.get("uncertainty"), "behavior.uncertainty")?,
        })
    }
}

fn agent_templates(value: Option<&Value>) -> crate::Result<Vec<AgentTemplate>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_model("missing agent-templates array"))?
        .iter()
        .map(|entry| {
            let entry = entry
                .as_object()
                .ok_or_else(|| invalid_model("agent-templates contains a non-object"))?;
            Ok(AgentTemplate {
                name: agent_template_field(entry, "name")?.to_string(),
                description: agent_template_field(entry, "description")?.to_string(),
                summary: agent_template_field(entry, "summary")?.to_string(),
                model_tier: entry
                    .get("model-tier")
                    .map(|_| agent_template_field(entry, "model-tier").map(str::to_string))
                    .transpose()?,
            })
        })
        .collect()
}

fn agent_template_field<'a>(
    entry: &'a serde_json::Map<String, Value>,
    field: &str,
) -> crate::Result<&'a str> {
    entry
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_model(format!("agent-templates entry has no {field}")))
}

fn write_operations(value: Option<&Value>) -> crate::Result<Vec<WriteOperation>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_model("missing write-operations array"))?
        .iter()
        .map(|entry| {
            let object = entry
                .as_object()
                .ok_or_else(|| invalid_model("write-operations contains a non-object"))?;
            if let Some(value) = object.get("const").and_then(Value::as_str) {
                return Ok(WriteOperation::Unit(value.to_string()));
            }
            let required = strings(object.get("required"), "write-operations.required")?;
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| invalid_model("write-operation object has no properties"))?;
            let [field] = required.as_slice() else {
                return Err(invalid_model(
                    "write-operation object must require exactly one field",
                ));
            };
            if properties.len() != 1
                || properties
                    .get(field)
                    .and_then(Value::as_object)
                    .and_then(|schema| schema.get("type"))
                    .and_then(Value::as_str)
                    != Some("string")
            {
                return Err(invalid_model(
                    "write-operation object must contain one string field",
                ));
            }
            Ok(WriteOperation::Field(field.clone()))
        })
        .collect()
}

fn schema_wrappers(value: Option<&Value>) -> crate::Result<Vec<SchemaWrapper>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_model("missing schema-forms.wrappers array"))?
        .iter()
        .map(|entry| {
            let entry = entry
                .as_object()
                .ok_or_else(|| invalid_model("schema-forms.wrappers contains a non-object"))?;
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_model("schema-forms.wrappers entry has no name"))?;
            let type_template = entry
                .get("type-template")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_model("schema-forms.wrappers entry has no type-template"))?;
            let prefix = schema_wrapper_part(entry, "prefix")?;
            let separator = schema_wrapper_part(entry, "separator")?;
            let suffix = schema_wrapper_part(entry, "suffix")?;
            if name.is_empty()
                || type_template.is_empty()
                || [prefix, separator, suffix, type_template]
                    .iter()
                    .any(|part| part.contains(['"', '\\', '`', '\n', '\r']))
            {
                return Err(invalid_model("invalid schema wrapper descriptor"));
            }
            Ok(SchemaWrapper {
                name: name.to_string(),
                prefix: prefix.to_string(),
                separator: separator.to_string(),
                suffix: suffix.to_string(),
                type_template: type_template.to_string(),
            })
        })
        .collect()
}

fn schema_wrapper_part<'a>(
    entry: &'a serde_json::Map<String, Value>,
    name: &str,
) -> crate::Result<&'a str> {
    entry
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_model(format!("schema-forms.wrappers entry has no {name}")))
}

pub(super) fn invalid_model(message: impl Into<String>) -> crate::Error {
    crate::Error::Command {
        message: format!("invalid SDK schema export: {}", message.into()),
    }
}

fn strings(value: Option<&Value>, name: &str) -> crate::Result<Vec<String>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_model(format!("missing {name} array")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid_model(format!("{name} contains a non-string")))
        })
        .collect()
}

fn builtins_for(value: Option<&Value>, category: &str) -> crate::Result<Vec<Builtin>> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_model(format!("missing builtins.{category} array")))?
        .iter()
        .map(|entry| {
            let entry = entry.as_object().ok_or_else(|| {
                invalid_model(format!("builtins.{category} contains a non-object"))
            })?;
            let slug = builtin_field(entry, category, "slug")?;
            let summary = builtin_field(entry, category, "summary")?;
            let description = builtin_field(entry, category, "description")?;
            check_jsdoc_safe(summary, category)?;
            check_jsdoc_safe(description, category)?;
            Ok(Builtin {
                slug: slug.to_string(),
                summary: summary.to_string(),
                description: description.to_string(),
            })
        })
        .collect()
}

fn builtin_field<'a>(
    entry: &'a serde_json::Map<String, Value>,
    category: &str,
    field: &str,
) -> crate::Result<&'a str> {
    entry
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_model(format!("builtins.{category} has no {field}")))
}

fn check_jsdoc_safe(text: &str, category: &str) -> crate::Result<()> {
    if text.contains("*/") || text.contains(['\n', '\r']) {
        return Err(invalid_model(format!(
            "builtins.{category} text cannot be emitted as JSDoc"
        )));
    }
    Ok(())
}

fn render(model: &Model) -> crate::Result<String> {
    let ref_kinds: Vec<_> = model.ref_kinds.iter().cloned().collect();
    let mut output = format!(
        "// Generated by `ctx traits sdk-generate` from Rust SDK vocabulary.\n// Do not edit by hand.\n\nexport type JsonValue = null | boolean | number | string | readonly JsonValue[] | JsonObject;\nexport type JsonObject = {{ readonly [key: string]: JsonValue | undefined }};\n\nexport type RefKind = {};\nexport type SchemaBuiltin = {};\nexport type SchemaForm = SchemaBuiltin | {};\nexport const schemaForms = {{\n{}\n}} as const;\nexport type WriteOperation = {};\nexport type SynthFormat = {};\nexport type RuntimeSurface = {};\nexport type RuntimeOperation = {};\n\n",
        union(&ref_kinds),
        union(&model.schema_builtins),
        model
            .schema_wrappers
            .iter()
            .map(|wrapper| format!("`{}`", wrapper.type_template))
            .collect::<Vec<_>>()
            .join(" | "),
        model
            .schema_wrappers
            .iter()
            .map(|wrapper| format!(
                "  {}: {{ prefix: \"{}\", separator: \"{}\", suffix: \"{}\" }},",
                wrapper.name, wrapper.prefix, wrapper.separator, wrapper.suffix
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        model
            .write_operations
            .iter()
            .map(|operation| match operation {
                WriteOperation::Unit(value) => format!("\"{value}\""),
                WriteOperation::Field(field) => {
                    format!("{{ readonly \"{field}\": string }}")
                }
            })
            .collect::<Vec<_>>()
            .join(" | "),
        union(&model.synth_formats),
        union(&model.runtime_surfaces),
        union(&model.runtime_operations),
    );
    for (const_name, type_name, values) in [
        ("Tone", "ToneBuiltIn", &model.tone),
        ("Method", "MethodBuiltIn", &model.method),
        ("Verbosity", "VerbosityBuiltIn", &model.verbosity),
        ("Format", "FormatBuiltIn", &model.format),
        ("Directness", "DirectnessBuiltIn", &model.directness),
        ("ScopeControl", "ScopeControlBuiltIn", &model.scope_control),
        ("Initiative", "InitiativeBuiltIn", &model.initiative),
        ("Uncertainty", "UncertaintyBuiltIn", &model.uncertainty),
    ] {
        output.push_str(&builtin_union(type_name, values)?);
        output.push_str(&builtin_const(const_name, type_name, values)?);
        output.push('\n');
    }
    output.push_str(&render_intent(model)?);
    output.push_str(&render_agent_templates(&model.agent_templates)?);
    output.push('\n');
    output.push_str(&render_string_enum(
        "ResourceTrigger",
        &model.resource_triggers,
    )?);
    output.push('\n');
    output.push_str(&render_string_enum("ResourceRoot", &model.resource_roots)?);
    output.push('\n');
    output.push_str(&render_string_enum(
        "ResourceRender",
        &model.resource_renders,
    )?);
    output.push('\n');
    output.push_str(&render_string_enum("Status", &model.statuses)?);
    output.push('\n');
    output.push_str(&render_string_enum("Trust", &model.trust_levels)?);
    output.push('\n');
    output.push_str("export interface CdkDiagnostic {\n  readonly severity: \"warning\" | \"error\";\n  readonly code: string;\n  readonly fieldPath: string;\n  readonly message: string;\n}\n\nexport interface SynthPlan {\n  readonly delegate: \"ctx-traits-wasm-core-or-cli\";\n  readonly command: readonly string[];\n  readonly draft: JsonObject;\n  readonly diagnostics: readonly CdkDiagnostic[];\n  readonly format: SynthFormat;\n  readonly out?: string;\n}\n\nexport interface RuntimeDelegationPlan {\n  readonly surface: RuntimeSurface;\n  readonly operation: RuntimeOperation;\n  readonly wasmExport?: string;\n  readonly cliCommand?: readonly string[];\n  readonly request: JsonValue;\n  readonly note: \"delegates-to-rust-core\";\n}\n");
    Ok(output)
}

fn render_agent_templates(templates: &[AgentTemplate]) -> crate::Result<String> {
    if templates.is_empty() {
        return Err(invalid_model("AgentTemplateName has no values"));
    }
    let names: Vec<_> = templates
        .iter()
        .map(|template| template.name.clone())
        .collect();
    let mut model_tiers = Vec::new();
    for tier in templates
        .iter()
        .filter_map(|template| template.model_tier.as_ref())
    {
        if !model_tiers.contains(tier) {
            model_tiers.push(tier.clone());
        }
    }
    if model_tiers.is_empty() {
        return Err(invalid_model("AgentModelTier has no values"));
    }
    let mut output = format!(
        "export type AgentTemplateName = {};\nexport type AgentModelTier = {};\n\nexport interface AgentTemplateDefinition {{\n  readonly description: string;\n  readonly summary: string;\n  readonly modelTier?: AgentModelTier;\n}}\n\nexport const agentTemplates = {{\n",
        union(&names),
        union(&model_tiers),
    );
    for template in templates {
        let description = serde_json::to_string(&template.description).map_err(|error| {
            invalid_model(format!("cannot emit agent template description: {error}"))
        })?;
        let summary = serde_json::to_string(&template.summary).map_err(|error| {
            invalid_model(format!("cannot emit agent template summary: {error}"))
        })?;
        let model_tier = match &template.model_tier {
            Some(tier) => format!(
                ", modelTier: {}",
                serde_json::to_string(tier).map_err(|error| {
                    invalid_model(format!("cannot emit agent model tier: {error}"))
                })?
            ),
            None => String::new(),
        };
        output.push_str(&format!(
            "  {}: {{ description: {description}, summary: {summary}{model_tier} }},\n",
            template.name,
        ));
    }
    output.push_str("} as const satisfies Record<AgentTemplateName, AgentTemplateDefinition>;\n");
    Ok(output)
}

fn union(values: &[String]) -> String {
    values
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Renders a structural (non-builtin) string-enum vocabulary as a TypeScript
/// union type plus a `camelCase -> kebab-case` const object, e.g.
/// `export type ResourceTrigger = "on-activation" | "on-demand";`.
fn render_string_enum(name: &str, values: &[String]) -> crate::Result<String> {
    if values.is_empty() {
        return Err(invalid_model(format!("{name} has no values")));
    }
    let mut properties = String::new();
    for value in values {
        properties.push_str(&format!("  {}: \"{}\",\n", camel_property(value), value));
    }
    Ok(format!(
        "export type {name} = {};\nexport const {name} = {{\n{properties}}} as const satisfies Record<string, {name}>;\n",
        union(values),
    ))
}

fn builtin_union(name: &str, values: &[Builtin]) -> crate::Result<String> {
    if values.is_empty() {
        return Err(invalid_model(format!("{name} has no values")));
    }
    let mut output = format!("/** Built-in {name} values.\n");
    for value in values {
        output.push_str(&format!(" * @property {} {}\n", value.slug, value.summary));
    }
    output.push_str(" */\n");
    output.push_str(&format!(
        "export type {name} = {};\n",
        values
            .iter()
            .map(|value| format!("\"{}\"", value.slug))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    Ok(output)
}

/// Deterministic kebab-case-slug to lower-camel-case TypeScript property name.
fn camel_property(slug: &str) -> String {
    let mut result = String::new();
    for part in slug.split('-') {
        if part.is_empty() {
            continue;
        }
        if result.is_empty() {
            result.push_str(part);
            continue;
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            result.extend(first.to_uppercase());
            result.push_str(chars.as_str());
        }
    }
    result
}

/// Renders `{ /** summary description */ camelCaseSlug: "kebab-case-slug", ... }` property
/// lines for a catalog, indented by `indent`.
fn builtin_properties(values: &[Builtin], indent: &str) -> String {
    let mut output = String::new();
    for value in values {
        output.push_str(&format!(
            "{indent}/** {} {} */\n{indent}{}: \"{}\",\n",
            value.summary,
            value.description,
            camel_property(&value.slug),
            value.slug
        ));
    }
    output
}

fn builtin_const(const_name: &str, type_name: &str, values: &[Builtin]) -> crate::Result<String> {
    if values.is_empty() {
        return Err(invalid_model(format!("{const_name} has no values")));
    }
    Ok(format!(
        "export const {const_name} = {{\n{}}} as const satisfies Record<string, {type_name}>;\n",
        builtin_properties(values, "  ")
    ))
}

fn render_intent(model: &Model) -> crate::Result<String> {
    if model.intent.is_empty() {
        return Err(invalid_model("intent catalog has no values"));
    }
    // One flat catalog. Which facet an item belongs to — require, focus,
    // avoid, block — is the trait's decision at its own call site, so the
    // authoring surface is `intent.X`, not `intent.<facet>.X`.
    let mut output = builtin_union("IntentBuiltIn", &model.intent)?;
    output.push_str(&format!(
        "export const Intent = {{\n{}}} as const satisfies Record<string, IntentBuiltIn>;\n\n",
        builtin_properties(&model.intent, "  ")
    ));
    Ok(output)
}

fn validate_sources(source_root: &Path, model: &Model) -> crate::Result<()> {
    let agent_source = read_source(source_root.join("agent.ts"))?;
    let meta = read_source(source_root.join("meta.ts"))?;
    let normalize = read_source(source_root.join("normalize.ts"))?;
    let trait_source = read_source(source_root.join("trait.ts"))?;
    let schema_source = read_source(source_root.join("schema.ts"))?;
    let slot_source = read_source(source_root.join("slot.ts"))?;
    let decl_kinds = validate_union_literals(
        &meta,
        "export type DeclKind =",
        ";",
        &model.ref_kinds,
        &[],
        "meta.ts DeclKind",
    )?;

    let mut meta_allowed = model.ref_kinds.clone();
    meta_allowed.extend(decl_kinds);
    meta_allowed.extend(CDK_STRUCTURAL_KINDS.iter().map(|kind| (*kind).to_string()));
    validate_union_literals(
        &meta,
        "readonly kind?: DeclKind |",
        ";",
        &meta_allowed,
        &[],
        "meta.ts Meta.kind",
    )?;
    validate_array_literals(
        &normalize,
        "const declKinds:",
        "];",
        &model.ref_kinds,
        "normalize.ts declKinds",
    )?;
    validate_empty_array_record(
        &normalize,
        "const result: Record<DeclKind, JsonObject[]> = {",
        "};",
        &model.ref_kinds,
        "normalize.ts collectMany",
    )?;
    validate_explicit_declaration_record(
        &normalize,
        "export function explicitDeclarations(fields: TraitFields): Partial<Record<DeclKind, readonly JsonObject[]>> { return {",
        "};",
        &model.ref_kinds,
        "normalize.ts explicitDeclarations",
    )?;
    // P331 wrapped these three re-exports in a callable namespace (`tone`,
    // `method`, `verbosity`); 0046 deleted the deprecated PascalCase shims
    // (`Tone`/`Method`/`Verbosity`) from trait.ts entirely, so only the
    // namespace binding itself remains to trace to the generated value.
    for (axis, binding) in [
        ("tone", "GeneratedTone"),
        ("method", "GeneratedMethod"),
        ("verbosity", "GeneratedVerbosity"),
        ("directness", "GeneratedDirectness"),
        ("scopeControl", "GeneratedScopeControl"),
        ("initiative", "GeneratedInitiative"),
        ("uncertainty", "GeneratedUncertainty"),
        ("format", "GeneratedFormat"),
    ] {
        validate_behavior_axis_binding(&trait_source, axis, binding)?;
    }
    validate_callable_namespace_binding(
        &trait_source,
        "intent",
        "GeneratedIntent",
        "trait.ts intent",
    )?;
    validate_exact_const_object_literals(
        &schema_source,
        "const schemaBuiltins = {",
        "} as const satisfies",
        &model.schema_builtins.iter().cloned().collect(),
        "schema.ts schemaBuiltins",
    )?;
    let wrapper_names: BTreeSet<_> = model
        .schema_wrappers
        .iter()
        .map(|wrapper| wrapper.name.as_str())
        .collect();
    if wrapper_names != BTreeSet::from(["list", "union"]) {
        return Err(invalid_model(
            "schema-forms.wrappers must contain list and union descriptors",
        ));
    }
    validate_schema_builder_wiring(&schema_source)?;
    validate_write_operation_builder(&slot_source, &model.write_operations)?;
    for template in &model.agent_templates {
        let expected = format!("agentTemplates.{}", template.name);
        // P459's deprecatedBareAgentTemplate wrapper legitimately references
        // each generated template a second time (once in the record
        // literal, once in its top-level deprecated export), so — matching
        // the schemaForms wrapper-token check above — missing is the only
        // failure this proves; duplicate is not evidence of drift.
        if agent_source.matches(&expected).count() == 0 {
            return Err(crate::Error::Command {
                message: format!(
                    "agent.ts template helpers: missing generated template {:?}",
                    template.name
                ),
            });
        }
    }
    Ok(())
}

fn validate_write_operation_builder(
    source: &str,
    operations: &[WriteOperation],
) -> crate::Result<()> {
    let builder = anchored_region(
        source,
        "export const operation: OperationFunction = {",
        "};",
        "slot.ts operation builder",
    )?;
    for operation in operations {
        let value = match operation {
            WriteOperation::Unit(value) | WriteOperation::Field(value) => value,
        };
        let literal = format!("\"{value}\"");
        if !builder.contains(&literal) {
            return Err(crate::Error::Command {
                message: format!(
                    "slot.ts operation builder: missing generated write operation {value:?}"
                ),
            });
        }
    }
    Ok(())
}

fn validate_schema_builder_wiring(source: &str) -> crate::Result<()> {
    let builder = anchored_region(
        source,
        "export const schema: SchemaFunction = {",
        "};",
        "schema.ts schema builder",
    )?;
    // `list` is the one list spelling: the `array:` alias and the
    // `schemaArray` constructor name both retired when the CDK unified list
    // naming, so expecting them here only proved this check had not been
    // updated with the rename.
    for expected in ["list: schemaList", "union: schemaUnion"] {
        if builder.matches(expected).count() != 1 {
            return Err(crate::Error::Command {
                message: format!("schema.ts schema builder: missing or duplicate {expected:?}"),
            });
        }
    }
    for expected in [
        "schemaForms.list.prefix",
        "schemaForms.list.suffix",
        "schemaForms.union.prefix",
        "schemaForms.union.separator",
        "schemaForms.union.suffix",
    ] {
        // Wrapper tokens are legitimately used in more than one place
        // (wrapping in schemaList, unwrapping in schemaUnion's list-member
        // check); the guard proves the generated vocabulary is wired, so
        // missing is the only failure.
        if source.matches(expected).count() == 0 {
            return Err(crate::Error::Command {
                message: format!("schema.ts schema wrappers: missing {expected:?}"),
            });
        }
    }
    Ok(())
}

fn read_source(path: PathBuf) -> crate::Result<String> {
    fs::read_to_string(&path).map_err(|error| crate::Error::Command {
        message: format!("read {}: {error}", path.display()),
    })
}

/// Confirms one axis of the `behavior` namespace is built from its generated
/// const, so the whole vocabulary still traces to codegen rather than drifting
/// into a hand-written map.
fn validate_behavior_axis_binding(source: &str, axis: &str, binding: &str) -> crate::Result<()> {
    let expected = format!("{axis}: callableNamespace({binding}),");
    if source.matches(&expected).count() != 1 {
        return Err(crate::Error::Command {
            message: format!("trait.ts behavior.{axis}: expected exactly one `{expected}`"),
        });
    }
    Ok(())
}

/// Confirms `const_name` is itself built from the generated const via
/// `callableNamespace(...)`, so the alias
/// chain still traces to the generated vocabulary.
fn validate_callable_namespace_binding(
    source: &str,
    const_name: &str,
    binding: &str,
    label: &str,
) -> crate::Result<()> {
    let expected = format!("export const {const_name} = callableNamespace({binding});");
    if source.matches(&expected).count() != 1 {
        return Err(crate::Error::Command {
            message: format!("{label}: expected exactly one `{expected}`"),
        });
    }
    Ok(())
}

fn validate_union_literals(
    source: &str,
    anchor: &str,
    end: &str,
    allowed: &BTreeSet<String>,
    identifiers: &[&str],
    label: &str,
) -> crate::Result<BTreeSet<String>> {
    let region = anchored_region(source, anchor, end, label)?;
    let values = union_literals(region, identifiers, label)?;
    check_subset(&values, allowed, label)?;
    Ok(values.into_iter().collect())
}

fn validate_array_literals(
    source: &str,
    anchor: &str,
    end: &str,
    allowed: &BTreeSet<String>,
    label: &str,
) -> crate::Result<()> {
    let region = anchored_region(source, anchor, end, label)?;
    let values = array_literals(region, label)?;
    check_subset(&values, allowed, label)
}

fn validate_exact_const_object_literals(
    source: &str,
    anchor: &str,
    end: &str,
    allowed: &BTreeSet<String>,
    label: &str,
) -> crate::Result<()> {
    let region = anchored_region(source, anchor, end, label)?;
    let values = object_literals(region, label)?;
    check_subset(&values, allowed, label)?;
    let actual: BTreeSet<_> = values.iter().cloned().collect();
    if values.len() != actual.len() || &actual != allowed {
        return Err(crate::Error::Command {
            message: format!("{label}: literals do not exactly match generated vocabulary"),
        });
    }
    Ok(())
}

fn validate_empty_array_record(
    source: &str,
    anchor: &str,
    end: &str,
    allowed: &BTreeSet<String>,
    label: &str,
) -> crate::Result<()> {
    let region = anchored_region(source, anchor, end, label)?;
    let values = record_keys(region, label, RecordValue::EmptyArray)?;
    check_subset(&values, allowed, label)
}

fn validate_explicit_declaration_record(
    source: &str,
    anchor: &str,
    end: &str,
    allowed: &BTreeSet<String>,
    label: &str,
) -> crate::Result<()> {
    let region = anchored_region(source, anchor, end, label)?;
    let values = record_keys(region, label, RecordValue::ExplicitDeclaration)?;
    check_subset(&values, allowed, label)
}

fn anchored_region<'a>(
    source: &'a str,
    anchor: &str,
    end: &str,
    label: &str,
) -> crate::Result<&'a str> {
    if source.matches(anchor).count() != 1 {
        return Err(crate::Error::Command {
            message: format!("{label}: missing or duplicate anchor"),
        });
    }
    let after = source.split_once(anchor).expect("unique anchor").1;
    let Some((region, _)) = after.split_once(end) else {
        return Err(crate::Error::Command {
            message: format!("{label}: incomplete anchored region"),
        });
    };
    Ok(region)
}

fn union_literals(region: &str, identifiers: &[&str], label: &str) -> crate::Result<Vec<String>> {
    let mut values = Vec::new();
    let mut parser = LiteralParser::new(region, label);
    parser.whitespace();
    loop {
        if parser.consume_identifier(identifiers) {
            // DeclKind is the only non-literal permitted in the Meta.kind union.
        } else {
            values.push(parser.string()?);
        }
        parser.whitespace();
        if parser.end() {
            break;
        }
        parser.expect('|')?;
        parser.whitespace();
    }
    if values.is_empty() {
        return Err(crate::Error::Command {
            message: format!("{label}: expected quoted literals"),
        });
    }
    Ok(values)
}

fn array_literals(region: &str, label: &str) -> crate::Result<Vec<String>> {
    let mut parser = LiteralParser::new(region, label);
    parser.whitespace();
    parser.expect_text("readonly DeclKind[] =")?;
    parser.whitespace();
    parser.expect('[')?;
    parser.whitespace();
    let mut values = Vec::new();
    loop {
        values.push(parser.string()?);
        parser.whitespace();
        if parser.end() {
            break;
        }
        parser.expect(',')?;
        parser.whitespace();
        if parser.end() {
            break;
        }
    }
    Ok(values)
}

fn object_literals(region: &str, label: &str) -> crate::Result<Vec<String>> {
    let mut parser = LiteralParser::new(region, label);
    parser.whitespace();
    let mut values = Vec::new();
    loop {
        parser.identifier()?;
        parser.whitespace();
        parser.expect(':')?;
        parser.whitespace();
        values.push(parser.string()?);
        parser.whitespace();
        if parser.end() {
            break;
        }
        parser.expect(',')?;
        parser.whitespace();
        if parser.end() {
            break;
        }
    }
    Ok(values)
}

struct LiteralParser<'a> {
    remaining: &'a str,
    label: &'a str,
}

impl<'a> LiteralParser<'a> {
    fn new(remaining: &'a str, label: &'a str) -> Self {
        Self { remaining, label }
    }

    fn whitespace(&mut self) {
        self.remaining = self.remaining.trim_start_matches(char::is_whitespace);
    }

    fn end(&self) -> bool {
        self.remaining.is_empty()
    }

    fn expect(&mut self, expected: char) -> crate::Result<()> {
        let Some(rest) = self.remaining.strip_prefix(expected) else {
            return Err(self.unsupported());
        };
        self.remaining = rest;
        Ok(())
    }

    fn expect_text(&mut self, expected: &str) -> crate::Result<()> {
        let Some(rest) = self.remaining.strip_prefix(expected) else {
            return Err(self.unsupported());
        };
        self.remaining = rest;
        Ok(())
    }

    fn string(&mut self) -> crate::Result<String> {
        self.expect('"')?;
        let Some(end) = self.remaining.find('"') else {
            return Err(crate::Error::Command {
                message: format!("{}: unterminated string literal", self.label),
            });
        };
        let literal = &self.remaining[..end];
        if literal.is_empty() || literal.contains(['\\', '\n', '\r']) {
            return Err(self.unsupported());
        }
        self.remaining = &self.remaining[end + 1..];
        Ok(literal.to_string())
    }

    fn identifier(&mut self) -> crate::Result<&'a str> {
        let length = self
            .remaining
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        if length == 0 || !self.remaining.as_bytes()[0].is_ascii_alphabetic() {
            return Err(self.unsupported());
        }
        let identifier = &self.remaining[..length];
        self.remaining = &self.remaining[length..];
        Ok(identifier)
    }

    fn key(&mut self) -> crate::Result<&'a str> {
        let length = self
            .remaining
            .bytes()
            .take_while(|byte| byte.is_ascii_lowercase() || *byte == b'-')
            .count();
        if length == 0 {
            return Err(self.unsupported());
        }
        let key = &self.remaining[..length];
        self.remaining = &self.remaining[length..];
        Ok(key)
    }

    fn consume_identifier(&mut self, allowed: &[&str]) -> bool {
        let mut candidate = Self::new(self.remaining, self.label);
        let Ok(identifier) = candidate.identifier() else {
            return false;
        };
        if allowed.contains(&identifier) {
            self.remaining = candidate.remaining;
            true
        } else {
            false
        }
    }

    fn unsupported(&self) -> crate::Error {
        crate::Error::Command {
            message: format!("{}: unsupported syntax", self.label),
        }
    }
}

#[derive(Clone, Copy)]
enum RecordValue {
    EmptyArray,
    ExplicitDeclaration,
}

fn record_keys(region: &str, label: &str, value: RecordValue) -> crate::Result<Vec<String>> {
    let mut parser = LiteralParser::new(region, label);
    parser.whitespace();
    let mut keys = Vec::new();
    loop {
        let key = parser.key()?;
        parser.whitespace();
        parser.expect(':')?;
        parser.whitespace();
        match value {
            RecordValue::EmptyArray => parser.expect_text("[]")?,
            RecordValue::ExplicitDeclaration => {
                parser.expect_text("explicitDeclarationItems(")?;
                let literal = parser.string()?;
                parser.expect(',')?;
                parser.whitespace();
                parser.expect_text("fields.")?;
                let field = parser.identifier()?;
                parser.expect(')')?;
                if literal != key || field != key {
                    return Err(parser.unsupported());
                }
            }
        }
        keys.push(key.to_string());
        parser.whitespace();
        if parser.end() {
            break;
        }
        parser.expect(',')?;
        parser.whitespace();
        if parser.end() {
            break;
        }
    }
    if keys.is_empty() {
        return Err(crate::Error::Command {
            message: format!("{label}: incomplete object literal"),
        });
    }
    Ok(keys)
}

fn check_subset(values: &[String], allowed: &BTreeSet<String>, label: &str) -> crate::Result<()> {
    let mut invalid: Vec<_> = values
        .iter()
        .filter(|value| !allowed.contains(*value))
        .collect();
    invalid.sort();
    if let Some(value) = invalid.first() {
        return Err(crate::Error::Command {
            message: format!("{label}: unsupported literal {value:?}"),
        });
    }
    Ok(())
}
