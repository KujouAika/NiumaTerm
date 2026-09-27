//! Control messages on stream 0: JSON-RPC 2.0 message shapes without
//! batching. Error codes are names rather than numbers so a peer can match
//! on them without a shared numeric table.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

pub const TERMINAL_OPEN: &str = "terminal.open";
pub const TERMINAL_ATTACH: &str = "terminal.attach";
pub const TERMINAL_RESIZE: &str = "terminal.resize";
pub const TERMINAL_CLOSE: &str = "terminal.close";
pub const STREAM_CLOSE: &str = "stream.close";
pub const SESSIONS_LIST: &str = "sessions.list";

/// Notification: the host's session list changed; clients list it again.
pub const SESSIONS_CHANGED: &str = "sessions.changed";

#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Request {
        id: u64,
        method: String,
        params: Value,
    },
    Response {
        id: u64,
        outcome: Result<Value, RpcError>,
    },
    Notification {
        method: String,
        params: Value,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    InvalidParams,
    Unsupported,
    Busy,
    Denied,
    Internal,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The union of every field the three shapes use, so one decode classifies a
/// message by which fields are present.
#[derive(Default, Serialize, Deserialize)]
struct Raw {
    jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

impl Control {
    pub fn encode(&self) -> Vec<u8> {
        let mut raw = Raw {
            jsonrpc: "2.0".into(),
            ..Raw::default()
        };

        match self {
            Control::Request { id, method, params } => {
                raw.id = Some(*id);
                raw.method = Some(method.clone());
                raw.params = Some(params.clone());
            }
            Control::Response { id, outcome } => {
                raw.id = Some(*id);

                match outcome {
                    Ok(result) => raw.result = Some(result.clone()),
                    Err(error) => raw.error = Some(error.clone()),
                }
            }
            Control::Notification { method, params } => {
                raw.method = Some(method.clone());
                raw.params = Some(params.clone());
            }
        }

        serde_json::to_vec(&raw).expect("JSON values always serialize")
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let raw: Raw = serde_json::from_slice(bytes)?;
        let params = raw.params.unwrap_or(Value::Null);

        match (raw.id, raw.method, raw.result, raw.error) {
            (Some(id), Some(method), None, None) => Ok(Control::Request { id, method, params }),
            (None, Some(method), None, None) => Ok(Control::Notification { method, params }),
            (Some(id), None, Some(result), None) => Ok(Control::Response {
                id,
                outcome: Ok(result),
            }),
            (Some(id), None, None, Some(error)) => Ok(Control::Response {
                id,
                outcome: Err(error),
            }),
            _ => Err(Error::Malformed("control message")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOpen {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef {
    pub session: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attached {
    pub stream: u32,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResize {
    pub session: String,
    pub cols: u16,
    pub rows: u16,
}

/// Payload of an EXIT frame. The code is absent when the host could not
/// observe it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exit {
    pub code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamRef {
    pub stream: u32,
}

/// Where a host session came from. Only remote-created sessions can be ended
/// remotely; a host tab belongs to the person at the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Tab,
    Remote,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session: String,
    pub title: String,
    pub origin: Origin,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionList {
    pub sessions: Vec<SessionInfo>,
}
