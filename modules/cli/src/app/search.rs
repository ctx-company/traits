//! Search command handler.

use crate::app::entry::{print_json_report, resolve_repo_root};
use ctx_traits_core::response::{CommandOutput, Envelope};

#[derive(serde::Serialize)]
#[serde(rename_all = "kebab-case")]
struct SearchReport {
    query: String,
    results: Vec<ctx_traits_core::search::Hit>,
    target_profile_evidence: String,
    target_profile_manifest_path: Option<String>,
    target_profile_manifest_encoding: Option<String>,
    target_profile_manifest_status: Option<String>,
    note: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    scanned: Option<String>,
}

pub(crate) fn handle_search(
    query: &str,
    repo_root: Option<&str>,
    json: bool,
) -> crate::Result<CommandOutput<()>> {
    let root_path = resolve_repo_root(repo_root)?;
    let packages = ctx_traits_io::discovery::trait_packages(&root_path)?;

    // Discover project manifest for target-profile evidence.
    let manifest_result = ctx_traits_io::discovery::manifest(&root_path)?;
    let mut manifest_path: Option<String> = None;
    let mut manifest_encoding: Option<String> = None;
    let (target_profiles, manifest_source_kinds, target_profile_evidence) = match &manifest_result {
        ctx_traits_io::discovery::ManifestDiscovery::NotFound => {
            (Vec::new(), std::collections::BTreeMap::new(), "not-loaded")
        }
        ctx_traits_io::discovery::ManifestDiscovery::Found(found) => {
            let text = ctx_traits_io::read::read_text(&found.path)?;
            let encoding = ctx_traits_core::encoding::Encoding::from_path(&found.path)?;
            let manifest = ctx_traits_core::encoding::decode_manifest(encoding, &text)?;
            let mut profiles: Vec<(String, String)> = Vec::new();
            let mut source_kinds = std::collections::BTreeMap::new();
            for entry in &manifest.trait_entries {
                source_kinds.insert(
                    entry.id.clone(),
                    entry.source.manifest_source_kind().to_string(),
                );
                let mut entry_targets: Vec<String> =
                    entry.target.iter().map(|s| s.to_string()).collect();
                if entry_targets.is_empty() {
                    if let Some(proj) = &manifest.project {
                        entry_targets = proj.default_target.iter().map(|s| s.to_string()).collect();
                    }
                }
                entry_targets.sort();
                entry_targets.dedup();
                for t in entry_targets {
                    profiles.push((entry.id.clone(), t));
                }
            }
            manifest_path = Some(found.path.to_string());
            manifest_encoding = Some(found.encoding.to_string());
            (profiles, source_kinds, "loaded")
        }
        ctx_traits_io::discovery::ManifestDiscovery::Conflict { found } => {
            let paths: Vec<String> = found.iter().map(|m| m.path.to_string()).collect();
            return Err(crate::Error::Command {
                message: format!(
                    "multiple project manifests found: {}; use explicit manifest path",
                    paths.join(", ")
                ),
            });
        }
    };

    // Build target profile pairs for BuildContext.
    let target_profile_pairs: Vec<(&str, &str)> = target_profiles
        .iter()
        .map(|(id, p)| (id.as_str(), p.as_str()))
        .collect();
    let manifest_status = if target_profile_evidence == "loaded" {
        Some("loaded")
    } else {
        None
    };

    if packages.is_empty() {
        if json {
            let output = SearchReport {
                query: query.to_string(),
                results: Vec::new(),
                target_profile_evidence: target_profile_evidence.to_string(),
                target_profile_manifest_path: manifest_path.clone(),
                target_profile_manifest_encoding: manifest_encoding.clone(),
                target_profile_manifest_status: manifest_status.map(str::to_string),
                note: "search is discovery, not activation",
                scanned: Some(root_path.as_str().to_string()),
            };
            print_json_report(&Envelope::ok(output), "search output")?;
        } else {
            println!("ctx traits search");
            println!("  query: {query}");
            println!("  note: search is discovery, not activation");
            println!("  target-profile-evidence: {target_profile_evidence}");
            if let Some(path) = &manifest_path {
                println!("  target-profile-manifest-path: {path}");
            }
            if let Some(encoding) = &manifest_encoding {
                println!("  target-profile-manifest-encoding: {encoding}");
            }
            println!("  no trait packages found under {}", root_path);
        }
        return Ok(CommandOutput::new(()));
    }

    let mut documents = Vec::new();
    for pkg in &packages {
        let (trait_ref, trait_root, _source_digest, canonical_digest) =
            ctx_traits_io::run::load_trait(pkg.trait_path.as_str())?;
        let (status, trust) = ctx_traits_io::lifecycle::resolve_named(
            &trait_root,
            trait_ref.id.as_str(),
            canonical_digest.as_str(),
        )?;
        let status_text = status.display_name();
        let trust_text = trust.display_name();

        let package_path = trait_root.to_string();
        let manifest_source_kind = manifest_source_kinds
            .get(trait_ref.id.as_str())
            .map(String::as_str)
            .unwrap_or("");
        let ctx = ctx_traits_core::search::BuildContext {
            target_profiles: &target_profile_pairs,
            target_profile_evidence,
            source_layout: "local-package",
            package_path: &package_path,
            manifest_source_kind,
            status: status_text,
            trust: trust_text,
        };
        documents.push(ctx_traits_core::search::build_search_document_with_context(
            &trait_ref, ctx,
        ));
    }

    let results = ctx_traits_core::search::search_traits(query, &documents);

    if json {
        let output = SearchReport {
            query: query.to_string(),
            results,
            target_profile_evidence: target_profile_evidence.to_string(),
            target_profile_manifest_path: manifest_path.clone(),
            target_profile_manifest_encoding: manifest_encoding.clone(),
            target_profile_manifest_status: manifest_status.map(str::to_string),
            note: "search is discovery, not activation",
            scanned: None,
        };
        print_json_report(&Envelope::ok(output), "search output")?;
    } else {
        println!("ctx traits search");
        println!("  query: {query}");
        println!("  note: search is discovery, not activation");
        println!("  target-profile-evidence: {target_profile_evidence}");
        if let Some(path) = &manifest_path {
            println!("  target-profile-manifest-path: {path}");
        }
        if let Some(encoding) = &manifest_encoding {
            println!("  target-profile-manifest-encoding: {encoding}");
        }
        println!("  results: {}", results.len());
        for result in &results {
            println!(
                "    {} ({}) score={}",
                result.trait_id, result.name, result.score
            );
            for reason in &result.match_reasons {
                println!("      {}: {}", reason.field, reason.matched_term);
            }
        }
    }

    Ok(CommandOutput::new(()))
}

pub(crate) use handle_search as handle;
