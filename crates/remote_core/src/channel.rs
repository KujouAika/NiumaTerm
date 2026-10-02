//! The mutually authenticated channel every connection runs, on the LAN and
//! through the relay alike: `Noise_IK_25519_ChaChaPoly_BLAKE2s`.
//!
//! The client already knows the host key from pairing, so IK finishes in one
//! round trip and hides the client's identity from passive observers. The
//! host learns the client key from msg1 and must check it against its trust
//! store, and the hello's `hello_ms` against the last accepted value, before
//! answering; an unknown or revoked key gets no reply at all.
//!
//! Transport messages use Noise counter nonces over reliable, ordered
//! transports, so a modified, replayed, reordered, or dropped message fails
//! to decrypt and the caller closes the channel.

use snow::{Builder, HandshakeState, TransportState};

use crate::identity::DeviceKey;
use crate::messages::{self, ClientHello, HostHello};
use crate::noise::{read, remote_static, write};
use crate::preface::{self, Preface};
use crate::{Error, MAX_NOISE_MESSAGE, TAG_LEN};

const PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE_LABEL: &[u8] = b"NiumaTerm remote";

/// Largest plaintext one transport message can carry.
pub const MAX_PLAINTEXT: usize = MAX_NOISE_MESSAGE - TAG_LEN;

/// Client side between msg1 and msg2.
pub struct ClientHandshake {
    noise: HandshakeState,
}

impl ClientHandshake {
    pub fn start(
        key: &DeviceKey,
        host_key: &[u8; 32],
        offer: &Preface,
        answer: &Preface,
        hello: &ClientHello,
    ) -> Result<(Self, Vec<u8>), Error> {
        let mut noise = Builder::new(PARAMS.parse()?)
            .local_private_key(key.private())?
            .remote_public_key(host_key)?
            .prologue(&preface::prologue(PROLOGUE_LABEL, offer, answer))?
            .build_initiator()?;

        let msg1 = write(&mut noise, &messages::encode(hello)?)?;

        Ok((Self { noise }, msg1))
    }

    pub fn finish(mut self, msg2: &[u8]) -> Result<(Channel, HostHello), Error> {
        let hello = messages::decode(&read(&mut self.noise, msg2)?)?;

        Ok((Channel::new(self.noise)?, hello))
    }
}

/// Host side after decrypting msg1, before deciding whether to answer.
pub struct HostHandshake {
    noise: HandshakeState,
    client_key: [u8; 32],
    hello: ClientHello,
}

impl HostHandshake {
    pub fn read(
        key: &DeviceKey,
        offer: &Preface,
        answer: &Preface,
        msg1: &[u8],
    ) -> Result<Self, Error> {
        let mut noise = Builder::new(PARAMS.parse()?)
            .local_private_key(key.private())?
            .prologue(&preface::prologue(PROLOGUE_LABEL, offer, answer))?
            .build_responder()?;

        let hello = messages::decode(&read(&mut noise, msg1)?)?;

        Ok(Self {
            client_key: remote_static(&noise)?,
            noise,
            hello,
        })
    }

    /// The client's static key, proven by msg1. Look it up in the trust store
    /// before calling [`HostHandshake::accept`].
    pub fn client_key(&self) -> &[u8; 32] {
        &self.client_key
    }

    pub fn hello(&self) -> &ClientHello {
        &self.hello
    }

    pub fn accept(mut self, hello: &HostHello) -> Result<(Channel, Vec<u8>), Error> {
        let msg2 = write(&mut self.noise, &messages::encode(hello)?)?;

        Ok((Channel::new(self.noise)?, msg2))
    }
}

/// An established channel. Each call seals or opens exactly one transport
/// message, in order.
pub struct Channel {
    transport: TransportState,
    remote_key: [u8; 32],
}

impl Channel {
    fn new(noise: HandshakeState) -> Result<Self, Error> {
        let remote_key = remote_static(&noise)?;

        Ok(Self {
            transport: noise.into_transport_mode()?,
            remote_key,
        })
    }

    pub fn remote_key(&self) -> &[u8; 32] {
        &self.remote_key
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(Error::TooLarge {
                limit: MAX_PLAINTEXT,
            });
        }

        let mut out = vec![0; plaintext.len() + TAG_LEN];

        let len = self.transport.write_message(plaintext, &mut out)?;

        out.truncate(len);

        Ok(out)
    }

    pub fn open(&mut self, message: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = vec![0; message.len()];

        let len = self.transport.read_message(message, &mut out)?;

        out.truncate(len);

        Ok(out)
    }
}
