// Pure provider-error evidence classification for harness output.
//
// Provider-level failure detected inside otherwise raw harness output.
//
// A model the harness cannot resolve, or a provider account that has run out
// of credits or quota, answers every retry identically: the harness never
// spawns a real turn, so a correction retry only burns budget (wall-clock
// for `InvalidModel`, the account's dying balance for `CreditsExhausted`)
// confirming what the first attempt already proved. Classifying these here
// lets a caller (`ctx-traits-cli`'s drive loop) skip retries entirely for
// them instead of exhausting the correction budget.
//
// Extraction into pure evidence (already-decoded harness stream events plus
// raw stdout/stderr and exit status) happens at the IO/CLI boundary, which
// alone knows how to parse a given harness's NDJSON/balanced-JSON output
// shape. This module performs no parsing beyond string/JSON inspection and
// no filesystem, process, or network access.

use crate::procedure::activity::RateLimitObservation;

/// Bounded, provider-attributable evidence that an account ran out of
/// payment credits or usage quota.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditsExhaustedEvidence {
    pub provider: String,
    pub top_up_url: String,
    pub detail: String,
}

/// Bounded, provider-attributable evidence that a subscription harness
/// rejected a turn on usage/rate-limit pressure (P556/0117) — distinct from
/// [`CreditsExhaustedEvidence`], which carries a top-up URL for API-key
/// billing. A subscription limit carries a reset instant instead.
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionLimitEvidence {
    pub limit_type: Option<String>,
    pub utilization: Option<f64>,
    pub resets_at_epoch: u64,
    pub detail: String,
}

/// Result of inspecting harness evidence for a provider-reported failure.
/// `InvalidModel`, `CreditsExhausted`, and `RateLimited` are the
/// non-retryable cases: every retry would answer identically without ever
/// spawning a real turn. `Generic` covers all other provider-flagged
/// failures, which retain `harness-failed` handling.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderErrorClassification {
    InvalidModel(String),
    CreditsExhausted(CreditsExhaustedEvidence),
    RateLimited(SubscriptionLimitEvidence),
    Generic(String),
}

/// Already-decoded harness stream events (NDJSON/balanced-JSON extracted at
/// the IO/CLI boundary) plus the harness's raw stdout/stderr text and exit
/// status. `stream_events` is populated only for harness output shapes whose
/// caller knows how to decode them (e.g. `claude-stream-json`); other output
/// ids pass an empty slice and rely on the raw-text scan below.
pub struct ProviderErrorEvidence<'a> {
    pub output_id: &'a str,
    pub stream_events: &'a [JsonValue],
    pub stdout: &'a str,
    pub stderr: &'a str,
    pub exit_code: Option<i32>,
}

/// Bounded provider signature table: a structured, provider-unique substring
/// paired with the account's top-up URL. Deliberately narrow — a broad
/// billing-adjacent substring would let a frame whose own subject matter is
/// provider-error handling misclassify its own successful result (the same
/// failure mode the `claude-stream-json` billing-text branch below already
/// guards against with zero-work corroboration).
const CREDIT_SIGNATURES: &[(&str, &str, &str)] = &[
    (
        "openrouter",
        "requires more credits",
        "https://openrouter.ai/credits",
    ),
    (
        "openai",
        "insufficient_quota",
        "https://platform.openai.com/account/billing",
    ),
];

fn classify_credits_exhausted_text(text: &str) -> Option<CreditsExhaustedEvidence> {
    let lower = text.to_ascii_lowercase();
    for (provider, signature, top_up_url) in CREDIT_SIGNATURES {
        if lower.contains(signature) {
            return Some(CreditsExhaustedEvidence {
                provider: (*provider).to_string(),
                top_up_url: (*top_up_url).to_string(),
                detail: text.trim().chars().take(200).collect(),
            });
        }
    }
    None
}

