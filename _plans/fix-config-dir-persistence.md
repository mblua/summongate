# Plan: Fix config_dir Persistence on Restart & Restore

**Branch:** `feature/per-project-coding-agents`
**Status:** Reviewed & Enriched by dev-rust
**Created:** 2026-04-11

---

## Problem Statement

When a session uses a custom Claude binary with a non-standard config directory (e.g., `claude-phi` with `configDir: ~/.claude-phi`), the `config_dir` is lost on session restart and app restore. This causes `--continue` injection and the Telegram JSONL watcher to look in the wrong directory (`~/.claude` instead of `~/.claude-phi`).

The `config_dir` field already exists on `Session`, `SessionInfo`, and `PersistedSession` structs, and is correctly set during initial session creation (in `create_session_inner`). But three code paths fail to preserve it.

---

## Root Cause Analysis

### Bug 1: Snapshot Discards config_dir

**File:** `src-tauri/src/config/sessions_persistence.rs`, line 222

```rust
config_dir: None, // re-resolved on restore from shell command
```

The comment says "re-resolved on restore from shell command" — but this is wrong. On restore, `create_session_inner` is called with `skip_continue = false`, which does re-resolve `config_dir` from the agent. However, on restore the `agent_id` is `None` (line 523 in lib.rs: `None, // No agent_id on restore`), so `resolve_config_dir` only sees the shell command basename, not the `AgentConfig.config_dir` override. Standard `claude` works (basename maps to `~/.claude`), but custom binaries like `claude-phi` lose their explicit `configDir` override.

**Fix:** Persist the actual `config_dir` from the live session.

### Bug 2: restart_session Doesn't Preserve config_dir

**File:** `src-tauri/src/commands/session.rs`, lines 563–574

The destructuring reads 6 fields from the old session but omits `config_dir`:

```rust
let (shell, shell_args, cwd, name, git_branch_source, git_branch_prefix) = {
    let mgr = session_mgr.read().await;
    let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
    (
        session.shell.clone(),
        session.shell_args.clone(),
        session.working_directory.clone(),
        session.name.clone(),
        session.git_branch_source.clone(),
        session.git_branch_prefix.clone(),
    )
};
```

After `create_session_inner` returns (line 598), `skip_continue = true` means the --continue block (where `config_dir` is resolved and set via `mgr.set_config_dir()`) is **entirely skipped**. The new session gets `config_dir: None`.

**Fix:** Read `config_dir` from old session, set it on the new session after creation.

### Bug 3: App Restore Doesn't Set Persisted config_dir

**File:** `src-tauri/src/lib.rs`, lines 515–541

After `create_session_inner` succeeds (line 530), the code does nothing with `ps.config_dir`. Even if Bug 1 is fixed and `config_dir` is persisted correctly, the restored session would still not have it because `create_session_inner` with `agent_id = None` can only auto-detect standard binaries.

**Fix:** After successful restore, if `ps.config_dir` is set, call `mgr.set_config_dir()` on the new session.

---

## Fixes

### Fix 1: sessions_persistence.rs — Persist config_dir in Snapshot

**File:** `src-tauri/src/config/sessions_persistence.rs`
**Line:** 222

**Change:**
```rust
// BEFORE (line 222):
config_dir: None, // re-resolved on restore from shell command

// AFTER:
config_dir: s.config_dir.clone(),
```

That's it. The `PersistedSession` struct already has the field (line 32), serde handles serialization. The comment was misleading — remove it.

### Fix 2: session.rs restart_session — Carry config_dir Across Restart

**File:** `src-tauri/src/commands/session.rs`

**Step 2a:** Add `config_dir` to the destructuring (line 563).

**Change lines 563–574:**
```rust
// BEFORE:
let (shell, shell_args, cwd, name, git_branch_source, git_branch_prefix) = {
    let mgr = session_mgr.read().await;
    let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
    (
        session.shell.clone(),
        session.shell_args.clone(),
        session.working_directory.clone(),
        session.name.clone(),
        session.git_branch_source.clone(),
        session.git_branch_prefix.clone(),
    )
};

// AFTER:
let (shell, shell_args, cwd, name, git_branch_source, git_branch_prefix, old_config_dir) = {
    let mgr = session_mgr.read().await;
    let session = mgr.get_session(uuid).await.ok_or("Session not found")?;
    (
        session.shell.clone(),
        session.shell_args.clone(),
        session.working_directory.clone(),
        session.name.clone(),
        session.git_branch_source.clone(),
        session.git_branch_prefix.clone(),
        session.config_dir.clone(),
    )
};
```

**Step 2b:** After `create_session_inner` returns and the new session UUID is parsed, set config_dir on the new session.

**Insert after line 604** (after `let new_uuid = Uuid::parse_str(...)`) and before the switch_session block:

