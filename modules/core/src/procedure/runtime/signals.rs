// Procedure runtime signals.
// Procedure runtime signal handling.

// This boundary receives the procedure position and caller provenance separately;
// grouping them would obscure their independently persisted ledger fields.
#[allow(clippy::too_many_arguments)]
fn validate_signal_with_context(
    trait_ref: &Trait,
    sequence_index: usize,
    allowed_signals: &BTreeSet<&str>,
    signal: StepSignalOutput,
    position_path: &[PathSegment],
    loop_context: Option<&LoopContext>,
    for_each_context: Option<&ForEachContext>,
    source: Option<SignalSource>,
) -> crate::Result<SignalEmission> {
    let evidence = signal
        .evidence
        .as_deref()
        .map_or(signal.ref_text.as_str(), |s| s);
    let evidence_digest = Digest::source(evidence);
    let parsed = Reference::parse(&signal.ref_text)?;
    let declared = if parsed.is_qualified() {
        None
    } else {
        trait_ref.signals.iter().find(|declared| declared.id == parsed.base_id())
    };
    let allowed =
        { parsed.kind() == Kind::Signal && allowed_signals.contains(signal.ref_text.as_str()) };
    let (accepted, reason, payload_digest, schema_validation) = if !allowed {
        (
            false,
            "signal is not declared in current item on-complete or is not a valid signal:* ref".to_string(),
            None,
            Vec::new(),
        )
    } else if parsed.is_qualified() && signal.payload.is_some() {
        let reason = format!(
            "signal:{} payload schema validation is unsupported for dependency signals",
            parsed.id()
        );
        (
            false,
            reason.clone(),
            None,
            vec![SchemaValidation {
                ref_text: signal.ref_text.clone(),
                schema_ref: None,
                status: SchemaStatus::Unsupported,
                reason,
            }],
        )
    } else if let Some(schema) = declared.and_then(|signal| signal.schema.as_ref()) {
        let schema_ref = schema.to_string();
        match signal.payload.as_ref() {
            None => (
                false,
                format!("signal:{} requires a payload matching {schema_ref}", parsed.id()),
                None,
                Vec::new(),
            ),
            Some(payload) => {
                let validation =
                    validate_value_schema(trait_ref, &signal.ref_text, &schema_ref, payload)?;
                let accepted = validation.status == SchemaStatus::Accepted;
                let reason = if accepted {
                    "signal is declared in current item on-complete".to_string()
                } else {
                    validation.reason.clone()
                };
                (
                    accepted,
                    reason,
                    accepted.then(|| value_digest(payload)).transpose()?,
                    vec![validation],
                )
            }
        }
    } else if signal.payload.is_some() {
        (
            false,
            format!("signal:{} declares no payload schema", parsed.id()),
            None,
            Vec::new(),
        )
    } else {
        (
            true,
            "signal is declared in current item on-complete".to_string(),
            None,
            Vec::new(),
        )
    };
    Ok(SignalEmission {
        signal_ref: parsed,
        emission_order: 0,
        sequence_index,
        evidence_digest,
        payload: signal.payload,
        payload_digest,
        schema_validation,
        position_path: position_path.to_vec(),
        source,
        runtime_control: None,
        loop_id: loop_context.map(|context| context.loop_id.clone()),
        iteration_index: loop_context.map(|context| context.iteration_index),
        for_each_id: for_each_context.map(|context| context.for_each_id.clone()),
        item_index: for_each_context.map(|context| context.item_index),
        producer_agent: signal.producer_agent,
        producer_harness: signal.producer_harness,
        acceptance: if accepted {
            AcceptanceStatus::Accepted
        } else {
            AcceptanceStatus::Rejected
        },
        reason,
    })
}

#[cfg(test)]
mod signal_payload_validation_tests {
    use super::*;
    use serde_json::json;

    fn fixture_trait() -> Trait {
        crate::encoding::decode_trait(
            crate::encoding::Encoding::Toml,
            r#"
id = "signal-payload-runtime"
schema-version = "0.3"
version = "0.1.0"
name = "Signal payload runtime"
description = "Test fixture."

[[signal]]
id = "review"
description = "Review result."
schema = "schema:text"

[[signal]]
id = "bare"
description = "Legacy signal."
"#,
        )
        .expect("fixture trait decodes")
    }

    fn validate(ref_text: &str, payload: Option<JsonValue>) -> SignalEmission {
        let trait_ref = fixture_trait();
        validate_signal_with_context(
            &trait_ref,
            0,
            &BTreeSet::from([ref_text]),
            StepSignalOutput {
                ref_text: ref_text.to_string(),
                evidence: None,
                payload,
                producer_agent: None,
                producer_harness: None,
            },
            &[],
            None,
            None,
            None,
        )
        .expect("signal validation completes")
    }

    #[test]
    fn payload_boundary_acceptance_matrix_preserves_legacy_bare_signals() {
        let accepted = validate("signal:review", Some(json!("ready")));
        assert_eq!(accepted.acceptance, AcceptanceStatus::Accepted);
        assert_eq!(accepted.payload_digest, Some(value_digest(&json!("ready")).unwrap()));
        assert_eq!(accepted.schema_validation[0].status, SchemaStatus::Accepted);

        let missing = validate("signal:review", None);
        assert_eq!(missing.acceptance, AcceptanceStatus::Rejected);
        assert!(missing.reason.contains("requires a payload matching schema:text"));

        let invalid = validate("signal:review", Some(json!(false)));
        assert_eq!(invalid.acceptance, AcceptanceStatus::Rejected);
        assert_eq!(invalid.schema_validation[0].status, SchemaStatus::Rejected);
        assert!(invalid.reason.contains("expected JSON string"));

        let unexpected = validate("signal:bare", Some(json!("not allowed")));
        assert_eq!(unexpected.acceptance, AcceptanceStatus::Rejected);
        assert!(unexpected.reason.contains("declares no payload schema"));

        let legacy = validate("signal:bare", None);
        assert_eq!(legacy.acceptance, AcceptanceStatus::Accepted);
        assert!(legacy.payload.is_none() && legacy.payload_digest.is_none());
    }

    #[test]
    fn qualified_payload_is_unsupported_while_payload_free_is_legacy_compatible() {
        let payload = validate("signal:dep/review", Some(json!("ready")));
        assert_eq!(payload.acceptance, AcceptanceStatus::Rejected);
        assert_eq!(payload.schema_validation[0].status, SchemaStatus::Unsupported);
        assert!(payload.schema_validation[0]
            .reason
            .contains("dependency signals"));

        let bare = validate("signal:dep/review", None);
        assert_eq!(bare.acceptance, AcceptanceStatus::Accepted);
        assert!(bare.schema_validation.is_empty());
    }
}
