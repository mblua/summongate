# Issue 212 follow-up: env-only credentials, no visible fallback

## Requirement

The current `feature/212-env-first-credentials` branch starts agent PTY children with `AGENTSCOMMANDER_*` credentials in the child environment, but it still keeps visible PTY credential delivery as a compatibility fallback.

The new requirement is env-only credentials:

- Agent credentials are delivered only through per-child environment variables:
  - `AGENTSCOMMANDER_TOKEN`
  - `AGENTSCOMMANDER_ROOT`
  - `AGENTSCOMMANDER_BINARY`
  - `AGENTSCOMMANDER_BINARY_PATH`
  - `AGENTSCOMMANDER_LOCAL_DIR`
- Do not inject an initial visible `# === Session Credentials ===` block into the PTY.
- `/clear` must not depend on, trigger, or wait for credential re-injection.
- Token refresh/rotation for an already-running child process is explicitly unsupported without respawn. A parent process cannot portably mutate an existing child process environment on Windows, Linux, or macOS.
- Keep non-credential PTY bootstrap behavior that is still needed, currently the auto-title prompt, but split it from credential delivery.
- Remove associated user-facing text and test names that describe visible injected credentials as credential delivery or fallback.
- Preserve existing security behavior:
  - Agent PTY children receive only their own per-session `AGENTSCOMMANDER_*` env values.
  - Non-agent PTY children and helper child processes scrub inherited `AGENTSCOMMANDER_*` values.
  - Logs never print token values.
  - `validate_cli_token` remains non-disclosing.

No frontend IPC changes are required.

## Design Decisions

1. **Credential delivery is env-only.** Keep `build_credentials_env(...)` and `apply_credential_env_to_pty_command(...)`; remove `build_credentials_block(...)` and all PTY writes that used it.
2. **Live token refresh is unsupported.** Delete the visible "fresh token" PTY notice path. If a sender uses a stale or malformed token, reject the message with a non-disclosing error that says env-only credentials cannot be refreshed into a live process and the sender must restart/respawn.
3. **`/clear` is independent of credentials.** After `/clear`, only inject the optional follow-up body after idle. Do not run a credential phase first.
4. **Auto-title remains as a non-credential bootstrap.** `commands/session.rs` should inject the auto-title prompt only when the existing auto-title gates pass. The prompt must tell the agent to use `AGENTSCOMMANDER_*` env vars only.
5. **Cross-platform contract.** Use `portable_pty::CommandBuilder::env` / `env_remove` for PTY children and `std::process::Command::env_remove` or `tokio::process::Command::as_std_mut().env_remove` for helpers. Do not add runtime `std::env::set_var`, Windows-global env mutation, shell profile mutation, registry writes, or parent-process env mutation.

## Affected Files

### 1. `src-tauri/src/pty/credentials.rs`

Purpose: keep env construction and scrubbing helpers, remove visible credential block construction.

#### 1.1 Replace module docs at lines 1-7

Replace:

```rust
//! Agent credential helpers.
//!
//! Produces the visible `# === Session Credentials ===` fallback block, the
//! env var payload used for agent PTY children, and shared scrubbing helpers
//! for child processes that must not inherit parent `AGENTSCOMMANDER_*` values.
//! The visible block output must stay byte-for-byte identical across spawn and
//! `/clear` call sites so agents parse consistently.
```

with:

```rust
//! Agent credential environment helpers.
//!
//! Builds the per-session `AGENTSCOMMANDER_*` environment payload for agent PTY
//! children and provides shared scrubbing helpers for child processes that must
//! not inherit parent `AGENTSCOMMANDER_*` values.
//!
//! Credentials are never formatted as visible PTY text.
```

#### 1.2 Add a cross-platform fallback binary-path helper before line 34

Insert immediately before `pub fn build_credential_values(...)`:

```rust
fn fallback_binary_path() -> &'static str {
    if cfg!(windows) {
        "agentscommander.exe"
    } else {
        "agentscommander"
    }
}
```

Then replace line 52:

```rust
.unwrap_or_else(|| "agentscommander.exe".to_string());
```

with:

```rust
.unwrap_or_else(|| fallback_binary_path().to_string());
```

Reason: the rare `current_exe()` fallback should not hardcode a Windows `.exe` name on Linux/macOS.

#### 1.3 Update the current_exe warning at lines 37-40

Replace:

```rust
log::warn!(
    "[credentials] current_exe() unavailable; credentials will use fallback \
     binary name. Agent may be unable to invoke the CLI."
);
```

with:

```rust
log::warn!(
    "[credentials] current_exe() unavailable; credential env will use fallback \
     binary path/name. Agent may be unable to invoke the CLI."
);
```

No token value is logged.

#### 1.4 Delete `build_credentials_block` at lines 116-153

Delete the full doc comment and function:

```rust
/// Build the credentials block for a session.
...
pub fn build_credentials_block(token: &Uuid, cwd: &str) -> String {
    ...
}
```

Do not leave any helper that formats `Token`, `Root`, `BinaryPath`, or `LocalDir` as visible PTY text.

#### 1.5 Delete the visible-block unit test at lines 159-189

Delete `fn block_structure_is_byte_stable()`.

Keep and update the env/scrub tests:

- `env_contains_expected_keys_and_values`
- `pty_apply_helper_removes_stale_credentials_when_extra_env_empty`
- `pty_apply_helper_overrides_stale_credentials_when_extra_env_present`
- `std_and_tokio_scrub_helpers_remove_explicit_credentials`

Add this direct fallback regression test inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn fallback_binary_path_is_platform_specific() {
    let p = super::fallback_binary_path();
    if cfg!(windows) {
        assert_eq!(p, "agentscommander.exe");
    } else {
        assert_eq!(p, "agentscommander");
        assert!(!p.ends_with(".exe"));
    }
}
```

Reason: the earlier idea of asserting on `build_credentials_env(...)` was too weak because `current_exe()` normally succeeds during tests. The direct private-helper test proves the fallback string itself is platform-specific without making the helper public.

### 2. `src-tauri/src/commands/session.rs`

Purpose: keep per-child env application before spawn; replace credential bootstrap injection with optional title-prompt-only injection.

#### 2.1 Update `build_title_prompt_appendage` comment at lines 562-572

Replace the current wording:

