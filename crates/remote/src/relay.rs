//! The user's own relay: a Cloudflare Worker (under `relay/`) that pairs a
//! host's sockets with its clients' and forwards their frames without
//! reading them. Every channel through it is the same end-to-end Noise
//! channel as on the LAN, so the relay can cost availability, never
//! confidentiality. Its access key only guards the owner's quota.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use futures::StreamExt as _;
use nmt_net::connect_websocket;
use nmt_remote_core::identity::DeviceId;
use nmt_remote_core::messages::RelayAccess;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub(crate) type RelaySocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How long a client waits for the host to pick up through the relay.
const OPEN_TIMEOUT: Duration = Duration::from_secs(12);

/// Connect to `path` on the relay with the access key and optional host
/// token, through the proxy in the settings. `https://` and `http://` URLs
/// are accepted for their WebSocket equivalents.
pub(crate) async fn open(
    relay: &RelayAccess,
    path: &str,
    host_token: Option<&str>,
) -> Result<RelaySocket> {
    let base = relay.url.trim_end_matches('/');

    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    };

    let mut request = format!("{base}{path}").into_client_request()?;

    let headers = request.headers_mut();

    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", relay.access_key))?,
    );

    if let Some(token) = host_token {
        headers.insert("X-Host-Token", HeaderValue::from_str(token)?);
    }

    let (ws, _) = connect_websocket(request)
        .await
        .with_context(|| format!("reaching the relay at {}", relay.url))?;

    Ok(ws)
}

/// Join a host's room as a client and wait until the host picks up. The
/// relay forwards nothing before that, so the handshake starts afterwards.
pub(crate) async fn dial_host(relay: &RelayAccess, host: &DeviceId) -> Result<RelaySocket> {
    let ws = open(relay, &format!("/v1/client/{}", host.as_str()), None).await?;

    wait_for_open(ws).await
}

/// Join, through the relay, the host showing a pairing code with `slot`.
pub(crate) async fn dial_pairing(relay: &RelayAccess, slot: &str) -> Result<RelaySocket> {
    let ws = open(relay, &format!("/v1/pair/{slot}"), None).await?;

    wait_for_open(ws).await
}

async fn wait_for_open(mut ws: RelaySocket) -> Result<RelaySocket> {
    timeout(OPEN_TIMEOUT, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) if text.contains("\"open\"") => return Ok(()),
                Some(Ok(Message::Close(frame))) => {
                    let offline = frame
                        .as_ref()
                        .is_some_and(|frame| frame.code == CloseCode::from(4404));

                    if offline {
                        bail!("the host is not connected to the relay");
                    }

                    bail!("the relay closed the connection");
                }
                Some(Ok(_)) => {}
                Some(Err(error)) => return Err(error.into()),
                None => bail!("the relay closed the connection"),
            }
        }
    })
    .await
    .map_err(|_| anyhow!("the host did not answer through the relay"))??;

    Ok(ws)
}
