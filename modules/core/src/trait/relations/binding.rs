// Trait relation binding definitions.
/// Trait relation binding definitions.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Compatibility {
    /// Exact schema match.
    Exact,
    /// Provider fields are a superset of consumer required fields.
    ProviderSuperset,
    /// `schema:any` wildcard — compatible with warning.
    AnyWildcard,
    /// Resource-backed schema cannot be compared by pure core.
    IoPending,
    /// Missing required fields or incompatible field schemas.
    Incompatible,
}

/// Status of a binding proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Status {
    /// Proposed by resolver/compose/explain but not yet accepted.
    Proposed,
    /// Explicitly accepted by a user/profile/lock decision.
    Accepted,
    /// Explicitly rejected.
    Rejected,
    /// Stale due to digest/schema/source changes.
    Stale,
}

/// A runtime/project binding proposal between a consumer port and a provider
/// port. Lives outside atomic trait source in a runtime plan, profile, lock,
/// or report artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Proposal {
    pub consumer: PortEndpoint,
    pub provider: PortEndpoint,
    pub compatibility: Compatibility,
    /// Schema evidence text/records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_evidence: Option<String>,
    /// Optional field mapping for provider-superset bindings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_mapping: Vec<FieldMapping>,
    pub status: Status,
    /// Who accepts/rejects the binding. Default is always `proposed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepter: Option<String>,
    pub reason: String,
    /// Stale reasons if status is `stale`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_reasons: Vec<String>,
}

/// One field mapping entry for provider-superset bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FieldMapping {
    pub consumer_field: String,
    pub provider_field: String,
}
