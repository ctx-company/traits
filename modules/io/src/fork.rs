//! IO-layer support for `ctx traits fork` (0213): copying the authoring
//! subset out of a vendored package into a freshly claimed authored root,
//! and recording forked-from provenance in the authored `trait.toml`.

use camino::Utf8Path;

/// The vendored evidence subset a fork copies: the package manifest plus
/// every authoring/reference directory the build and `check` need. Deliberately
/// excludes `generated/` and the vendored `package.json` — the CDK rebuild
/// regenerates both, and `check`'s package.json drift check demands the
/// build-owned bytes, not a copy of the vendored ones.
const AUTHORING_SUBSET_DIRS: &[&str] = &["source", "resources", "reference"];

/// Copy `trait.toml` plus the authoring subset directories from a vendored
/// package root into a freshly claimed (empty) authored package root.
/// No-symlink recursive copy via [`crate::publish::copy_safe`], the one safe
/// local-tree copy primitive in this crate.
pub fn copy_authoring_subset(
    vendored_root: &Utf8Path,
    authored_root: &Utf8Path,
) -> crate::Result<()> {
    crate::publish::copy_safe(
        &crate::layout::package_manifest_path(vendored_root),
        &crate::layout::package_manifest_path(authored_root),
        &[],
    )?;
    for dir in AUTHORING_SUBSET_DIRS {
        let source = vendored_root.join(dir);
        if source.exists() {
            crate::publish::copy_safe(&source, &authored_root.join(dir), &[])?;
        }
    }
    Ok(())
}

/// Fork provenance to write into the authored package's `[forked-from]`
/// table.
pub struct ForkProvenance<'a> {
    pub id: &'a str,
    pub version: &'a str,
    pub canonical_digest: &'a str,
}

/// Write/refresh the `[forked-from]` table in `manifest_path`, preserving
/// every other table byte-for-byte via `toml_edit` — the same discipline
/// [`crate::family_manifest::write_family_table`] uses for `[family]`.
pub fn write_forked_from(
    manifest_path: &Utf8Path,
    provenance: ForkProvenance<'_>,
) -> crate::Result<()> {
    let text = crate::read::read_text(manifest_path)?;
    let mut document = text.parse::<toml_edit::DocumentMut>().map_err(|source| {
        crate::parse::Error::TomlEditDecode {
            context: format!("parse {manifest_path} for fork provenance"),
            source: Box::new(source),
        }
    })?;
    document["forked-from"] = toml_edit::table();
    document["forked-from"]["id"] = toml_edit::value(provenance.id);
    document["forked-from"]["version"] = toml_edit::value(provenance.version);
    document["forked-from"]["canonical-digest"] = toml_edit::value(provenance.canonical_digest);
    crate::write::write_text(manifest_path, &document.to_string())?;
    Ok(())
}