/// A harness whose process exits 0 never reaches this branch, so a
/// successful run that merely discusses one of the bounded signatures in its
/// own (paid, completed) result text cannot be misclassified — the same
/// zero-work-corroboration guard the `claude-stream-json` billing branch
/// below applies for its own, differently-shaped success signal. This is
/// what makes OpenCode-style failures (which report via nonzero exit and
/// never reach the successful-result parser) classifiable at all.
///
/// Only a *concrete* nonzero exit status counts as provider-failure
/// corroboration. `exit_code: None` means the process was killed by a
/// timeout or signal rather than returning its own status — that case must
/// retain the drive loop's existing timeout/idle-timeout handling instead of
/// being reinterpreted as a resumable credits pause from partial output.
fn classify_credits_exhausted_from_exit_failure(
    evidence: &ProviderErrorEvidence<'_>,
) -> Option<CreditsExhaustedEvidence> {
    match evidence.exit_code {
        Some(0) | None => return None,
        Some(_) => {}
    }
    classify_credits_exhausted_text(evidence.stdout)
        .or_else(|| classify_credits_exhausted_text(evidence.stderr))
}

pub fn classify_provider_error(
    evidence: &ProviderErrorEvidence<'_>,
) -> Option<ProviderErrorClassification> {
    if let Some(credits) = classify_credits_exhausted_from_exit_failure(evidence) {
        return Some(ProviderErrorClassification::CreditsExhausted(credits));
    }
    if evidence.output_id != "claude-stream-json" {
        return None;
    }
    classify_claude_stream_error(evidence.stream_events)
}

/// claude surfaces account and model-resolution failures as the stream's
/// `result` event — observed live as `"Credit balance is too low"` result
/// text, and as a `model_not_found` / HTTP 404 API error for an unresolved
/// model — which the slot parser then rejects as invalid output, and drive
/// retries a frame the provider will answer identically. Detect the
/// error-flagged result event (or the known billing text on a nominal one) so
/// drive can fail with the provider's own message instead, classifying the
/// definitive model-resolution and credit-exhaustion signatures separately so
/// callers can skip retries entirely for them.
///
/// An unresolved model is sometimes split across two stream events (observed
/// live in run 3a48a246): the preceding `assistant` event carries a top-level
/// `"error": "model_not_found"` string marker, while the `result` event that
/// follows carries only the corroborating `is_error`/`api_error_status: 404`
/// flags without repeating `model_not_found` in its result text. Track that
/// marker across the whole stream so it can still be correlated once the
/// result event is reached.
fn classify_claude_stream_error(stream_events: &[JsonValue]) -> Option<ProviderErrorClassification> {
    let mut model_not_found_marker = false;
    // Last rejected `rate_limit_info` observation, in stream order — a
    // subscription harness can reject on more than one limit type across a
    // stream; `rejected_last` tracks whichever rejection was observed most
    // recently so the pause names the type that actually caused this lost
    // turn (P556/0117). `latest_rate_limit_observations` below separately
    // captures every observed limit type (any status) for ledger evidence.
    let mut rejected_last: Option<RateLimitObservation> = None;
    for value in stream_events {
        if !model_not_found_marker {
            model_not_found_marker = value
                .get("error")
                .and_then(JsonValue::as_str)
                .is_some_and(|error| error.eq_ignore_ascii_case("model_not_found"));
        }
        if value.get("type").and_then(JsonValue::as_str) == Some("rate_limit_event") {
            if let Some(observation) = value
                .get("rate_limit_info")
                .and_then(RateLimitObservation::decode)
                && observation.status == "rejected"
            {
                rejected_last = Some(observation);
            }
            continue;
        }
        if value.get("type").and_then(JsonValue::as_str) != Some("result") {
            continue;
        }
        let subtype = value.get("subtype").and_then(JsonValue::as_str);
        let flagged = value
            .get("is_error")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false)
            || subtype.is_some_and(|subtype| subtype.starts_with("error"));
        let text = value
            .get("result")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let billing_text = text
            .to_ascii_lowercase()
            .contains("credit balance is too low");
        // Content sniffing alone must never classify a SUCCESSFUL result as
        // a billing error: a frame whose subject matter is provider-error
        // handling legitimately quotes the phrase in its result text (run
        // AYCguytU deadlocked on exactly this — every retry re-matched its
        // own work summary). A genuine credit refusal never completes real
        // work, so an unflagged event needs zero-work corroboration.
        let zero_work = value
            .get("total_cost_usd")
            .and_then(JsonValue::as_f64)
            .is_none_or(|cost| cost == 0.0)
            || value
                .get("num_turns")
                .and_then(JsonValue::as_i64)
                .is_none_or(|turns| turns <= 1);
        let billing = billing_text && (flagged || zero_work);
        // A rejected rate-limit event corroborated by this result losing the
        // turn (error-flagged, or zero-work — the same guard `billing` uses
        // above) is the AYCguytU-shaped check: an earlier transient
        // `rate_limit_event` must never reclassify a result that went on to
        // do real, successful work.
        let rate_limited = rejected_last.is_some() && (flagged || zero_work);
        if !flagged && !billing && !rate_limited {
            return None;
        }
        if model_not_found_marker || is_model_resolution_error(value, text) {
            let detail = if text.is_empty() {
                "model_not_found".to_string()
            } else {
                text.chars().take(200).collect()
            };
            return Some(ProviderErrorClassification::InvalidModel(detail));
        }
        if rate_limited && let Some(latest) = &rejected_last {
            let detail = if text.is_empty() {
                format!(
                    "rate limit rejected ({})",
                    latest.limit_type.as_deref().unwrap_or("unknown")
                )
            } else {
                text.chars().take(200).collect()
            };
            return Some(ProviderErrorClassification::RateLimited(
                SubscriptionLimitEvidence {
                    limit_type: latest.limit_type.clone(),
                    utilization: latest.utilization,
                    resets_at_epoch: latest.resets_at_epoch,
                    detail,
                },
            ));
        }
        if billing {
            let detail = if text.is_empty() {
                "credit balance is too low".to_string()
            } else {
                text.chars().take(200).collect()
            };
            return Some(ProviderErrorClassification::CreditsExhausted(
                CreditsExhaustedEvidence {
                    provider: "claude".to_string(),
                    top_up_url: "https://console.anthropic.com/settings/billing".to_string(),
                    detail,
                },
            ));
        }
        if text.is_empty() {
            return Some(ProviderErrorClassification::Generic(format!(
                "provider reported {} with no result text",
                subtype.unwrap_or("an error")
            )));
        }
        return Some(ProviderErrorClassification::Generic(
            text.chars().take(200).collect(),
        ));
    }
    None
}

