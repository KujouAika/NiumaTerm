use std::fs;

use crate::unix::SPAWNS_LOGIN_SHELL;
use crate::unix::shell::{
    BASH_FILES, BASH_HOOKS, BASH_RC, ZSH_FILES, ZSH_HOOKS, install_files, prompt_integration,
    resolved_shell, shell_name,
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
/// its own startup files suppressed and nothing put back in their place.
/// `/bin/sh` is bash on some systems, but in `sh` mode it reads neither the rc
/// file nor the profile chain the bootstrap replays.
#[test]
fn a_shell_without_an_integration_reports_none() {
    assert!(prompt_integration(Some("/usr/local/bin/fish")).is_none());
    assert!(prompt_integration(Some("/bin/sh")).is_none());
    assert!(prompt_integration(Some("/usr/bin/tcsh")).is_none());
}

/// zsh is handed the integration by being typed at, so the user's own
/// `ZDOTDIR` is never touched. The launch suppresses zsh's startup files
/// instead — and with them its line editor, which would draw the injected
/// line, and the history entry that line would otherwise leave behind.
#[test]
fn zsh_is_launched_bare_and_handed_its_bootstrap() {
    let integration = prompt_integration(Some("/bin/zsh")).expect("zsh is integrated");

    assert_eq!(integration.args, ["-f", "+Z", "-o", "histignorespace"]);
    assert!(integration.environment.is_empty());

    let bootstrap = integration.bootstrap.expect("zsh is bootstrapped");
    assert!(bootstrap.starts_with(" source '"), "{bootstrap:?}");
    assert!(
        bootstrap.ends_with(&format!("{ZSH_HOOKS}'\n")),
        "{bootstrap:?}"
    );
}

/// bash cannot be handed its integration the same way: hiding the injected
/// line means launching without a line editor, and bash then drops the
/// non-printing region of PS1 that carries the prompt-end mark — taking the
/// whole prompt with it. So bash keeps the `--rcfile` delivery, behind the
/// login hop that makes bash honour it.
#[test]
fn bash_is_integrated_through_its_rc_file() {
    let integration = prompt_integration(Some("/bin/bash")).expect("bash is integrated");

    assert!(integration.bootstrap.is_none());

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
    } else {
        assert_eq!(integration.args[0], "--rcfile");
        assert!(integration.args[1].ends_with(BASH_RC));
    }
}

/// The leading space is what keeps the injected line out of the session's
/// history, and the newline is what submits it. Both are load-bearing, and
/// neither is visible in the rendered result, so nothing else would catch
/// their loss.
#[test]
fn the_bootstrap_line_is_a_single_submitted_command() {
    let bootstrap = prompt_integration(Some("/bin/zsh"))
        .and_then(|integration| integration.bootstrap)
        .expect("zsh is bootstrapped");

    assert_eq!(bootstrap.matches('\n').count(), 1);
    assert!(bootstrap.ends_with('\n'));
    assert!(bootstrap.starts_with(' '));
}

/// Pointing a shell at a script that is not there would leave it with no
/// integration *and* none of its own startup files, so every script has to be
/// on disk before the launch is offered.
#[test]
fn the_bootstrap_scripts_are_installed() {
    for (shell, files) in [("zsh", &ZSH_FILES[..]), ("bash", &BASH_FILES[..])] {
        let dir = install_files(shell, files).expect("scripts install");

        for (name, contents) in files {
            assert_eq!(
                fs::read_to_string(dir.join(name)).expect("script is readable"),
                *contents,
                "{name} does not match the bundled copy"
            );
        }
    }
}
