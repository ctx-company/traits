// Trait relation validation.
// Trait relation validation.

// ===========================================================================
// Validation
// ===========================================================================

/// Validate a `[relations]` section.
pub fn validate(relations: &Model) -> crate::Result<()> {
    for (i, entry) in relations.requires.iter().enumerate() {
        let base = format!("relations.requires[{i}]");
        validate_target_ref(
            &entry.target,
            &format!("{base}.target"),
            &[Kind::Port, Kind::Trait],
        )?;
        validate_reason(&entry.reason, &base)?;
        validate_when(&entry.when, &base)?;
    }
    for (i, entry) in relations.suggests.iter().enumerate() {
        let base = format!("relations.suggests[{i}]");
        validate_target_ref(
            &entry.target,
            &format!("{base}.target"),
            &[Kind::Trait, Kind::Port],
        )?;
        validate_reason(&entry.reason, &base)?;
        validate_when(&entry.when, &base)?;
    }
    for (i, entry) in relations.conflicts.iter().enumerate() {
        let base = format!("relations.conflicts[{i}]");
        validate_reason(&entry.reason, &base)?;
        validate_when(&entry.when, &base)?;
    }
    Ok(())
}

fn validate_target_ref(raw: &str, path: &str, allowed: &[Kind]) -> crate::Result<()> {
    if raw.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: path.to_string(),
            message: "must not be empty".to_string(),
        }
        .into());
    }
    let parsed = Reference::parse(raw).map_err(|e| crate::manifest::Error::InvalidField {
        field_path: path.to_string(),
        message: format!("invalid ref {raw:?}: {e}"),
    })?;
    if !allowed.contains(&parsed.kind()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: path.to_string(),
            message: format!(
                "ref kind {:?} not allowed; expected one of {:?}",
                parsed.kind(),
                allowed.iter().map(|k| k.as_str()).collect::<Vec<_>>()
            ),
        }
        .into());
    }
    Ok(())
}

fn validate_reason(reason: &str, base: &str) -> crate::Result<()> {
    if reason.trim().is_empty() {
        return Err(crate::manifest::Error::InvalidField {
            field_path: format!("{base}.reason"),
            message: "must not be empty".to_string(),
        }
        .into());
    }
    Ok(())
}

fn validate_when(when: &RefList, base: &str) -> crate::Result<()> {
    for (j, raw) in when.iter().enumerate() {
        if raw.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{base}.when[{j}]"),
                message: "must not be empty".to_string(),
            }
            .into());
        }
        let parsed =
            Reference::parse(raw.as_str()).map_err(|e| crate::manifest::Error::InvalidField {
                field_path: format!("{base}.when[{j}]"),
                message: format!("invalid ref {:?}: {e}", raw.as_str()),
            })?;
        match parsed.kind() {
            Kind::Rule | Kind::Signal => {}
            other => {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("{base}.when[{j}]"),
                    message: format!(
                        "when ref kind {:?} not allowed; expected rule or signal",
                        other
                    ),
                }
                .into());
            }
        }
    }
    Ok(())
}