```rust
/// Issue #107 round 5 -- build the title-prompt segment to concat with the
/// cred-block, OR `Ok(None)` if the auto-title preconditions don't hold.
...
/// The caller is the post-spawn task in `create_session_inner`; it
/// concatenates the returned `Some(prompt)` with the cred-block and issues a
/// single `inject_text_into_session` call (Round 4 R4.2.3 -- preserved in
/// Round 5).
```

with:

```rust
/// Issue #107 round 5 -- build the optional title prompt, or `Ok(None)` if the
/// auto-title preconditions do not hold.
///
/// Synchronous: filesystem reads only, no PTY, no await, no snapshot.
/// (#137 introduced `brief-set-title` which creates its own atomic backup;
/// the backend no longer snapshots before injection.)
///
/// The caller is the post-spawn task in `create_session_inner`; it injects this
/// prompt by itself. Credentials are not part of this payload.
```

Preserve the existing gate list at lines 574-582.

#### 2.2 Preserve env construction at lines 821-825

Keep:

```rust
let extra_env = if agent_id.is_some() {
    crate::pty::credentials::build_credentials_env(&session.token, &cwd)
} else {
    Vec::new()
};
```

Do not move this after `pty_mgr.spawn`; the child env must be present before `spawn_command`.

#### 2.3 Replace the post-spawn bootstrap block at lines 842-950

Replace the entire block beginning at:

```rust
// Auto-inject bootstrap text for agent sessions after PTY spawn.
```

through the closing `});` at line 950 with:

```rust
    // Auto-inject optional non-credential bootstrap text for agent sessions
    // after PTY spawn. Credentials are already present in child environment
    // variables; no credentials are written through PTY.
    //
    // Currently the only bootstrap payload is the Coordinator auto-title prompt.
    if agent_id.is_some() && is_coordinator {
        let auto_title_enabled = {
            let settings_state = app.state::<SettingsState>();
            let cfg = settings_state.read().await;
            cfg.auto_generate_brief_title
        };

        if auto_title_enabled {
            let app_clone = app.clone();
            let session_id = id;
            let cwd_clone = cwd.clone();

            tokio::spawn(async move {
                let prompt = match build_title_prompt_appendage(&cwd_clone) {
                    Ok(Some(prompt)) => {
                        log::info!(
                            "[session] Auto-title prompt built for session {}",
                            session_id
                        );
                        prompt
                    }
                    Ok(None) => {
                        log::info!(
                            "[session] Auto-title prompt skipped (gate not passed) for session {}",
                            session_id
                        );
                        return;
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Auto-title prompt skipped for session {}: {}",
                            session_id,
                            e
                        );
                        return;
                    }
                };

                let max_wait = std::time::Duration::from_secs(30);
                let poll = std::time::Duration::from_millis(500);
                let start = std::time::Instant::now();

                loop {
                    if start.elapsed() >= max_wait {
                        log::warn!(
                            "[session] Timeout waiting for idle before auto-title prompt injection for session {}",
                            session_id
                        );
                        break;
                    }
                    tokio::time::sleep(poll).await;

                    let session_mgr =
                        app_clone.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
                    let mgr = session_mgr.read().await;
                    let sessions = mgr.list_sessions().await;
                    match sessions.iter().find(|s| s.id == session_id.to_string()) {
                        Some(s) if s.waiting_for_input => break,
                        Some(_) => {}
                        None => {
                            log::warn!(
                                "[session] Session {} gone before auto-title prompt injection",
                                session_id
                            );
                            return;
                        }
                    }
                }

                match crate::pty::inject::inject_text_into_session(
                    &app_clone,
                    session_id,
                    &prompt,
                )
                .await
                {
                    Ok(()) => {
                        log::info!(
                            "[session] Auto-title prompt injected for session {}",
                            session_id
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Failed to inject auto-title prompt for {}: {}",
                            session_id,
                            e
                        );
                    }
                }
            });
        }
    }
```

Important details:

- Remove the `let token = session.token;` capture.
- Remove all calls to `crate::pty::credentials::build_credentials_block`.
- Remove `combined`, `cred_block`, and `auto_title_was_appended`.
- The idle wait remains only for the title prompt.
- If auto-title is disabled, not a coordinator, or `build_title_prompt_appendage` returns `Ok(None)`, no PTY bootstrap is injected.

#### 2.4 Extend the existing title appendage test at lines 2192-2214

In `build_title_prompt_appendage_returns_some_when_brief_has_no_title`, after line 2205:

```rust
let legacy_header = ["# === Session", "Credentials ==="].join(" ");
assert!(prompt.contains("<AGENTSCOMMANDER_TOKEN>"));
assert!(!prompt.contains(&legacy_header));
assert!(!prompt.to_ascii_lowercase().contains("fallback"));
assert!(!prompt.to_ascii_lowercase().contains("visible"));
```

This catches regressions where title prompt text reintroduces credential fallback wording.

#### 2.5 Update root-agent creation comment at lines 1589-1594

Replace:

```rust
/// Create or reuse a root agent session.
/// Derives the root agent path from the current binary name:
///   {exe_dir}/.{binary_name}/ac-root-agent
/// If a session already exists at that path, switches to it instead.
/// Uses the first configured coding agent from settings.
/// Injects session credentials immediately after creation.
```

with:

```rust
/// Create or reuse a root agent session.
/// Derives the root agent path from the current binary name:
///   {exe_dir}/.{binary_name}/ac-root-agent
/// If a session already exists at that path, switches to it instead.
/// Uses the first configured coding agent from settings.
/// Starts the root agent with per-child credential env when a configured coding agent is launched.
```

Reason: root-agent creation no longer injects credentials through PTY text after session creation.

### 2A. `src-tauri/src/session/session.rs`

Purpose: session model comments must describe env-only credential delivery.

#### 2A.1 Update token field comment at line 87

Replace:

```rust
/// Unique token for CLI authentication. Passed to agents via init prompt.
```

with:

```rust
/// Unique token for CLI authentication. Agent PTY children receive it via per-child `AGENTSCOMMANDER_TOKEN` env at spawn.
```

Reason: the token is no longer passed via init prompt or conversation text.

### 3. `src-tauri/src/phone/mailbox.rs`

Purpose: remove visible credential re-injection after `/clear` and remove visible fresh-token injection.

#### 3.1 Replace stale-token recovery at lines 415-442

