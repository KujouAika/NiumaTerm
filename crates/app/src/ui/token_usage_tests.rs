use crate::daily_usage::parse_usage;

#[test]
fn parse_usage_reads_daily_totals_and_model_details() {
    let json = br#"{
        "daily": [{
            "date": "2026-08-12",
            "inputTokens": 481653,
            "outputTokens": 429113,
            "cacheCreationTokens": 1350888,
            "cacheReadTokens": 107575141,
            "totalTokens": 109836795,
            "totalCost": 42.375,
            "modelBreakdowns": [{
                "modelName": "claude-opus-5",
                "inputTokens": 15409,
                "outputTokens": 161930,
                "cacheCreationTokens": 1173121,
                "cacheReadTokens": 50280148,
                "cost": 17.125
            }]
        }],
        "totals": { "totalTokens": 109836795, "totalCost": 42.375 }
    }"#;

    let usage = parse_usage(json, "2026-08-12").unwrap();

    assert_eq!(usage.counts.total(), 109_836_795);
    assert_eq!(usage.counts.input_tokens, 481_653);
    assert_eq!(usage.price_usd, 42.375);
    assert_eq!(usage.model_breakdowns.len(), 1);
    assert_eq!(usage.model_breakdowns[0].model_name, "claude-opus-5");
    assert_eq!(usage.model_breakdowns[0].counts.total(), 51_630_608);
    assert_eq!(usage.model_breakdowns[0].price_usd, 17.125);
}

#[test]
fn parse_usage_uses_report_totals_when_day_details_are_absent() {
    let json = br#"{
        "daily": [],
        "totals": {
            "inputTokens": 12,
            "outputTokens": 8,
            "totalTokens": 20,
            "totalCost": 1.5
        }
    }"#;

    let usage = parse_usage(json, "2026-08-12").unwrap();

    assert_eq!(usage.date, "2026-08-12");
    assert_eq!(usage.counts.total(), 20);
    assert_eq!(usage.price_usd, 1.5);
    assert!(usage.model_breakdowns.is_empty());
}
