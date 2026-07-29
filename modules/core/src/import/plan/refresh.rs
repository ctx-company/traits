// Plans refreshes for imported artifacts.
// Import refresh planning.

pub fn compute_artifact_diff(old: &[TraitLockArtifact], new: &[TraitLockArtifact]) -> ArtifactDiff {
    let mut old_map: BTreeMap<&str, &TraitLockArtifact> = BTreeMap::new();
    for a in old {
        old_map.insert(a.normalized_path.as_str(), a);
    }
    let mut new_map: BTreeMap<&str, &TraitLockArtifact> = BTreeMap::new();
    for a in new {
        new_map.insert(a.normalized_path.as_str(), a);
    }

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();

    for (path, new_artifact) in &new_map {
        match old_map.get(path) {
            None => added.push(path.to_string()),
            Some(old_artifact) => {
                if old_artifact.byte_digest != new_artifact.byte_digest {
                    modified.push(ArtifactModification {
                        path: path.to_string(),
                        before_digest: old_artifact.byte_digest.clone(),
                        after_digest: new_artifact.byte_digest.clone(),
                        before_classification: Some(old_artifact.file_classification.clone()),
                        after_classification: Some(new_artifact.file_classification.clone()),
                    });
                }
            }
        }
    }
    for path in old_map.keys() {
        if !new_map.contains_key(path) {
            removed.push(path.to_string());
        }
    }

    added.sort();
    removed.sort();
    modified.sort_by(|a, b| a.path.cmp(&b.path));

    ArtifactDiff {
        added,
        removed,
        modified,
    }
}

/// Determine the refresh decision from artifact and trait diffs.
pub fn determine_refresh_decision(
    artifact_diff: &ArtifactDiff,
    trait_diff: &TraitDiff,
) -> RefreshDecision {
    if !artifact_diff.has_changes() && !trait_diff.canonical_changed {
        RefreshDecision::NoChange
    } else if artifact_diff.has_changes() && !trait_diff.canonical_changed {
        RefreshDecision::SourceOnlyChange
    } else {
        RefreshDecision::TraitChange
    }
}

/// Compute a full refresh diff from old/new snapshots and canonical digests.
///
/// When `old_canonical_json` and `new_canonical_json` are supplied, the
/// trait diff includes field-level changes and mapping attribution entries.
/// When they are absent, the function falls back to digest-level comparison.
pub fn compute_refresh_diff(
    trait_id: &str,
    old_snapshot: Option<&TraitLockSnapshot>,
    new_snapshot: &TraitLockSnapshot,
    new_canonical_digest: Option<&str>,
) -> RefreshDiffReport {
    compute_refresh_diff_full(
        trait_id,
        old_snapshot,
        new_snapshot,
        new_canonical_digest,
        None,
        None,
    )
}

