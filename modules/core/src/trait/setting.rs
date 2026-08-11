//! Setting declarations: typed operator knobs.
//!
//! `[[setting]]` sections declare an operator-tunable value: a schema kind,
//! a default, and (for numbers) optional bounds. Declarations are pure data
//! and land inside the canonical digest; resolved values are computed at
//! ACTIVATION from config layers and never enter this model.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Declared scalar kind for a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum SettingSchema {
    Number,
    Text,
    Boolean,
}

/// A `[[setting]]` definition: a typed operator knob.
///
/// Declaration only — `default` is the fallback used when no config layer
/// overrides the value; the resolved value at activation is computed
/// elsewhere and never stored on this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Setting {
    /// Setting identifier (e.g. `"review-rounds"`).
    pub id: String,

    /// Declared scalar kind.
    pub schema: SettingSchema,

    /// Human-readable description of what this setting controls.
    pub description: String,

    /// Default value used when no config layer overrides it.
    pub default: JsonValue,

    /// Inclusive lower bound. Only valid for `schema = "number"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<serde_json::Number>,

    /// Inclusive upper bound. Only valid for `schema = "number"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<serde_json::Number>,
}

/// Validate a list of setting declarations.
///
/// Checks for:
/// - Empty or invalid setting IDs (must be valid kebab-case slugs usable as
///   `setting:<id>` refs)
/// - Duplicate setting IDs
/// - `default` matches the declared `schema` kind
/// - `min`/`max` are only present on `schema = "number"`, and `min <= default
///   <= max` when present
///
/// Returns the first error found, or `Ok(())` if all settings are valid.
/// All diagnostics use field-specific paths such as `setting[0].id`.
pub fn validate_settings(settings: &[Setting]) -> crate::Result<()> {
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (i, setting) in settings.iter().enumerate() {
        let id_path = format!("setting[{i}].id");
        crate::shared::validate_slug_shape(&setting.id, &id_path)?;

        if !seen.insert(setting.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate setting id {:?}", setting.id),
            }
            .into());
        }

        let default_path = format!("setting[{i}].default");
        let default_matches = match setting.schema {
            SettingSchema::Number => setting.default.is_number(),
            SettingSchema::Text => setting.default.is_string(),
            SettingSchema::Boolean => setting.default.is_boolean(),
        };
        if !default_matches {
            return Err(crate::manifest::Error::InvalidField {
                field_path: default_path,
                message: format!(
                    "default {:?} does not match declared schema {:?}",
                    setting.default, setting.schema
                ),
            }
            .into());
        }

        if setting.schema != SettingSchema::Number
            && (setting.min.is_some() || setting.max.is_some())
        {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("setting[{i}].min"),
                message: "min/max are only valid for schema = \"number\"".to_string(),
            }
            .into());
        }

        if let (Some(min), Some(max)) = (&setting.min, &setting.max) {
            let (min, max) = (
                min.as_f64().unwrap_or(f64::MIN),
                max.as_f64().unwrap_or(f64::MAX),
            );
            if min > max {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("setting[{i}].min"),
                    message: format!("min {min} is greater than max {max}"),
                }
                .into());
            }
        }

        if setting.schema == SettingSchema::Number
            && let Some(default_num) = setting.default.as_f64()
        {
            if let Some(min) = setting.min.as_ref().and_then(serde_json::Number::as_f64)
                && default_num < min
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: default_path.clone(),
                    message: format!("default {default_num} is below min {min}"),
                }
                .into());
            }
            if let Some(max) = setting.max.as_ref().and_then(serde_json::Number::as_f64)
                && default_num > max
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: default_path,
                    message: format!("default {default_num} is above max {max}"),
                }
                .into());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number_setting(default: JsonValue, min: Option<f64>, max: Option<f64>) -> Setting {
        Setting {
            id: "review-rounds".to_string(),
            schema: SettingSchema::Number,
            description: "Number of reviewer loop iterations.".to_string(),
            default,
            min: min.and_then(serde_json::Number::from_f64),
            max: max.and_then(serde_json::Number::from_f64),
        }
    }

    #[test]
    fn accepts_number_setting_with_default_inside_bounds() {
        assert!(
            validate_settings(&[number_setting(JsonValue::from(3), Some(1.0), Some(10.0))]).is_ok()
        );
    }

    #[test]
    fn rejects_duplicate_setting_ids() {
        let setting = number_setting(JsonValue::from(3), None, None);
        assert!(validate_settings(&[setting.clone(), setting]).is_err());
    }

    #[test]
    fn rejects_default_type_mismatch() {
        let mut setting = number_setting(JsonValue::from(3), None, None);
        setting.default = JsonValue::from("not-a-number");
        assert!(validate_settings(&[setting]).is_err());
    }

    #[test]
    fn rejects_bounds_on_non_number_schema() {
        let setting = Setting {
            id: "flag".to_string(),
            schema: SettingSchema::Boolean,
            description: "A flag.".to_string(),
            default: JsonValue::from(true),
            min: serde_json::Number::from_f64(0.0),
            max: None,
        };
        assert!(validate_settings(&[setting]).is_err());
    }

    #[test]
    fn rejects_default_below_min() {
        assert!(
            validate_settings(&[number_setting(JsonValue::from(0), Some(1.0), Some(10.0))])
                .is_err()
        );
    }

    #[test]
    fn rejects_default_above_max() {
        assert!(
            validate_settings(&[number_setting(JsonValue::from(11), Some(1.0), Some(10.0))])
                .is_err()
        );
    }

    #[test]
    fn rejects_fractional_default_bound_is_still_a_number_default() {
        // A fractional default is a valid `number` default at declaration
        // time; the loop-bound integerness rule is enforced at the
        // reference site (`validate.rs`), not here.
        assert!(
            validate_settings(&[number_setting(JsonValue::from(2.5), Some(1.0), Some(10.0))])
                .is_ok()
        );
    }
}
