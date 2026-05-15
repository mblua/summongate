//! Agent credential helpers.
//!
//! Produces the visible `# === Session Credentials ===` fallback block, the
//! env var payload used for agent PTY children, and shared scrubbing helpers
//! for child processes that must not inherit parent `AGENTSCOMMANDER_*` values.
//! The visible block output must stay byte-for-byte identical across spawn and
//! `/clear` call sites so agents parse consistently.

use uuid::Uuid;

pub const ENV_AGENTSCOMMANDER_TOKEN: &str = "AGENTSCOMMANDER_TOKEN";
pub const ENV_AGENTSCOMMANDER_ROOT: &str = "AGENTSCOMMANDER_ROOT";
pub const ENV_AGENTSCOMMANDER_BINARY: &str = "AGENTSCOMMANDER_BINARY";
pub const ENV_AGENTSCOMMANDER_BINARY_PATH: &str = "AGENTSCOMMANDER_BINARY_PATH";
pub const ENV_AGENTSCOMMANDER_LOCAL_DIR: &str = "AGENTSCOMMANDER_LOCAL_DIR";

pub const CREDENTIAL_ENV_KEYS: [&str; 5] = [
    ENV_AGENTSCOMMANDER_TOKEN,
    ENV_AGENTSCOMMANDER_ROOT,
    ENV_AGENTSCOMMANDER_BINARY,
    ENV_AGENTSCOMMANDER_BINARY_PATH,
    ENV_AGENTSCOMMANDER_LOCAL_DIR,
];

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialValues {
    pub token: String,
    pub root: String,
    pub binary: String,
    pub binary_path: String,
    pub local_dir: String,
}