In the `match mgr.find_by_token(token_uuid).await` `None =>` branch, replace lines 415-442, from the `// Token is stale/invalid.` comment through the closing brace of the `else { return self.reject_message(...).await; }` branch:

```rust
// Token is stale/invalid. Try to find the sender's active session
// by CWD match -- if found, the sender is legit (verified by outbox
// anti-spoofing above), so refresh their token and continue.
drop(mgr);
if let Some(session_id) = self.find_active_session(app, &msg.from).await
{
    log::info!(
        "[mailbox] Stale token from '{}' -- found active session {}, refreshing token",
        msg.from, session_id
    );
    ...
    // Continue processing -- sender verified by CWD match
} else {
    return self
        .reject_message(
            path,
            &msg,
            "Invalid session token and no active session to refresh",
        )
        .await;
}
```

with:

```rust
// Token is stale/invalid. Env-only credentials cannot be refreshed into
// an already-running child process, so reject instead of injecting a new token.
drop(mgr);
if let Some(session_id) = self.find_active_session(app, &msg.from).await {
    log::warn!(
        "[mailbox] Stale token from '{}' matches active session {}, but env-only credentials cannot be refreshed in-place",
        msg.from,
        session_id
    );
}
return self
    .reject_message(
        path,
        &msg,
        "Invalid session token. Env-only credentials cannot be refreshed into a live process; restart or respawn the sender session.",
    )
    .await;
```

This log does not print the token value.

#### 3.2 Replace malformed-token recovery at lines 469-491

Replace lines 469-491, from the `// Token is not a valid UUID` comment through the closing brace of the `else { return self.reject_message(...).await; }` branch:

```rust
// Token is not a valid UUID (e.g. "none"). Treat like a stale token:
// try to find the sender's active session by CWD and refresh.
drop(mgr);
if let Some(session_id) = self.find_active_session(app, &msg.from).await {
    log::info!(
        "[mailbox] Malformed token from '{}' -- found active session {}, refreshing token",
        msg.from, session_id
    );
    // Detach: see comment on the stale-token branch above.
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        Self::inject_fresh_token_static(&app_clone, session_id).await;
    });
} else {
    return self
        .reject_message(
            path,
            &msg,
            "Malformed token and no active session to refresh",
        )
        .await;
}
```

with:

```rust
// Token is not a valid UUID. Env-only credentials cannot be refreshed
// into an already-running child process, so reject instead of injecting.
drop(mgr);
if let Some(session_id) = self.find_active_session(app, &msg.from).await {
    log::warn!(
        "[mailbox] Malformed token from '{}' matches active session {}, but env-only credentials cannot be refreshed in-place",
        msg.from,
        session_id
    );
}
return self
    .reject_message(
        path,
        &msg,
        "Malformed session token. Env-only credentials cannot be refreshed into a live process; restart or respawn the sender session.",
    )
    .await;
```

#### 3.3 Replace `/clear` post-command comments and logic at lines 820-858

Replace the comment and spawned task section at lines 820-858, ending at the closing `});` of the spawned task:

```rust
// Post-command background work:
//  - For `/clear` on an agent session: best-effort visible credential
//    re-inject for compatibility. Env credentials remain available
//    to the still-live child process, so fallback failure must not
//    block the body follow-up.
//  - For `/compact` (or `/clear` on a plain shell): body follow-up only.
// Never block the delivery pipeline -- spawn as a detached task.
let is_clear = command == "clear";
let app_clone = app.clone();
let msg_clone = msg.clone();
let command_owned = command.clone();
tauri::async_runtime::spawn(async move {
    if is_clear {
        if let Err(e) =
            Self::reinject_credentials_after_clear_static(&app_clone, session_id).await
        {
            ...
        }
    }
    if !msg_clone.body.is_empty() {
        ...
    }
});
```

with:

```rust
// Post-command background work:
//  - `/clear` and `/compact` both keep the still-live child process environment.
//  - Credentials are env-only, so there is no PTY credential re-injection phase.
//  - If the message has a follow-up body, inject it after the agent becomes idle.
// Never block the delivery pipeline -- spawn as a detached task.
let app_clone = app.clone();
let msg_clone = msg.clone();
let command_owned = command.clone();
tauri::async_runtime::spawn(async move {
    if !msg_clone.body.is_empty() {
        if let Err(e) =
            Self::inject_followup_after_idle_static(&app_clone, session_id, &msg_clone)
                .await
        {
            log::warn!(
                "[mailbox] Failed to inject follow-up after /{} for session {}: {}",
                command_owned,
                session_id,
                e
            );
        }
    }
});
```

The replacement includes the follow-up injection branch, so delete the old branch at lines 845-857 as part of this replacement.

#### 3.4 Update the security comment at lines 896-899

Replace:

```rust
// SECURITY: this `first_100` log MUST NOT see credential blocks.
// Cred re-inject (reinject_credentials_after_clear_static) and spawn-path
// cred inject call inject_text_into_session directly, bypassing this
// branch. Do not refactor without re-verifying.
```

with:

```rust
// SECURITY: this `first_100` log MUST NOT see credential values.
// Credentials are env-only and must never be routed through PTY payloads.
// Keep this log limited to standard message payloads.
```

#### 3.5 Delete `reinject_credentials_after_clear_static` at lines 981-1049

Delete the entire function:

```rust
async fn reinject_credentials_after_clear_static(...)
```

After deletion, no `/clear` code path should call `build_credentials_block` or any credential injection helper.

#### 3.6 Delete `inject_fresh_token_static` at lines 1864-1946

Delete the entire function:

```rust
async fn inject_fresh_token_static(...)
```

Also delete the comments at lines 425-428 that mention detached token injection.

After deletion, `rg -n "inject_fresh_token_static|TOKEN REFRESHED|Fresh token injected" src-tauri/src/phone/mailbox.rs` must return no matches.

### 4. `src-tauri/src/pty/title_prompt.rs`

Purpose: keep auto-title prompt injection, remove credential fallback wording.

#### 4.1 Replace docs at lines 11-15

Replace:

