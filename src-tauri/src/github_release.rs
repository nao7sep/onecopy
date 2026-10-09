use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
// Sized to the endpoint's real payload: an ordinary release with a handful of
// assets and Keep-a-Changelog release notes runs well past 16 KiB (measured up
// to ~36 KiB for 14 assets), and the parser only ever reads `tag_name` out of
// whatever arrives. 1 MiB still bounds memory against a hostile or broken
// response while never rejecting a normal release (R6-06).
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

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

/// Sends the one and only request a release check makes. `url` is the
/// endpoint to hit — always `LATEST_RELEASE_API` in the running app — kept as
/// a parameter so a test can point this exact request-building and
/// response-handling code at a local listener instead of GitHub, with no
/// fake GitHub-shaped server standing behind it: headers, the absent
/// Authorization header, the bounded timeout, and the single attempt (no
/// retry loop exists here or anywhere above this function) are all real
/// behavior of this one function, not restated constants.
async fn request_latest_tag(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("build GitHub release client: {error}"))?;
    let response = client
        .get(url)
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

/// Runs `write_timestamp` to completion first, then `request`. The marker
/// throttles automatic checks across launches, so an automatic check whose
/// marker could not be saved sends no request; a manual one (`manual()` is
/// read when the write has failed, so a click that joined meanwhile counts)
/// logs the failure and still asks. Isolated from `AppHandle`-bound state
/// persistence and HTTP transport so a test can drive both sides with local
/// futures and prove the ordering rather than merely read it.
async fn write_attempt_marker_then<T>(
    write_timestamp: impl std::future::Future<Output = Result<(), String>>,
    manual: impl FnOnce() -> bool,
    request: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    if let Err(error) = write_timestamp.await {
        if !manual() {
            return Err(error);
        }
        crate::logging::warn(
            "release check attempt not saved; the manual check continues",
            json!({ "error": { "message": error } }),
        );
    }
    request.await
}

async fn run_check(_app: &AppHandle, manual: &AtomicBool) -> Result<ReleaseCheckOutcome, String> {
    let attempted_at_utc = crate::logging::now_iso_millis();
    // The facts-file write is filesystem I/O; this function runs on a tokio
    // worker (spawned by `check()` below), so it goes through the same
    // blocking-pool dispatch every other filesystem write in the app uses
    // instead of blocking the async worker directly.
    let attempt_marker = attempted_at_utc.clone();
    let write_timestamp = async {
        crate::dispatch(move || {
            let root = crate::paths::data_root()?;
            crate::binaries_manager::save_check_attempt(
                &root,
                crate::binaries_manager::GITHUB_RELEASE_ATTEMPT_KEY,
                &attempt_marker,
            )
        })
        .await?;
        crate::logging::info("GitHub release check started", json!({}));
        Ok(())
    };
    let result = write_attempt_marker_then(write_timestamp, || manual.load(Ordering::SeqCst), async {
        let tag = request_latest_tag(LATEST_RELEASE_API).await?;
        let installed = Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|error| format!("invalid installed version: {error}"))?;
        compare_tag(&tag, &installed)
    })
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
type InFlight = (tokio::sync::watch::Receiver<Option<SharedResult>>, Arc<AtomicBool>);
static IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<Option<InFlight>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// The frontend also joins ordinary same-webview callers, while this boundary
/// preserves the invariant across a renderer reload during an active command.
/// A manual caller joining an automatic check marks it manual.
pub async fn check(app: &AppHandle, manual: bool) -> SharedResult {
    let mut receiver = {
        let mut active = IN_FLIGHT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((receiver, manual_requested)) = active.as_ref() {
            if manual {
                manual_requested.store(true, Ordering::SeqCst);
            }
            receiver.clone()
        } else {
            let (sender, receiver) = tokio::sync::watch::channel(None);
            let manual_requested = Arc::new(AtomicBool::new(manual));
            *active = Some((receiver.clone(), manual_requested.clone()));
            let owned_app = app.clone();
            tokio::spawn(async move {
                let result = run_check(&owned_app, &manual_requested).await;
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
