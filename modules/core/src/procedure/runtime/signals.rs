// Procedure runtime signals.
// Procedure runtime signal handling.

fn validate_signal_with_context(
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
    let accepted =
        { parsed.kind() == Kind::Signal && allowed_signals.contains(signal.ref_text.as_str()) };
    Ok(SignalEmission {
        signal_ref: parsed,
        sequence_index,
        evidence_digest,
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
        reason: if accepted {
            "signal is declared in current item emits".to_string()
        } else {
            "signal is not declared in current item emits or is not a valid signal:* ref"
                .to_string()
        },
    })
}