```rust
//! No I/O. Pure string format. The agent substitutes
//! `<AGENTSCOMMANDER_TOKEN>`, `<AGENTSCOMMANDER_ROOT>`, and
//! `<AGENTSCOMMANDER_BINARY_PATH>` from environment variables first, falling
//! back to the visible `# === Session Credentials ===` block only if env vars
//! are unavailable (Round 4 R4.2 still provides the compatibility paste).
```

with:

```rust
//! No I/O. Pure string format. The agent substitutes
//! `<AGENTSCOMMANDER_TOKEN>`, `<AGENTSCOMMANDER_ROOT>`, and
//! `<AGENTSCOMMANDER_BINARY_PATH>` from environment variables only.
```

#### 4.2 Replace prompt text at lines 30-33

Replace:

```rust
"`<AGENTSCOMMANDER_BINARY_PATH>`, `<AGENTSCOMMANDER_TOKEN>`, and ",
"`<AGENTSCOMMANDER_ROOT>` mean the environment variables of the same names. ",
"If env vars are unavailable, use `BinaryPath`, `Token`, and `Root` from ",
"the latest visible `# === Session Credentials ===` fallback block. ",
```

with:

```rust
"`<AGENTSCOMMANDER_BINARY_PATH>`, `<AGENTSCOMMANDER_TOKEN>`, and ",
"`<AGENTSCOMMANDER_ROOT>` mean the environment variables of the same names. ",
"If any of these env vars are unavailable, run nothing; the session was not ",
"started with valid AgentsCommander credential env. ",
```

#### 4.3 Rename and invert the fallback test at lines 66-72

Replace:

```rust
#[test]
fn prompt_documents_env_first_credentials_with_visible_fallback() {
    let p = build_title_prompt("/tmp/BRIEF.md");
    assert!(p.contains("environment variables"));
    assert!(p.contains("visible"));
    assert!(p.contains("`# === Session Credentials ===`"));
}
```

with:

```rust
#[test]
fn prompt_documents_env_only_credentials() {
    let p = build_title_prompt("/tmp/BRIEF.md");
    let legacy_header = ["# === Session", "Credentials ==="].join(" ");
    assert!(p.contains("environment variables"));
    assert!(p.contains("<AGENTSCOMMANDER_TOKEN>"));
    assert!(!p.contains(&legacy_header));
    assert!(!p.to_ascii_lowercase().contains("fallback"));
    assert!(!p.to_ascii_lowercase().contains("visible"));
}
```

### 5. `src-tauri/src/config/session_context.rs`

Purpose: generated initial agent instructions must describe env-only credentials.

#### 5.1 Replace the CLI executable rule at line 588

Replace:

```markdown
**RULE:** Never hardcode or guess the binary path. Prefer the environment variables above. If they are unavailable, fall back to the latest `# === Session Credentials ===` block in your conversation.
```

with:

```markdown
**RULE:** Never hardcode or guess the binary path. Use the environment variables above. If they are unavailable in an agent session, restart or respawn the session.
```

#### 5.2 Replace the session credentials section at lines 604-608

Replace:

```markdown
## Session credentials

Your session credentials are delivered through the `AGENTSCOMMANDER_*` environment variables listed above. A visible `# === Session Credentials ===` block may also appear in your conversation as a compatibility fallback.

Use environment variables first. If AgentsCommander later injects a token refresh notice, that visible refresh is authoritative until the session respawns, because a live process environment cannot be mutated.
```

with:

```markdown
## Session credentials

Your session credentials are delivered only through the `AGENTSCOMMANDER_*` environment variables listed above.

Live token refresh without respawn is not supported, because a parent process cannot portably mutate an already-running child process environment. If credential validation fails, restart or respawn the session so AgentsCommander can create a new child process with fresh env values.
```

Keep the rest of the inter-agent messaging section unchanged.

#### 5.3 Add generated-context regression test before the tests module closing brace at line 772

In `#[cfg(test)] mod tests`, insert this test after `default_context_replica_with_matrix_and_messaging_renders_both_sections` and before the final module `}`:

```rust
#[test]
fn default_context_documents_env_only_credentials() {
    let out = default_context("C:/fake/wg-7-dev-team/__agent_architect", None);
    let legacy_header = ["# === Session", "Credentials ==="].join(" ");
    let legacy_compat = ["compatibility", "fallback"].join(" ");
    let legacy_refresh_notice = ["token refresh", "notice"].join(" ");
    let legacy_visible_refresh = ["visible", "refresh"].join(" ");

    assert!(out.contains("AGENTSCOMMANDER_TOKEN"));
    assert!(out.contains("delivered only through"));
    assert!(out.contains("restart or respawn"));
    assert!(!out.contains(&legacy_header));
    let lower = out.to_ascii_lowercase();
    assert!(!lower.contains(&legacy_compat));
    assert!(!lower.contains(&legacy_refresh_notice));
    assert!(!lower.contains(&legacy_visible_refresh));
}
```

Reason: the generated default agent context is the bootstrap credential contract agents read first. Guard it directly instead of relying only on source greps.

### 6. `src-tauri/src/cli/mod.rs`

Purpose: CLI help and token validation errors must stop describing visible fallback credentials.

#### 6.1 Replace top-level CLI `after_help` at lines 16-19

Replace:

```rust
TOKEN: In agent sessions, pass AGENTSCOMMANDER_TOKEN from the environment. \
If the env var is unavailable, use the latest visible '# === Session Credentials ===' fallback block. \
If a token expires, any failed `send` triggers an automatic token refresh.\n\n\
```

with:

```rust
TOKEN: In agent sessions, pass AGENTSCOMMANDER_TOKEN from the environment. \
Credentials are not delivered through visible PTY text. \
If token validation fails, restart or respawn the sender session; live token refresh is not supported.\n\n\
```

#### 6.2 Replace missing-token error at lines 105-109

Replace:

```rust
"Error: --token is required. In agent sessions, pass AGENTSCOMMANDER_TOKEN \
 from the environment, or use the latest '# === Session Credentials ===' \
 fallback block if the env var is unavailable."
```

with:

```rust
"Error: --token is required. In agent sessions, pass AGENTSCOMMANDER_TOKEN \
 from the environment. If it is unavailable, restart or respawn the session."
```

#### 6.3 Replace invalid-token error at lines 131-136

Replace:

```rust
"Error: invalid token supplied. Expected a valid session token (UUID) or root token. \
 In agent sessions, use AGENTSCOMMANDER_TOKEN from the environment, or the latest \
 visible credentials fallback block if the env var is unavailable."
```

with:

