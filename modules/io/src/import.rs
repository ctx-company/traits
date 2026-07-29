//! Import IO facade.
//!
//! Import is split by product concern: raw-source copy/package writes,
//! package-local `trait.lock` snapshots, multi-file skill reading, and remote
//! source classification. Public names are re-exported here for compatibility.

mod copy;
mod lock;
mod remote;
mod skill_source;
mod support;

pub use copy::{
    ImportCopyEntry, ImportCopyPlan, ImportCopyWarning, ImportWriteResult, LoadedAgentSkillSource,
    digest_import_source, plan_import_copy, read_agent_skill_source, stage_package_manifest,
    validate_managed_import_overwrite, write_import_package,
};
pub use lock::{
    build_artifact_snapshot, build_artifact_snapshot_with_locator, read_trait_lock,
    write_trait_lock,
};
pub use remote::{RemoteFetchResult, fetch_remote_source};
pub use skill_source::{
    DoctorDiscovery, DoctorDiscoveryError, DoctorSourceFile, MultiFileSkillSource,
    discover_doctor_sources, read_multi_file_skill_source,
};
