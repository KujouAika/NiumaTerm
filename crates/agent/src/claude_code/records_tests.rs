use crate::claude_code::records::*;

/// A child's conversation decodes server-side tool calls like the parent's,
/// and a message wrapping an API failure reads as an error.
#[test]
fn child_content_keeps_server_tools_and_api_errors() {
    let mut open_tools = HashMap::new();

    let tool = serde_json::json!({
        "type": "assistant", "uuid": "a1",
        "message": {"content": [
            {"type": "text", "text": "\n\nSearching"},
            {"type": "server_tool_use", "id": "srv", "name": "web_search", "input": {"query": "x"}},
        ]},
    });

    let items = child_content_items(&tool, &mut open_tools);

    assert!(
        matches!(&items[0], Item::AgentMessage { text: Some(text), .. } if text == "Searching")
    );
    assert_eq!(items[1].id(), Some("srv"));
    assert!(open_tools.contains_key("srv"));

    let failure = serde_json::json!({
        "type": "assistant", "uuid": "a2", "isApiErrorMessage": true,
        "message": {"content": [{"type": "text", "text": "API Error: overloaded"}]},
    });

    assert_eq!(
        child_content_items(&failure, &mut open_tools),
        vec![Item::Error {
            text: "API Error: overloaded".into(),
        }]
    );
}
