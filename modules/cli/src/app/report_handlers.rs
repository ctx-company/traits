//! Compatibility facade for report command routes in `entry`.

pub(crate) use super::report_check::{CheckInputs, build_check_report, handle_check};
pub(crate) use super::report_claim::handle_claim_gate;
pub(crate) use super::report_drift::{handle_diff, load_trait_files};
pub(crate) use super::report_render::{ExportInputs, handle_export, handle_prompt};