```rust
// Carry config_dir from the old session to the new one.
// create_session_inner with skip_continue=true skips the config_dir resolution block.
if let Some(ref dir) = old_config_dir {
    let mgr = session_mgr.read().await;
    mgr.set_config_dir(new_uuid, Some(dir.clone())).await;
}
```

**Step 2c [ENRICHMENT]:** Fix line 629 — Telegram bridge re-attach reads stale `session_info.config_dir`.

`session_info` is the `SessionInfo` returned from `create_session_inner`. Since `skip_continue = true`, the config_dir resolution block inside `create_session_inner` resolves `None` for custom binaries (because `agent_id = None`). Step 2b sets the correct value on the SessionManager's internal session, but the local `session_info` variable is already captured and still has `config_dir: None`. Line 629 reads from this stale copy, so the Telegram JSONL watcher would fall back to `~/.claude`.

**Change line 629:**
```rust
// BEFORE:
let bridge_config_dir = session_info.config_dir.as_ref().map(std::path::PathBuf::from);

// AFTER:
let bridge_config_dir = old_config_dir.as_ref().map(std::path::PathBuf::from);
```

This uses the preserved `old_config_dir` directly instead of reading from the stale `session_info`.

### Fix 3: lib.rs — Set config_dir After Restore

**File:** `src-tauri/src/lib.rs`

**Inside the `Ok(info)` arm (lines 530–533)**, after recording `active_id`, set the persisted `config_dir`:

```rust
// BEFORE (lines 530-533):
Ok(info) => {
    if ps.was_active {
        active_id = Some(info.id);
    }
}

// AFTER:
Ok(info) => {
    // Restore config_dir from persisted state (not auto-detectable for custom binaries)
    if ps.config_dir.is_some() {
        if let Ok(new_uuid) = uuid::Uuid::parse_str(&info.id) {
            let mgr = session_mgr_clone.read().await;
            mgr.set_config_dir(new_uuid, ps.config_dir.clone()).await;
        }
    }
    if ps.was_active {
        active_id = Some(info.id);
    }
}
```

---

## Implementation Sequence

| Step | File | Line(s) | What |
|------|------|---------|------|
| 1 | `src-tauri/src/config/sessions_persistence.rs` | 222 | Change `config_dir: None` → `s.config_dir.clone()` |
| 2a | `src-tauri/src/commands/session.rs` | 563–574 | Add `old_config_dir` to destructuring |
| 2b | `src-tauri/src/commands/session.rs` | after 604 | Set `config_dir` on new session via `mgr.set_config_dir()` |
| 2c | `src-tauri/src/commands/session.rs` | 629 | Use `old_config_dir` instead of stale `session_info.config_dir` |
| 3 | `src-tauri/src/lib.rs` | 530–533 | Set `config_dir` on restored session |
| 4 | Verify | — | `cargo check` passes |

---

## Files Modified

| File | Change |
|------|--------|
| `src-tauri/src/config/sessions_persistence.rs` | 1 line: `None` → `s.config_dir.clone()` |
| `src-tauri/src/commands/session.rs` | ~9 lines: add `old_config_dir` to destructuring + set on new session + fix telegram re-attach |
| `src-tauri/src/lib.rs` | ~5 lines: set `config_dir` after restore |

**Total: ~15 lines changed across 3 files. No new files, no new structs, no new commands.**

---

## Validation

| Test | Expected |
|------|----------|
| Create session with custom Claude binary (configDir: `~/.claude-phi`) | `config_dir` set on session |
| Restart that session | New session has same `config_dir` |
| Close and reopen the app | Restored session has same `config_dir` |
| Check `sessions.json` after snapshot | `configDir` field present with correct value |
| Standard `claude` binary (no explicit configDir) | Still auto-resolves to `~/.claude` — no regression |
| Non-Claude binary (codex, gemini) | `config_dir` remains `None` — no regression |
| Telegram JSONL watcher on restart | Uses correct config_dir path for custom binary |

---

## Risk Assessment

| Risk | Mitigation |
|------|-----------|
| Existing `sessions.json` without `configDir` field | `#[serde(default)]` on `PersistedSession.config_dir` means old files deserialize fine as `None` |
| `set_config_dir` acquires write lock | Same pattern used in `create_session_inner` already — no new contention |
| `restart_session` has `skip_continue=true` so --continue won't fire anyway | True for restart, but `config_dir` is also used by the JSONL watcher (line 629) which IS re-attached on restart. Without the fix, the watcher gets `None` and falls back to `~/.claude` |
| Fix 3 sets `config_dir` AFTER `create_session_inner` returns — `--continue` injection for custom binaries is missed on app restore | Pre-existing limitation, not a regression. On restore with `agent_id = None`, `resolve_config_dir(None, shell, args)` cannot detect custom binaries. `--continue` is a convenience feature; starting a fresh conversation on full app restart is acceptable. The critical fix (JSONL watcher path) is handled correctly. To fully fix this, `create_session_inner` would need a new `override_config_dir: Option<String>` parameter — out of scope for this patch. |

