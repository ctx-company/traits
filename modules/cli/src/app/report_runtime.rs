//! Runtime evidence for check reports.

pub(crate) fn read_run_ledger(
    path: Option<&str>,
) -> crate::Result<Option<ctx_traits_core::procedure::runtime::State>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let text = ctx_traits_io::read::read_text(camino::Utf8Path::new(path))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| crate::Error::json(format!("parse run ledger JSON {path}"), e))
}

pub(crate) fn runtime_evidence_section(
    trait_ref: &ctx_traits_core::Trait,
    ledger: &Option<ctx_traits_core::procedure::runtime::State>,
    source_digest: &str,
    canonical_digest: &str,
) -> crate::Result<ctx_traits_core::check::CheckSection> {
    if trait_ref.procedure.is_none() {
        return Ok(ctx_traits_core::check::CheckSection {
            name: "runtime-evidence".to_string(),
            summary: "guidance-only trait; no procedure dry plan".to_string(),
            ok: true,
        });
    }
    let Some(ledger) = ledger else {
        return Ok(ctx_traits_core::check::CheckSection {
            name: "runtime-evidence".to_string(),
            summary: "procedure dry plan available; run ledger absent".to_string(),
            ok: true,
        });
    };
    let stale = ledger.source_digest.as_deref() != Some(source_digest)
        || ledger.canonical_digest.as_deref() != Some(canonical_digest);
    let trait_mismatch = ledger.trait_id != trait_ref.id.as_str();
    let validation =
        ctx_traits_core::procedure::runtime::validate_run_ledger_contract(trait_ref, ledger)?;
    let ok = !stale
        && !trait_mismatch
        && validation.status
            == ctx_traits_core::procedure::runtime::LedgerContractStatus::ValidCompleted;
    let state_summary = if stale {
        "run ledger stale"
    } else if trait_mismatch {
        "run ledger trait-mismatched"
    } else if !validation.contract_valid {
        "run ledger contract invalid"
    } else {
        match validation.status {
            ctx_traits_core::procedure::runtime::LedgerContractStatus::ValidCompleted => {
                "run ledger completed"
            }
            ctx_traits_core::procedure::runtime::LedgerContractStatus::ValidRunning => {
                "run ledger running/incomplete"
            }
            ctx_traits_core::procedure::runtime::LedgerContractStatus::ValidBlocked => {
                "run ledger blocked"
            }
            ctx_traits_core::procedure::runtime::LedgerContractStatus::ValidRejected => {
                "run ledger rejected"
            }
            ctx_traits_core::procedure::runtime::LedgerContractStatus::ValidFailed => {
                "run ledger failed"
            }
            ctx_traits_core::procedure::runtime::LedgerContractStatus::Invalid => {
                "run ledger contract invalid"
            }
        }
    };
    let diagnostics = if validation.diagnostics.is_empty() {
        "none".to_string()
    } else {
        validation.diagnostics.join("; ")
    };
    Ok(ctx_traits_core::check::CheckSection {
        name: "runtime-evidence".to_string(),
        summary: format!(
            "{state_summary}; stale={}, trait-mismatch={}, contract-valid={}, accepted-slots={}, rejected-attempts={}, output-ports={}, diagnostics={}",
            stale,
            trait_mismatch,
            validation.contract_valid,
            ledger.accepted_slot_values.len(),
            ledger.rejected_attempts.len(),
            ledger.output_ports.len(),
            diagnostics,
        ),
        ok,
    })
}
