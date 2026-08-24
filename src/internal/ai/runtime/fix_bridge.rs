//! Admission-only bridge for `libra review --fix`.
//!
//! The review CLI never starts its own runtime worker. Instead it discovers an
//! already running, write-enabled `libra code` session and submits one fixed
//! plain-text planning request through that session's authenticated control
//! surface. Plain messages use the existing Phase 0 path, which cannot apply a
//! patch before the Code runtime's normal review gates. DF-09 owns every
//! controlled execution outcome; this module only admits the request.

use std::{path::Path, time::Duration};

use reqwest::Client;
use serde_json::{Value, json};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::command::{
    code_control::{apply_control_headers, ensure_loopback_control_url, read_control_token},
    code_control_files::discover_control_stdio_endpoint,
};

/// Fixed, trusted request text admitted through the running Code runtime.
///
/// No reviewer stdout, finding, environment value, or user-supplied seed is
/// included here. This keeps observed external-agent data out of the mutating
/// boundary until DF-09 supplies its controlled execution contract.
pub const REVIEW_FIX_ADMISSION_MESSAGE: &str = "Prepare a controlled review-fix plan. This is an admission-only request from `libra review --fix`: do not apply a patch, do not run mutating tools, and do not consume external reviewer findings. Produce only the normal plan/review gate; controlled fix execution is not enabled yet.";

/// A stale control sidecar must not make the foreground CLI wait forever.
const CONTROL_ADMISSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Provenance classification required before any request can enter the
/// mutating-capable Code runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewFixInput {
    /// The CLI's fixed admission request; it contains no observed-agent data.
    TrustedAdmission,
    /// An issue, transcript, finding, or other observed external-agent seed.
    UntrustedSeed,
}

/// Fail-closed result from the review-fix admission bridge.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ReviewFixBridgeError {
    #[error("no authorized active Code runtime is available")]
    Unavailable,
    #[error("untrusted seed content cannot enter a mutating workflow")]
    UntrustedSeed,
}

/// Admit the fixed review-fix planning request to an existing Code runtime.
///
/// The control sidecar discovery, token-permission check, loopback-only URL
/// check, controller lease, and runtime submission all fail closed. A missing
/// runtime, missing authority, or rejected request is deliberately one
/// [`ReviewFixBridgeError::Unavailable`] outcome for the review CLI.
pub async fn admit_review_fix(
    working_dir: &Path,
    input: ReviewFixInput,
) -> Result<(), ReviewFixBridgeError> {
    if input == ReviewFixInput::UntrustedSeed {
        return Err(ReviewFixBridgeError::UntrustedSeed);
    }

    let endpoint = discover_control_stdio_endpoint(working_dir, None, None, None)
        .await
        .map_err(|_| ReviewFixBridgeError::Unavailable)?;
    let base_url = Url::parse(&endpoint.base_url).map_err(|_| ReviewFixBridgeError::Unavailable)?;
    ensure_loopback_control_url(&base_url).map_err(|_| ReviewFixBridgeError::Unavailable)?;
    let control_token =
        read_control_token(&endpoint.token_file).map_err(|_| ReviewFixBridgeError::Unavailable)?;
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(CONTROL_ADMISSION_TIMEOUT)
        .build()
        .map_err(|_| ReviewFixBridgeError::Unavailable)?;

    let client_id = format!("review-fix-{}", Uuid::new_v4());
    let controller_token =
        attach_controller(&client, &base_url, &control_token, &client_id).await?;
    let submission = submit_admission(&client, &base_url, &control_token, &controller_token).await;
    let detach = detach_controller(
        &client,
        &base_url,
        &control_token,
        &controller_token,
        &client_id,
    )
    .await;
    // Do not report an admission as successful while its automation lease is
    // still held. A request may already have reached the runtime if a network
    // failure races the response, but returning a fail-closed error avoids
    // silently blocking the next Code controller until lease expiry.
    submission?;
    detach
}

async fn attach_controller(
    client: &Client,
    base_url: &Url,
    control_token: &str,
    client_id: &str,
) -> Result<String, ReviewFixBridgeError> {
    let response = apply_control_headers(
        client
            .post(control_endpoint(base_url, "/api/code/controller/attach"))
            .json(&json!({ "clientId": client_id, "kind": "automation" })),
        control_token,
        None,
    )
    .send()
    .await
    .map_err(|_| ReviewFixBridgeError::Unavailable)?;
    if !response.status().is_success() {
        return Err(ReviewFixBridgeError::Unavailable);
    }
    let payload = response
        .json::<Value>()
        .await
        .map_err(|_| ReviewFixBridgeError::Unavailable)?;
    payload
        .get("controllerToken")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .map(str::to_owned)
        .ok_or(ReviewFixBridgeError::Unavailable)
}

async fn submit_admission(
    client: &Client,
    base_url: &Url,
    control_token: &str,
    controller_token: &str,
) -> Result<(), ReviewFixBridgeError> {
    let response = apply_control_headers(
        client
            .post(control_endpoint(base_url, "/api/code/messages"))
            .json(&json!({ "text": REVIEW_FIX_ADMISSION_MESSAGE })),
        control_token,
        Some(controller_token),
    )
    .send()
    .await
    .map_err(|_| ReviewFixBridgeError::Unavailable)?;
    response
        .status()
        .is_success()
        .then_some(())
        .ok_or(ReviewFixBridgeError::Unavailable)
}

async fn detach_controller(
    client: &Client,
    base_url: &Url,
    control_token: &str,
    controller_token: &str,
    client_id: &str,
) -> Result<(), ReviewFixBridgeError> {
    let response = apply_control_headers(
        client
            .post(control_endpoint(base_url, "/api/code/controller/detach"))
            .json(&json!({ "clientId": client_id })),
        control_token,
        Some(controller_token),
    )
    .send()
    .await
    .map_err(|_| ReviewFixBridgeError::Unavailable)?;
    response
        .status()
        .is_success()
        .then_some(())
        .ok_or(ReviewFixBridgeError::Unavailable)
}

fn control_endpoint(base_url: &Url, endpoint: &str) -> Url {
    let mut url = base_url.clone();
    let base_path = url.path().trim_end_matches('/');
    let endpoint = endpoint.trim_start_matches('/');
    url.set_path(&format!("{base_path}/{endpoint}"));
    url.set_query(None);
    url.set_fragment(None);
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn untrusted_seed_is_refused_before_control_discovery() {
        let error = admit_review_fix(
            Path::new("/path-that-must-not-be-read"),
            ReviewFixInput::UntrustedSeed,
        )
        .await
        .expect_err("untrusted input must not reach runtime discovery");
        assert_eq!(error, ReviewFixBridgeError::UntrustedSeed);
    }
}
