//! Process entry boundary for the CLI.

pub use super::command_handlers::run;

// Keep shared command helpers available while command families continue their
// incremental extraction into sibling modules.
pub(crate) use super::command_handlers::{
    apply_assist_check_drift, attach_assist_check_report, build_file_evidence_from_io,
    print_assist_candidate, print_json_report, print_lock_update, print_synth_provenance,
    resolve_repo_root, safe_assist_context_text, to_json_value, trait_package_output_paths,
};
