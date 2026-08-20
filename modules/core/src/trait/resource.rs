//! Resource declarations: opaque blob references for schema bodies and
//! external data.
//!
//! `[[resource]]` sections declare opaque resource paths relative to a typed
//! root (`package`, the default, or `repo`). Resources are simplified MVP
//! declarations with `id`, `path`, `root`, and optional `hint` and `trigger`
//! metadata. Core does not read files, check existence, compute
//! digests, or load resource blobs.
//!
//! Path safety is validated at declaration time regardless of root: resource
//! paths must be relative (no absolute paths, no `..` traversal, no backslash
//! separators, no empty segments). This prevents declaration-level escape
//! attempts before IO ever touches the filesystem.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;
use crate::reference::{Kind, Reference};

/// Where a resource's declared path is rooted.
///
/// `Package` (the default, and omitted from canonical serialization) resolves
/// against the trait package directory, jailed as today. `Repo` resolves
/// against the invocation repository root discovered at the IO boundary —
/// repo-root resources are repo-coupled and not portable across checkouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ResourceRoot {
    /// The trait package directory (default).
    #[default]
    Package,
    /// The invocation repository root.
    Repo,
}

impl ResourceRoot {
    fn is_package(&self) -> bool {
        matches!(self, ResourceRoot::Package)
    }
}

/// When a resource is needed at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ResourceTrigger {
    /// Loaded when the trait is activated.
    OnActivation,
    /// Loaded when the first step that needs it is reached.
    OnDemand,
}

/// How a resource's body reaches the model: presented as an openable file
/// reference or embedded inline in model-visible output.
///
/// This is a separate axis from `trigger` (when a resource loads) and from
/// `path`/`content` (where its body comes from). A `path`-backed resource may
/// still declare `render = "inline"` to have its file body read at the IO
/// edge and embedded, exactly as a `content` resource is embedded today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ResourceRender {
    /// Presented as an openable file reference; bytes stay on disk.
    Reference,
    /// Embedded verbatim in model-visible output.
    Inline,
}

crate::shared::string_list_wrapper! {
    /// Normalized typed-ref inputs for resource templates.
    #[schemars(extend("x-ctx-authoring" = "scalar-or-array"))]
    pub struct ResourceInputList
}

/// The `variant` slug marking a resource whose body is typed checklist items.
pub const CHECKLIST_VARIANT: &str = "checklist";

/// One declared checklist item: the unit that receives exactly one verdict.
///
/// `id` is the item's identity and is stable across rewordings of `text` — a
/// reworded criterion keeps its id and changes its digest, which is what lets
/// a receipt distinguish "this item was answered again" from "this item now
/// asks something else". Deriving an id from `text` would collapse that
/// distinction, so ids are declared, never computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct ChecklistItem {
    /// Item identifier, unique within its checklist.
    pub id: String,

    /// The criterion as the reviewer reads it.
    pub text: String,

    /// Elaboration presented alongside `text`. Detail is never separately
    /// verdict-bearing: one item is one verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,

    /// Whether a verdict for this item must carry evidence. When any item in
    /// a checklist requires it, the synthesized verdict schema makes the
    /// `evidence` field required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_evidence: Option<bool>,
}

impl ChecklistItem {
    /// Whether a verdict for this item must cite evidence. Defaults to `false`.
    pub fn requires_evidence(&self) -> bool {
        self.requires_evidence.unwrap_or(false)
    }
}

