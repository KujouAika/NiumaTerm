use nmt_config::profile::Profile;

use crate::ui::tab_bar::menu::profile_root_choices;

/// A terminal Profile named `name` running `shell`.
fn profile(name: &str, shell: &str) -> Profile {
    Profile {
        name: name.to_string(),
        shell: shell.to_string(),
        args: String::new(),
    }
}

#[test]
fn combinations_of_an_unavailable_directory_stay_listed_and_disabled() {
    let profiles = [profile("P1", "pwsh.exe"), profile("P2", "cmd.exe")];

    let roots = vec![
        ("C:/A".to_string(), true),
        ("Z:/detached".to_string(), false),
    ];

    let choices = profile_root_choices(&profiles, &roots);

    assert_eq!(choices.len(), 4);

    let enabled: Vec<_> = choices.iter().map(|choice| choice.enabled).collect();

    assert_eq!(enabled, [true, false, true, false]);
}
