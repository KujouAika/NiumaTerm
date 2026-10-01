use serde_json::json;

use crate::rpc::{Control, ErrorCode, HostInfo, RpcError};

#[test]
fn control_messages_decode_to_the_shape_they_were_encoded_from() {
    let messages = [
        Control::Request {
            id: 1,
            method: "terminal.open".into(),
            params: json!({ "cols": 80, "rows": 24 }),
        },
        Control::Response {
            id: 1,
            outcome: Ok(json!({ "session": "s1" })),
        },
        Control::Response {
            id: 2,
            outcome: Err(RpcError::new(ErrorCode::NotFound, "no session")),
        },
        Control::Notification {
            method: "sessions.changed".into(),
            params: json!({}),
        },
    ];

    for message in messages {
        assert_eq!(Control::decode(&message.encode()).unwrap(), message);
    }

    let newer = br#"{"jsonrpc":"2.0","id":3,"error":{"code":"rate_limited","message":"x"}}"#;

    assert!(matches!(
        Control::decode(newer).unwrap(),
        Control::Response {
            outcome: Err(RpcError {
                code: ErrorCode::Unknown,
                ..
            }),
            ..
        }
    ));
    assert!(Control::decode(br#"{"jsonrpc":"2.0","id":4}"#).is_err());
}

#[test]
fn host_info_from_a_host_without_workspace_ids_still_decodes() {
    let older = json!({
        "agents": [],
        "workspaces": [{ "name": "app", "path": "C:/app" }],
    });

    let info: HostInfo = serde_json::from_value(older).unwrap();

    assert_eq!(info.workspaces[0].id, None);
    assert_eq!(info.workspaces[0].name, "app");
}