/// An `[[resource]]` declaration: an external blob reference or inline text.
///
/// File-backed resources are materialized from IO evidence. Inline content is
/// materialized entirely within core without filesystem access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Resource {
    /// Resource identifier (e.g. `"scope-schema"`).
    pub id: String,

    /// Path to the resource blob, relative to `root`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// The digest of this resource's file bytes, in `sha256:<hex>` form.
    ///
    /// COMPUTED at build time, never authored: `ctx traits build` reads the
    /// file and writes this. It sat here before as a hand-typed pin, which is
    /// why nothing ever carried one — an author had to paste a hex string
    /// that went stale on the next legitimate edit.
    ///
    /// It lives in the canonical rather than the lock because it has to
    /// travel with the resource that declares it. Evidence kept anywhere else
    /// gets separated from the bytes it describes: a package materialized
    /// into the built-in store, vendored into a consumer, or written by hand
    /// each loses it a different way, and the verifier then has nothing to
    /// compare against.
    ///
    /// The cost is deliberate. Editing a protected resource changes the
    /// trait's canonical digest, which moves this machine's trust decision to
    /// unreviewed — correct, because a resource is content a model reads, and
    /// changing it changes what the trait does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Digest>,

    /// Whether this resource's bytes are verified against `digest`.
    ///
    /// `None` means "derive from `root`", which is what almost every
    /// declaration wants: a package-owned resource ships with the trait, so
    /// its bytes are the trait's own and are verified; a `root = "repo"`
    /// resource is an input the consuming project supplies — a task board
    /// differs in every repository and changes between runs — so there is
    /// nothing stable to verify it against.
    ///
    /// Set it explicitly only to disagree: `false` on a package resource that
    /// legitimately churns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected: Option<bool>,

    /// Which root the declared `path` is relative to. Defaults to `package`
    /// and is omitted from canonical serialization when default, so existing
    /// package-relative declarations stay byte-identical.
    #[serde(default, skip_serializing_if = "ResourceRoot::is_package")]
    pub root: ResourceRoot,

    /// Inline resource body. Inline resources require no host IO.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Advisory hint about the resource contents or usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,

    /// Optional resource kind. `template` resources may declare typed inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,

    /// Typed inputs required by a resource template.
    #[serde(default, skip_serializing_if = "ResourceInputList::is_empty")]
    pub input: ResourceInputList,

    /// Declared checklist items. Valid only alongside `variant = "checklist"`,
    /// where they replace `path`/`content` as the resource body: the items are
    /// canonical and the presented text is rendered from them, so no prose
    /// copy of the checklist can drift from the typed one.
    #[serde(default, rename = "item", skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ChecklistItem>,

    /// When to load the resource at runtime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<ResourceTrigger>,

    /// How this resource's body is presented to the model. Defaults to
    /// `reference` for path-backed resources and `inline` for content or
    /// checklist resources (see `effective_render()`); omitted from
    /// canonical serialization when unset, so existing declarations stay
    /// byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render: Option<ResourceRender>,
}

impl Resource {
    /// The root this resource's declared path resolves against.
    pub fn effective_root(&self) -> ResourceRoot {
        self.root
    }

    /// The trigger governing this resource's inclusion, defaulting an
    /// omitted declaration to `on-demand`. Every consumer that branches on
    /// trigger semantics (planning, body scanning, digesting) must read
    /// through this method rather than matching `self.trigger` directly, so
    /// the omission default cannot drift between them.
    pub fn effective_trigger(&self) -> ResourceTrigger {
        self.trigger.unwrap_or(ResourceTrigger::OnDemand)
    }

    /// The presentation mode governing this resource's body, defaulting an
    /// omitted declaration to source-aware defaults: `reference` for a
    /// path-backed resource, `inline` for a `content` or checklist resource.
    /// Every consumer that decides whether to open a path or embed a body
    /// must read through this method rather than inferring presentation from
    /// `path` vs `content`, so the default cannot drift between them.
    pub fn effective_render(&self) -> ResourceRender {
        self.render.unwrap_or(if self.path.is_some() {
            ResourceRender::Reference
        } else {
            ResourceRender::Inline
        })
    }

    /// Whether this resource's body is typed checklist items.
    pub fn is_checklist(&self) -> bool {
        self.variant.as_deref() == Some(CHECKLIST_VARIANT)
    }

    /// Whether this resource's bytes are verified at every point of use.
    ///
    /// Only a path-backed resource can be: an inline or checklist body is
    /// already canonical bytes with no file to compare against.
    pub fn is_protected(&self) -> bool {
        self.path.is_some() && self.protected.unwrap_or(self.root.is_package())
    }

