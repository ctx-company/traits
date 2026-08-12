//! Project manifest model.
//!
//! The repo-level `.ctx/traits.toml` project manifest declares which traits a
//! repo uses. TOML, JSON, YAML, and YML are alternate encodings of one
//! canonical schema. TOML is preferred for human authoring.
//!
//! All field names are kebab-case. Scalar-or-array authoring sugar is handled
//! by `OneOrMany` field types from `shared` on domain struct fields directly.

use thiserror::Error as ThisError;

mod dependency;
mod package;
mod project;
mod source;

pub use dependency::{Dependency, validate_dependencies};
pub use package::{
    ManifestBudget, ManifestDefaults, PackageDependencySpec, PackageManifest, PackageMetadata,
    PackageStatus, PackageVariantConfig, decode_package_manifest,
};
pub use project::{ProjectManifest, ProjectMetadata, ProjectPackageDependency, TraitEntry};
pub use source::TraitSource;

/// Manifest identity and field-shape errors.
#[derive(Debug, ThisError)]
pub enum Error {
    // Manifest identity.
    #[error("duplicate manifest at {path}")]
    DuplicateManifest { path: String },

    #[error("duplicate identifier: {id}")]
    DuplicateId { id: String },

    // Manifest field validation.
    #[error("invalid manifest at {field_path}: {message}")]
    InvalidField { field_path: String, message: String },

    #[error("shape mismatch at {field_path}: expected {expected}, got {actual}")]
    ShapeMismatch {
        field_path: String,
        expected: String,
        actual: String,
    },
}