```rust
"Error: invalid token supplied. Expected a valid session token (UUID) or root token. \
 In agent sessions, use AGENTSCOMMANDER_TOKEN from the environment. If validation \
 keeps failing, restart or respawn the session."
```

#### 6.4 Extend token validation tests at lines 181-189

In `validate_cli_token_does_not_echo_invalid_input`, add:

```rust
let legacy_phrase = ["Session", "Credentials"].join(" ");
assert!(!err.contains(&legacy_phrase));
assert!(!err.to_ascii_lowercase().contains("fallback"));
assert!(!err.to_ascii_lowercase().contains("visible"));
```

Add a second test after it:

```rust
#[test]
fn validate_cli_token_missing_token_documents_env_only() {
    let err = validate_cli_token(&None).unwrap_err();

    assert!(err.contains("AGENTSCOMMANDER_TOKEN"));
    assert!(err.contains("restart or respawn"));
    let legacy_phrase = ["Session", "Credentials"].join(" ");
    assert!(!err.contains(&legacy_phrase));
    assert!(!err.to_ascii_lowercase().contains("fallback"));
    assert!(!err.to_ascii_lowercase().contains("visible"));
}
```

### 7. CLI subcommand token help strings

Purpose: generated `--help` output must not mention visible credential fallback.

Replace each `/// Session token for authentication (from AGENTSCOMMANDER_TOKEN or visible credentials fallback)` comment with:

```rust
/// Session token for authentication (from AGENTSCOMMANDER_TOKEN)
```

Affected exact locations:

- `src-tauri/src/cli/send.rs:22`
- `src-tauri/src/cli/list_peers.rs:21`
- `src-tauri/src/cli/close_session.rs:20`
- `src-tauri/src/cli/brief_set_title.rs:28`
- `src-tauri/src/cli/brief_append_body.rs:29`

Do not change the semantics of the `#[arg(long)] pub token: Option<String>` fields.

### 8. Active docs and README

Current active docs inspected:

- `README.md:225` describes remote commands and file-message notification PTY injection. This is non-credential PTY injection and should remain.
- `README.md:223` describes file-based messaging and PTY notification. This is non-credential PTY injection and should remain.
- `FIXES_CODEX.md` is active root-level Markdown and currently contains obsolete ACRC credential-paste guidance and visible-block snippets. Rewrite it in place; do not leave the old snippets in active docs.

#### 8.1 Rewrite `FIXES_CODEX.md`

Replace the entire contents of `FIXES_CODEX.md` with:

```markdown
# FIXES_CODEX.md

Historical note: this document described an April 2026 ACRC/PTY paste investigation that is obsolete after issue #212.

Current contract:

- Agent credentials are delivered only through `AGENTSCOMMANDER_*` environment variables set on the PTY child before spawn.
- Live token refresh for a running child process is unsupported; restart or respawn the session.
- PTY injection remains for normal message delivery and non-credential prompts only.

Do not use this file as implementation guidance for credential transport.
```

Reason: this root doc is discoverable active documentation. It must no longer preserve operational snippets that format credentials into PTY-visible text.

Do not edit old `_plans/` or `_logbooks/` entries for historical text. The validation search below intentionally scopes to active code/docs.

## Dependencies

No new crates, npm packages, IPC types, frontend stores, or Tauri commands.

The implementation continues to depend on existing APIs:

- `portable_pty::CommandBuilder::env`
- `portable_pty::CommandBuilder::env_remove`
- `std::process::Command::env_remove`
- `tokio::process::Command::as_std_mut`
- existing `crate::pty::inject::inject_text_into_session` for non-credential title prompt and message delivery only

## Validation Plan

Run formatting and targeted tests:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml pty::credentials
cargo test --manifest-path src-tauri/Cargo.toml fallback_binary_path_is_platform_specific
cargo test --manifest-path src-tauri/Cargo.toml pty::title_prompt
cargo test --manifest-path src-tauri/Cargo.toml validate_cli_token
cargo test --manifest-path src-tauri/Cargo.toml build_title_prompt_appendage
cargo test --manifest-path src-tauri/Cargo.toml default_context_documents_env_only_credentials
cargo test --manifest-path src-tauri/Cargo.toml
```

Run the active-source text audit. This must return no matches:

```powershell
rg -n "Session Credentials|visible credentials fallback|visible .*credential|fallback block|credential re-inject|fresh token injected|Fresh token injected|TOKEN REFRESHED|build_credentials_block|reinject_credentials_after_clear|inject_fresh_token|Passed to agents via init prompt|Injects session credentials immediately|credentials? .*init prompt|init prompt.*credentials?|conversation text|compatibility paste|token refresh notice|visible refresh" src-tauri/src src-tauri/tests README.md docs FIXES_CODEX.md -g "!target/**" -S
```

Run the active Markdown audit. This must return no matches outside historical `_plans/` and `_logbooks/`:

```powershell
rg -n "Session Credentials|visible credentials fallback|fallback block|TOKEN REFRESHED|credential re-inject|visible .*credential" . -g "*.md" -g "!_plans/**" -g "!_logbooks/**" -g "!target/**" -S
```

Run the env-delivery audit:

```powershell
rg -n "AGENTSCOMMANDER_TOKEN|AGENTSCOMMANDER_ROOT|AGENTSCOMMANDER_BINARY|AGENTSCOMMANDER_BINARY_PATH|AGENTSCOMMANDER_LOCAL_DIR" src-tauri/src -g "!target/**" -S
```

Expected remaining matches:

- `pty/credentials.rs` constants, env builder, env tests, scrub tests.
- `commands/session.rs` `build_credentials_env(&session.token, &cwd)` before `pty_mgr.spawn`.
- `pty/title_prompt.rs` placeholder prompt text.
- `config/session_context.rs` generated env-only instructions.
- CLI examples/help that tell agents to pass `AGENTSCOMMANDER_TOKEN`.

Run the runtime env mutation audit:

```powershell
rg -n "std::env::set_var\\(|set_var\\([^\\n]*AGENTSCOMMANDER|AGENTSCOMMANDER[^\\n]*set_var" src-tauri/src -g "!target/**" -S
```

Expected:

- No runtime `AGENTSCOMMANDER_*` `set_var` matches.
- Existing unrelated test-only `std::env::set_var` calls in `commands/session.rs` may remain if they do not set `AGENTSCOMMANDER_*`.

Run the helper child process scrub audit:

```powershell
rg -n "Command::new|process::Command::new" src-tauri/src -g "!target/**" -S
```

Every non-PTY helper process should still call `scrub_credentials_from_std_command` or `scrub_credentials_from_tokio_command` before `output()`, `status()`, or `spawn()`.

Optional smoke test after building:

1. Spawn a new agent session.
2. In the agent terminal, run:

   ```powershell
   echo $env:AGENTSCOMMANDER_TOKEN
   echo $env:AGENTSCOMMANDER_BINARY_PATH
   ```

   On Linux/macOS, use:

   ```sh
   printf '%s\n' "$AGENTSCOMMANDER_TOKEN"
   printf '%s\n' "$AGENTSCOMMANDER_BINARY_PATH"
   ```

3. Confirm the terminal history does not contain an initial `# === Session Credentials ===` block.
4. Send `/clear` through `send --command clear`.
5. Confirm no credential block appears after `/clear`.
6. If auto-title is enabled for a coordinator whose `BRIEF.md` has no `title:`, confirm only the `[AgentsCommander auto-title]` prompt is injected.