    /// The declared item ids, in declaration order. Empty for non-checklists.
    ///
    /// Declaration order is the presentation order and the order the
    /// synthesized verdict schema lists in `allowed`, so it is part of the
    /// canonical document rather than an incidental detail.
    pub fn checklist_item_ids(&self) -> Vec<&str> {
        self.items.iter().map(|item| item.id.as_str()).collect()
    }

    /// Whether any declared item requires its verdict to cite evidence.
    pub fn checklist_requires_evidence(&self) -> bool {
        self.items.iter().any(ChecklistItem::requires_evidence)
    }
}

/// Validate the body of a `variant = "checklist"` resource.
///
/// Checklist items are the body, so `path` and `content` are rejected rather
/// than merged: two sources of the same criteria is exactly the drift the
/// typed form exists to remove.
fn validate_checklist_body(resource: &Resource, i: usize) -> crate::Result<()> {
    validate_render_for_pathless_body(resource, i)?;

    if resource.path.is_some() || resource.content.is_some() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("resource[{i}]"),
            message: format!(
                "a {CHECKLIST_VARIANT:?} resource declares item entries as its body; path and content are not allowed"
            ),
        }
        .into());
    }

    if resource.items.is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("resource[{i}].item"),
            message: format!("a {CHECKLIST_VARIANT:?} resource must declare at least one item")
                .to_string(),
        }
        .into());
    }

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (j, item) in resource.items.iter().enumerate() {
        let id_path = format!("resource[{i}].item[{j}].id");
        crate::shared::validate_slug_shape(&item.id, &id_path)?;

        if !seen.insert(item.id.as_str()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate checklist item id {:?}", item.id),
            }
            .into());
        }

        if item.text.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("resource[{i}].item[{j}].text"),
                message: "must not be empty".to_string(),
            }
            .into());
        }
    }

    Ok(())
}

/// Validate that a pathless (`content` or checklist) resource does not
/// declare `render = "reference"`: with no path, there is nothing for a
/// reference to point at.
fn validate_render_for_pathless_body(resource: &Resource, i: usize) -> crate::Result<()> {
    if resource.render == Some(ResourceRender::Reference) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("resource[{i}].render"),
            message: "a resource with no path cannot declare render = \"reference\"; content and checklist resources are inline-only".to_string(),
        }
        .into());
    }
    Ok(())
}

/// Validate a list of resource declarations.
///
/// Checks:
/// - IDs are valid slugs and unique
/// - Paths are non-empty, relative to their declared root, and safe (no
///   absolute paths, no `..` traversal, no backslash separators, no empty
///   segments)
/// - Trigger values are valid enum variants (enforced by serde deserialization)
pub fn validate_resources(resources: &[Resource]) -> crate::Result<()> {
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (i, resource) in resources.iter().enumerate() {
        let id_path = format!("resource[{i}].id");
        crate::shared::validate_slug_shape(&resource.id, &id_path)?;

        // `protected` is only meaningful for a path-backed resource: an
        // inline or checklist body is already canonical bytes, so there is no
        // file whose drift it could describe. Declaring it there is an
        // authoring mistake worth naming rather than a no-op to ignore.
        if resource.protected.is_some() && resource.path.is_none() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("resource[{i}].protected"),
                message: "only a path-backed resource can be protected; inline and checklist \
                     resources have no filesystem bytes to verify"
                    .to_string(),
            }
            .into());
        }

        // A resource carries exactly one body form: a path, inline content, or
        // typed checklist items. The checklist form is gated on the variant so
        // `item` can never be a silently-ignored field on an ordinary resource.
        if resource.is_checklist() {
            validate_checklist_body(resource, i)?;
        } else {
            if !resource.items.is_empty() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("resource[{i}].item"),
                    message: format!(
                        "checklist items require variant = {CHECKLIST_VARIANT:?}; got variant {:?}",
                        resource.variant.as_deref().unwrap_or("<none>")
                    ),
                }
                .into());
            }

            match (&resource.path, &resource.content) {
                (Some(_), Some(_)) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("resource[{i}]"),
                        message: "expected exactly one of path or content, got both".to_string(),
                    }
                    .into());
                }
                (None, None) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("resource[{i}]"),
                        message: "expected exactly one of path or content, got neither".to_string(),
                    }
                    .into());
                }
                (Some(path), None) => {
                    if path.trim().is_empty() {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path: format!("resource[{i}].path"),
                            message: "must not be empty".to_string(),
                        }
                        .into());
                    }
                    validate_resource_path(path, i)?;
                }
                (None, Some(_)) => {
                    if resource.root != ResourceRoot::Package {
                        return Err(crate::manifest::Error::InvalidField {
                            field_path: format!("resource[{i}].root"),
                            message: "root is only meaningful for path-backed resources; inline resources must not declare a non-package root".to_string(),
                        }
                        .into());
                    }
                    validate_render_for_pathless_body(resource, i)?;
                }
            }
        }

        if !seen.insert(resource.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate resource id {:?}", resource.id),
            }
            .into());
        }

        if let Some(variant) = &resource.variant {
            crate::shared::validate_slug_shape(variant, &format!("resource[{i}].variant"))?;
        }

        for (j, ref_text) in resource.input.iter().enumerate() {
            let parsed =
                Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                    field_path: format!("resource[{i}].input[{j}]"),
                    message: format!("invalid typed ref {ref_text:?}"),
                })?;
            if !matches!(parsed.kind(), Kind::Port | Kind::Slot | Kind::Resource) {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("resource[{i}].input[{j}]"),
                    message: format!(
                        "resource template input kind {:?} not allowed; expected port, slot, or resource",
                        parsed.kind()
                    ),
                }.into());
            }
        }
    }

    Ok(())
}

