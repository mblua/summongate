# Issue 212: Env-first credentials for agent processes

## Requirement

AgentsCommander currently gives agent sessions their session credentials by pasting a visible `# === Session Credentials ===` block into the PTY after the child process is already running. That creates ordering races with wake delivery, `/clear`, and other injected messages, and a cleared context can leave an agent without credentials.

Change agent process creation so every agent child process is spawned with these per-process environment variables before `portable_pty::CommandBuilder::spawn_command`:

- `AGENTSCOMMANDER_TOKEN`
- `AGENTSCOMMANDER_ROOT`
- `AGENTSCOMMANDER_BINARY`
- `AGENTSCOMMANDER_BINARY_PATH`
- `AGENTSCOMMANDER_LOCAL_DIR`

Do not set process-global environment variables in the AgentsCommander parent. Do not add `std::env::set_var` in runtime code. Credentials must be applied only through each child process's `CommandBuilder`.

Also scrub all inherited `AGENTSCOMMANDER_*` credentials from every other child process spawned by AgentsCommander through `std::process::Command` or `tokio::process::Command`. If the GUI itself was launched from an agent-owned environment, non-agent helper processes such as `git`, `cmd`, `node`, `powershell.exe`, and `where.exe` must not inherit that token. Agent PTY spawn is the only path that re-applies per-session credential env values.

Do not echo invalid token input in CLI errors. `validate_cli_token` must not include any substring of a rejected `--token` value in stdout/stderr, because env-first usage makes `AGENTSCOMMANDER_TOKEN` the normal source for that argument.

Keep the visible PTY credential injection path for compatibility during this issue, but make environment variables the primary path. `/clear` should no longer require a successful credential re-paste for the still-live child process, because its environment survives context clearing.

## Current State

- `src-tauri/src/session/manager.rs:36-75` creates `Session` records and assigns `Session.token = Uuid::new_v4()` at line 69.
- `src-tauri/src/commands/session.rs:633-950` is the shared session creation path. It resolves `agent_id`, creates the `Session`, mutates provider args, and calls `pty_mgr.spawn` at lines 821-825.
- `src-tauri/src/pty/manager.rs:280-350` builds `portable_pty::CommandBuilder`, wraps non-`.exe` Windows commands in `cmd.exe /C`, applies existing env (`TERM`, `GIT_CEILING_DIRECTORIES`, git guard env), and calls `spawn_command`.
- `src-tauri/src/pty/credentials.rs:26-77` derives the visible credential block from `(token, cwd, current_exe())`.
- `src-tauri/src/commands/session.rs:827-937` injects the bootstrap PTY text for agent sessions after spawn.
- `src-tauri/src/phone/mailbox.rs:821-846` currently treats credential re-injection after `/clear` as required before delivering the follow-up body.
- `src-tauri/src/phone/mailbox.rs:992-1056` builds and injects the visible credential block after `/clear`.

## Data Flow

1. `SessionManager::create_session` creates the token at `src-tauri/src/session/manager.rs:69`.
2. `create_session_inner` receives the created `Session` at `src-tauri/src/commands/session.rs:664-676`.
3. Immediately before `pty_mgr.spawn`, `create_session_inner` derives `extra_env` from `session.token` and `cwd` if and only if the resolved `agent_id` is `Some(_)`.
4. `PtyManager::spawn` calls the shared credential env helper to remove all `AGENTSCOMMANDER_*` credential keys from the base inherited environment for every PTY child, then applies the supplied `extra_env` through `CommandBuilder::env`.
5. `spawn_command` starts the child with credentials already in its environment.
6. The existing visible bootstrap injection remains a fallback and still carries the auto-title prompt appendage.
7. Every non-PTY `std::process::Command` and `tokio::process::Command` call site calls the shared scrub helper before spawning or awaiting output/status, so inherited GUI credentials cannot leak to helper tools.

This preserves the existing PTY I/O path:

```
frontend xterm input -> pty_write -> PTY stdin
PTY stdout -> read loop -> pty_output event -> xterm.write
```

No frontend IPC type changes are required.

## Affected Files

### 1. `src-tauri/src/pty/credentials.rs`

Purpose: make credential derivation reusable for the visible block, agent child env vars, and inherited credential scrubbing on non-agent child processes.

At lines 1-5, replace the module docs with:

