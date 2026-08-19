//! Typed accessor over the first-party built-in meta-trait packages
//! (`generate`, `refine`, `critique`,
//! `explain`, `import`, `spec`), embedded into the binary
//! at compile time.
//!
//! Core performs no IO here: `build.rs` reads the package files, computes
//! their SHA-256 digests, and emits static byte tables that this module only
//! exposes through immutable views.

/// A single embedded file belonging to a built-in trait package.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinTraitFile {
    /// Path relative to the package root, e.g. `"trait.toml"` or
    /// `"resources/design-rubric.md"`.
    pub relative_path: &'static str,
    pub bytes: &'static [u8],
    /// `sha256:<hex>` digest of `bytes`, computed once at build time.
    pub digest: &'static str,
}

/// A built-in meta-trait package: its manifest, compiled trait manifest, and
/// any resources its compiled manifest declares.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinTraitPackage {
    pub id: &'static str,
    pub files: &'static [BuiltinTraitFile],
}

impl BuiltinTraitPackage {
    /// Look up an embedded file by its package-relative path.
    pub fn file(&self, relative_path: &str) -> Option<&'static BuiltinTraitFile> {
        self.files
            .iter()
            .find(|file| file.relative_path == relative_path)
    }
}

include!(concat!(env!("OUT_DIR"), "/builtin_trait_packages.rs"));

/// Look up a built-in trait package by id.
pub fn package(id: &str) -> Option<&'static BuiltinTraitPackage> {
    BUILTIN_TRAIT_PACKAGES
        .iter()
        .find(|package| package.id == id)
}

/// List all built-in trait packages.
pub fn packages() -> &'static [BuiltinTraitPackage] {
    BUILTIN_TRAIT_PACKAGES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Digest;

    const EXPECTED_IDS: &[&str] = &[
        "generate", "refine", "critique", "explain", "import", "spec",
    ];

    #[test]
    fn exposes_exactly_the_six_builtin_ids() {
        let mut ids: Vec<&str> = packages().iter().map(|package| package.id).collect();
        ids.sort_unstable();
        let mut expected = EXPECTED_IDS.to_vec();
        expected.sort_unstable();
        assert_eq!(ids, expected);
    }

    #[test]
    fn every_package_has_unique_relative_paths_and_the_two_required_manifests() {
        for package in packages() {
            let mut seen = std::collections::BTreeSet::new();
            for file in package.files {
                assert!(
                    seen.insert(file.relative_path),
                    "duplicate relative path {:?} in package {:?}",
                    file.relative_path,
                    package.id
                );
            }
            assert!(
                package.file("trait.toml").is_some(),
                "package {:?} missing trait.toml",
                package.id
            );
            assert!(
                package.file("generated/index.toml").is_some(),
                "package {:?} missing generated/index.toml",
                package.id
            );
        }
    }

    #[test]
    fn every_file_digest_matches_its_embedded_bytes() {
        for package in packages() {
            for file in package.files {
                let expected = Digest::from_bytes(file.bytes);
                assert_eq!(
                    expected.as_str(),
                    file.digest,
                    "digest mismatch for {:?} in package {:?}",
                    file.relative_path,
                    package.id
                );
            }
        }
    }

    #[test]
    fn declared_resources_match_embedded_non_manifest_files_exactly() {
        for package in packages() {
            let index_file = package
                .file("generated/index.toml")
                .expect("generated/index.toml present");
            let index_text = std::str::from_utf8(index_file.bytes).expect("index.toml is UTF-8");
            let document: toml::Value = toml::from_str(index_text).expect("index.toml parses");
            let declared: std::collections::BTreeSet<String> = document
                .get("resource")
                .and_then(toml::Value::as_array)
                .map(|resources| {
                    resources
                        .iter()
                        .map(|resource| {
                            resource
                                .get("path")
                                .and_then(toml::Value::as_str)
                                .expect("resource entry has string path")
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default();

            let embedded: std::collections::BTreeSet<String> = package
                .files
                .iter()
                .map(|file| file.relative_path.to_string())
                .filter(|path| path != "trait.toml" && path != "generated/index.toml")
                .collect();

            assert_eq!(
                declared, embedded,
                "declared resources vs embedded non-manifest files diverge for package {:?}",
                package.id
            );
        }
    }
}