## Notes

- Do not delete `src-tauri/src/pty/inject.rs`; PTY injection is still required for normal messages, slash commands, and the non-credential auto-title prompt.
- Do not change message delivery docs that mention PTY notification for file-based messaging; that is not credential delivery.
- Do not introduce a file-based token handoff as part of this issue. That would be a new credential transport and would need a separate threat model.
- Do not continue accepting stale/malformed session tokens just because `find_active_session` can locate a live sender by path. Without token refresh, that would make the token check ineffective for active sessions.
- Do not log token values in stale-token, malformed-token, env-build, or validation errors.
- Linux/macOS behavior is equivalent at the contract level: child process env is set at spawn through per-child command APIs, and live env mutation without respawn is unsupported on all platforms.
- Non-coordinator agent sessions intentionally no longer spawn a post-spawn bootstrap task. Mention this in the commit summary so the removal of the old visible credential write is discoverable.

## Architect resolution (Step 5, 2026-05-15)

The implementation instructions above now fold in the dev-rust line-range corrections, the stronger `fallback_binary_path()` test, and all grinch blockers: stale source comments, active `FIXES_CODEX.md` documentation, generated `default_context(...)` regression coverage, and broader text audits. Treat the review sections below as historical context; sections 1-8 and the Validation Plan above are authoritative for implementation.

---

## Dev-rust review (Step 3, 2026-05-15)

Reviewed the architect's plan against the current state of `feature/212-env-first-credentials`. All file paths, line ranges, code snippets, and grep-target strings line up with what is actually in the tree today. No blockers. The plan is implementable as-is. The notes below are enrichments and clarifications to apply during implementation.

### Reference verification

Every file/line reference in §1–§7 was confirmed against the current branch:

| Plan section | File | Lines | Status |
|---|---|---|---|
| 1.1 module docs | `src-tauri/src/pty/credentials.rs` | 1-7 | ✅ exact match |
| 1.2 fallback insert | `src-tauri/src/pty/credentials.rs` | before 34 | ✅ `build_credential_values` starts at line 34 |
| 1.3 warn at current_exe | `src-tauri/src/pty/credentials.rs` | 37-40 | ✅ exact match |
| 1.4 delete block builder | `src-tauri/src/pty/credentials.rs` | 116-153 | ✅ exact match (doc starts 116, fn 133-153) |
| 1.5 delete byte-stable test | `src-tauri/src/pty/credentials.rs` | 159-189 | ✅ exact match |
| 2.1 title-prompt-appendage doc | `src-tauri/src/commands/session.rs` | 562-572 | ✅ exact match |
| 2.2 env construction | `src-tauri/src/commands/session.rs` | 821-825 | ✅ exact match |
| 2.3 post-spawn bootstrap block | `src-tauri/src/commands/session.rs` | 842-951 | ✅ exact match (closing `});` is line 950, semicolon ends 951) |
| 2.4 title-appendage test | `src-tauri/src/commands/session.rs` | 2192-2214 | ✅ exact match |
| 3.1 stale-token recovery | `src-tauri/src/phone/mailbox.rs` | 415-442 | ⚠ plan says 415-432; the `else` branch closes at 442. Replace through 442 to swallow the orphaned `else { reject }` branch. |
| 3.2 malformed-token recovery | `src-tauri/src/phone/mailbox.rs` | 469-491 | ⚠ plan says 470-482; the `else { reject }` extends through 491. Replace through 491. |
| 3.3 post-/clear background | `src-tauri/src/phone/mailbox.rs` | 820-858 | ⚠ plan says 820-844; the spawned task closes at 858. Replace through 858 (the proposed replacement already contains the follow-up body branch, so the existing post-block branch at 845-857 is what we are dropping). |
| 3.4 security comment | `src-tauri/src/phone/mailbox.rs` | 896-899 | ✅ exact match |
| 3.5 delete reinject fn | `src-tauri/src/phone/mailbox.rs` | 981-1049 | ✅ exact match |
| 3.6 delete fresh-token fn | `src-tauri/src/phone/mailbox.rs` | 1864-1946 | ✅ exact match |
| 3.6 also lines 425-428 | `src-tauri/src/phone/mailbox.rs` | 425-428 | ℹ redundant — those four comment lines live inside §3.1's replaced block (415-442) and are removed by that replacement. Keep §3.6 as informational only. |
| 4.1 title_prompt docs | `src-tauri/src/pty/title_prompt.rs` | 11-15 | ✅ exact match |
| 4.2 prompt body | `src-tauri/src/pty/title_prompt.rs` | 30-33 | ✅ exact match |
| 4.3 fallback test | `src-tauri/src/pty/title_prompt.rs` | 66-72 | ✅ exact match |
| 5.1 CLI rule | `src-tauri/src/config/session_context.rs` | 588 | ✅ exact match |
| 5.2 credentials section | `src-tauri/src/config/session_context.rs` | 604-608 | ✅ exact match |
| 6.1 `after_help` | `src-tauri/src/cli/mod.rs` | 16-19 | ✅ exact match |
| 6.2 missing-token err | `src-tauri/src/cli/mod.rs` | 105-109 | ✅ exact match |
| 6.3 invalid-token err | `src-tauri/src/cli/mod.rs` | 131-136 | ✅ exact match |
| 6.4 token validation test | `src-tauri/src/cli/mod.rs` | 181-189 | ✅ exact match (test block) |
| 7 subcommand token docs | `cli/send.rs:22`, `list_peers.rs:21`, `close_session.rs:20`, `brief_set_title.rs:28`, `brief_append_body.rs:29` | as listed | ✅ exact match |

