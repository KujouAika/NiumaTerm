//! One node's turn cut out of its slot's conversation.
//!
//! A slot conversation holds the turns of several nodes. Items have no
//! provider turn id, but each transcript entry records the session's local
//! turn number, and the runtime saves that number when it sends a node.

use nmt_agent::chat::Item;
use nmt_agent::transcript::TranscriptEntry;
use nmt_agent::transcript::conversation::{ConversationState, EntryMetadata};

/// The items of `turn`, opening with `prompt` as its user message.
pub(super) fn turn_items(conversation: &ConversationState, turn: u64, prompt: &str) -> Vec<Item> {
    let mut items = vec![Item::UserMessage {
        text: Some(prompt.to_owned()),
    }];

    items.extend(
        conversation
            .content
            .entries()
            .iter()
            .filter(|entry| entry.turn == turn)
            .filter(|entry| !matches!(entry.item, Item::UserMessage { .. }))
            .map(|entry| entry.item.clone()),
    );

    items
}

/// Entries for a transcript view of `turn`.
pub(super) fn turn_entries(
    conversation: &ConversationState,
    turn: u64,
    prompt: &str,
) -> Vec<TranscriptEntry<EntryMetadata>> {
    let mut entries = vec![TranscriptEntry {
        turn: 1,
        item: Item::UserMessage {
            text: Some(prompt.to_owned()),
        },
        metadata: EntryMetadata::default(),
    }];

    entries.extend(
        conversation
            .content
            .entries()
            .iter()
            .filter(|entry| entry.turn == turn)
            .filter(|entry| !matches!(entry.item, Item::UserMessage { .. }))
            .map(|entry| TranscriptEntry {
                turn: 1,
                item: entry.item.clone(),
                metadata: EntryMetadata {
                    at: entry.metadata.at,
                    images: entry.metadata.images.clone(),
                },
            }),
    );

    entries
}

/// Entries for a transcript view of a saved turn.
pub(super) fn saved_entries(items: &[Item]) -> Vec<TranscriptEntry<EntryMetadata>> {
    items
        .iter()
        .map(|item| TranscriptEntry {
            turn: 1,
            item: item.clone(),
            metadata: EntryMetadata::default(),
        })
        .collect()
}
