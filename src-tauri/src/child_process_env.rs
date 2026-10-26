//! Minimal inherited environment for locally launched helper processes.
//!
//! This is credential isolation, not a filesystem or network sandbox. Callers
//! may add only the variables their specific helper needs after applying it.

use std::process::Command;

pub(crate) fn apply_minimal_child_environment(command: &mut Command) {
    command.env_clear();
    #[cfg(windows)]
    let launch_keys = [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "WINDIR",
        "ComSpec",
        "TEMP",
        "TMP",
        "APPDATA",
        "LOCALAPPDATA",
        "USERPROFILE",
        // Standard installation roots are needed by Node/Windows launchers
        // to discover installed programs, not model or service credentials.
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
    ];
    #[cfg(not(windows))]
    let launch_keys = ["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"];
    for key in launch_keys {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_environment_does_not_inherit_unrelated_credentials() {
        let mut command = Command::new("unused");
        command.env("ANGELBOT_MODEL_TEST_SECRET", "must-not-leak");
        apply_minimal_child_environment(&mut command);
        assert!(command.get_envs().any(|(key, _)| key == "PATH"));
        assert!(command
            .get_envs()
            .all(|(key, _)| key != "ANGELBOT_MODEL_TEST_SECRET"));
        #[cfg(windows)]
        for key in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
            if let Some(expected) = std::env::var_os(key) {
                assert!(
                    command.get_envs().any(|(inherited, value)| inherited
                        .to_string_lossy()
                        .eq_ignore_ascii_case(key)
                        && value == Some(expected.as_os_str())),
                    "missing Windows installation root: {key}"
                );
            }
        }
    }
}