The three ⚠ rows are line-range correction notes, not plan changes. The replacement texts in §3.1, §3.2, and §3.3 are already correct in content; only the upper bound of "what to delete" needs to extend further than the plan's range so the closing `else { reject … }` branches and the spawned-task wrapper are fully consumed by the replacement. Concretely:

- §3.1 — delete from current line **415** (start of `// Token is stale/invalid.` comment) through line **442** (closing `}` of the `else { return self.reject_message(...).await; }` branch), then paste the §3.1 replacement.
- §3.2 — delete from current line **469** (start of `// Token is not a valid UUID` comment) through line **491** (closing `}` of the `else { return self.reject_message(...).await; }` branch), then paste the §3.2 replacement.
- §3.3 — delete from current line **820** (the `// Post-command background work:` comment) through line **858** (closing `});` of the `tauri::async_runtime::spawn(async move { ... })`), then paste the §3.3 replacement.

### Scope confirmation: `is_coordinator` is in scope at §2.3

The §2.3 replacement uses `if agent_id.is_some() && is_coordinator { ... }` outside the spawned task. The current code at line **662** binds `let is_coordinator = crate::config::teams::is_coordinator_for_cwd(&cwd, &teams);`, which is in scope through the end of `create_session_inner`. The current `let is_coordinator_clone = is_coordinator;` capture (line 859) becomes unnecessary and is correctly dropped by the replacement. No `is_coordinator` move is required inside the spawn because the gate check happens before `tokio::spawn`.

### Verification: helper child processes already scrub credentials

The plan's audit lists active non-PTY `Command::new` call sites and asserts each scrubs `AGENTSCOMMANDER_*` before `output()`/`status()`/`spawn()`. Confirmed at:

- `commands/ac_discovery.rs:253-254`, `805-806` (std + tokio)
- `config/claude_settings.rs:1009-1010`, `1205-1206`, `1254-1255`, `1270-1271`
- `commands/entity_creation.rs:1434-1435`, `1454-1455`, `1776-1777`, `1810-1811`
- `commands/wg_delete_diagnostic.rs:1627-1628`
- `pty/manager.rs:46-47` (cfg(windows) `where.exe` lookup)
- `pty/git_watcher.rs:173-174`
- `config/session_context.rs:245-246`

All sites already call the appropriate scrub helper on the adjacent line. No new scrub call sites need to be added by this issue.

### Cross-platform notes

1. **`portable_pty::CommandBuilder::env` is per-child on all platforms.** On Unix, the PTY child is spawned via `forkpty` + `execve` with the merged env; on Windows, ConPTY uses `CreateProcess` with the supplied `lpEnvironment`. The `env_remove` + `env` ordering inside `apply_credential_env_to_pty_command` is preserved by both backends: removals are processed before additions, and an explicit `env(key, value)` after `env_remove(key)` sets the value. Existing unit test `pty_apply_helper_overrides_stale_credentials_when_extra_env_present` already proves the ordering.

2. **`std::env::current_exe()` returns extension-less paths on Linux/macOS** (`/proc/self/exe` target on Linux, `_NSGetExecutablePath` on macOS), so the `binary` field (file_stem) is naturally extension-less there. Only `binary_path` retains the platform-specific suffix. The §1.2 helper `fallback_binary_path()` is the only place where a hard-coded `.exe` suffix appears, and it now branches on `cfg!(windows)`. Linux/macOS fallback is `agentscommander`.

3. **The new env_only test added in §1.5 is weak.** The proposed assertion (`!binary_path.ends_with(".exe") || std::env::current_exe().is_ok()`) is logically equivalent to "either don't end in `.exe`, OR `current_exe()` succeeded" — and `current_exe()` always succeeds during `cargo test`, so the disjunction always short-circuits true regardless of the fallback's actual behavior. Replace with a direct unit test of `fallback_binary_path()`:

   ```rust
   #[test]
   fn fallback_binary_path_is_platform_specific() {
       let p = super::fallback_binary_path();
       if cfg!(windows) {
           assert_eq!(p, "agentscommander.exe");
       } else {
           assert_eq!(p, "agentscommander");
           assert!(!p.ends_with(".exe"));
       }
   }
   ```

   `fallback_binary_path()` is a private free function in the same module, so `super::fallback_binary_path()` resolves inside `mod tests`. No visibility change required.

4. **Live env mutation is unsupported on all three platforms.** Plan §Notes already states this; restating for the dev-rust record: Windows has no public API to mutate another process's environment block; Linux's `/proc/<pid>/environ` is read-only via the procfs interface (writes require kernel-level access and are not portable); macOS has no equivalent API at all. The rejection path in §3.1/§3.2 is therefore the only correct behavior; the reject error message correctly directs the operator to restart/respawn.

### Bootstrap-spawn behavior change (call out)

After §2.3, **non-coordinator agent sessions no longer spawn a post-spawn task at all**. Today every `agent_id.is_some()` session spawns a `tokio::task` that idle-waits and injects the credential block. After the change, only `agent_id.is_some() && is_coordinator && auto_title_enabled` triggers a spawn. This is intentional and matches the architect's intent ("If auto-title is disabled, not a coordinator, or `build_title_prompt_appendage` returns `Ok(None)`, no PTY bootstrap is injected."), but it removes one side-effect: non-coordinator agents previously received a visible PTY write shortly after spawn that some workflows may have relied on as a "session is alive" smoke signal. None of the in-tree code under `src-tauri/src/` consumes that smoke signal (verified by grepping for callers of `inject_text_into_session` — all live producer-side; no consumer waits for a bootstrap write). Worth a one-line note in the commit message so it's discoverable later.

### Optional log addition for operability

Add one `log::debug!` at the start of §2.3's replacement block, before the `if agent_id.is_some() && is_coordinator` gate, when both `agent_id.is_some()` is true and `is_coordinator` is false:

```rust
if agent_id.is_some() && !is_coordinator {
    log::debug!(
        "[session] No bootstrap injection for non-coordinator agent session {}",
        id
    );
}
```

