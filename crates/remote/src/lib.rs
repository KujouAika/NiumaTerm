//! Remote sessions over the network: the host service, the client
//! connection, pairing records, and the network PTY a client tab reads.
//! Protocol state machines and codecs live in `nmt_remote_core`; this crate
//! adds sockets, the runtime, storage, and host terminals.

pub use crate::network_pty::NetworkPty;

pub mod client;
pub mod connection;
pub mod discovery;
pub mod host;
pub mod lan;
pub mod local_view;
pub mod sessions;
pub mod store;

mod link;
mod netwatch;
mod network_pty;
mod relay;
mod secret;
mod stream;

#[cfg(test)]
mod lan_tests;
#[cfg(test)]
mod link_tests;
#[cfg(test)]
mod loopback_tests;
#[cfg(test)]
#[cfg(any(windows, target_os = "macos"))]
mod store_tests;
#[cfg(test)]
mod stream_tests;
