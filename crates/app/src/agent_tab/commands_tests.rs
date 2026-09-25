use nmt_agent::catalog::ParsedSlashCommand;

use crate::agent_tab::commands::*;
use crate::agent_tab::{CachedCatalog, SlashPalette};

fn info(name: &str, source: SlashCommandSource) -> SlashCommandInfo {
    SlashCommandInfo {
        name: name.to_string(),
        description: name.to_string(),
        argument_hint: None,
        source,
        arguments: SlashCommandArguments::None,
        run_policy: SlashCommandRunPolicy::Immediate,
    }
}

#[test]
fn parser_only_claims_a_leading_slash_and_preserves_argument_text() {
    assert_eq!(parse_slash_command("explain a/b"), None);
    assert_eq!(
        parse_slash_command("/review   path with spaces  "),
        Some(ParsedSlashCommand {
            name: "review".into(),
            arguments: "path with spaces  ".into(),
            has_argument_separator: true,
        })
    );
}

#[test]
fn enum_choice_requires_an_exact_or_unique_prefix_match() {
    let choices = vec![
        ("default".into(), "Default".into()),
        ("danger-full-access".into(), "Danger Full Access".into()),
        ("deny".into(), "Deny".into()),
    ];

    assert_eq!(resolve_choice("DEFAULT", &choices), Ok("default".into()));
    assert_eq!(
        resolve_choice("dang", &choices),
        Ok("danger-full-access".into())
    );
    assert!(resolve_choice("d", &choices).is_err());
    assert!(resolve_choice("unknown", &choices).is_err());
}

#[test]
fn clear_resets_discovery_state() {
    let mut palette = SlashPalette {
        selected: 3,
        dismissed: true,
        ..SlashPalette::default()
    };

    palette.catalog = Some(CachedCatalog {
        language: "en".into(),
        epoch: 1,
        commands: vec![info("review", SlashCommandSource::Provider)].into(),
    });

    palette.reset_discovery();

    assert!(palette.catalog.is_none());
    assert_eq!(palette.selected, 0);
    assert!(!palette.dismissed);
}

fn skill(name: &str, path: &str, scope: &str, enabled: bool) -> SkillInfo {
    SkillInfo {
        name: name.into(),
        description: format!("Use {name} for reviews"),
        path: path.into(),
        scope: scope.into(),
        enabled,
        display_name: None,
    }
}

#[test]
fn skill_filter_ranks_fields_and_preserves_duplicate_paths() {
    let mut plugin = skill(
        "browser:control-in-app-browser",
        "C:\\plugins\\browser\\SKILL.md",
        "system",
        true,
    );

    plugin.display_name = Some("Browser Control".into());

    let catalog = vec![
        skill("review", "C:\\user\\review\\SKILL.md", "user", true),
        skill("review", "C:\\repo\\review\\SKILL.md", "repo", false),
        plugin,
    ];

    let duplicate_results = filter_skill_catalog(&catalog, "review");

    assert_eq!(duplicate_results.len(), 3);
    assert_eq!(duplicate_results[0].name, "review");
    assert_eq!(duplicate_results[1].name, "review");
    assert_ne!(duplicate_results[0].path, duplicate_results[1].path);
    assert_eq!(duplicate_results[2].name, "browser:control-in-app-browser");
    assert_eq!(
        filter_skill_catalog(&catalog, "browser control")[0].name,
        "browser:control-in-app-browser"
    );
    assert_eq!(filter_skill_catalog(&catalog, "missing"), Vec::new());
}

#[test]
fn skill_binding_survives_argument_edits_but_not_token_edits() {
    let mut binding = Some(SkillReference {
        name: "review".into(),
        path: "C:\\skills\\review\\SKILL.md".into(),
    });

    reconcile_skill_binding("$review focus on parsing", &mut binding);

    assert!(binding.is_some());

    reconcile_skill_binding("$other focus on parsing", &mut binding);

    assert!(binding.is_none());
}

#[test]
fn skill_binding_validation_rejects_stale_and_disabled_paths() {
    let binding = SkillReference {
        name: "review".into(),
        path: "C:\\skills\\review\\SKILL.md".into(),
    };

    let enabled = SkillCatalog {
        skills: vec![skill("review", &binding.path, "user", true)],
        errors: Vec::new(),
    };

    let disabled = SkillCatalog {
        skills: vec![skill("review", &binding.path, "user", false)],
        errors: Vec::new(),
    };

    assert_eq!(
        validate_skill_binding("$review changes", Some(&binding), Some(&enabled)),
        Ok(Some(binding.clone()))
    );
    assert!(validate_skill_binding("$review changes", Some(&binding), Some(&disabled)).is_err());
    assert!(
        validate_skill_binding(
            "$review changes",
            Some(&binding),
            Some(&SkillCatalog::default())
        )
        .is_err()
    );
    assert_eq!(
        validate_skill_binding("$review changes", None, Some(&enabled)),
        Ok(None)
    );
}

#[test]
fn skill_selection_keeps_exact_path_and_rejects_disabled_rows() {
    let enabled = skill(
        "browser:control",
        "C:\\plugins\\browser\\SKILL.md",
        "system",
        true,
    );

    let disabled = skill(
        "browser:control",
        "C:\\repo\\browser\\SKILL.md",
        "repo",
        false,
    );

    assert_eq!(
        prepare_skill_selection(&enabled),
        Ok((
            "$browser:control ".into(),
            SkillReference {
                name: "browser:control".into(),
                path: "C:\\plugins\\browser\\SKILL.md".into(),
            }
        ))
    );
    assert!(prepare_skill_selection(&disabled).is_err());
}
