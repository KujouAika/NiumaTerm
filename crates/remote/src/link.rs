//! Moves binary messages between a WebSocket and the protocol state machines.
//! One WebSocket binary message carries one Noise message, on the LAN and
//! later through the relay, so both paths share this code.

use std::time::Duration;

use anyhow::{Result, bail};
use futures::{Sink, SinkExt as _, Stream, StreamExt as _};
use nmt_remote_core::channel::Channel;
use nmt_remote_core::frame::{
    CONTROL_STREAM, Frame, Message as FrameMessage, Reassembler, encode_message, kind,
};
use tokio::select;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// One message to send on a channel stream.
pub(crate) struct Outbound {
    pub stream: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

pub(crate) async fn recv_binary<S>(ws: &mut S) -> Result<Vec<u8>>
where
    S: Stream<Item = Result<Message, WsError>> + Unpin,
{
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(bytes))) => return Ok(bytes.to_vec()),
            Some(Ok(Message::Close(_))) | None => bail!("connection closed"),
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(error.into()),
        }
    }
}

pub(crate) async fn send_binary<S>(ws: &mut S, bytes: Vec<u8>) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    ws.send(Message::Binary(bytes.into())).await?;

    Ok(())
}

/// Probe a quiet channel after this long without inbound frames.
const IDLE_BEFORE_PING: Duration = Duration::from_secs(30);

/// An unanswered probe counts as missed after this long.
const PING_TIMEOUT: Duration = Duration::from_secs(10);

/// Two missed probes in a row mark the channel dead, so one lost to a
/// momentary stall does not tear down working sessions.
const MAX_MISSED_PINGS: u32 = 2;

/// Run an established channel until either side closes it: seal and send
/// `outbound`, open and reassemble what arrives into `inbound`. One task owns
/// both directions because the Noise transport keeps both ciphers in one
/// state.
///
/// The pump also keeps the channel honest about liveness: a network that
/// drops packets without closing the socket (sleep, a pulled cable, a NAT
/// timeout) would otherwise leave both sides waiting forever. It answers
/// probes itself, so the layers above never see them.
pub(crate) async fn pump<S>(
    mut ws: S,
    mut channel: Channel,
    mut outbound: UnboundedReceiver<Outbound>,
    inbound: UnboundedSender<FrameMessage>,
) -> Result<()>
where
    S: Stream<Item = Result<Message, WsError>> + Sink<Message, Error = WsError> + Unpin,
{
    let mut reassembler = Reassembler::default();
    let mut deadline = Instant::now() + IDLE_BEFORE_PING;
    let mut missed = 0;
    let mut probing = false;

    loop {
        select! {
            received = recv_binary(&mut ws) => {
                let plaintext = channel.open(&received?)?;
                let frame = Frame::decode(&plaintext)?;

                deadline = Instant::now() + IDLE_BEFORE_PING;
                missed = 0;
                probing = false;

                match (frame.stream, frame.kind) {
                    (CONTROL_STREAM, kind::PING) => {
                        send_frames(&mut ws, &mut channel, CONTROL_STREAM, kind::PONG, frame.payload)
                            .await?;
                    }
                    (CONTROL_STREAM, kind::PONG) => {}
                    _ => {
                        if let Some(message) = reassembler.push(frame)?
                            && inbound.send(message).is_err()
                        {
                            return Ok(());
                        }
                    }
                }
            }

            next = outbound.recv() => {
                let Some(next) = next else {
                    let _ = ws.close().await;

                    return Ok(());
                };

                send_frames(&mut ws, &mut channel, next.stream, next.kind, &next.payload).await?;
            }

            () = sleep_until(deadline) => {
                if probing {
                    missed += 1;

                    if missed >= MAX_MISSED_PINGS {
                        bail!("the peer stopped answering");
                    }
                }

                probing = true;
                deadline = Instant::now() + PING_TIMEOUT;

                send_frames(&mut ws, &mut channel, CONTROL_STREAM, kind::PING, &[]).await?;
            }
        }
    }
}

async fn send_frames<S>(
    ws: &mut S,
    channel: &mut Channel,
    stream: u32,
    kind: u8,
    payload: &[u8],
) -> Result<()>
where
    S: Sink<Message, Error = WsError> + Unpin,
{
    for frame in encode_message(stream, kind, payload) {
        let sealed = channel.seal(&frame)?;

        send_binary(ws, sealed).await?;
    }

    Ok(())
}
