use std::time::Duration;

use semver::Version;
use serde::Serialize;
use serde_json::json;
use tauri::AppHandle;

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/nao7sep/onecopy/releases/latest";
const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
const USER_AGENT: &str = "OneCopy";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u64 = 16 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ReleaseCheckOutcome {
    Newer {
        version: String,
        attempted_at_utc: String,
    },
    Current {
        published_version: String,
        attempted_at_utc: String,
    },
    Failed {
        attempted_at_utc: String,
    },
}

fn published_version(tag: &str) -> Result<Version, String> {
    let value = tag
        .strip_prefix('v')
        .ok_or_else(|| "latest release tag does not start with v".to_string())?;
    let version = Version::parse(value).map_err(|error| format!("invalid release tag: {error}"))?;
    if !version.pre.is_empty()
        || !version.build.is_empty()
        || tag != format!("v{}.{}.{}", version.major, version.minor, version.patch)
    {
        return Err("latest release tag is not vX.Y.Z".to_string());
    }
    Ok(version)
}

fn compare_tag(tag: &str, installed: &Version) -> Result<(bool, String), String> {
    let published = published_version(tag)?;
    Ok((published > *installed, published.to_string()))
}

async fn request_latest_tag() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("build GitHub release client: {error}"))?;
    let response = client
        .get(LATEST_RELEASE_API)
        .header(reqwest::header::ACCEPT, GITHUB_ACCEPT)
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .map_err(|error| format!("request latest GitHub release: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub release response status: {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES)
    {
        return Err("GitHub release response is too large".to_string());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("read GitHub release response: {error}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("GitHub release response is too large".to_string());
    }
    parse_latest_tag(&bytes)
}

fn parse_latest_tag(bytes: &[u8]) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("parse GitHub release response: {error}"))?;
    value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "GitHub release response has no tag_name".to_string())
}

async fn run_check(app: &AppHandle) -> Result<ReleaseCheckOutcome, String> {
    let attempted_at_utc = crate::logging::now_iso_millis();
    let saved = crate::storage::patch_state(
        app,
        &json!({ "githubReleaseLastAttemptAtUtc": attempted_at_utc }),
    )?;
    if let Some(record) = saved.quarantined {
        crate::failure_runtime::emit_or_record(
            app,
            "storage://quarantined",
            json!({ "quarantines": [record] }),
        );
    }
    crate::logging::info("GitHub release check started", json!({}));
    let result = async {
        let tag = request_latest_tag().await?;
        let installed = Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| format!("invalid installed version: {error}"))?;
        compare_tag(&tag, &installed)
    }
    .await;
    match result {
        Ok((true, version)) => {
            crate::logging::info(
                "GitHub release check completed",
                json!({ "newer": true, "version": version }),
            );
            Ok(ReleaseCheckOutcome::Newer {
                version,
                attempted_at_utc,
            })
        }
        Ok((false, published_version)) => {
            crate::logging::info(
                "GitHub release check completed",
                json!({ "newer": false, "publishedVersion": published_version }),
            );
            Ok(ReleaseCheckOutcome::Current {
                published_version,
                attempted_at_utc,
            })
        }
        Err(error) => {
            crate::logging::warn(
                "GitHub release check failed",
                json!({ "error": { "message": error } }),
            );
            Ok(ReleaseCheckOutcome::Failed { attempted_at_utc })
        }
    }
}

type SharedResult = Result<ReleaseCheckOutcome, String>;
static IN_FLIGHT: std::sync::LazyLock<
    std::sync::Mutex<Option<tokio::sync::watch::Receiver<Option<SharedResult>>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// The frontend also joins ordinary same-webview callers, while this boundary
/// preserves the invariant across a renderer reload during an active command.
pub async fn check(app: &AppHandle) -> SharedResult {
    let mut receiver = {
        let mut active = IN_FLIGHT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(receiver) = active.as_ref() {
            receiver.clone()
        } else {
            let (sender, receiver) = tokio::sync::watch::channel(None);
            *active = Some(receiver.clone());
            let owned_app = app.clone();
            tokio::spawn(async move {
                let result = run_check(&owned_app).await;
                let _ = sender.send(Some(result));
                *IN_FLIGHT
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            });
            receiver
        }
    };
    let result = receiver
        .wait_for(Option::is_some)
        .await
        .map_err(|_| "GitHub release check ended without a result".to_string())?
        .clone()
        .expect("wait_for accepted only a present release result");
    result
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises tag parsing, comparison
// and request constants of a module that is private to the crate.
#[path = "../tests/unit/github_release.rs"]
mod tests;
