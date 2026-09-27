//! Moves binary messages between a WebSocket and the protocol state machines.
//! One WebSocket binary message carries one Noise message, on the LAN and
//! later through the relay, so both paths share this code.

use anyhow::{Result, bail};
use futures::{Sink, SinkExt as _, Stream, StreamExt as _};
use nmt_remote_core::channel::Channel;
use nmt_remote_core::frame::{Frame, Message as FrameMessage, Reassembler, encode_message};
use tokio::select;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
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

/// Run an established channel until either side closes it: seal and send
/// `outbound`, open and reassemble what arrives into `inbound`. One task owns
/// both directions because the Noise transport keeps both ciphers in one
/// state.
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

    loop {
        select! {
            received = recv_binary(&mut ws) => {
                let plaintext = channel.open(&received?)?;
                let frame = Frame::decode(&plaintext)?;

                if let Some(message) = reassembler.push(frame)?
                    && inbound.send(message).is_err()
                {
                    return Ok(());
                }
            }

            next = outbound.recv() => {
                let Some(next) = next else {
                    let _ = ws.close().await;

                    return Ok(());
                };

                for frame in encode_message(next.stream, next.kind, &next.payload) {
                    let sealed = channel.seal(&frame)?;

                    send_binary(&mut ws, sealed).await?;
                }
            }
        }
    }
}
