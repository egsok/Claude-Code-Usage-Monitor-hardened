use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use super::{build_agent, parse_iso8601, windows_credentials, PollError};
use crate::diagnose;
use crate::models::{limit_slug, UsageData, UsageLimit, UsageSection};

const ANTIGRAVITY_CREDENTIAL_TARGET: &str = "gemini:antigravity";
const EXPIRY_SKEW: Duration = Duration::from_secs(60);
// The daily summary returns the "Gemini 4 Argon" group only for the agy CLI
// form of the UA; "antigravity", "antigravity/cli/0.0.1" and other forms omit it.
const ANTIGRAVITY_USER_AGENT: &str = "antigravity/cli/1.4.0";
const ANTIGRAVITY_ENDPOINTS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
];

#[derive(Deserialize)]
struct AntigravityAuthFile {
    token: AntigravityTokenData,
}

#[derive(Deserialize)]
struct AntigravityTokenData {
    access_token: String,
    expiry: Option<String>,
}

#[derive(Deserialize)]
struct AntigravityLoadResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityModelsResponse {
    pub(super) models: HashMap<String, AntigravityModelInfo>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityModelInfo {
    #[serde(rename = "quotaInfo")]
    pub(super) quota_info: Option<AntigravityQuotaInfo>,
}

#[derive(Clone, Deserialize)]
pub(super) struct AntigravityQuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityQuotaSummaryResponse {
    groups: Option<Vec<AntigravityQuotaSummaryGroup>>,
}

#[derive(Deserialize)]
pub(super) struct AntigravityQuotaSummaryGroup {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    description: Option<String>,
    buckets: Option<Vec<AntigravityQuotaSummaryBucket>>,
}

#[derive(Clone, Deserialize)]
pub(super) struct AntigravityQuotaSummaryBucket {
    #[serde(rename = "bucketId")]
    bucket_id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    window: Option<String>,
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

pub(super) fn poll_antigravity() -> Result<UsageData, PollError> {
    let creds = match read_antigravity_credentials() {
        Some(creds) => creds,
        None => {
            diagnose::log("Antigravity usage poll failed: no Antigravity credentials found");
            return Err(PollError::NoCredentials);
        }
    };

    poll_with_credentials(&creds, fetch_antigravity_usage)
}

fn poll_with_credentials(
    creds: &AntigravityTokenData,
    fetch: impl FnOnce(&str) -> Result<UsageData, PollError>,
) -> Result<UsageData, PollError> {
    if creds.access_token.is_empty() {
        return Err(PollError::AuthRequired);
    }
    if parse_iso8601(creds.expiry.as_deref())
        .is_some_and(|expiry| expiry <= SystemTime::now() + EXPIRY_SKEW)
    {
        return Err(PollError::TokenExpired);
    }
    fetch(&creds.access_token)
}

pub(super) fn antigravity_credential_watch_signature() -> String {
    let Some(content) = windows_credentials::read_generic(ANTIGRAVITY_CREDENTIAL_TARGET) else {
        return format!("{ANTIGRAVITY_CREDENTIAL_TARGET}|missing");
    };

    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!(
        "{ANTIGRAVITY_CREDENTIAL_TARGET}|present|{}|{}",
        content.len(),
        hasher.finish()
    )
}

pub(super) fn fetch_antigravity_usage(token: &str) -> Result<UsageData, PollError> {
    select_antigravity_usage(ANTIGRAVITY_ENDPOINTS, |base_url| {
        fetch_antigravity_usage_from_endpoint(base_url, token)
    })
}

pub(super) fn select_antigravity_usage(
    endpoints: &[&str],
    mut fetch: impl FnMut(&str) -> Result<(UsageData, AntigravitySource), PollError>,
) -> Result<UsageData, PollError> {
    let mut model_fallback: Option<UsageData> = None;
    let mut auth_error = false;
    let mut last_error = PollError::RequestFailed;

    for base_url in endpoints {
        match fetch(base_url) {
            // A quota summary names every window it has; it is authoritative
            // for both pools, so later endpoints are neither asked nor merged.
            Ok((data, AntigravitySource::Summary)) => return Ok(data),
            // Model quotas do not name their window; keep them only as a last
            // resort in case no endpoint answers with a summary.
            Ok((data, AntigravitySource::Models)) => {
                model_fallback.get_or_insert(data);
            }
            Err(PollError::AuthRequired) => auth_error = true,
            Err(error) => last_error = error,
        }
    }

    if let Some(data) = model_fallback {
        return Ok(data);
    }
    if auth_error {
        Err(PollError::AuthRequired)
    } else {
        Err(last_error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AntigravitySource {
    Summary,
    Models,
}

pub(super) fn fetch_antigravity_usage_from_endpoint(
    base_url: &str,
    token: &str,
) -> Result<(UsageData, AntigravitySource), PollError> {
    let project = fetch_antigravity_project(base_url, token)?;
    let summary_result = if let Some(project) = project.as_deref() {
        match fetch_antigravity_quota_summary(base_url, token, project) {
            Ok(data) => Some(data),
            Err(PollError::AuthRequired) => return Err(PollError::AuthRequired),
            Err(error) => {
                diagnose::log(format!(
                    "Antigravity retrieveUserQuotaSummary failed: {error:?}"
                ));
                None
            }
        }
    } else {
        None
    };

    // Per-model quotas do not say which window (5h or weekly) they belong to,
    // so when the summary answered, both pools are taken from it alone and the
    // models request is skipped.
    if let Some(data) = summary_result {
        return Ok((data, AntigravitySource::Summary));
    }
    let models_result = fetch_antigravity_models_response(base_url, token, project.as_deref());

    match models_result {
        Ok(models) => antigravity_usage_from_models(&models)
            .map(|data| (data, AntigravitySource::Models))
            .ok_or(PollError::RequestFailed),
        Err(err) => Err(err),
    }
}

/// Fallback when the quota summary is unavailable: Gemini models only.
pub(super) fn antigravity_usage_from_models(
    models: &AntigravityModelsResponse,
) -> Option<UsageData> {
    let gemini = best_gemini_model_quota(models);
    // Claude/GPT model quotas do not say whether they are the 5h or the weekly
    // window, so the external pool is left as "no limit data" here rather
    // than guessed.
    Some(UsageData {
        session: gemini?,
        ..Default::default()
    })
}

pub(super) fn best_gemini_model_quota(models: &AntigravityModelsResponse) -> Option<UsageSection> {
    best_antigravity_section(models.models.iter().filter_map(|(model, info)| {
        if !model.starts_with("gemini") {
            return None;
        }
        let quota = info.quota_info.as_ref().cloned()?;
        antigravity_section_from_quota(quota)
    }))
}

pub(super) fn fetch_antigravity_project(
    base_url: &str,
    token: &str,
) -> Result<Option<String>, PollError> {
    let agent = build_agent()?;
    let body = serde_json::json!({
        "metadata": {
            "ideType": "ANTIGRAVITY"
        }
    });

    let mut resp = match agent
        .post(&format!("{base_url}/v1internal:loadCodeAssist"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .send_json(&body)
        .and_then(super::check_http_status)
    {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(code)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Antigravity loadCodeAssist returned auth error status {code}"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity loadCodeAssist request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityLoadResponse = match resp.body_mut().read_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error("unable to parse Antigravity loadCodeAssist response", error);
            return Err(PollError::RequestFailed);
        }
    };

    Ok(response.project.filter(|project| !project.is_empty()))
}

pub(super) fn fetch_antigravity_models_response(
    base_url: &str,
    token: &str,
    project: Option<&str>,
) -> Result<AntigravityModelsResponse, PollError> {
    let agent = build_agent()?;
    let body = match project {
        Some(project) => serde_json::json!({ "project": project }),
        None => serde_json::json!({}),
    };

    let mut resp = match agent
        .post(&format!("{base_url}/v1internal:fetchAvailableModels"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .send_json(&body)
        .and_then(super::check_http_status)
    {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(code)) if code == 401 || code == 403 => {
            diagnose::log(format!(
                "Antigravity fetchAvailableModels returned auth error status {code}"
            ));
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity fetchAvailableModels request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityModelsResponse = match resp.body_mut().read_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error(
                "unable to parse Antigravity fetchAvailableModels response",
                error,
            );
            return Err(PollError::RequestFailed);
        }
    };

    Ok(response)
}

#[allow(dead_code)]
pub(super) fn fetch_antigravity_model_quota(
    base_url: &str,
    token: &str,
    project: Option<&str>,
) -> Result<UsageSection, PollError> {
    let response = fetch_antigravity_models_response(base_url, token, project)?;

    best_antigravity_section(response.models.into_iter().filter_map(|(model, info)| {
        let quota = info.quota_info?;
        if !is_antigravity_display_model(&model) {
            return None;
        }
        antigravity_section_from_quota(quota)
    }))
    .ok_or(PollError::RequestFailed)
}

pub(super) fn fetch_antigravity_quota_summary(
    base_url: &str,
    token: &str,
    project: &str,
) -> Result<UsageData, PollError> {
    let agent = build_agent()?;
    let body = serde_json::json!({ "project": project });

    let mut resp = match agent
        .post(&format!("{base_url}/v1internal:retrieveUserQuotaSummary"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("User-Agent", ANTIGRAVITY_USER_AGENT)
        .send_json(&body)
        .and_then(super::check_http_status)
    {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(code)) if code == 401 || code == 403 => {
            return Err(PollError::AuthRequired);
        }
        Err(error) => {
            diagnose::log_error("Antigravity retrieveUserQuotaSummary request failed", error);
            return Err(PollError::RequestFailed);
        }
    };

    let response: AntigravityQuotaSummaryResponse = match resp.body_mut().read_json() {
        Ok(response) => response,
        Err(error) => {
            diagnose::log_error(
                "unable to parse Antigravity retrieveUserQuotaSummary response",
                error,
            );
            return Err(PollError::RequestFailed);
        }
    };

    antigravity_usage_from_summary(response).ok_or(PollError::RequestFailed)
}

pub(super) fn antigravity_section_from_quota(quota: AntigravityQuotaInfo) -> Option<UsageSection> {
    let remaining = quota.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        available: true,
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(quota.reset_time.as_deref()),
    })
}

pub(super) fn antigravity_section_from_summary_bucket(
    bucket: &AntigravityQuotaSummaryBucket,
) -> Option<UsageSection> {
    let remaining = bucket.remaining_fraction?.clamp(0.0, 1.0);
    Some(UsageSection {
        available: true,
        percentage: (1.0 - remaining) * 100.0,
        resets_at: parse_iso8601(bucket.reset_time.as_deref()),
    })
}

pub(super) fn antigravity_usage_from_summary(
    response: AntigravityQuotaSummaryResponse,
) -> Option<UsageData> {
    let mut gemini_usage = None;
    let mut external_usage = None;
    let mut has_named_pool = false;
    let mut other_groups = Vec::new();

    // A summary that returns a group list (even an empty one) answered
    // authoritatively: no recognised window means "no limit data", not a
    // failure to be papered over with model quotas or another endpoint.
    let answered = response.groups.is_some();
    let groups = response.groups.unwrap_or_default();
    let limits = antigravity_limits_from_summary_groups(&groups);
    for group in groups {
        // Argon has its own pool; it must not stand in for the main Gemini pool.
        if is_antigravity_separate_gemini_group(&group) {
            // Its presence also rules out assigning an unrelated sole group
            // through the unnamed-pool fallback below.
            has_named_pool = true;
            continue;
        }
        let is_gemini = is_antigravity_gemini_summary_group(&group);
        let is_external = is_antigravity_external_summary_group(&group);
        has_named_pool |= is_gemini || is_external;
        let usage = antigravity_usage_from_summary_group(group);

        if is_gemini {
            if gemini_usage.is_none() {
                gemini_usage = usage;
            }
        } else if is_external {
            if external_usage.is_none() {
                external_usage = usage;
            }
        } else if let Some(usage) = usage {
            other_groups.push(usage);
        }
    }

    // An unrecognised group is assigned only when it is the sole group with
    // quota and no named pool exists; otherwise a position guess could show one
    // pool's usage under the other's label.
    if !has_named_pool && other_groups.len() == 1 {
        gemini_usage = other_groups.pop();
    }

    let mut usage = if let Some(mut gemini) = gemini_usage {
        if let Some(external) = external_usage {
            gemini.external_pool = Some(Box::new(external));
        }
        gemini
    } else if let Some(external) = external_usage {
        let mut gemini = UsageData::default();
        gemini.session.available = false;
        gemini.weekly.available = false;
        gemini.external_pool = Some(Box::new(external));
        gemini
    } else if answered {
        UsageData::default()
    } else {
        return None;
    };
    usage.limits = limits;
    Some(usage)
}

pub(super) fn is_antigravity_separate_gemini_group(group: &AntigravityQuotaSummaryGroup) -> bool {
    let is_argon = |value: Option<&str>| {
        value.is_some_and(|value| value.to_ascii_lowercase().contains("argon"))
    };
    is_argon(group.display_name.as_deref())
        || is_argon(group.description.as_deref())
        || group.buckets.as_ref().is_some_and(|buckets| {
            buckets.iter().any(|bucket| {
                is_argon(bucket.bucket_id.as_deref()) || is_argon(bucket.display_name.as_deref())
            })
        })
}

// The main and external pools show two groups only; themes need every group
// (e.g. "Gemini 4 Argon") and every window to show per-group quota.
pub(super) fn antigravity_limits_from_summary_groups(
    groups: &[AntigravityQuotaSummaryGroup],
) -> Vec<UsageLimit> {
    let mut limits = Vec::new();
    for group in groups {
        let Some(name) = group
            .display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        else {
            continue;
        };
        for bucket in group.buckets.iter().flatten() {
            let kind = match bucket.window.as_deref() {
                Some(window) if window.eq_ignore_ascii_case("5h") => "session",
                Some(window) if window.eq_ignore_ascii_case("weekly") => "weekly",
                _ => continue,
            };
            let Some(usage) = antigravity_section_from_summary_bucket(bucket) else {
                continue;
            };
            limits.push(UsageLimit {
                stale: false,
                key: format!("{}_{kind}", limit_slug(name)),
                kind: kind.to_string(),
                label: name.to_string(),
                model: Some(name.to_string()),
                model_id: bucket.bucket_id.clone(),
                scope: None,
                is_active: false,
                usage,
            });
        }
    }
    limits
}

pub(super) fn is_antigravity_external_summary_group(group: &AntigravityQuotaSummaryGroup) -> bool {
    group.display_name.as_deref().is_some_and(|name| {
        let n = name.to_ascii_lowercase();
        n.contains("claude") || n.contains("gpt") || n.contains("external")
    }) || group.description.as_deref().is_some_and(|description| {
        let d = description.to_ascii_lowercase();
        d.contains("claude") || d.contains("gpt") || d.contains("external")
    }) || group.buckets.as_ref().is_some_and(|buckets| {
        buckets.iter().any(|bucket| {
            bucket.bucket_id.as_deref().is_some_and(|id| {
                let id = id.to_ascii_lowercase();
                id.starts_with("claude-") || id.starts_with("gpt-") || id.starts_with("3p-")
            }) || bucket.display_name.as_deref().is_some_and(|name| {
                let n = name.to_ascii_lowercase();
                n.contains("claude") || n.contains("gpt")
            })
        })
    })
}

pub(super) fn antigravity_usage_from_summary_group(
    group: AntigravityQuotaSummaryGroup,
) -> Option<UsageData> {
    let mut data = UsageData::default();
    let mut has_quota = false;

    for bucket in group.buckets.unwrap_or_default() {
        let Some(section) = antigravity_section_from_summary_bucket(&bucket) else {
            continue;
        };

        match bucket.window.as_deref() {
            Some(window) if window.eq_ignore_ascii_case("5h") => {
                data.session = section;
                has_quota = true;
            }
            Some(window) if window.eq_ignore_ascii_case("weekly") => {
                data.weekly = section;
                has_quota = true;
            }
            _ => {}
        }
    }

    has_quota.then_some(data)
}

pub(super) fn is_antigravity_gemini_summary_group(group: &AntigravityQuotaSummaryGroup) -> bool {
    group
        .display_name
        .as_deref()
        .is_some_and(|name| name.to_ascii_lowercase().contains("gemini"))
        || group
            .description
            .as_deref()
            .is_some_and(|description| description.to_ascii_lowercase().contains("gemini"))
        || group.buckets.as_ref().is_some_and(|buckets| {
            buckets.iter().any(|bucket| {
                bucket
                    .bucket_id
                    .as_deref()
                    .is_some_and(|id| id.to_ascii_lowercase().starts_with("gemini-"))
                    || bucket
                        .display_name
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains("gemini"))
            })
        })
}

pub(super) fn best_antigravity_section<I>(sections: I) -> Option<UsageSection>
where
    I: IntoIterator<Item = UsageSection>,
{
    sections.into_iter().max_by(|a, b| {
        a.percentage
            .partial_cmp(&b.percentage)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.resets_at.cmp(&b.resets_at))
    })
}

pub(super) fn is_antigravity_display_model(model: &str) -> bool {
    model.starts_with("gemini")
        || model.starts_with("claude")
        || model.starts_with("gpt")
        || model.starts_with("image")
        || model.starts_with("imagen")
}

fn read_antigravity_credentials() -> Option<AntigravityTokenData> {
    let content = windows_credentials::read_generic(ANTIGRAVITY_CREDENTIAL_TARGET)?;
    let auth: AntigravityAuthFile = serde_json::from_str(&content).ok()?;
    (!auth.token.access_token.is_empty()).then_some(auth.token)
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    #[test]
    fn expired_tokens_wait_for_the_provider_without_a_usage_request() {
        let creds = AntigravityTokenData {
            access_token: "expired".into(),
            expiry: Some("2020-01-01T00:00:00Z".into()),
        };
        assert_eq!(
            poll_with_credentials(&creds, |_| panic!("expired token must stay passive")),
            Err(PollError::TokenExpired)
        );
    }
    #[test]
    fn provider_auth_errors_are_not_retried_or_refreshed() {
        let creds = AntigravityTokenData {
            access_token: "existing".into(),
            expiry: None,
        };
        let mut calls = 0;
        let result = poll_with_credentials(&creds, |token| {
            calls += 1;
            assert_eq!(token, "existing");
            Err(PollError::AuthRequired)
        });
        assert_eq!(result, Err(PollError::AuthRequired));
        assert_eq!(calls, 1);
    }
    #[test]
    fn credential_files_with_refresh_tokens_remain_readable_but_unused() {
        let auth: AntigravityAuthFile = serde_json::from_str(
            r#"{"token":{"access_token":"existing","refresh_token":"unused"}}"#,
        )
        .unwrap();
        assert!(poll_with_credentials(&auth.token, |_| Ok(UsageData::default())).is_ok());
    }
}