/// Latest rate-limit observation per limit type, across every status
/// (`allowed`, `allowed_warning`, `rejected`) — unlike
/// [`classify_claude_stream_error`], which only acts on rejections that
/// corroborate a lost turn. Lets a caller persist run pressure evidence into
/// the ledger even when the drive never paused (P556/0117 done-when).
pub fn latest_rate_limit_observations(
    stream_events: &[JsonValue],
) -> BTreeMap<String, RateLimitObservation> {
    let mut latest: BTreeMap<String, RateLimitObservation> = BTreeMap::new();
    for value in stream_events {
        if value.get("type").and_then(JsonValue::as_str) != Some("rate_limit_event") {
            continue;
        }
        if let Some(observation) = value
            .get("rate_limit_info")
            .and_then(RateLimitObservation::decode)
        {
            let key = observation
                .limit_type
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            latest.insert(key, observation);
        }
    }
    latest
}

/// Definitive structured evidence that claude could not resolve the
/// requested model: either an explicit `model_not_found` error code/type, or
/// the pairing of an HTTP 404 API error status with `model_not_found` in the
/// result text. Deliberately narrower than a generic 404 or stderr scan —
/// those can be produced by unrelated failures and would misclassify them as
/// non-retryable.
fn is_model_resolution_error(value: &JsonValue, text: &str) -> bool {
    let structured_code = value
        .get("error")
        .and_then(|error| error.get("type").or_else(|| error.get("code")))
        .and_then(JsonValue::as_str)
        .is_some_and(|code| code.eq_ignore_ascii_case("model_not_found"));
    let status_404 = value.get("api_error_status").and_then(JsonValue::as_i64) == Some(404);
    structured_code || (status_404 && text.contains("model_not_found"))
}