pub fn build_credential_values(token: &Uuid, cwd: &str) -> CredentialValues {
    let exe = std::env::current_exe().ok();
    if exe.is_none() {
        log::warn!(
            "[credentials] current_exe() unavailable; credentials will use fallback \
             binary name. Agent may be unable to invoke the CLI."
        );
    }

    let binary = exe
        .as_ref()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "agentscommander".to_string());

    let binary_path = {
        let raw = exe
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "agentscommander.exe".to_string());
        raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
    };

    let local_dir = exe
        .as_ref()
        .and_then(|p| p.parent())
        .map(|parent| {
            parent
                .join(format!(".{}", &binary))
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| format!(".{}", &binary));

    CredentialValues {
        token: token.to_string(),
        root: cwd.to_string(),
        binary,
        binary_path,
        local_dir: local_dir
            .strip_prefix(r"\\?\")
            .unwrap_or(&local_dir)
            .to_string(),
    }
}

pub fn build_credentials_env(token: &Uuid, cwd: &str) -> Vec<(String, String)> {
    let values = build_credential_values(token, cwd);
    vec![
        (ENV_AGENTSCOMMANDER_TOKEN.to_string(), values.token),
        (ENV_AGENTSCOMMANDER_ROOT.to_string(), values.root),
        (ENV_AGENTSCOMMANDER_BINARY.to_string(), values.binary),
        (
            ENV_AGENTSCOMMANDER_BINARY_PATH.to_string(),
            values.binary_path,
        ),
        (ENV_AGENTSCOMMANDER_LOCAL_DIR.to_string(), values.local_dir),
    ]
}

pub fn apply_credential_env_to_pty_command(
    command: &mut portable_pty::CommandBuilder,
    extra_env: &[(String, String)],
) {
    for key in CREDENTIAL_ENV_KEYS {
        command.env_remove(key);
    }

    for (key, value) in extra_env {
        command.env(key.as_str(), value.as_str());
    }
}

pub fn scrub_credentials_from_std_command(command: &mut std::process::Command) {
    for key in CREDENTIAL_ENV_KEYS {
        command.env_remove(key);
    }
}

pub fn scrub_credentials_from_tokio_command(command: &mut tokio::process::Command) {
    scrub_credentials_from_std_command(command.as_std_mut());
}

/// Build the credentials block for a session.
///
/// The block is terminated by `\n` (no trailing Enter) — the caller is
/// responsible for calling `inject_text_into_session` which adds the Enter
/// keystrokes for agents that need them.
///
/// `token` is `Display`'d lowercase with dashes (standard `Uuid` format).
/// `cwd` is the session's working directory, verbatim.
///
/// `Binary`, `BinaryPath`, and `LocalDir` are derived from the current
/// process executable — the running `agentscommander*.exe`. This matches
/// the original inline behavior in `commands/session.rs` and is what
/// agents use to invoke back into the CLI.
///
/// No I/O except `std::env::current_exe()` and a single `log::warn!`
/// when `current_exe()` returns `Err` (per plan §17.6 for operator
/// observability on the rare "agent cannot find its binary" path).
pub fn build_credentials_block(token: &Uuid, cwd: &str) -> String {
    let values = build_credential_values(token, cwd);

    format!(
        concat!(
            "\n\n",
            "# === Session Credentials ===\n",
            "# Token: {token}\n",
            "# Root: {root}\n",
            "# Binary: {binary}\n",
            "# BinaryPath: {binary_path}\n",
            "# LocalDir: {local_dir}\n",
            "# === End Credentials ===\n",
        ),
        token = values.token,
        root = values.root,
        binary = values.binary,
        binary_path = values.binary_path,
        local_dir = values.local_dir,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_structure_is_byte_stable() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let block = build_credentials_block(&token, r"C:\example\root");

        // Split on '\n' (keep empty trailing element → final "" after last \n).
        let lines: Vec<&str> = block.split('\n').collect();

        // Expected structure, in order (10 elements: 9 \n-terminated lines + trailing empty):
        //  0: ""                                   (first leading \n)
        //  1: ""                                   (second leading \n)
        //  2: "# === Session Credentials ==="
        //  3: "# Token: 00000000-0000-0000-0000-000000000001"
        //  4: "# Root: C:\example\root"
        //  5: "# Binary: <runtime-derived>"
        //  6: "# BinaryPath: <runtime-derived>"
        //  7: "# LocalDir: <runtime-derived>"
        //  8: "# === End Credentials ==="
        //  9: ""                                   (trailing \n)
        assert_eq!(lines.len(), 10, "line count drift: {}", lines.len());
        assert_eq!(lines[0], "", "missing first leading newline");
        assert_eq!(lines[1], "", "missing second leading newline");
        assert_eq!(lines[2], "# === Session Credentials ===");
        assert_eq!(lines[3], "# Token: 00000000-0000-0000-0000-000000000001");
        assert_eq!(lines[4], r"# Root: C:\example\root");
        assert!(lines[5].starts_with("# Binary: "), "Binary line prefix");
        assert!(lines[6].starts_with("# BinaryPath: "), "BinaryPath prefix");
        assert!(lines[7].starts_with("# LocalDir: "), "LocalDir prefix");
        assert_eq!(lines[8], "# === End Credentials ===");
        assert_eq!(lines[9], "", "missing trailing newline");
    }

    #[test]
    fn env_contains_expected_keys_and_values() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let env = build_credentials_env(&token, r"C:\example\root");
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();

        assert_eq!(map.len(), 5);
        assert_eq!(
            map.get(ENV_AGENTSCOMMANDER_TOKEN).map(String::as_str),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(
            map.get(ENV_AGENTSCOMMANDER_ROOT).map(String::as_str),
            Some(r"C:\example\root")
        );
        assert!(map
            .get(ENV_AGENTSCOMMANDER_BINARY)
            .is_some_and(|v| !v.is_empty()));
        assert!(map
            .get(ENV_AGENTSCOMMANDER_BINARY_PATH)
            .is_some_and(|v| !v.is_empty()));
        assert!(map
            .get(ENV_AGENTSCOMMANDER_LOCAL_DIR)
            .is_some_and(|v| !v.is_empty()));
    }

    #[test]
    fn pty_apply_helper_removes_stale_credentials_when_extra_env_empty() {
        let mut command = portable_pty::CommandBuilder::new("agent.exe");
        for key in CREDENTIAL_ENV_KEYS {
            command.env(key, "stale-parent-value");
        }

        apply_credential_env_to_pty_command(&mut command, &[]);

        for key in CREDENTIAL_ENV_KEYS {
            assert!(
                command.get_env(key).is_none(),
                "{key} should be removed from non-agent PTY children"
            );
        }
    }

    #[test]
    fn pty_apply_helper_overrides_stale_credentials_when_extra_env_present() {
        let token = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        let extra_env = build_credentials_env(&token, r"C:\fresh\root");
        let mut command = portable_pty::CommandBuilder::new("agent.exe");

        for key in CREDENTIAL_ENV_KEYS {
            command.env(key, "stale-parent-value");
        }

        apply_credential_env_to_pty_command(&mut command, &extra_env);

        for (key, value) in extra_env {
            assert_eq!(
                command.get_env(key.as_str()).and_then(|v| v.to_str()),
                Some(value.as_str())
            );
        }
    }

    #[test]
    fn std_and_tokio_scrub_helpers_remove_explicit_credentials() {
        fn explicit_env_is_removed(command: &std::process::Command, key: &str) -> bool {
            command
                .get_envs()
                .any(|(env_key, value)| env_key == std::ffi::OsStr::new(key) && value.is_none())
        }

        let mut std_cmd = std::process::Command::new("git");
        for key in CREDENTIAL_ENV_KEYS {
            std_cmd.env(key, "stale-parent-value");
        }
        scrub_credentials_from_std_command(&mut std_cmd);
        for key in CREDENTIAL_ENV_KEYS {
            assert!(explicit_env_is_removed(&std_cmd, key));
        }

        let mut tokio_cmd = tokio::process::Command::new("git");
        for key in CREDENTIAL_ENV_KEYS {
            tokio_cmd.env(key, "stale-parent-value");
        }
        scrub_credentials_from_tokio_command(&mut tokio_cmd);
        for key in CREDENTIAL_ENV_KEYS {
            assert!(explicit_env_is_removed(tokio_cmd.as_std(), key));
        }
    }
}
