//! The user's own relay: a Cloudflare Worker (under `relay/`) that pairs a
//! host's sockets with its clients' and forwards their frames without
//! reading them. Every channel through it is the same end-to-end Noise
//! channel as on the LAN, so the relay can cost availability, never
//! confidentiality. Its access key only guards the owner's quota.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use futures::{SinkExt as _, StreamExt as _};
use nmt_remote_core::identity::DeviceId;
use nmt_remote_core::messages::RelayAccess;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::select;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, info, warn};

pub(crate) type RelaySocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How long a client waits for the host to pick up through the relay.
const OPEN_TIMEOUT: Duration = Duration::from_secs(12);

const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Keep-alive for the host's control socket, answered by the relay without
/// waking its Durable Object.
const CONTROL_PING: Duration = Duration::from_secs(30);

/// Messages the relay and the host exchange on the control socket.
#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ControlMessage {
    Conn { id: String },
    Slot { slot: String, ttl: u32 },
    SlotOk { slot: String },
    SlotTaken { slot: String },
    SlotRelease { slot: String },
}

/// What the host service asks of its relay link.
pub(crate) enum RelayCommand {
    ClaimSlot(String),
    ReleaseSlot(String),
}

/// Connect to `path` on the relay with the access key and optional host
/// token. `https://` and `http://` URLs are accepted for their WebSocket
/// equivalents.
async fn open(relay: &RelayAccess, path: &str, host_token: Option<&str>) -> Result<RelaySocket> {
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

    let (ws, _) = connect_async(request)
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

/// Keep a host registered on its relay: hold the control socket, reconnect
/// it with backoff, open a data socket for each client the relay announces,
/// and claim pairing slots on request. `serve` handles each data socket like
/// an accepted LAN connection.
pub(crate) async fn run_host_link(
    relay: RelayAccess,
    host: DeviceId,
    token: String,
    mut commands: UnboundedReceiver<RelayCommand>,
    serve: Arc<dyn Fn(RelaySocket) + Send + Sync>,
) {
    let mut backoff = MIN_BACKOFF;
    let mut claimed: Option<String> = None;

    loop {
        let control = open(&relay, &format!("/v1/host/{}", host.as_str()), Some(&token)).await;

        let mut control = match control {
            Ok(control) => control,
            Err(error) => {
                debug!(%error, "relay control socket failed");

                // Commands arriving meanwhile still update the wanted slot.
                select! {
                    () = sleep(backoff) => {}
                    command = commands.recv() => match command {
                        Some(RelayCommand::ClaimSlot(slot)) => claimed = Some(slot),
                        Some(RelayCommand::ReleaseSlot(_)) => claimed = None,
                        None => return,
                    },
                }

                backoff = (backoff * 2).min(MAX_BACKOFF);

                continue;
            }
        };

        info!(relay = %relay.url, "registered on the relay");

        backoff = MIN_BACKOFF;

        // A slot claimed before a reconnect is claimed again.
        if let Some(slot) = &claimed {
            let _ = send_control(
                &mut control,
                &ControlMessage::Slot {
                    slot: slot.clone(),
                    ttl: 300,
                },
            )
            .await;
        }

        loop {
            select! {
                message = control.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ControlMessage>(&text) {
                            Ok(ControlMessage::Conn { id }) => {
                                let relay = relay.clone();
                                let token = token.clone();
                                let path = format!("/v1/host/{}/accept/{id}", host.as_str());
                                let serve = Arc::clone(&serve);

                                tokio::spawn(async move {
                                    match open(&relay, &path, Some(&token)).await {
                                        Ok(ws) => serve(ws),
                                        Err(error) => debug!(%error, "relay data socket failed"),
                                    }
                                });
                            }
                            Ok(ControlMessage::SlotTaken { slot }) => {
                                warn!(%slot, "another host on this relay holds the pairing slot");
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        debug!(%error, "relay control socket dropped");

                        break;
                    }
                    None => break,
                },
                command = commands.recv() => {
                    let message = match command {
                        Some(RelayCommand::ClaimSlot(slot)) => {
                            claimed = Some(slot.clone());

                            ControlMessage::Slot { slot, ttl: 300 }
                        }
                        Some(RelayCommand::ReleaseSlot(slot)) => {
                            claimed = None;

                            ControlMessage::SlotRelease { slot }
                        }
                        None => {
                            let _ = control.close(None).await;

                            return;
                        }
                    };

                    if send_control(&mut control, &message).await.is_err() {
                        break;
                    }
                }

                () = sleep(CONTROL_PING) => {
                    if control.send(Message::Text("ping".into())).await.is_err() {
                        break;
                    }
                }
            }
        }

        sleep(backoff).await;
    }
}

async fn send_control(control: &mut RelaySocket, message: &ControlMessage) -> Result<()> {
    control
        .send(Message::Text(serde_json::to_string(message)?.into()))
        .await?;

    Ok(())
}
