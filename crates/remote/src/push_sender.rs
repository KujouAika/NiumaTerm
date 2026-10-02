//! Hands a sealed push to the forwarder a device registered.
//!
//! The forwarder holds the APNs key for the app build and answers with what
//! APNs said. A token APNs no longer accepts is reported back so the host
//! stops using it; every other failure is only logged, because a missed push
//! is replaced by what the phone sees when it next opens the session.

use std::time::Duration;

use nmt_net::http_client;
use nmt_remote_core::push::{PushEnvironment, PushRegistration};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// A forwarder that has not answered by now is not going to; the push is
/// dropped instead of held.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Sent,
    /// APNs no longer accepts the token: the app was removed, or the token
    /// was issued for another environment or app.
    TokenGone,
    Failed,
}

#[derive(Serialize)]
struct ForwardRequest<'a> {
    token: &'a str,
    environment: PushEnvironment,
    host: &'a str,
    sealed: &'a str,

    /// Pushes about one session replace each other on the lock screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    collapse: Option<&'a str>,
}

#[derive(Deserialize)]
struct ForwardResponse {
    status: u16,
    reason: Option<String>,
}

pub(crate) fn client() -> reqwest::Client {
    http_client()
        .timeout(SEND_TIMEOUT)
        .build()
        .unwrap_or_default()
}

pub(crate) async fn deliver(
    client: &reqwest::Client,
    registration: &PushRegistration,
    host: &str,
    session: &str,
    sealed: &str,
) -> Delivery {
    let request = ForwardRequest {
        token: &registration.token,
        environment: registration.environment,
        host,
        sealed,
        collapse: Some(collapse_id(session)).filter(|id| !id.is_empty()),
    };

    let response = match client
        .post(&registration.endpoint)
        .header("Content-Type", "application/json")
        .body(serde_json::to_vec(&request).unwrap_or_default())
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            warn!(%error, "the push forwarder is unreachable");

            return Delivery::Failed;
        }
    };

    if !response.status().is_success() {
        warn!(status = %response.status(), "the push forwarder refused a push");

        return Delivery::Failed;
    }

    let answer = match response
        .bytes()
        .await
        .map(|body| serde_json::from_slice::<ForwardResponse>(&body))
    {
        Ok(Ok(answer)) => answer,
        _ => {
            warn!("the push forwarder sent an unreadable answer");

            return Delivery::Failed;
        }
    };

    let delivery = classify(answer.status, answer.reason.as_deref());

    info!(status = answer.status, reason = ?answer.reason, "APNs answered a push");

    delivery
}

pub(crate) fn classify(status: u16, reason: Option<&str>) -> Delivery {
    match (status, reason) {
        (200, _) => Delivery::Sent,
        (410, _) | (400, Some("BadDeviceToken" | "DeviceTokenNotForTopic")) => Delivery::TokenGone,
        (status, reason) => {
            debug!(status, ?reason, "APNs did not take a push");

            Delivery::Failed
        }
    }
}

/// APNs collapse ids are at most 64 bytes, and the forwarder accepts letters,
/// digits, `-` and `_`; session ids are made of those.
fn collapse_id(session: &str) -> &str {
    let end = session
        .char_indices()
        .find(|&(index, c)| index >= 64 || !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .map_or(session.len(), |(index, _)| index);

    &session[..end]
}