/// Validate template input refs against local declarations and detect local
/// resource-input cycles. Dependency-qualified refs remain pending evidence.
pub fn validate_resource_template_refs(
    resources: &[Resource],
    port_ids: &BTreeSet<&str>,
    slot_ids: &BTreeSet<&str>,
) -> crate::Result<()> {
    let resource_ids: BTreeSet<&str> = resources
        .iter()
        .map(|resource| resource.id.as_str())
        .collect();

    for (i, resource) in resources.iter().enumerate() {
        for (j, ref_text) in resource.input.iter().enumerate() {
            let parsed =
                Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
                    field_path: format!("resource[{i}].input[{j}]"),
                    message: format!("invalid typed ref {ref_text:?}"),
                })?;
            if parsed.is_qualified() {
                continue;
            }
            match parsed.kind() {
                Kind::Port if !port_ids.contains(parsed.id()) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("resource[{i}].input[{j}]"),
                        message: format!(
                            "resource input references undeclared port {:?}",
                            parsed.id()
                        ),
                    }
                    .into());
                }
                Kind::Slot if !slot_ids.contains(parsed.id()) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("resource[{i}].input[{j}]"),
                        message: format!(
                            "resource input references undeclared slot {:?}",
                            parsed.id()
                        ),
                    }
                    .into());
                }
                Kind::Resource if !resource_ids.contains(parsed.id()) => {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("resource[{i}].input[{j}]"),
                        message: format!(
                            "resource input references undeclared resource {:?}",
                            parsed.id()
                        ),
                    }
                    .into());
                }
                _ => {}
            }
        }
    }

    if let Some(cycle) = local_resource_cycle(resources) {
        let id_segment = cycle.ids.join(" -> ");
        let edge_details: Vec<String> = cycle
            .edges
            .iter()
            .map(|edge| format!("{}={}", edge.field_path, edge.ref_text))
            .collect();
        let primary_field = cycle
            .edges
            .first()
            .map(|e| e.field_path.clone())
            .unwrap_or_else(|| "resource.input".to_string());
        return Err(crate::manifest::Error::InvalidField {
            field_path: primary_field,
            message: format!(
                "resource template input cycle detected: {id_segment}; edges: {}",
                edge_details.join("; ")
            ),
        }
        .into());
    }

    Ok(())
}

#[derive(Clone)]
struct ResourceCycleEdge {
    field_path: String,
    ref_text: String,
}