```rust
//! Agent credential helpers.
//!
//! Produces the visible `# === Session Credentials ===` fallback block, the
//! env var payload used for agent PTY children, and shared scrubbing helpers
//! for child processes that must not inherit parent `AGENTSCOMMANDER_*` values.
//! The visible block output must stay byte-for-byte identical across spawn and
//! `/clear` call sites so agents parse consistently.
```

At line 7, after `use uuid::Uuid;`, add credential env constants and a reusable value struct:

```rust
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
```

Replace the body of `build_credentials_block` at lines 26-77 with helper-based formatting. Extract the existing `current_exe` logic into a new `build_credential_values` function placed immediately before `build_credentials_block`:

```rust
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
        local_dir: local_dir.strip_prefix(r"\\?\").unwrap_or(&local_dir).to_string(),
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
        (
            ENV_AGENTSCOMMANDER_LOCAL_DIR.to_string(),
            values.local_dir,
        ),
    ]
}
```

Immediately after `build_credentials_env`, add shared scrub/apply helpers:

```rust
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
```

Then make `build_credentials_block` call `build_credential_values` and format from `values.*`. Keep the exact visible block shape from lines 60-76:

```rust
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
```

Extend the existing test module at lines 79-114:

- Keep `block_structure_is_byte_stable`.
- Add `env_contains_expected_keys_and_values`:

```rust
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
```

- Add `pty_apply_helper_removes_stale_credentials_when_extra_env_empty`:

```rust
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
```

- Add `pty_apply_helper_overrides_stale_credentials_when_extra_env_present`:

```rust
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
```

- Add `std_and_tokio_scrub_helpers_remove_explicit_credentials`:

```rust
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
```

Notes:

- Do not add new crates.
- Keep `std::env::current_exe()`; this reads process state and is not the prohibited global mutation.
- Do not log the token or any derived credential value.
- Do not gate credential scrubbing on `!extra_env.is_empty()`. Empty `extra_env` is the non-agent path and still requires removal.

### 2. `src-tauri/src/pty/manager.rs`

Purpose: accept per-child env vars and apply them with the shared scrub/apply helper before `spawn_command`.

Change the `PtyManager::spawn` signature at lines 280-289 to add `extra_env` before `app_handle`:

```rust
pub fn spawn(
    &self,
    id: Uuid,
    cmd: &str,
    args: &[String],
    cwd: &str,
    cols: u16,
    rows: u16,
    extra_env: &[(String, String)],
    app_handle: AppHandle,
) -> Result<(), AppError> {
```

After `command.env("TERM", "xterm-256color");` at line 327, call the shared helper, then log only the count:

```rust
        crate::pty::credentials::apply_credential_env_to_pty_command(&mut command, extra_env);

        if !extra_env.is_empty() {
            log::info!(
                "[pty] Applied {} per-process credential environment variables for session {}",
                extra_env.len(),
                id
            );
        }
```

Why remove first:

- `portable_pty::CommandBuilder::new` starts from the parent process environment.
- If the AgentsCommander GUI was launched from an agent-owned shell, the parent environment might already contain `AGENTSCOMMANDER_TOKEN`.
- Calling the shared helper on all credential keys guarantees non-agent shells do not inherit a stale or unrelated token.

Keep existing `TERM`, `GIT_CEILING_DIRECTORIES`, and git guard env logic intact. The credential env keys do not overlap with `PATH`, `PATHEXT`, or `AC_REAL_GIT`.

At lines 46-48 in `resolve_real_git_path`, scrub the Windows `where.exe` helper process before `output()`:

```rust
    let mut cmd = std::process::Command::new("where.exe");
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    cmd.arg("git.exe").creation_flags(CREATE_NO_WINDOW);
```

### 3. `src-tauri/src/commands/session.rs`

Purpose: derive credentials from the actual session token and cwd, then pass them into PTY spawn only for agent sessions.

At lines 817-825, between `session.effective_shell_args = Some(effective);` and `pty_mgr.lock()...spawn(...)`, add:

```rust
    let extra_env = if agent_id.is_some() {
        crate::pty::credentials::build_credentials_env(&session.token, &cwd)
    } else {
        Vec::new()
    };
```

Change the spawn call at lines 821-825 from:

```rust
        .spawn(id, &shell, &shell_args, &cwd, 120, 30, app.clone())
```

to:

```rust
        .spawn(id, &shell, &shell_args, &cwd, 120, 30, &extra_env, app.clone())
```

Update the comment at lines 827-829 from "Auto-inject credentials" to "Auto-inject bootstrap text". Suggested replacement:

```rust
    // Auto-inject bootstrap text for agent sessions after PTY spawn.
    // Credentials are already present in child environment variables; the
    // visible credential block remains as a compatibility fallback and the
    // same injection still carries the optional auto-title prompt.
```

Do not change the `create_session_inner` public signature. All callers already converge through this function, including:

- UI create: `src-tauri/src/commands/session.rs:1032-1045`
- restart: `src-tauri/src/commands/session.rs:1271-1284`
- root agent: `src-tauri/src/commands/session.rs:1661-1674`
- startup restore: `src-tauri/src/lib.rs:635-648`
- cold wake spawn: `src-tauri/src/phone/mailbox.rs:681-694`
- session request spawn: `src-tauri/src/phone/mailbox.rs:1834`
- web create: `src-tauri/src/web/commands.rs:84-98`

Because `create_session_inner` resolves the final `agent_id` at lines 647-657, the env gating must use that resolved local `agent_id`, not only the caller-provided option.

### 4. `src-tauri/src/phone/mailbox.rs`

Purpose: `/clear` should no longer require successful credential re-injection before the follow-up body can be delivered.

At lines 821-824, replace the post-command comment with:

```rust
            // Post-command background work:
            //  - For `/clear` on an agent session: best-effort visible credential
            //    re-inject for compatibility. Env credentials remain available
            //    to the still-live child process, so fallback failure must not
            //    block the body follow-up.
            //  - For `/compact` (or `/clear` on a plain shell): body follow-up only.
```

At lines 831-846, change the failure branch so it logs and continues instead of returning:

```rust
                if is_clear {
                    if let Err(e) =
                        Self::reinject_credentials_after_clear_static(&app_clone, session_id).await
                    {
                        log::warn!(
                            "[mailbox] Compatibility credential re-inject after /clear failed \
                             (session={}): {}. Continuing because env credentials remain set \
                             for the live process.",
                            session_id,
                            e
                        );
                    }
                }
```

Update the doc comment at lines 984-991 to describe compatibility fallback:

```rust
    /// Wait for agent to become idle after `/clear`, then best-effort re-inject
    /// the visible credentials block for compatibility with agents that still
    /// rely on conversation text. Env-first credentials remain available in the
    /// still-live child process and are not affected by `/clear`.
```

Do not remove `reinject_credentials_after_clear_static` in this issue. It is still useful for older agent instructions and for the visible fallback path.

Also do not remove `inject_fresh_token_static` at lines 1871-1953. A live process environment cannot be changed after spawn, so any future token refresh for a still-live session still needs a visible notice until the session respawns.

### 5. `src-tauri/src/config/session_context.rs`

Purpose: make env vars the primary documented credential path for agents.

At lines 572-580, replace the current `## CLI executable` text with env-first guidance:

````md
## CLI executable

Your AgentsCommander session credentials are available as environment variables:

- `AGENTSCOMMANDER_TOKEN`: your session authentication token
- `AGENTSCOMMANDER_ROOT`: your working directory (agent root)
- `AGENTSCOMMANDER_BINARY`: the CLI binary name
- `AGENTSCOMMANDER_BINARY_PATH`: the full path to the CLI executable you must use
- `AGENTSCOMMANDER_LOCAL_DIR`: the config directory name for this instance

Use `AGENTSCOMMANDER_BINARY_PATH` when invoking the CLI. This ensures you use the correct binary for your instance, whether it is the installed version or a dev/WG build.

```
"<AGENTSCOMMANDER_BINARY_PATH>" <subcommand> [args]
```

**RULE:** Never hardcode or guess the binary path. Prefer the environment variables above. If they are unavailable, fall back to the latest `# === Session Credentials ===` block in your conversation.
````

At lines 588-591, update the help examples to use env placeholders:

````md
```
"<AGENTSCOMMANDER_BINARY_PATH>" --help                  # List all subcommands
"<AGENTSCOMMANDER_BINARY_PATH>" send --help             # Full docs for sending messages
"<AGENTSCOMMANDER_BINARY_PATH>" list-peers --help       # Full docs for discovering peers
```
````

At lines 596-608, replace the `## Session credentials` section with:

```md
## Session credentials

Your session credentials are delivered through the `AGENTSCOMMANDER_*` environment variables listed above. A visible `# === Session Credentials ===` block may also appear in your conversation as a compatibility fallback.

Use environment variables first. If AgentsCommander later injects a token refresh notice, that visible refresh is authoritative until the session respawns, because a live process environment cannot be mutated.

Your agent root is your current working directory.
```

At lines 625-627, update the send example to use env placeholders:

````md
```
"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>" --to "<agent_name>" --send <filename> --mode wake
```
````

At lines 641-645, update the list-peers example to use env placeholders:

````md
```
"<AGENTSCOMMANDER_BINARY_PATH>" list-peers --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>"
```
````

Do not change the surrounding file-based messaging rules or `--send` filename-only warning.

### 6. `src-tauri/src/cli/mod.rs`

Purpose: avoid misleading CLI help/errors that mention only the visible credential block, and prevent partial token disclosure.

At lines 16-18, replace the top-level `after_help` `TOKEN:` text with:

```rust
TOKEN: In agent sessions, pass AGENTSCOMMANDER_TOKEN from the environment. \
If the env var is unavailable, use the latest visible '# === Session Credentials ===' fallback block. \
If a token expires, any failed `send` triggers an automatic token refresh.\n\n\
```

At lines 104-108 and 129-135, update the error strings to mention `AGENTSCOMMANDER_TOKEN` as the primary source. Keep the same validation accept/reject semantics, but remove all echoing of invalid token input.

Suggested missing-token text:

```rust
"Error: --token is required. In agent sessions, pass AGENTSCOMMANDER_TOKEN \
 from the environment, or use the latest '# === Session Credentials ===' \
 fallback block if the env var is unavailable."
```

Suggested invalid-token suffix:

```rust
"Error: invalid token supplied. Expected a valid session token (UUID) or root token. \
 In agent sessions, use AGENTSCOMMANDER_TOKEN from the environment, or the latest \
 visible credentials fallback block if the env var is unavailable."
```

Implementation detail for lines 129-135:

- Delete `let display = if token.len() > 8 { &token[..8] } else { &token };`.
- Replace `return Err(format!(... display ...));` with `return Err("...".to_string());`.
- Do not include `{}` formatting or `token` in the invalid-token error.

Add a `#[cfg(test)] mod tests` at the end of `src-tauri/src/cli/mod.rs` if none exists. Include:

```rust
#[test]
fn validate_cli_token_does_not_echo_invalid_input() {
    let supplied = "super-secret-token-with-hidden-garbage";
    let err = validate_cli_token(&Some(supplied.to_string())).unwrap_err();

    assert!(err.contains("invalid token supplied"));
    assert!(!err.contains(supplied));
    assert!(!err.contains(&supplied[..8]));
}
```

### 7. Non-PTY child process credential scrubbing

Purpose: prevent inherited GUI `AGENTSCOMMANDER_*` values from leaking into helper processes created outside PTY agent spawn.

Use the helpers added in `src-tauri/src/pty/credentials.rs`. For each `std::process::Command`, call:

```rust
crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
```

For each `tokio::process::Command`, call:

```rust
crate::pty::credentials::scrub_credentials_from_tokio_command(&mut cmd);
```

Apply these exact edits:

- `src-tauri/src/config/session_context.rs:245-247`: after `let mut cmd = std::process::Command::new("git");`, call the std scrub helper before `cmd.args(...)`.
- `src-tauri/src/commands/ac_discovery.rs:253-255`: after `let mut cmd = std::process::Command::new("git");`, call the std scrub helper before `cmd.args(...)`.
- `src-tauri/src/commands/ac_discovery.rs:804-807`: after `let mut cmd = tokio::process::Command::new("git");`, call the tokio scrub helper before `cmd.args(...)`.
- `src-tauri/src/commands/entity_creation.rs:1435-1439`: after `let mut cmd = std::process::Command::new("git");`, call the std scrub helper before `cmd.args(...)`.
- `src-tauri/src/commands/entity_creation.rs:1454-1458`: after `let mut cmd2 = std::process::Command::new("git");`, call the std scrub helper before `cmd2.args(...)`.
- `src-tauri/src/commands/entity_creation.rs:1779-1781`: after `let mut cmd = tokio::process::Command::new("git");`, call the tokio scrub helper before `cmd.args(...)`.
- `src-tauri/src/commands/entity_creation.rs:1812-1813`: after `let mut reset_cmd = tokio::process::Command::new("git");`, call the tokio scrub helper before `reset_cmd.args(...)`.
- `src-tauri/src/pty/git_watcher.rs:173-176`: after `let mut cmd = tokio::process::Command::new("git");`, call the tokio scrub helper before `cmd.args(...)`.
- `src-tauri/src/config/claude_settings.rs:994-1002`: replace the chained `std::process::Command::new("cmd").args(...).status()` with:

```rust
            let mut cmd = std::process::Command::new("cmd");
            crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
            let status = cmd
                .args([
                    "/C",
                    "mklink",
                    "/J",
                    link_path.to_str().unwrap(),
                    real_target.to_str().unwrap(),
                ])
                .status();
```

- `src-tauri/src/config/claude_settings.rs:1185-1193`: replace the chained `std::process::Command::new("cmd").args(...).status()` with the same pattern, using `link.to_str().unwrap()` and `outside.to_str().unwrap()`.
- `src-tauri/src/config/claude_settings.rs:1232`: replace `let node_check = Command::new("node").arg("--version").output();` with:

```rust
        let mut node_check_cmd = Command::new("node");
        crate::pty::credentials::scrub_credentials_from_std_command(&mut node_check_cmd);
        let node_check = node_check_cmd.arg("--version").output();
```

- `src-tauri/src/config/claude_settings.rs:1246-1252`: replace the chained `Command::new("node").arg(...).spawn()` with:

```rust
        let mut node_cmd = Command::new("node");
        crate::pty::credentials::scrub_credentials_from_std_command(&mut node_cmd);
        let mut child = node_cmd
            .arg("-e")
            .arg(js_body)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("node spawn");
```

- `src-tauri/src/commands/wg_delete_diagnostic.rs:1623-1629`: replace the chained `Command::new("powershell.exe").args(...).spawn()` with a mutable `let mut cmd = Command::new("powershell.exe");`, call the std scrub helper, then chain `.args(...)`, `.current_dir(...)`, stdio setup, and `.spawn()`.

### 8. Remaining visible-block-only help strings

Purpose: update all visible-block-only user guidance in this issue; do not defer this.

Apply these exact documentation edits:

- `src-tauri/src/cli/send.rs:22`: replace with `/// Session token for authentication (from AGENTSCOMMANDER_TOKEN or visible credentials fallback)`.
- `src-tauri/src/cli/list_peers.rs:21`: same replacement.
- `src-tauri/src/cli/close_session.rs:20`: same replacement.
- `src-tauri/src/cli/brief_set_title.rs:28`: same replacement.
- `src-tauri/src/cli/brief_append_body.rs:29`: same replacement.

In `src-tauri/src/pty/title_prompt.rs`:

- At lines 11-13, replace the module comment with:

```rust
//! No I/O. Pure string format. The agent substitutes
//! `<AGENTSCOMMANDER_TOKEN>`, `<AGENTSCOMMANDER_ROOT>`, and
//! `<AGENTSCOMMANDER_BINARY_PATH>` from environment variables first, falling
//! back to the visible `# === Session Credentials ===` block only if env vars
//! are unavailable (Round 4 §R4.2 still provides the compatibility paste).
```

- At lines 28-31, replace the command and credential-source strings with:

```rust
            "  \"<AGENTSCOMMANDER_BINARY_PATH>\" brief-set-title --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\" --title \"<your title>\"\n\n",
            "`<AGENTSCOMMANDER_BINARY_PATH>`, `<AGENTSCOMMANDER_TOKEN>`, and ",
            "`<AGENTSCOMMANDER_ROOT>` mean the environment variables of the same names. ",
            "If env vars are unavailable, use `BinaryPath`, `Token`, and `Root` from ",
            "the latest visible `# === Session Credentials ===` fallback block. ",
            "The CLI writes BRIEF.md atomically and ",
```

- Update tests at lines 47-68:
  - `prompt_contains_path_and_cli_verb_invocation` should assert the new `<AGENTSCOMMANDER_BINARY_PATH>`, `<AGENTSCOMMANDER_TOKEN>`, and `<AGENTSCOMMANDER_ROOT>` placeholders.
  - Rename `prompt_references_credentials_block_for_substitution` to `prompt_documents_env_first_credentials_with_visible_fallback`.
  - That test should assert the prompt contains `environment variables`, `visible`, and `` `# === Session Credentials ===` ``.

## Dependencies

No new crates, frontend dependencies, Tauri plugins, or IPC types are needed.

Use existing APIs only:

- `portable_pty::CommandBuilder::env`
- `portable_pty::CommandBuilder::env_remove`
- `portable_pty::CommandBuilder::get_env` for helper unit tests
- `std::process::Command::env_remove`
- `std::process::Command::get_envs` for helper unit tests
- `tokio::process::Command::env_remove`
- `tokio::process::Command::as_std` / `as_std_mut`
- existing `uuid::Uuid`
- existing `log`

## Windows Compatibility

- `PtyManager::spawn` currently wraps non-`.exe` commands with `cmd.exe /C` at `src-tauri/src/pty/manager.rs:303-325`.
- Apply `env_remove` and `env` to the final `CommandBuilder` after the wrapper is built. On Windows wrapper launches, the env is assigned to `cmd.exe`, and `cmd.exe /C` passes it to the resolved `.cmd`, `.bat`, `.ps1`, or PATH command through normal process inheritance.
- Direct `.exe` launches receive the same env directly.
- For non-PTY Windows helper processes, call the std/tokio scrub helper before `creation_flags(CREATE_NO_WINDOW)`, `status()`, `output()`, or `spawn()`. The helper only removes `AGENTSCOMMANDER_*` keys and does not affect `PATH`, `PATHEXT`, `AC_REAL_GIT`, `GIT_CEILING_DIRECTORIES`, or command-specific stdio settings.
- Do not implement env delivery by prefixing `set KEY=VALUE && ...` into the command line. That would expose secrets in command strings, interact badly with quoting, and duplicate shell-specific behavior.
- Do not call `std::env::set_var` or mutate the parent process environment.
- Keep the existing `\\?\` prefix stripping in credential derivation so `AGENTSCOMMANDER_BINARY_PATH` and `AGENTSCOMMANDER_LOCAL_DIR` match the visible block on Windows.

## Security and Logging

- Never log `AGENTSCOMMANDER_TOKEN` or any full credential env value.
- Logging `extra_env.len()` is acceptable. Avoid logging the whole `extra_env` vector.
- Continue stripping tokens when moving messages to delivered storage; this issue does not change mailbox persistence.
- Remove inherited credential env keys from every child process before applying per-agent credentials. This prevents accidental leakage into plain shell sessions and non-agent helper tools if the AgentsCommander parent process was itself launched from an agent environment.
- `validate_cli_token` must never include rejected token input in its error message, including prefixes such as the first eight characters.
- Do not expose env values over frontend IPC or add them to `SessionInfo`.

## Validation Plan

Unit tests:

- Run `cd src-tauri; cargo test pty::credentials::tests::block_structure_is_byte_stable`.
- Add and run `cd src-tauri; cargo test pty::credentials::tests::env_contains_expected_keys_and_values`.
- Add and run `cd src-tauri; cargo test pty::credentials::tests::pty_apply_helper_removes_stale_credentials_when_extra_env_empty`.
- Add and run `cd src-tauri; cargo test pty::credentials::tests::pty_apply_helper_overrides_stale_credentials_when_extra_env_present`.
- Add and run `cd src-tauri; cargo test pty::credentials::tests::std_and_tokio_scrub_helpers_remove_explicit_credentials`.
- Add and run `cd src-tauri; cargo test cli::tests::validate_cli_token_does_not_echo_invalid_input`.
- Run `cd src-tauri; cargo test commands::session::tests::should_inject_continue` as a smoke check that the session command test module still compiles after the spawn signature change.

Manual validation:

- Normal UI agent spawn: configure a temporary agent command that prints only env presence, not token value, for example `powershell.exe -NoProfile -Command "Write-Output ('has-token=' + [bool]$env:AGENTSCOMMANDER_TOKEN); Write-Output ('root=' + $env:AGENTSCOMMANDER_ROOT); Start-Sleep 3"`. Create it through the UI with an `agentId`; expect `has-token=True` and root equal to the session cwd.
- Cold wake agent spawn: close/destroy the target session, send a wake message that causes `src-tauri/src/phone/mailbox.rs:681-694` to spawn it, and verify the spawned process has `AGENTSCOMMANDER_TOKEN` before the wake body is injected.
- `/clear` credential persistence: run an agent session, send a `/clear` command with a non-empty follow-up body, and verify the body still delivers even if the visible credential re-inject path logs a warning. The live process env should remain available.
- Non-agent shell exclusion: create a plain shell session with `agent_id = None` and verify `AGENTSCOMMANDER_TOKEN` is absent. This should hold even if the AgentsCommander parent process was launched from a shell that already had `AGENTSCOMMANDER_TOKEN`, because `PtyManager::spawn` removes inherited credential keys.
- No token in logs: search runtime logs for the UUID token from the manual agent session. Expected result: no match. It is acceptable to see the count log from `PtyManager::spawn`.
- Windows wrapper compatibility: configure an agent command that resolves through a `.cmd` or bare command path and prints `[bool]$env:AGENTSCOMMANDER_TOKEN` from the launched tool. Confirm the wrapper path receives the env just like direct `.exe` launches.
- Non-PTY helper scrub review: run `rg -n "Command::new|std::process::Command::new|tokio::process::Command::new|env_remove" src-tauri/src` after implementation. Inspect every `Command::new` match and confirm it either calls `scrub_credentials_from_std_command`, calls `scrub_credentials_from_tokio_command`, or is the agent PTY `CommandBuilder` path that calls `apply_credential_env_to_pty_command`.
- Help/doc review: run `rg -n "Session Credentials|YOUR_TOKEN|YOUR_BINARY_PATH|AGENTSCOMMANDER_TOKEN|AGENTSCOMMANDER_BINARY_PATH" src-tauri/src/cli src-tauri/src/pty/title_prompt.rs src-tauri/src/config/session_context.rs`. Confirm remaining visible credential references describe fallback behavior, not primary guidance.

Suggested full compile check after implementation:

```powershell
cd src-tauri
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features
cargo test
```

## Notes and Constraints

- Keep env derivation tied to `session.token` and `cwd` from `create_session_inner`; do not derive from user-provided args or frontend state after spawn.
- Do not change `SessionInfo`, TypeScript shared types, or frontend stores.
- Do not remove visible credential injection in this issue. It remains a compatibility fallback and currently shares the same injection with the auto-title prompt.
- Token rotation without respawn cannot update environment variables in an already-running child process. If a live session needs a new token, keep using the visible `inject_fresh_token_static` notice until a future respawn.
- The visible block and env vars must stay semantically aligned. The token, root, binary, binary path, and local dir should come from the same `CredentialValues` helper.
- Do not defer the remaining CLI/title/session-context wording in this issue. The plan now requires those strings to become env-first with visible-block fallback language.

## Open Questions and Risks

- Some providers may not expose process env directly to the model unless the agent runs shell commands. The context instruction update tells agents to use env vars in CLI commands, while visible injection remains as fallback.
- If future work removes visible credential injection entirely, auto-title prompt delivery needs its own bootstrap injection path because it is currently appended to the credential block in `create_session_inner`.
- If a future feature adds session token rotation without respawn, env-first credentials alone will not solve it. The process must respawn or receive an explicit visible/tooling update.

## Architect Round-1 Resolution (2026-05-15)

The blocking review findings are folded into the implementation plan:

- Non-PTY child process leakage is addressed by shared std/tokio scrub helpers in `src-tauri/src/pty/credentials.rs` plus explicit call-site edits for `where.exe`, `git`, `cmd`, `node`, and `powershell.exe`.
- Partial token disclosure is addressed by changing `validate_cli_token` to a non-echoing invalid-token error and adding a test that the supplied token text is absent.
- Security-critical env scrub behavior is covered by helper-level unit tests for empty PTY env, non-empty PTY env, and std/tokio scrub helpers.
- Remaining visible-block-only guidance is no longer deferred; the plan updates CLI argument docs, top-level CLI help, session context examples, and the auto-title prompt/tests.

No unresolved architecture questions remain for this issue. The next step is implementation by `dev-rust` against the amended plan.

## Dev-Rust Review (2026-05-15)

Status: plan is feasible as written. No blocking issue found.

Verified against the current codebase:

- `src-tauri/src/session/manager.rs:36-75` still creates `Session.token` with `Uuid::new_v4()` during `SessionManager::create_session`.
- `src-tauri/src/commands/session.rs:647-657` resolves the final local `agent_id` before session creation, so gating env credentials on that resolved value is the correct implementation point.
- `src-tauri/src/commands/session.rs:821-825` is the only direct call site for `PtyManager::spawn`, so the spawn signature change has one direct caller to update.
- All known session creation paths still converge through `create_session_inner`: `lib.rs:635`, `phone/mailbox.rs:681`, `phone/mailbox.rs:1834`, `web/commands.rs:84`, and the command wrappers in `commands/session.rs`.
- `portable-pty 0.8.1` `CommandBuilder::new` initializes from the parent environment, and `CommandBuilder::env_remove` / `env` exist with `AsRef<OsStr>` arguments. The planned `&str` keys and `&String` values are compatible with existing usage in `pty/manager.rs`.
- `src-tauri/src/phone/mailbox.rs:831-846` currently returns on `/clear` credential re-inject failure, so the planned change is required for env-first behavior.

Implementation notes to apply with the plan:

- Update the module-level comments and function docs in `src-tauri/src/pty/credentials.rs`; they currently describe this module as only a visible credential block builder. After this issue it also owns env credential derivation.
- Keep `env_remove` unconditional for every PTY child, including `extra_env.is_empty()`. That unconditional removal is the actual non-agent leakage guard.
- Apply credential `env_remove` / `env` on the final `CommandBuilder` after the Windows wrapper branch has produced `command`, and before `spawn_command`. Placing it near the existing `TERM` / git env setup is fine because the keys do not overlap; keep the count-only log and do not log values.
- The `extra_env` vector can stay local to `create_session_inner`; `PtyManager::spawn` consumes it synchronously into `CommandBuilder`, so no lifetime or async ownership issue is introduced.
- Existing test-only `std::env::set_var` calls in `commands/session.rs` are outside the runtime prohibition. After implementation, a quick `rg "std::env::set_var" src-tauri/src` should show no newly added runtime use.
- `src-tauri/src/cli/mod.rs:16-18` still has `after_help` text saying the token is injected only as a visible credentials block. This is not a compile blocker, but it will make `--help` contradict env-first guidance unless updated in this issue or explicitly left as a known follow-up. Token arg help strings in individual CLI subcommands have the same residual documentation risk.

Validation additions:

- Run `cd src-tauri; cargo fmt --check` after edits.
- Run `cd src-tauri; cargo check`.
- Run `cd src-tauri; cargo clippy --all-targets --all-features`.
- Keep the targeted tests from the architect plan, but also run either full `cargo test` or at minimum the full relevant modules if full test time is not acceptable.
- Add a post-change search for credential leakage patterns: `rg "AGENTSCOMMANDER_TOKEN|extra_env|build_credentials_env|std::env::set_var" src-tauri/src`, then inspect matches to confirm token values are not logged and parent env mutation was not introduced.

## Dev-Rust-Grinch Review (2026-05-15)

Status: FAIL until the blocking leakage/disclosure gaps below are folded into the plan.

1. **What** — inherited `AGENTSCOMMANDER_*` credentials are scrubbed only from PTY `CommandBuilder` children, not from other non-agent child processes.
   **Why** — the plan correctly handles the case where the AgentsCommander GUI is launched from an agent-owned environment, but that inherited parent environment is also used by every `std::process::Command` / `tokio::process::Command` in the app unless explicitly scrubbed. Current non-PTY launches include `src-tauri/src/pty/manager.rs:46` (`where.exe`), `src-tauri/src/config/session_context.rs:245` (`git`), `src-tauri/src/commands/ac_discovery.rs:253` and `:804` (`git`), `src-tauri/src/commands/entity_creation.rs:1435`, `:1454`, `:1779`, and `:1812` (`git`), `src-tauri/src/config/claude_settings.rs:994` and `:1185` (`cmd`), `src-tauri/src/config/claude_settings.rs:1232` and `:1246` (`node`), `src-tauri/src/commands/wg_delete_diagnostic.rs:1623` (`powershell.exe`), and `src-tauri/src/pty/git_watcher.rs:173` (`git`). If the parent process inherited an agent token, those non-agent tools inherit it too. The planned PTY-only `env_remove` prevents plain terminal sessions from receiving the token, but it does not prevent leakage to these other subprocesses, wrappers, hooks, or diagnostics.
   **Fix** — add a plan step for scrubbing `CREDENTIAL_ENV_KEYS` from all non-agent `std::process::Command` and `tokio::process::Command` spawns, or introduce small shared helpers for both command types and require the listed call sites to use them. Keep the agent PTY path as the only path that re-applies `build_credentials_env`. Extend validation with a post-change `rg -n "Command::new|std::process::Command::new|tokio::process::Command::new|env_remove" src-tauri/src` review so new unsanitized process launches are visible.

2. **What** — the planned CLI error update leaves partial token disclosure in `validate_cli_token`.
   **Why** — `src-tauri/src/cli/mod.rs:132` currently formats `Error: invalid token '{}...'` using the first eight characters of the supplied token. With env-first credentials, agents will pass `AGENTSCOMMANDER_TOKEN` values through CLI arguments; if a token is malformed, stale, copied with hidden characters, or a root token is supplied to the wrong instance, this path emits part of the secret into terminal output. That output can be captured by response markers, logs, screenshots, or pasted back into an agent transcript. The issue explicitly calls out token disclosure, and the suggested replacement only changes the suffix.
   **Fix** — make the plan replace the invalid-token message with a non-echoing form such as `Error: invalid token supplied. Expected a valid session token (UUID) or root token...`; do not include any substring of the provided token. Add or adjust the CLI validation test to assert the rejected token text is absent from the error.

3. **What** — the security-critical `env_remove` behavior has only manual coverage.
   **Why** — the central regression for this issue is "plain/non-agent children must not inherit a stale token from the parent". Manual UI validation is useful, but it will not catch a later refactor that moves the credential scrub behind `if !extra_env.is_empty()` or applies it before reconstructing the Windows wrapper `CommandBuilder`. `portable_pty::CommandBuilder` exposes `get_env`, so this can be tested without spawning a PTY.
   **Fix** — add a testable helper in `pty::manager` or `pty::credentials`, for example `apply_credential_env(&mut CommandBuilder, extra_env)`, and unit-test both cases: empty `extra_env` removes pre-seeded `CREDENTIAL_ENV_KEYS`, and non-empty `extra_env` overrides stale pre-seeded values with the per-session values. Keep the manual Windows wrapper validation, but do not rely on it as the only guard.

4. **What** — several user-facing instructions still teach visible-block-first credentials.
   **Why** — the plan updates `session_context.rs` and two `cli/mod.rs` errors, but `rg` still finds visible-block-only guidance in `src-tauri/src/cli/send.rs:22`, `src-tauri/src/cli/list_peers.rs:21`, `src-tauri/src/cli/close_session.rs:20`, `src-tauri/src/cli/brief_set_title.rs:28`, `src-tauri/src/cli/brief_append_body.rs:29`, and `src-tauri/src/pty/title_prompt.rs:30`. The inter-agent examples in `session_context.rs` also still use `<YOUR_TOKEN>` / `<YOUR_BINARY_PATH>` placeholders without saying those should resolve from `AGENTSCOMMANDER_TOKEN`, `AGENTSCOMMANDER_ROOT`, and `AGENTSCOMMANDER_BINARY_PATH`. After `/clear`, the env vars may be the only reliable primary source, so stale visible-block wording will keep agents using the fallback path.
   **Fix** — either update all visible-block-only help/doc strings in this issue, or mark the remaining strings as an explicit follow-up with a reason. Prefer changing the command argument help to `from AGENTSCOMMANDER_TOKEN or the visible fallback block` and updating the title prompt to mention env-first credentials.
