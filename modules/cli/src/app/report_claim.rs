//! Claim gate report command.

use crate::app::entry::print_json_report;
use ctx_traits_core::response::CommandOutput;

pub(crate) fn handle_claim_gate(json: bool) -> crate::Result<CommandOutput<()>> {
    let report = ctx_traits_core::launch::claim_evidence_matrix();
    if json {
        print_json_report(&report, "claim gate")?;
    } else {
        println!("ctx traits internal claim-gate");
        println!("  summary: {}", report.summary);
        println!("  blocked-claims: {}", report.blocked_count);
        for row in &report.rows {
            println!(
                "  - {}: {} / {}; wording: {}",
                row.claim, row.implementation_status, row.source_review_status, row.allowed_wording
            );
        }
    }
    Ok(CommandOutput::new(()))
}