/// Full refresh diff with optional canonical JSON for field-level comparison.
pub fn compute_refresh_diff_full(
    trait_id: &str,
    old_snapshot: Option<&TraitLockSnapshot>,
    new_snapshot: &TraitLockSnapshot,
    new_canonical_digest: Option<&str>,
    old_canonical_json: Option<&serde_json::Value>,
    new_canonical_json: Option<&serde_json::Value>,
) -> RefreshDiffReport {
    let artifact_diff = match old_snapshot {
        Some(old) => compute_artifact_diff(&old.artifacts, &new_snapshot.artifacts),
        None => {
            let added: Vec<String> = new_snapshot
                .artifacts
                .iter()
                .map(|a| a.normalized_path.clone())
                .collect();
            ArtifactDiff {
                added,
                removed: Vec::new(),
                modified: Vec::new(),
            }
        }
    };

    let before_canonical = old_snapshot.and_then(|s| s.canonical_output_digest.clone());
    let after_canonical = new_canonical_digest.map(Digest::from_unvalidated);
    let canonical_changed = match (&before_canonical, &after_canonical) {
        (Some(before), Some(after)) => before != after,
        (None, None) => false,
        _ => artifact_diff.has_changes(),
    };

    let (field_changes, mapping_diff) =
        compute_canonical_field_diffs(old_canonical_json, new_canonical_json);

    let mut warnings: Vec<String> = new_snapshot
        .artifacts
        .iter()
        .filter(|a| matches!(a.content, ArtifactContent::Blocked { .. }))
        .map(|a| {
            let reason = match &a.content {
                ArtifactContent::Blocked { reason } => reason.clone(),
                _ => String::new(),
            };
            format!("blocked artifact {}: {reason}", a.normalized_path)
        })
        .collect();

    if !artifact_diff.modified.is_empty() {
        warnings.push(
            "hunks-unsupported: text hunk summaries are not implemented; \
             artifact modifications are reported at byte-digest level only"
                .to_string(),
        );
    }

    let blocked = warnings.iter().any(|w| w.starts_with("blocked artifact"));

    let summary = if blocked {
        "blocked artifacts prevent refresh apply".to_string()
    } else if canonical_changed {
        "canonical trait output changed".to_string()
    } else if artifact_diff.has_changes() {
        "source artifacts changed but canonical output did not".to_string()
    } else {
        "no changes detected".to_string()
    };

    let trait_diff = TraitDiff {
        canonical_changed: canonical_changed || blocked,
        field_changes,
        summary,
        before_canonical_digest: before_canonical,
        after_canonical_digest: after_canonical,
    };

    let decision = if blocked {
        RefreshDecision::Blocked
    } else {
        determine_refresh_decision(&artifact_diff, &trait_diff)
    };

    let before_digest = old_snapshot.map(|s| s.snapshot_digest.clone());

    RefreshDiffReport {
        trait_id: trait_id.to_string(),
        before_snapshot_digest: before_digest,
        after_snapshot_digest: new_snapshot.snapshot_digest.clone(),
        artifact_diff,
        trait_diff,
        mapping_diff,
        decision,
        warnings,
    }
}

/// Compute field-level changes and mapping attribution from two canonical JSONs.
fn compute_canonical_field_diffs(
    old: Option<&serde_json::Value>,
    new: Option<&serde_json::Value>,
) -> (Vec<String>, Vec<MappingDiffEntry>) {
    let mut field_changes = Vec::new();
    let mut mapping_diff = Vec::new();

    let (Some(old), Some(new)) = (old, new) else {
        return (field_changes, mapping_diff);
    };

    let old_obj = old.as_object();
    let new_obj = new.as_object();
    let (Some(old_obj), Some(new_obj)) = (old_obj, new_obj) else {
        return (field_changes, mapping_diff);
    };

    let mut all_keys: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for key in old_obj.keys() {
        all_keys.insert(key.as_str());
    }
    for key in new_obj.keys() {
        all_keys.insert(key.as_str());
    }

    for key in &all_keys {
        let old_val = old_obj.get(*key);
        let new_val = new_obj.get(*key);
        if old_val.is_none() && new_val.is_some() {
            field_changes.push(format!("+ {key}: added"));
            mapping_diff.push(MappingDiffEntry {
                canonical_field: key.to_string(),
                source_attribution: "unknown".to_string(),
            });
        } else if old_val.is_some() && new_val.is_none() {
            field_changes.push(format!("- {key}: removed"));
            mapping_diff.push(MappingDiffEntry {
                canonical_field: key.to_string(),
                source_attribution: "unknown".to_string(),
            });
        } else if old_val != new_val {
            report_value_change(
                key,
                old_val.unwrap_or(&serde_json::Value::Null),
                new_val.unwrap_or(&serde_json::Value::Null),
                &mut field_changes,
                &mut mapping_diff,
            );
        }
    }

    (field_changes, mapping_diff)
}

