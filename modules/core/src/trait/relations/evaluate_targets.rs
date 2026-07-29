// Trait relation target evaluation.
/// Trait relation target evaluation.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum PortTargetOutcomeKind {
    /// `when` conditions not matched.
    WhenUnmatched,
    /// Target port does not exist on the source trait.
    UnavailableTarget,
    /// Target port exists but is output-direction (requires needs input).
    WrongDirection,
    /// Valid input-direction port; creates a binding requirement.
    Requirement,
    /// Valid input-direction port with at least one compatible provider.
    #[serde(rename = "binding-proposed")]
    #[schemars(rename = "binding-proposed")]
    Proposed,
    /// Suggests relation: advisory port evidence only.
    Advisory,
}

/// Structured outcome for a `port:*` target relation edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct PortTargetOutcome {
    pub kind: PortTargetOutcomeKind,
    pub source_trait_id: String,
    pub target_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port_id: Option<String>,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binding_proposal_keys: Vec<String>,
}

/// Evaluate a `port:*` relation target and produce structured outcomes.
///
/// - Parses `target_ref` and requires local unqualified `Kind::Port`.
/// - Finds that exact port on the source trait.
/// - For `requires`: `UnavailableTarget` if missing, `WrongDirection` if
///   output-direction, `Requirement` if input-direction with no providers,
///   `Proposed` if input-direction with compatible providers.
/// - For `suggests`: `Advisory` for a valid local port.
///
/// Binding proposals are scoped to exactly the target consumer input port.
fn evaluate_port_target(
    kind: EdgeKind,
    source_trait_id: &str,
    target_ref: &str,
    when_matched: bool,
    traits: &[Trait],
    binding_proposals: &mut Vec<Proposal>,
) -> Vec<PortTargetOutcome> {
    if !when_matched {
        return vec![PortTargetOutcome {
            kind: PortTargetOutcomeKind::WhenUnmatched,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: None,
            reason: "when conditions not matched".to_string(),
            binding_proposal_keys: Vec::new(),
        }];
    }

    let Ok(parsed) = Reference::parse(target_ref) else {
        return vec![PortTargetOutcome {
            kind: PortTargetOutcomeKind::UnavailableTarget,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: None,
            reason: format!("invalid port target ref {target_ref:?}"),
            binding_proposal_keys: Vec::new(),
        }];
    };

    if parsed.kind() != Kind::Port || parsed.is_qualified() {
        return vec![PortTargetOutcome {
            kind: PortTargetOutcomeKind::UnavailableTarget,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: Some(parsed.id().to_string()),
            reason: "port target must be a local unqualified port ref".to_string(),
            binding_proposal_keys: Vec::new(),
        }];
    }

    let port_id = parsed.id();
    let Some(source_trait) = traits.iter().find(|t| t.id.as_str() == source_trait_id) else {
        return vec![PortTargetOutcome {
            kind: PortTargetOutcomeKind::UnavailableTarget,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: Some(port_id.to_string()),
            reason: "source trait not found among loaded traits".to_string(),
            binding_proposal_keys: Vec::new(),
        }];
    };

    let Some(port) = source_trait.ports.iter().find(|p| p.id == port_id) else {
        return vec![PortTargetOutcome {
            kind: PortTargetOutcomeKind::UnavailableTarget,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: Some(port_id.to_string()),
            reason: format!("port {port_id:?} not declared on source trait"),
            binding_proposal_keys: Vec::new(),
        }];
    };

    // Suggests: advisory only.
    if kind == EdgeKind::Suggests {
        return vec![PortTargetOutcome {
            kind: PortTargetOutcomeKind::Advisory,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: Some(port_id.to_string()),
            reason: "suggests relation: advisory port evidence only".to_string(),
            binding_proposal_keys: Vec::new(),
        }];
    }

    // Requires: check direction.
    if !matches!(port.direction, crate::r#trait::PortDirection::Input) {
        return vec![PortTargetOutcome {
            kind: PortTargetOutcomeKind::WrongDirection,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: Some(port_id.to_string()),
            reason: format!("port {port_id:?} is output-direction; requires needs input-direction"),
            binding_proposal_keys: Vec::new(),
        }];
    }

    // Find compatible providers scoped to this exact port first.
    let mut proposal_keys = Vec::new();
    for provider in traits {
        if provider.id.as_str() == source_trait_id {
            continue;
        }
        let scoped_proposals = produce_proposals_for_port(source_trait, provider, port_id);
        for proposal in scoped_proposals {
            if proposal.compatibility == Compatibility::Incompatible {
                continue;
            }
            let key = format!(
                "{}:{} -> {}:{}",
                proposal
                    .consumer
                    .port_ref
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("port:{}", proposal.consumer.port_id)),
                proposal.consumer.trait_id,
                proposal
                    .provider
                    .port_ref
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("port:{}", proposal.provider.port_id)),
                proposal.provider.trait_id
            );
            proposal_keys.push(key);
            binding_proposals.push(proposal);
        }
    }

    // Emit Requirement evidence with reason reflecting provider availability.
    let mut outcomes = vec![PortTargetOutcome {
        kind: PortTargetOutcomeKind::Requirement,
        source_trait_id: source_trait_id.to_string(),
        target_ref: target_ref.to_string(),
        target_port_id: Some(port_id.to_string()),
        reason: if proposal_keys.is_empty() {
            format!(
                "input port {port_id:?} is required and has no compatible providers among loaded traits"
            )
        } else {
            format!("input port {port_id:?} is required and must be bound to a provider")
        },
        binding_proposal_keys: Vec::new(),
    }];

    // If providers were found, also emit proposed-binding evidence.
    if !proposal_keys.is_empty() {
        outcomes.push(PortTargetOutcome {
            kind: PortTargetOutcomeKind::Proposed,
            source_trait_id: source_trait_id.to_string(),
            target_ref: target_ref.to_string(),
            target_port_id: Some(port_id.to_string()),
            reason: format!(
                "input port {port_id:?} has {} compatible provider proposal(s)",
                proposal_keys.len()
            ),
            binding_proposal_keys: proposal_keys,
        });
    }

    outcomes
}
