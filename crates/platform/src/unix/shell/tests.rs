use std::fs;

use crate::unix::shell::{ZSH_FILES, install_zsh_files, prompt_integration, shell_name};

#[test]
fn a_configured_shell_is_identified_by_its_file_name() {
    assert_eq!(shell_name(Some("/bin/zsh")).as_deref(), Some("zsh"));
    assert_eq!(
        shell_name(Some("  /opt/bin/bash  ")).as_deref(),
        Some("bash")
    );
    assert_eq!(shell_name(Some("fish")).as_deref(), Some("fish"));
}

/// An empty or absent shell means "whatever the user's login shell is", which
/// only the default resolution can answer.
#[test]
fn a_blank_shell_falls_back_to_the_default() {
    assert_eq!(shell_name(Some("")), shell_name(None));
    assert_eq!(shell_name(Some("   ")), shell_name(None));
}

/// Only zsh has an integration; every other shell must be told so rather than
/// launched with startup files that would not load.
#[test]
fn only_zsh_reports_an_integration() {
    assert!(prompt_integration(Some("/bin/bash")).is_none());
    assert!(prompt_integration(Some("/usr/local/bin/fish")).is_none());
    assert!(prompt_integration(Some("/bin/sh")).is_none());
}

/// zsh is reached through the environment, not through arguments: startup
/// arguments would either be ignored or replace the user's own launch command.
#[test]
fn zsh_is_integrated_through_zdotdir_alone() {
    let integration = prompt_integration(Some("/bin/zsh")).expect("zsh is integrated");

    assert!(integration.args.is_empty());

    let names: Vec<&str> = integration
        .environment
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(names, ["ZDOTDIR", "NMT_ZDOTDIR", "NMT_USER_ZDOTDIR"]);

    let zdotdir = &integration.environment[0].1;
    assert_eq!(zdotdir, &integration.environment[1].1);
    // Forwarding to our own directory would make every startup file source
    // itself, so the user's directory must be a different one.
    assert_ne!(zdotdir, &integration.environment[2].1);
}

/// A `ZDOTDIR` with no startup files in it would silently drop the user's own
/// zsh configuration, so every file has to be on disk before the launch is
/// offered.
#[test]
fn the_advertised_zdotdir_holds_the_whole_startup_series() {
    let dir = install_zsh_files().expect("startup files install");

    for (name, contents) in ZSH_FILES {
        assert_eq!(
            fs::read_to_string(dir.join(name)).expect("startup file is readable"),
            contents,
            "{name} does not match the bundled copy"
        );
    }
}