fn report_value_change(
    key: &str,
    old: &serde_json::Value,
    new: &serde_json::Value,
    field_changes: &mut Vec<String>,
    mapping_diff: &mut Vec<MappingDiffEntry>,
) {
    if key == "resource" {
        if let (Some(old_arr), Some(new_arr)) = (old.as_array(), new.as_array()) {
            diff_resource_array(old_arr, new_arr, field_changes, mapping_diff);
            return;
        }
    }

    if let (Some(old_arr), Some(new_arr)) = (old.as_array(), new.as_array()) {
        if key == "prompt" || key == "port" || key == "schema" {
            let old_ids: std::collections::BTreeSet<String> = old_arr
                .iter()
                .filter_map(|v| {
                    v.get("id")
                        .or_else(|| v.get("key"))
                        .and_then(|i| i.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            let new_ids: std::collections::BTreeSet<String> = new_arr
                .iter()
                .filter_map(|v| {
                    v.get("id")
                        .or_else(|| v.get("key"))
                        .and_then(|i| i.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            for added_id in new_ids.difference(&old_ids) {
                field_changes.push(format!("+ {key}[{added_id}]: added"));
                mapping_diff.push(MappingDiffEntry {
                    canonical_field: format!("{key}[{added_id}]"),
                    source_attribution: "unknown".to_string(),
                });
            }
            for removed_id in old_ids.difference(&new_ids) {
                field_changes.push(format!("- {key}[{removed_id}]: removed"));
                mapping_diff.push(MappingDiffEntry {
                    canonical_field: format!("{key}[{removed_id}]"),
                    source_attribution: "unknown".to_string(),
                });
            }
            if old_ids == new_ids {
                field_changes.push(format!("~ {key}: content changed (same IDs)"));
                mapping_diff.push(MappingDiffEntry {
                    canonical_field: key.to_string(),
                    source_attribution: "unknown".to_string(),
                });
            }
            return;
        }
    }

    let old_summary = summarize_json_value(old);
    let new_summary = summarize_json_value(new);
    field_changes.push(format!("~ {key}: {old_summary} -> {new_summary}"));
    mapping_diff.push(MappingDiffEntry {
        canonical_field: key.to_string(),
        source_attribution: "unknown".to_string(),
    });
}

/// Diff a `resource` array by resource id, and — for a resource that is a
/// checklist on both sides — by item id within it. A preserved item id whose
/// text changed is reported as an explicit `resource[id].item[itemId].text`
/// modification rather than a remove/add pair, matching the id-preserving
/// reconciliation `derive_checklist_resource` already performed.
fn diff_resource_array(
    old_arr: &[serde_json::Value],
    new_arr: &[serde_json::Value],
    field_changes: &mut Vec<String>,
    mapping_diff: &mut Vec<MappingDiffEntry>,
) {
    let by_id = |arr: &[serde_json::Value]| -> BTreeMap<String, serde_json::Value> {
        arr.iter()
            .filter_map(|v| {
                v.get("id")
                    .and_then(|i| i.as_str())
                    .map(|id| (id.to_string(), v.clone()))
            })
            .collect()
    };
    let old_by_id = by_id(old_arr);
    let new_by_id = by_id(new_arr);

    for added_id in new_by_id.keys().filter(|id| !old_by_id.contains_key(*id)) {
        field_changes.push(format!("+ resource[{added_id}]: added"));
        mapping_diff.push(MappingDiffEntry {
            canonical_field: format!("resource[{added_id}]"),
            source_attribution: "unknown".to_string(),
        });
    }
    for removed_id in old_by_id.keys().filter(|id| !new_by_id.contains_key(*id)) {
        field_changes.push(format!("- resource[{removed_id}]: removed"));
        mapping_diff.push(MappingDiffEntry {
            canonical_field: format!("resource[{removed_id}]"),
            source_attribution: "unknown".to_string(),
        });
    }

    for (id, new_resource) in &new_by_id {
        let Some(old_resource) = old_by_id.get(id) else {
            continue;
        };
        if old_resource == new_resource {
            continue;
        }

        let old_items = old_resource.get("item").and_then(|v| v.as_array());
        let new_items = new_resource.get("item").and_then(|v| v.as_array());
        let (Some(old_items), Some(new_items)) = (old_items, new_items) else {
            field_changes.push(format!("~ resource[{id}]: changed"));
            mapping_diff.push(MappingDiffEntry {
                canonical_field: format!("resource[{id}]"),
                source_attribution: "unknown".to_string(),
            });
            continue;
        };

        let item_by_id = |arr: &[serde_json::Value]| -> BTreeMap<String, serde_json::Value> {
            arr.iter()
                .filter_map(|v| {
                    let item_id = v.get("id").and_then(|i| i.as_str())?;
                    Some((item_id.to_string(), v.clone()))
                })
                .collect()
        };
        let old_item_ids = item_by_id(old_items);
        let new_item_ids = item_by_id(new_items);

        let mut resource_changed = false;

        for added_item in new_item_ids.keys().filter(|k| !old_item_ids.contains_key(*k)) {
            resource_changed = true;
            field_changes.push(format!("+ resource[{id}].item[{added_item}]: added"));
            mapping_diff.push(MappingDiffEntry {
                canonical_field: format!("resource[{id}].item[{added_item}]"),
                source_attribution: "unknown".to_string(),
            });
        }
        for removed_item in old_item_ids.keys().filter(|k| !new_item_ids.contains_key(*k)) {
            resource_changed = true;
            field_changes.push(format!("- resource[{id}].item[{removed_item}]: removed"));
            mapping_diff.push(MappingDiffEntry {
                canonical_field: format!("resource[{id}].item[{removed_item}]"),
                source_attribution: "unknown".to_string(),
            });
        }
        for (item_id, new_item) in &new_item_ids {
            let Some(old_item) = old_item_ids.get(item_id) else {
                continue;
            };
            if old_item == new_item {
                continue;
            }
            let old_text = old_item.get("text").and_then(|t| t.as_str()).unwrap_or_default();
            let new_text = new_item.get("text").and_then(|t| t.as_str()).unwrap_or_default();
            if old_text != new_text {
                resource_changed = true;
                field_changes.push(format!(
                    "~ resource[{id}].item[{item_id}].text: {old_text:?} -> {new_text:?}"
                ));
                mapping_diff.push(MappingDiffEntry {
                    canonical_field: format!("resource[{id}].item[{item_id}].text"),
                    source_attribution: "unknown".to_string(),
                });
            } else {
                resource_changed = true;
                field_changes.push(format!("~ resource[{id}].item[{item_id}]: changed"));
                mapping_diff.push(MappingDiffEntry {
                    canonical_field: format!("resource[{id}].item[{item_id}]"),
                    source_attribution: "unknown".to_string(),
                });
            }
        }

        // The specialized comparisons above cover item membership and text.
        // Any remaining difference (e.g. only `detail` changed, or a
        // non-item resource field) still needs to be represented, so a
        // checklist resource change is never silently dropped.
        if !resource_changed {
            field_changes.push(format!("~ resource[{id}]: changed"));
            mapping_diff.push(MappingDiffEntry {
                canonical_field: format!("resource[{id}]"),
                source_attribution: "unknown".to_string(),
            });
        }
    }
}

fn summarize_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => {
            if s.len() > 40 {
                format!("\"{}...\"", &s[..37])
            } else {
                format!("\"{s}\"")
            }
        }
        serde_json::Value::Array(arr) => format!("[{} items]", arr.len()),
        serde_json::Value::Object(obj) => format!("{{{} keys}}", obj.len()),
    }
}

// ---------------------------------------------------------------------------
// P92: Multi-file artifact graph
// ---------------------------------------------------------------------------
