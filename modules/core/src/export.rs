//! Export artifact identity and generated-file marker protocol.

use crate::{digest::Digest, render::ExtendedRenderProfile, r#trait as trait_model};

const GENERATED_MARKER_PREFIX: &str = "> GENERATED FILE - DO NOT EDIT DIRECTLY";
const SOURCE_TRAIT_PREFIX: &str = "> Source trait: ";
const SOURCE_DIGEST_PREFIX: &str = "> Source digest: ";
const RENDER_PROFILE_PREFIX: &str = "> Render profile: ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Compat,
    Skill,
    Agents,
    /// A body-free "run `ctx traits prompt <id>` and follow it" projection.
    /// Shares [`OwnershipKey::Skill`] with `Skill` (both occupy the same
    /// `SKILL.md` path and are equally ctx-managed) but keeps distinct lock
    /// evidence via [`Format::lock_target`], since a stub and a fully
    /// rendered skill of the same trait are different artifacts.
    Stub,
}

impl Format {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "compat" => Some(Self::Compat),
            "skill" => Some(Self::Skill),
            "agents" => Some(Self::Agents),
            "stub" => Some(Self::Stub),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compat => "compat",
            Self::Skill => "skill",
            Self::Agents => "agents",
            Self::Stub => "stub",
        }
    }
    /// The fixed leaf filename this format writes at `<out>/<trait-id>/<filename>`.
    pub fn filename(self) -> &'static str {
        match self {
            Self::Compat | Self::Skill | Self::Stub => "SKILL.md",
            Self::Agents => "AGENTS.md",
        }
    }
    pub fn ownership(self, profile: ExtendedRenderProfile) -> OwnershipKey {
        match self {
            Self::Compat | Self::Agents => OwnershipKey::RenderProfile(profile),
            Self::Skill | Self::Stub => OwnershipKey::Skill,
        }
    }
    /// Lock evidence target name for this format's ownership.
    ///
    /// `Agents` shares its `OwnershipKey` (and generated-file marker) with
    /// `Compat` for the same render profile, so exporting both to the same
    /// profile must not collapse into one lock entry: the target name is
    /// prefixed to keep the two formats' evidence distinct. `Stub` shares its
    /// `OwnershipKey` with `Skill` for the same reason, so it gets the same
    /// treatment.
    pub fn lock_target(self, ownership: OwnershipKey) -> String {
        match self {
            Self::Agents => format!("agents:{}", ownership.as_str()),
            Self::Stub => format!("stub:{}", ownership.as_str()),
            Self::Compat | Self::Skill => ownership.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipKey {
    RenderProfile(ExtendedRenderProfile),
    Skill,
}

impl OwnershipKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RenderProfile(profile) => profile.as_str(),
            Self::Skill => "skill",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    source_trait: trait_model::Id,
    source_digest: Digest,
    ownership: OwnershipKey,
}

impl Identity {
    pub fn new(
        source_trait: trait_model::Id,
        source_digest: Digest,
        ownership: OwnershipKey,
    ) -> Self {
        Self {
            source_trait,
            source_digest,
            ownership,
        }
    }
    pub fn source_trait(&self) -> &trait_model::Id {
        &self.source_trait
    }
    pub fn source_digest(&self) -> &Digest {
        &self.source_digest
    }
    pub fn ownership(&self) -> OwnershipKey {
        self.ownership
    }
    pub fn render_marker(&self) -> String {
        format!(
            "{GENERATED_MARKER_PREFIX}\n{SOURCE_TRAIT_PREFIX}{}\n{SOURCE_DIGEST_PREFIX}{}\n{RENDER_PROFILE_PREFIX}{}\n> Edit the canonical source trait file instead.",
            self.source_trait.as_str(),
            self.source_digest.as_str(),
            self.ownership.as_str()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    source_trait: String,
    source_digest: Option<String>,
    ownership: String,
}

impl Marker {
    pub fn parse(content: &str) -> Option<Self> {
        let content = strip_yaml_frontmatter(content);
        let mut lines = content.lines();
        if lines.next()? != GENERATED_MARKER_PREFIX {
            return None;
        }
        let mut source_trait = None;
        let mut source_digest = None;
        let mut ownership = None;
        for line in lines.take(5) {
            if let Some(value) = line.strip_prefix(SOURCE_TRAIT_PREFIX) {
                source_trait = Some(value.to_string());
            }
            if let Some(value) = line.strip_prefix(SOURCE_DIGEST_PREFIX) {
                source_digest = Some(value.to_string());
            }
            if let Some(value) = line.strip_prefix(RENDER_PROFILE_PREFIX) {
                ownership = Some(value.to_string());
            }
        }
        Some(Self {
            source_trait: source_trait?,
            source_digest,
            ownership: ownership?,
        })
    }
    pub fn source_trait(&self) -> &str {
        &self.source_trait
    }
    pub fn source_digest(&self) -> Option<&str> {
        self.source_digest.as_deref()
    }
    pub fn ownership(&self) -> &str {
        &self.ownership
    }
    pub fn is_owned_by(&self, identity: &Identity) -> bool {
        self.source_trait == identity.source_trait.as_str()
            && self.ownership == identity.ownership.as_str()
    }
    pub fn into_identity(self) -> Option<Identity> {
        let source_trait = trait_model::Id::new(self.source_trait).ok()?;
        let source_digest = Digest::parse(self.source_digest?.as_str()).ok()?;
        let ownership = if self.ownership == "skill" {
            OwnershipKey::Skill
        } else {
            OwnershipKey::RenderProfile(ExtendedRenderProfile::parse(&self.ownership)?)
        };
        Some(Identity::new(source_trait, source_digest, ownership))
    }
}

pub fn claims_skill_format(markdown: &str) -> bool {
    markdown.contains("> Render profile: skill")
}

fn strip_yaml_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n") else {
        return content;
    };
    let Some(end) = rest.find("\n---\n") else {
        return content;
    };
    rest[end + "\n---\n".len()..].trim_start_matches('\n')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(ownership: OwnershipKey) -> Identity {
        Identity::new(
            trait_model::Id::new("example-trait").expect("valid trait ID"),
            Digest::from_bytes(b"source"),
            ownership,
        )
    }

    #[test]
    fn marker_rendering_is_pinned_for_compat_and_skill() {
        let digest = Digest::from_bytes(b"source");
        let compat = identity(OwnershipKey::RenderProfile(ExtendedRenderProfile::Opencode));
        let skill = identity(OwnershipKey::Skill);
        let expected_prefix = "> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Source trait: example-trait\n> Source digest: ";

        assert_eq!(
            compat.render_marker(),
            format!(
                "{expected_prefix}{}\n> Render profile: opencode\n> Edit the canonical source trait file instead.",
                digest.as_str()
            )
        );
        assert_eq!(
            skill.render_marker(),
            format!(
                "{expected_prefix}{}\n> Render profile: skill\n> Edit the canonical source trait file instead.",
                digest.as_str()
            )
        );
        assert!(!compat.render_marker().ends_with('\n'));
        assert!(!skill.render_marker().ends_with('\n'));
    }

    #[test]
    fn canonical_marker_round_trips_and_frontmatter_is_accepted() {
        let identity = identity(OwnershipKey::RenderProfile(ExtendedRenderProfile::Pi));
        assert_eq!(
            Marker::parse(&identity.render_marker()).and_then(Marker::into_identity),
            Some(identity.clone())
        );
        assert_eq!(
            Marker::parse(&format!(
                "---\nname: example\n---\n\n{}",
                identity.render_marker()
            ))
            .and_then(Marker::into_identity),
            Some(identity)
        );
    }

    #[test]
    fn marker_parser_preserves_five_line_window_and_last_match_wins() {
        let content = "> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Render profile: pi\nignored\n> Source trait: first\n> Source trait: final\n> Render profile: opencode\n> Source digest: ignored";
        let marker = Marker::parse(content).expect("marker parses");
        assert_eq!(marker.source_trait(), "final");
        assert_eq!(marker.ownership(), "opencode");
        assert_eq!(marker.source_digest(), None);
        assert!(Marker::parse("> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Source trait: example-trait\n> Render profile: pi\n> Source digest: later").is_some());
    }

    #[test]
    fn parsed_digest_is_optional_and_frontmatter_stays_lf_only() {
        let identity = identity(OwnershipKey::Skill);
        let without_digest = "> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Source trait: example-trait\n> Render profile: skill";
        let opaque_digest = "> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Source trait: example-trait\n> Source digest: opaque\n> Render profile: skill";
        assert!(
            Marker::parse(without_digest)
                .expect("marker parses")
                .is_owned_by(&identity)
        );
        assert!(
            Marker::parse(opaque_digest)
                .expect("marker parses")
                .is_owned_by(&identity)
        );
        assert!(Marker::parse("---\r\nname: example\r\n---\r\n> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Source trait: example-trait\n> Render profile: skill").is_none());
        assert!(Marker::parse("---\nname: example\n> GENERATED FILE - DO NOT EDIT DIRECTLY\n> Source trait: example-trait\n> Render profile: skill").is_none());
    }

    #[test]
    fn skill_claim_probe_is_the_exact_legacy_substring_check() {
        for markdown in [
            "",
            "> Render profile: skill",
            "prefix > Render profile: skill suffix",
            "> Render profile: Skill",
        ] {
            assert_eq!(
                claims_skill_format(markdown),
                markdown.contains("> Render profile: skill")
            );
        }
    }
}