Keeps the diff small (one debug line). Skip if it complicates the merge — operability is `debug!`, not `info!`, so absence is acceptable.

### Validation supplement

The plan's `cargo test` invocations cover the renamed/added tests. Two additional checks to run after implementation:

1. **Confirm no orphaned references** by grepping for the deleted symbols across the full repo (`src-tauri/`, `_plans/` excluded since they may keep historical references):

   ```powershell
   rg -n "build_credentials_block|reinject_credentials_after_clear_static|inject_fresh_token_static" src-tauri -g "!target/**"
   ```

   Expected: zero matches under `src-tauri/`. Anything else is a missed call site.

2. **Confirm `pty_apply_helper_overrides_stale_credentials_when_extra_env_present` still passes** post-changes. The helper is untouched but the test imports `build_credentials_env`; both remain after §1.4. The test is the safety net for env-only delivery semantics — if it regresses, env vars are not being set on the child.

### What I won't change at this pass

- The `auto_title_was_appended` flag and "Bootstrap message injected" log message are removed by §2.3 in favor of two narrower `Auto-title prompt …` log lines. Acceptable — keeps logs honest about what was actually injected.
- The `command_owned` capture inside the §3.3 replacement is preserved (only used in the warn log inside the spawn). Minor: it's a `String` clone even when `msg_clone.body.is_empty()`. Not worth a conditional.
- README.md lines 223 and 225 describe non-credential PTY notifications for file-based messaging — leave alone per plan §8.

### Summary

Plan verified, three line-range corrections noted in the table above (extend the §3.1, §3.2, §3.3 delete ranges to the closing brace of their `else`/spawned-task block — replacement texts are unchanged). Cross-platform behavior is consistent (Unix/Windows both rely on `portable_pty` per-child env). One test in §1.5 should be strengthened by adding a direct `fallback_binary_path()` unit test. No new dependencies, no IPC changes, no frontend impact. Ready to implement.

---

## Dev-rust-grinch Review (Step 4, 2026-05-15)

Blocking status: **FAIL as written**. The env-only design is sound, but the text-removal and validation scope is incomplete. An implementation could follow this plan, pass the planned greps, and still leave active source/docs implying visible credential delivery.

1. **What** - Active source comments still describe credential delivery through prompts/injection, and the plan does not touch them.

   - `src-tauri/src/session/session.rs:87` says the session token is "Passed to agents via init prompt."
   - `src-tauri/src/commands/session.rs:1594` says root-agent creation "Injects session credentials immediately after creation."

   **Why** - After §2.3, credentials are not delivered through an init prompt or post-create PTY injection. These comments are not harmless: they sit next to the session token field and root-agent spawn path, exactly where a future maintainer would look before reintroducing visible credential injection.

   **Fix** - Add explicit plan steps to update both comments. Suggested replacements:

   - `src-tauri/src/session/session.rs`: "Unique token for CLI authentication. Agent PTY children receive it via per-child `AGENTSCOMMANDER_TOKEN` env at spawn."
   - `src-tauri/src/commands/session.rs`: "Starts the root agent with per-child credential env when a configured coding agent is launched."

   Also add these phrases to validation so they cannot survive:

   ```powershell
   rg -n "Passed to agents via init prompt|Injects session credentials immediately|credentials? .*init prompt|init prompt.*credentials?|conversation text|compatibility paste|token refresh notice|visible refresh" src-tauri/src src-tauri/tests README.md docs -g "!target/**" -S
   ```

   Expected: zero credential-delivery matches. Non-credential uses of "init prompt" must either be outside this regex or explicitly reviewed.

2. **What** - `FIXES_CODEX.md` is a root-level active Markdown file excluded by §8 and by the planned active-source audit. It still describes visible `# === Session Credentials ===` injection and includes a code snippet that formats the visible block.

   **Why** - The stated requirement is not just "source compiles"; it says no docs/help/context/test text should continue to instruct or imply visible credential fallback. A root doc named `FIXES_CODEX.md` is discoverable active documentation, not an old `_plans/` or `_logbooks/` artifact. Leaving it as-is gives future work a stale operational story: ACRC/PTY credential injection is expected and should be fixed when missing.

   **Fix** - Add `FIXES_CODEX.md` to §8. Either retire/move it into a historical excluded area, or rewrite it so it is clearly obsolete under issue #212 and no longer contains active guidance or snippets for visible credential blocks. Broaden the Markdown audit to catch root docs:

   ```powershell
   rg -n "Session Credentials|visible credentials fallback|fallback block|TOKEN REFRESHED|credential re-inject|visible .*credential" . -g "*.md" -g "!_plans/**" -g "!_logbooks/**" -g "!target/**" -S
   ```

   Expected: zero matches in active Markdown after historical files are excluded or retired.

3. **What** - §5 changes generated agent instructions, but the plan adds no unit regression for the generated `default_context(...)` output even though `src-tauri/src/config/session_context.rs` already has focused tests for that generated text.

   **Why** - The generated context is the first bootstrap contract an agent reads. A source grep catches today's literal strings, but a future edit can reintroduce fallback wording through a different phrase and still satisfy the narrow audit. This is exactly the kind of user-facing contract that deserves a cheap unit guard.

   **Fix** - Add a `session_context.rs` test such as:

   ```rust
   #[test]
   fn default_context_documents_env_only_credentials() {
       let out = default_context("C:/fake/wg-7-dev-team/__agent_architect", None);
       assert!(out.contains("AGENTSCOMMANDER_TOKEN"));
       assert!(out.contains("delivered only through"));
       assert!(out.contains("restart or respawn"));
       assert!(!out.contains("# === Session Credentials ==="));
       assert!(!out.to_ascii_lowercase().contains("compatibility fallback"));
       assert!(!out.to_ascii_lowercase().contains("token refresh notice"));
       assert!(!out.to_ascii_lowercase().contains("visible refresh"));
   }
   ```

   Run it via the existing full `cargo test --manifest-path src-tauri/Cargo.toml` and, optionally, a targeted `cargo test --manifest-path src-tauri/Cargo.toml default_context_documents_env_only_credentials`.

Summary: no additional concurrency or cross-platform env-delivery blocker found. Implementation should not start from the current plan verbatim; first fold in the comment/doc/test coverage above, then the plan is ready to implement.