---

## Grinch Review

**VERDICT: APPROVED — no blocking issues.**

Verified all three bugs against codebase at current HEAD. Line numbers match. Fix logic is correct. Dev-rust's Step 2c enrichment catches a real stale-variable bug that the original plan missed.

### Verification Summary

**Fix 1** (`sessions_persistence.rs:222`): Confirmed. `s` is `SessionInfo` (from `mgr.list_sessions()`), which has `config_dir: Option<String>` (session.rs:88). `PersistedSession.config_dir` is also `Option<String>` with `#[serde(default)]` (sessions_persistence.rs:32). Types match. Backward-compatible with old `sessions.json` files missing the field.

**Fix 2a** (`session.rs:563–574`): Confirmed. `session.config_dir` is `Option<String>` (session.rs:52). The destructuring reads from `mgr.get_session(uuid)` which returns `&Session`. Adding `session.config_dir.clone()` to the tuple is straightforward.

**Fix 2b** (`session.rs` after line 604): Confirmed. `set_config_dir` acquires inner write lock on `sessions` HashMap while the caller holds outer read lock on `SessionManager` — same nested lock pattern used in `create_session_inner` at line 101. No deadlock risk. The set completes before `persist_current_state` at line 640, so the snapshot includes the correct value.

**Fix 2c** (`session.rs:629`): Confirmed critical. `session_info` is the return value of `create_session_inner(skip_continue=true, agent_id=None)`. For custom binaries like `claude-phi`, `resolve_config_dir(None, shell, args)` returns `None` because basename != "claude" exactly (session.rs:797). `set_config_dir` in Step 2b updates the manager's internal `Session`, but the local `session_info` was already built from `SessionInfo::from(&session)` at line 311 inside `create_session_inner` — before Step 2b ran. Using `old_config_dir` directly is the correct fix.

**Fix 3** (`lib.rs:530–533`): Confirmed. `uuid::Uuid` is already used at lib.rs:545 (in scope). The read-lock + inner write-lock pattern is consistent. `info.id` is borrowed (`&info.id`) in the parse call, then moved into `active_id` — ownership rules satisfied (borrow ends before move). Sequential execution within the restore loop means no race with the `persist_merging_failed` at line 554.

### Non-blocking Findings

**1. [INFO] Redundant lock acquisition in Fix 2b** — The proposed code acquires a separate `session_mgr.read().await` for `set_config_dir`, then immediately drops it. Lines 605–608 acquire another read lock for `switch_session`. These two could be combined into one block:
```rust
{
    let mgr = session_mgr.read().await;
    if let Some(ref dir) = old_config_dir {
        mgr.set_config_dir(new_uuid, Some(dir.clone())).await;
    }
    let _ = mgr.switch_session(new_uuid).await;
}
```
Not a correctness issue — just unnecessary lock churn (two acquire-release cycles vs one). Dev can combine or leave separate at their discretion.

**2. [INFO] Stale frontend `session_created` event** — Both Fix 2b (restart) and Fix 3 (restore) set `config_dir` AFTER `create_session_inner` returns. Inside `create_session_inner`, line 312 emits `session_created` with the `SessionInfo` built at line 311, which has `config_dir: None` for custom binaries. The frontend's session store will hold the stale value until the next `list_sessions` refresh. Not a functional bug — critical paths (JSONL watcher, `--continue`) read from the `SessionManager`'s internal `Session`, not the frontend's cached `SessionInfo`. Noting for completeness.

### What I Checked That Passed

- No Telegram auto-attach in the lib.rs restore path — confirmed (lib.rs restore only calls `create_session_inner`, no bridge attach). Fix 3's value is: (a) correct JSONL watcher path for any future manual `telegram_attach`, and (b) correct snapshot persistence.
- `is_claude` detection works correctly on restart/restore even without `agent_id` — confirmed (detected from binary name at session.rs:65, not from agent config).
- `old_config_dir` type matches `bridge_config_dir` derivation — both `Option<String>` → `.as_ref().map(PathBuf::from)` → `Option<PathBuf>`.
- No concurrent persist can sneak between `create_session_inner` returning and Fix 3 setting `config_dir` — confirmed (restore loop is sequential within a single tokio task).
- Known limitation (`--continue` on restore for custom binaries) is pre-existing, not a regression — confirmed. Standard `claude` binary works correctly on restore because `resolve_config_dir(None, shell, args)` detects basename == "claude" → `~/.claude`.
