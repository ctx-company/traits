/// Defines import lockfile planning data.
/// Import lockfile planning.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ArtifactClassification {
    /// UTF-8 text file safe for inline embedding.
    Text,
    /// Binary file requiring base64 encoding.
    Binary,
    /// Special file, symlink, or unsafe path that blocks reproducible import.
    Special,
}

/// Embedded content of an imported artifact in a `trait.lock` snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ArtifactContent {
    /// Literal UTF-8 text content for text files.
    Text { text: String },
    /// Base64-encoded content for binary files.
    Base64 { data: String },
    /// Content could not be embedded safely; import/refresh is blocked.
    Blocked { reason: String },
}

/// One imported artifact recorded in a `trait.lock` snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitLockArtifact {
    /// Normalized relative path inside the imported artifact set.
    pub normalized_path: String,
    /// Original source URI or path if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_source_uri: Option<String>,
    /// SHA-256 digest of the raw file bytes.
    pub byte_digest: Digest,
    /// File size in bytes.
    pub byte_size: u64,
    /// Text/binary/special classification.
    pub file_classification: ArtifactClassification,
    /// Media/profile guess: `skill-md`, `markdown`, `yaml-frontmatter`, `resource`, `unknown`.
    pub media_guess: String,
    /// Symlink/special-file/missing-file warnings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Whether the artifact participated in conversion or was ancillary only.
    pub participated_in_conversion: bool,
    /// Embedded content (literal text, base64, or blocked diagnostic).
    pub content: ArtifactContent,
}

/// Metadata for one import snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitLockSnapshotMetadata {
    /// Local source root/file locator for refresh. When absent, refresh
    /// requires an explicit `--source` override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_locator: Option<String>,
    /// Frontmatter mapping evidence from P84.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmatter_mapping: Option<FrontmatterEvidence>,
    /// Multi-file graph digest from P92 when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_digest: Option<Digest>,
    /// Multi-file source path to canonical resource ID mappings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_mappings: Vec<ResourceIdMapping>,
    /// Remote source evidence from P93 when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_source: Option<RemoteSourceEvidence>,
}

/// One immutable snapshot of imported source artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitLockSnapshot {
    /// Deterministic digest of the artifact set.
    pub snapshot_digest: Digest,
    /// Source profile: `agent-skills`, future `agents-md`, etc.
    pub source_profile: String,
    /// Import command/profile version.
    pub import_command_version: String,
    /// Canonical output digest created from this snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_output_digest: Option<Digest>,
    /// Artifact entries with embedded content.
    pub artifacts: Vec<TraitLockArtifact>,
    /// Snapshot metadata.
    pub metadata: TraitLockSnapshotMetadata,
}

/// Import evidence nested in a package-local `trait.lock`.
///
/// Pins exact imported artifact bytes and conversion evidence alongside the
/// package's dependency and digest evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitLock {
    /// Lock schema version.
    pub schema_version: String,
    /// Trait ID this lock belongs to.
    pub trait_id: String,
    /// Digest of the current snapshot, or `None` if no import has completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_snapshot_digest: Option<Digest>,
    /// Immutable snapshots keyed by snapshot digest. Old entries are never
    /// deleted automatically.
    pub snapshots: BTreeMap<String, TraitLockSnapshot>,
}

impl TraitLock {
    /// Create a new empty lock for a trait ID.
    pub fn new(trait_id: impl Into<String>) -> Self {
        Self {
            schema_version: TRAIT_LOCK_SCHEMA_VERSION.to_string(),
            trait_id: trait_id.into(),
            current_snapshot_digest: None,
            snapshots: BTreeMap::new(),
        }
    }

    /// Return the current snapshot if one exists.
    pub fn current_snapshot(&self) -> Option<&TraitLockSnapshot> {
        self.current_snapshot_digest
            .as_ref()
            .and_then(|digest| self.snapshots.get(digest.as_str()))
    }

    /// Insert a new snapshot and set it as current. Old snapshots are preserved.
    pub fn insert_snapshot(&mut self, snapshot: TraitLockSnapshot) {
        let digest = snapshot.snapshot_digest.clone();
        self.snapshots.insert(digest.as_str().to_string(), snapshot);
        self.current_snapshot_digest = Some(digest);
    }
}

/// Compute a deterministic source-set digest from a list of artifacts.
pub fn source_set_digest(artifacts: &[TraitLockArtifact]) -> Digest {
    let mut sorted: Vec<&TraitLockArtifact> = artifacts.iter().collect();
    sorted.sort_by(|a, b| a.normalized_path.cmp(&b.normalized_path));
    let mut seed = String::new();
    for artifact in &sorted {
        seed.push_str(&artifact.normalized_path);
        seed.push(':');
        seed.push_str(artifact.byte_digest.as_str());
        seed.push('\n');
    }
    Digest::source(&seed)
}

// ---------------------------------------------------------------------------
// P91: Refresh diff
// ---------------------------------------------------------------------------

/// A modification to one artifact between snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ArtifactModification {
    /// Normalized path of the modified artifact.
    pub path: String,
    /// Previous byte digest.
    pub before_digest: Digest,
    /// New byte digest.
    pub after_digest: Digest,
    /// Previous text/binary/special classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_classification: Option<ArtifactClassification>,
    /// New text/binary/special classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_classification: Option<ArtifactClassification>,
}

/// Artifact-level diff between two import snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ArtifactDiff {
    /// Added artifact paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// Removed artifact paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Modified artifacts with before/after digests.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified: Vec<ArtifactModification>,
}

impl ArtifactDiff {
    /// Whether any artifact changed.
    pub fn has_changes(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty() || !self.modified.is_empty()
    }
}

/// Canonical trait-level diff from a refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitDiff {
    /// Whether canonical output changed.
    pub canonical_changed: bool,
    /// Human-readable field change descriptions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_changes: Vec<String>,
    /// Summary of the trait-level diff.
    pub summary: String,
    /// Previous canonical digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_canonical_digest: Option<Digest>,
    /// New canonical digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_canonical_digest: Option<Digest>,
}

/// One mapping attribution entry in a refresh diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct MappingDiffEntry {
    /// Which canonical field changed.
    pub canonical_field: String,
    /// Source artifact/path that caused the change, or `unknown`.
    pub source_attribution: String,
}

/// Overall decision from a refresh diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum RefreshDecision {
    /// No artifact or canonical changes detected.
    NoChange,
    /// Source artifacts changed but canonical output did not.
    SourceOnlyChange,
    /// Canonical trait output changed.
    TraitChange,
    /// A blocking issue prevents refresh apply.
    Blocked,
    /// Refresh requires human review before applying.
    NeedsReview,
    /// Refresh is unsupported for this source/profile.
    Unsupported,
}

/// Dual-layer refresh diff report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RefreshDiffReport {
    /// Trait ID.
    pub trait_id: String,
    /// Previous snapshot digest.
    pub before_snapshot_digest: Option<Digest>,
    /// New snapshot digest.
    pub after_snapshot_digest: Digest,
    /// Artifact-level diff.
    pub artifact_diff: ArtifactDiff,
    /// Canonical trait-level diff.
    pub trait_diff: TraitDiff,
    /// Mapping attribution entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mapping_diff: Vec<MappingDiffEntry>,
    /// Overall refresh decision.
    pub decision: RefreshDecision,
    /// Warnings (blocked artifacts, converter drift, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}
