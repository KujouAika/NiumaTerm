use std::fs;

use crate::unix::SPAWNS_LOGIN_SHELL;
use crate::unix::shell::{
    BASH_FILES, BASH_HOOKS, BASH_RC, ZSH_FILES, install_files, prompt_integration, resolved_shell,
    shell_name,
};

#[test]
fn a_configured_shell_is_identified_by_its_file_name() {
    assert_eq!(shell_name("/bin/zsh").as_deref(), Some("zsh"));
    assert_eq!(shell_name("/opt/bin/bash").as_deref(), Some("bash"));
    assert_eq!(shell_name("fish").as_deref(), Some("fish"));
}

/// An empty or absent shell means "whatever the user's login shell is", which
/// only the default resolution can answer.
#[test]
fn a_blank_shell_falls_back_to_the_default() {
    assert_eq!(resolved_shell(Some("")), resolved_shell(None));
    assert_eq!(resolved_shell(Some("   ")), resolved_shell(None));
    assert_eq!(resolved_shell(Some("  /bin/zsh  ")), "/bin/zsh");
}

/// A shell without an integration must be told so rather than launched with
/// startup files it would never read. `/bin/sh` is bash on some systems, but
/// in `sh` mode it reads neither the rc file nor the profile chain the
/// integration relies on.
#[test]
fn a_shell_without_an_integration_reports_none() {
    assert!(prompt_integration(Some("/usr/local/bin/fish")).is_none());
    assert!(prompt_integration(Some("/bin/sh")).is_none());
    assert!(prompt_integration(Some("/usr/bin/tcsh")).is_none());
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
    let dir = install_files("zsh", &ZSH_FILES).expect("startup files install");

    for (name, contents) in ZSH_FILES {
        assert_eq!(
            fs::read_to_string(dir.join(name)).expect("startup file is readable"),
            contents,
            "{name} does not match the bundled copy"
        );
    }
}

/// bash is reached through `--rcfile`, which it ignores as a login shell — so
/// where the backend launches login shells the rc has to arrive behind an
/// `exec` from one, and the shell path and rc path both travel quoted.
#[test]
fn bash_is_integrated_through_its_rc_file() {
    let integration = prompt_integration(Some("/bin/bash")).expect("bash is integrated");

    let hooks = integration
        .environment
        .iter()
        .find(|(name, _)| name == "NMT_BASH_INTEGRATION")
        .map(|(_, value)| value.clone())
        .expect("the hooks are named in the environment");
    assert!(hooks.ends_with(BASH_HOOKS));

    if SPAWNS_LOGIN_SHELL {
        assert_eq!(integration.args[0], "-lc");
        assert!(
            integration.args[1].starts_with("exec '/bin/bash' --rcfile '"),
            "{}",
            integration.args[1]
        );
        assert!(integration.args[1].ends_with(&format!("{BASH_RC}' -i")));
        // The profile chain the outer login shell runs already named the rc it
        // wanted; sourcing one here too would run it twice.
        assert!(
            !integration
                .environment
                .iter()
                .any(|(name, _)| name == "NMT_BASH_USER_RC")
        );
    } else {
        assert_eq!(integration.args[0], "--rcfile");
        assert!(integration.args[1].ends_with(BASH_RC));
        assert!(
            integration
                .environment
                .iter()
                .any(|(name, _)| name == "NMT_BASH_USER_RC")
        );
    }
}

/// A shell path with a space in it has to survive the `-lc` hop as one word.
#[test]
fn an_awkward_bash_path_is_quoted_into_the_login_hop() {
    if !SPAWNS_LOGIN_SHELL {
        return;
    }

    let integration = prompt_integration(Some("/opt/my shells/bash")).expect("bash is integrated");

    assert!(
        integration.args[1].starts_with("exec '/opt/my shells/bash' --rcfile '"),
        "{}",
        integration.args[1]
    );
}

/// The bash rc has the same standing as the zsh series: pointing bash at a
/// file that is not there would drop the user's own configuration.
#[test]
fn the_bash_rc_and_hooks_are_installed() {
    let dir = install_files("bash", &BASH_FILES).expect("startup files install");

    for (name, contents) in BASH_FILES {
        assert_eq!(
            fs::read_to_string(dir.join(name)).expect("startup file is readable"),
            contents,
            "{name} does not match the bundled copy"
        );
    }
}
