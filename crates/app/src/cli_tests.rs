use std::env;

use crate::cli::*;

#[test]
fn rejects_unknown_verb_and_scheme() {
    assert!(parse_nmt_url("nmt://action/open?path=C:/A").is_err());
    assert!(parse_nmt_url("http://example.com").is_err());
}

#[test]
fn rejects_missing_or_empty_path() {
    assert!(parse_nmt_url("nmt://action/new_tab").is_err());
    assert!(parse_nmt_url("nmt://action/new_tab?path=").is_err());
    assert!(parse_nmt_url("nmt://action/new_tab?other=1").is_err());
}

#[test]
fn resolves_relative_path_against_cwd() {
    let action = parse_nmt_url("nmt://action/new_tab?path=sub%2Fdir").unwrap();

    let CliAction::NewTab { path } = action else {
        panic!("expected NewTab");
    };

    assert_eq!(path, env::current_dir().unwrap().join("sub").join("dir"));
}

#[test]
fn focus_notification_round_trips_and_rejects_invalid_ids() {
    let action = CliAction::FocusNotification {
        route: AgentRoute::parse("process:pane").unwrap(),
        notification_id: "process:pane:1".into(),
    };

    let url: String = (&action).into();

    assert_eq!(parse_nmt_url(&url).unwrap(), action);
    assert!(parse_nmt_url("nmt://action/focus_notification?route=a").is_err());
    assert!(
        parse_nmt_url(&format!(
            "nmt://action/focus_notification?route=a&notification_id={}",
            "x".repeat(513)
        ))
        .is_err()
    );
}
