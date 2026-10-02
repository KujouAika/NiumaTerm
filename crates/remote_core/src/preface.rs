//! The cleartext first message of every connection. It names the exchange
//! and the protocol majors each side speaks, so incompatible releases fail
//! with a message naming the side to update instead of a decryption error.
//! Both the offer and the answer are bound into the handshake prologue, so a
//! relay that rewrites either one to force an older major breaks the
//! handshake.

use crate::{Error, PAIR_MAJOR, PROTO_MAJOR, PROTO_MIN_MAJOR};

const MAGIC: &[u8; 4] = b"NMTR";

pub(crate) const PREFACE_LEN: usize = 7;

const KIND_CHANNEL: u8 = 0x01;
const KIND_PAIRING: u8 = 0x02;
const KIND_REJECTED: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefaceKind {
    Channel,
    Pairing,
    /// The host shares no major with the offer; `major..=min_major` then
    /// carries the host's own range.
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preface {
    pub kind: PrefaceKind,
    pub major: u8,
    pub min_major: u8,
}

impl Preface {
    /// This build's offer for a new connection of `kind`.
    pub fn offer(kind: PrefaceKind) -> Self {
        let (major, min_major) = own_range(kind);

        Self {
            kind,
            major,
            min_major,
        }
    }

    /// The host's answer to a client offer: the highest shared major, or a
    /// rejection carrying the host's range so the client can tell which side
    /// is older.
    pub fn answer(&self) -> Self {
        let (major, min_major) = own_range(self.kind);
        let chosen = major.min(self.major);

        if chosen >= min_major.max(self.min_major) {
            Self {
                kind: self.kind,
                major: chosen,
                min_major: chosen,
            }
        } else {
            Self {
                kind: PrefaceKind::Rejected,
                major,
                min_major,
            }
        }
    }

    pub fn encode(&self) -> [u8; PREFACE_LEN] {
        let kind = match self.kind {
            PrefaceKind::Channel => KIND_CHANNEL,
            PrefaceKind::Pairing => KIND_PAIRING,
            PrefaceKind::Rejected => KIND_REJECTED,
        };

        let mut out = [0; PREFACE_LEN];

        out[..4].copy_from_slice(MAGIC);

        out[4] = kind;
        out[5] = self.major;
        out[6] = self.min_major;

        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let Ok(bytes) = <[u8; PREFACE_LEN]>::try_from(bytes) else {
            return Err(Error::Malformed("preface length"));
        };

        if &bytes[..4] != MAGIC {
            return Err(Error::Malformed("preface magic"));
        }

        let kind = match bytes[4] {
            KIND_CHANNEL => PrefaceKind::Channel,
            KIND_PAIRING => PrefaceKind::Pairing,
            KIND_REJECTED => PrefaceKind::Rejected,
            _ => return Err(Error::Malformed("preface kind")),
        };

        Ok(Self {
            kind,
            major: bytes[5],
            min_major: bytes[6],
        })
    }
}

/// The prologue both handshake sides feed Noise: a label for the exchange
/// followed by the exact offer and answer bytes.
pub(crate) fn prologue(label: &[u8], offer: &Preface, answer: &Preface) -> Vec<u8> {
    [label, &offer.encode(), &answer.encode()].concat()
}

fn own_range(kind: PrefaceKind) -> (u8, u8) {
    match kind {
        PrefaceKind::Pairing => (PAIR_MAJOR, PAIR_MAJOR),
        PrefaceKind::Channel | PrefaceKind::Rejected => (PROTO_MAJOR, PROTO_MIN_MAJOR),
    }
}
