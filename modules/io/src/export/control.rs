use super::{Error, Format, Identity, OwnershipKey};
use camino::{Utf8Path, Utf8PathBuf};
use ctx_traits_core::digest::Digest;
use std::sync::Arc;

/// How a companion resource file's existing target (if any) is checked
/// before it may be overwritten. A companion cannot carry the leaf's
/// `> GENERATED FILE ...` marker without changing the bytes its digest
/// pins, so ownership here is always digest-keyed rather than marker-keyed
/// — but *which* digest check applies depends on what the caller already
/// knows about this path:
///
/// - [`Self::Fresh`]: nothing was ever recorded for this path (a first
///   `export`, or a companion newly added to the trait since the last
///   install/update). An existing file is then unmanaged and refused,
///   exactly like a marker-less leaf — never silently overwritten.
/// - [`Self::RecordedDigest`]: the placement manifest recorded this exact
///   digest for this path at the last install/update. An existing file must
///   be missing or match it; a mismatch means a human edited it since.
/// - [`Self::Force`]: the caller has deliberately chosen to overwrite a
///   previously recorded path regardless of on-disk drift (`--force`).
///   Distinct from `Fresh`: it only ever applies to a path the caller
///   already has a record for, so `--force` cannot be used to silently
///   clobber some unrelated unmanaged file at a brand-new companion path.
#[derive(Clone, Copy)]
pub enum CompanionOwnership<'a> {
    Fresh,
    RecordedDigest(&'a Digest),
    Force,
}

/// One companion resource file to place alongside the leaf.
pub struct Companion<'a> {
    pub relative_target: &'a Utf8Path,
    pub bytes: &'a [u8],
    pub ownership: CompanionOwnership<'a>,
}

pub struct Request<'a> {
    output_root: &'a Utf8Path,
    identity: &'a Identity,
    content: &'a str,
    format: Format,
    relative_target: Option<&'a Utf8Path>,
    companions: &'a [Companion<'a>],
}
impl<'a> Request<'a> {
    pub fn new(
        output_root: &'a Utf8Path,
        identity: &'a Identity,
        content: &'a str,
        format: Format,
    ) -> Self {
        Self {
            output_root,
            identity,
            content,
            format,
            relative_target: None,
            companions: &[],
        }
    }
    /// Override the default `<trait-id>/<format.filename()>` relative
    /// target with an explicit relative path (host-placement templates).
    pub fn with_relative_target(mut self, relative_target: &'a Utf8Path) -> Self {
        self.relative_target = Some(relative_target);
        self
    }
    /// Attach companion resource files to write alongside the leaf, in the
    /// same validated-then-written transaction: every companion's ancestors
    /// are probed no-follow and its existing target (if any) must be
    /// provably ours, exactly as the leaf's are — there is no second writer.
    pub fn with_companions(mut self, companions: &'a [Companion<'a>]) -> Self {
        self.companions = companions;
        self
    }
    pub fn output_root(&self) -> &Utf8Path {
        self.output_root
    }
    pub fn identity(&self) -> &Identity {
        self.identity
    }
    pub fn content(&self) -> &str {
        self.content
    }
    pub fn format(&self) -> Format {
        self.format
    }
    pub fn companions(&self) -> &'a [Companion<'a>] {
        self.companions
    }
    /// The relative target this request writes to: the explicit override if
    /// set, otherwise the default `<trait-id>/<format.filename()>` shape.
    pub fn relative_target(&self) -> Utf8PathBuf {
        match self.relative_target {
            Some(target) => target.to_owned(),
            None => Utf8PathBuf::from(self.identity.source_trait().as_str())
                .join(self.format.filename()),
        }
    }
}
/// The written outcome for one companion resource file.
pub struct CompanionResponse {
    pub path: Utf8PathBuf,
    pub content_digest: Digest,
    pub byte_size: u64,
}
pub struct Response {
    pub ownership: OwnershipKey,
    pub path: Utf8PathBuf,
    pub content_digest: Digest,
    pub byte_size: u64,
    pub companions: Vec<CompanionResponse>,
}
pub type Result<T> = std::result::Result<T, Error>;
pub trait Interface: Send + Sync {
    fn write(&self, request: Request<'_>) -> Result<Response>;
}
pub type Handle = Arc<dyn Interface>;