struct ResourceCycle {
    ids: Vec<String>,
    edges: Vec<ResourceCycleEdge>,
}

struct ResourceGraph<'a> {
    resources: &'a [Resource],
    resource_ids: BTreeSet<&'a str>,
    decl_index: std::collections::BTreeMap<&'a str, usize>,
}

fn local_resource_cycle(resources: &[Resource]) -> Option<ResourceCycle> {
    let resource_ids: BTreeSet<&str> = resources
        .iter()
        .map(|resource| resource.id.as_str())
        .collect();
    let decl_index: std::collections::BTreeMap<&str, usize> = resources
        .iter()
        .enumerate()
        .map(|(i, r)| (r.id.as_str(), i))
        .collect();
    let graph = ResourceGraph {
        resources,
        resource_ids,
        decl_index,
    };
    for resource in resources {
        let mut id_stack = Vec::new();
        let mut edge_stack = Vec::new();
        if let Some(cycle) =
            visit_resource(resource.id.as_str(), &graph, &mut id_stack, &mut edge_stack)
        {
            return Some(cycle);
        }
    }
    None
}

fn visit_resource(
    id: &str,
    graph: &ResourceGraph<'_>,
    id_stack: &mut Vec<String>,
    edge_stack: &mut Vec<ResourceCycleEdge>,
) -> Option<ResourceCycle> {
    if let Some(position) = id_stack.iter().position(|existing| existing == id) {
        let cycle_ids = id_stack[position..]
            .iter()
            .cloned()
            .chain(std::iter::once(id.to_string()))
            .collect::<Vec<_>>();
        let cycle_edges = edge_stack[position..].to_vec();
        return Some(ResourceCycle {
            ids: cycle_ids,
            edges: cycle_edges,
        });
    }
    let resource = graph.resources.iter().find(|resource| resource.id == id)?;
    let from_decl_idx = graph.decl_index.get(id).copied().unwrap_or(0);
    id_stack.push(id.to_string());
    for (j, ref_text) in resource.input.iter().enumerate() {
        let Ok(parsed) = Reference::parse(ref_text) else {
            continue;
        };
        if parsed.kind() == Kind::Resource
            && !parsed.is_qualified()
            && graph.resource_ids.contains(parsed.id())
        {
            edge_stack.push(ResourceCycleEdge {
                field_path: format!("resource[{from_decl_idx}].input[{j}]"),
                ref_text: ref_text.clone(),
            });
            if let Some(cycle) = visit_resource(parsed.id(), graph, id_stack, edge_stack) {
                return Some(cycle);
            }
            edge_stack.pop();
        }
    }
    id_stack.pop();
    None
}

/// Validate that a resource path is relative to its declared root and safe.
///
/// Rejects, regardless of the declared root:
/// - Absolute paths (starting with `/` or a Windows drive prefix)
/// - Paths containing `..` (parent traversal)
/// - Paths containing backslash separators (use forward slash only)
/// - Paths with empty segments (`//`, leading/trailing `/`)
fn validate_resource_path(path: &str, resource_index: usize) -> crate::Result<()> {
    let field_path = format!("resource[{resource_index}].path");

    if path.starts_with('/') {
        return Err(crate::manifest::Error::InvalidField {
            field_path,
            message: "must be relative to its declared root, not absolute".to_string(),
        }
        .into());
    }

    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        return Err(crate::manifest::Error::InvalidField {
            field_path,
            message: "must be relative to its declared root, not a Windows drive path".to_string(),
        }
        .into());
    }

    if path.contains('\\') {
        return Err(crate::manifest::Error::InvalidField {
            field_path,
            message: "must use forward slash separators, not backslashes".to_string(),
        }
        .into());
    }

    for segment in path.split('/') {
        if segment == ".." {
            return Err(crate::manifest::Error::InvalidField {
                field_path,
                message: "must not contain '..' parent traversal".to_string(),
            }
            .into());
        }
        if segment.is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path,
                message: "must not contain empty path segments".to_string(),
            }
            .into());
        }
    }

    Ok(())
}
