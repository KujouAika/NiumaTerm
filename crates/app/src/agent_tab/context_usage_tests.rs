use nmt_agent::chat::ScopedTokenUsage;

use crate::agent_tab::context_usage::*;

#[test]
fn the_cache_share_is_measured_over_the_turn_not_its_last_request() {
    // The shape a tool loop produces: the turn wrote 30k tokens into the
    // cache up front, while its final request replayed an almost entirely
    // cached prefix.
    let last_request = TokenUsageBreakdown {
        total_tokens: 101_000,
        input_tokens: Some(100_000),
        cache_read_input_tokens: Some(99_800),
        cache_write_input_tokens: Some(200),
        output_tokens: Some(1_000),
        reasoning_output_tokens: None,
    };

    let turn = TokenUsageBreakdown {
        total_tokens: 310_000,
        input_tokens: Some(300_000),
        cache_read_input_tokens: Some(270_000),
        cache_write_input_tokens: Some(30_000),
        output_tokens: Some(10_000),
        reasoning_output_tokens: None,
    };

    let usage = ContextWindowUsage {
        current: last_request,
        cumulative: Some(ScopedTokenUsage {
            scope: ContextUsageScope::LastTurn,
            breakdown: turn,
        }),
        max_tokens: Some(200_000),
    };

    assert_eq!(cache_hit_percent(usage), Some(90));

    // Without an aggregate — a sparse or post-compaction snapshot — the
    // newest request is still worth reporting.
    assert_eq!(
        cache_hit_percent(ContextWindowUsage {
            cumulative: None,
            ..usage
        }),
        Some(100)
    );
    assert_eq!(
        cache_hit_percent(ContextWindowUsage {
            cumulative: Some(ScopedTokenUsage {
                scope: ContextUsageScope::Thread,
                breakdown: TokenUsageBreakdown::total_only(500_000),
            }),
            ..usage
        }),
        Some(100)
    );
}

#[test]
fn the_live_context_and_the_last_turn_report_the_same_categories() {
    let usage = TokenUsageBreakdown {
        total_tokens: 12_345,
        input_tokens: Some(10_000),
        cache_read_input_tokens: Some(2_000),
        cache_write_input_tokens: Some(500),
        output_tokens: Some(345),
        reasoning_output_tokens: None,
    };

    // Both sections are built the same way, so the two can be read against
    // each other rather than one omitting a figure the other shows.
    let labels: Vec<_> = token_usage_rows(usage)
        .iter()
        .map(|row| row.label.clone())
        .collect();

    assert_eq!(
        labels,
        ["Total", "Input", "Cache read", "Cache write", "Output"]
    );
}