/// Typed context persisted on a session's `last-drive-outcome` when a drive
/// pauses for exhausted provider credits — enough for a resumed `ctx traits
/// drive <session>` invocation's caller, or an operator reading the ledger,
/// to know which provider, role, and frame paused without re-deriving it from
/// a harness transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ProviderCreditsPause {
    pub provider: String,
    pub role: String,
    pub frame_title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_item_id: Option<String>,
    /// The dispatched frame's `run_index` — a stable, always-present
    /// position (every `Step`/`Command` frame carries `Some(run_index)`;
    /// only the never-dispatched `Intro` frame omits it) used to identify
    /// the paused frame when both `frame_title` and `frame_item_id` are
    /// absent, so two different unnamed frames never render the same label.
    #[serde(default)]
    pub frame_run_index: usize,
    /// Absent for a subscription rate-limit pause (there is no top-up URL to
    /// offer); always present for an API-key credits pause. `default` +
    /// `skip_serializing_if` so a ledger written before this field was
    /// optional still deserializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_up_url: Option<String>,
    pub detail: String,
    /// The rejected limit's reset instant, epoch seconds (P556/0117). Never
    /// convert to a duration — only `Some` for a subscription rate-limit
    /// pause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence<'a>(
        output_id: &'a str,
        stream_events: &'a [JsonValue],
        stdout: &'a str,
        stderr: &'a str,
        exit_code: Option<i32>,
    ) -> ProviderErrorEvidence<'a> {
        ProviderErrorEvidence {
            output_id,
            stream_events,
            stdout,
            stderr,
            exit_code,
        }
    }

    #[test]
    fn successful_result_quoting_billing_phrase_is_not_an_error() {
        // A frame working ON provider-error classification quotes the billing
        // phrase in its (successful, costly, multi-turn) result — run
        // AYCguytU deadlocked when this classified as a provider error.
        let stdout = r#"{"type":"result","is_error":false,"num_turns":42,"total_cost_usd":1.1,"result":"work summary: detector now matches \"credit balance is too low\" refusals"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        assert!(
            classify_provider_error(&evidence(
                "claude-stream-json",
                &events,
                stdout,
                "",
                Some(0)
            ))
            .is_none()
        );
    }

    #[test]
    fn zero_work_billing_refusal_classifies_as_credits_exhausted() {
        let stdout = r#"{"type":"result","is_error":false,"num_turns":1,"total_cost_usd":0.0,"result":"Credit balance is too low"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(0),
        )) {
            Some(ProviderErrorClassification::CreditsExhausted(evidence)) => {
                assert_eq!(evidence.provider, "claude");
            }
            other => panic!("expected CreditsExhausted classification, got {other:?}"),
        }
    }

    #[test]
    fn classifies_model_not_found_result_event_as_invalid_model() {
        let stdout = r#"{"type":"result","is_error":true,"api_error_status":404,"result":"API Error: 404 model_not_found: the requested model does not exist"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(0),
        )) {
            Some(ProviderErrorClassification::InvalidModel(detail)) => {
                assert!(detail.contains("model_not_found"));
            }
            other => panic!("expected InvalidModel classification, got {other:?}"),
        }
    }

    #[test]
    fn classifies_structured_model_not_found_error_code_as_invalid_model() {
        let stdout = r#"{"type":"result","is_error":true,"error":{"type":"model_not_found","message":"no such model"},"result":"model resolution failed"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(0),
        )) {
            Some(ProviderErrorClassification::InvalidModel(_)) => {}
            other => panic!("expected InvalidModel classification, got {other:?}"),
        }
    }

    #[test]
    fn classifies_claude_billing_error_as_credits_exhausted() {
        let stdout = r#"{"type":"result","is_error":false,"result":"Credit balance is too low to make this request"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(0),
        )) {
            Some(ProviderErrorClassification::CreditsExhausted(evidence)) => {
                assert!(evidence.detail.contains("Credit balance"));
                assert_eq!(evidence.provider, "claude");
            }
            other => panic!("expected CreditsExhausted classification, got {other:?}"),
        }
    }

    #[test]
    fn does_not_classify_a_generic_404_without_model_not_found_marker() {
        let stdout = r#"{"type":"result","is_error":true,"api_error_status":404,"result":"resource not found"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(0),
        )) {
            Some(ProviderErrorClassification::Generic(_)) => {}
            other => panic!("expected Generic classification, got {other:?}"),
        }
    }

    #[test]
    fn valid_result_event_is_not_classified_as_an_error() {
        let stdout = r#"{"type":"result","is_error":false,"result":"{\"ok\":true}"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        assert!(
            classify_provider_error(&evidence(
                "claude-stream-json",
                &events,
                stdout,
                "",
                Some(0)
            ))
            .is_none()
        );
    }

    #[test]
    fn non_claude_output_id_is_never_classified_from_stream_events() {
        let stdout = r#"{"type":"result","is_error":true,"api_error_status":404,"result":"model_not_found"}"#;
        let events = vec![serde_json::from_str::<JsonValue>(stdout).unwrap()];
        assert!(
            classify_provider_error(&evidence("opencode-json", &events, stdout, "", Some(0)))
                .is_none()
        );
    }

    #[test]
    fn classifies_split_assistant_marker_and_result_404_as_invalid_model() {
        // Exact shape observed in run 3a48a246 (007-summarization-scribe):
        // the `model_not_found` marker lands as a top-level string on the
        // assistant event, not inside the result event's error object, and
        // the result text never repeats the "model_not_found" substring.
        let stdout = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"There's an issue with the selected model."}]},"error":"model_not_found"}"#,
            "\n",
            r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":404,"result":"There's an issue with the selected model. It may not exist or you may not have access to it.","total_cost_usd":0,"num_turns":1}"#,
        );
        let events: Vec<JsonValue> = stdout
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).unwrap())
            .collect();
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(0),
        )) {
            Some(ProviderErrorClassification::InvalidModel(_)) => {}
            other => panic!("expected InvalidModel classification, got {other:?}"),
        }
    }

    #[test]
    fn rejected_rate_limit_with_lost_turn_classifies_as_rate_limited() {
        // Verbatim shape from trace run-5a1316833f08.../004-implement-worker
        // (0117): rejected `rate_limit_event`, synthetic assistant message,
        // then a zero-cost, single-turn, error-flagged `result`.
        let stdout = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1784836800,"rateLimitType":"five_hour","overageStatus":"rejected","overageDisabledReason":"org_level_disabled","isUsingOverage":false},"uuid":"u1","session_id":"s1"}"#,
            "\n",
            r#"{"type":"assistant","message":{"model":"<synthetic>","content":[]}}"#,
            "\n",
            r#"{"type":"result","is_error":true,"total_cost_usd":0,"num_turns":1,"result":""}"#,
        );
        let events: Vec<JsonValue> = stdout
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).unwrap())
            .collect();
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(1),
        )) {
            Some(ProviderErrorClassification::RateLimited(evidence)) => {
                assert_eq!(evidence.resets_at_epoch, 1_784_836_800);
                assert_eq!(evidence.limit_type.as_deref(), Some("five_hour"));
            }
            other => panic!("expected RateLimited classification, got {other:?}"),
        }
    }

    #[test]
    fn rejected_rate_limit_before_a_successful_costly_result_is_not_reclassified() {
        // AYCguytU-shaped guard (0117): a transient rejected rate-limit
        // event earlier in the stream must never reclassify a result that
        // went on to do real, successful, multi-turn work.
        let stdout = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1784836800,"rateLimitType":"five_hour"},"uuid":"u1","session_id":"s1"}"#,
            "\n",
            r#"{"type":"result","is_error":false,"num_turns":12,"total_cost_usd":0.42,"result":"done"}"#,
        );
        let events: Vec<JsonValue> = stdout
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).unwrap())
            .collect();
        assert!(
            classify_provider_error(&evidence(
                "claude-stream-json",
                &events,
                stdout,
                "",
                Some(0)
            ))
            .is_none()
        );
    }

    #[test]
    fn rate_limited_names_the_latest_rejected_type_across_several() {
        let stdout = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":100,"rateLimitType":"five_hour"},"uuid":"u1","session_id":"s1"}"#,
            "\n",
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":200,"rateLimitType":"seven_day"},"uuid":"u2","session_id":"s1"}"#,
            "\n",
            r#"{"type":"result","is_error":true,"total_cost_usd":0,"num_turns":1,"result":""}"#,
        );
        let events: Vec<JsonValue> = stdout
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).unwrap())
            .collect();
        match classify_provider_error(&evidence(
            "claude-stream-json",
            &events,
            stdout,
            "",
            Some(1),
        )) {
            Some(ProviderErrorClassification::RateLimited(evidence)) => {
                assert_eq!(evidence.limit_type.as_deref(), Some("seven_day"));
                assert_eq!(evidence.resets_at_epoch, 200);
            }
            other => panic!("expected RateLimited classification, got {other:?}"),
        }
    }

    #[test]
    fn latest_rate_limit_observations_keeps_every_type_regardless_of_status() {
        let stdout = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":100,"rateLimitType":"five_hour","utilization":0.9}}"#,
            "\n",
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":200,"rateLimitType":"seven_day"}}"#,
        );
        let events: Vec<JsonValue> = stdout
            .lines()
            .map(|line| serde_json::from_str::<JsonValue>(line).unwrap())
            .collect();
        let observations = latest_rate_limit_observations(&events);
        assert_eq!(observations.len(), 2);
        assert_eq!(observations["five_hour"].utilization, Some(0.9));
        assert_eq!(observations["seven_day"].resets_at_epoch, 200);
    }

    #[test]
    fn pre_0117_pause_json_with_a_required_string_top_up_url_still_deserializes() {
        // Every pre-0117 ledger wrote `top_up_url` as a plain required
        // string; the relaxed `Option<String>` must still decode it.
        let json = r#"{
            "provider": "claude",
            "role": "default",
            "frame-title": "step",
            "frame-run-index": 0,
            "top-up-url": "https://console.anthropic.com/settings/billing",
            "detail": "Credit balance is too low"
        }"#;
        let pause: ProviderCreditsPause = serde_json::from_str(json).expect("decodes");
        assert_eq!(
            pause.top_up_url.as_deref(),
            Some("https://console.anthropic.com/settings/billing")
        );
        assert_eq!(pause.resets_at_epoch, None);
        assert_eq!(pause.limit_type, None);
    }

    #[test]
    fn subscription_pause_json_omits_top_up_url_and_carries_a_reset_epoch() {
        let json = r#"{
            "provider": "claude",
            "role": "default",
            "frame-title": "step",
            "frame-run-index": 0,
            "detail": "rate limit rejected (five_hour)",
            "resets-at-epoch": 1784836800,
            "limit-type": "five_hour"
        }"#;
        let pause: ProviderCreditsPause = serde_json::from_str(json).expect("decodes");
        assert_eq!(pause.top_up_url, None);
        assert_eq!(pause.resets_at_epoch, Some(1_784_836_800));
        assert_eq!(pause.limit_type.as_deref(), Some("five_hour"));
    }

    #[test]
    fn decode_tolerates_a_missing_utilization_and_rate_limit_type() {
        // 8 events observed in this repo's own traces with no `rateLimitType`
        // at all; `utilization` is likewise absent on many `allowed` events.
        let value: JsonValue =
            serde_json::from_str(r#"{"status":"allowed","resetsAt":1784836800}"#).unwrap();
        let observation = RateLimitObservation::decode(&value).expect("decodes");
        assert_eq!(observation.limit_type, None);
        assert_eq!(observation.utilization, None);
        assert_eq!(observation.resets_at_epoch, 1_784_836_800);
    }
}
