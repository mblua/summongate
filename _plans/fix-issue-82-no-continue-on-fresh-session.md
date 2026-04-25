# Fix issue #82 — fresh sessions inject `--continue` against ghost `~/.claude/projects/` dir

- **Issue**: https://github.com/mblua/AgentsCommander/issues/82
- **Branch**: `fix/82-no-continue-on-fresh-session` (already created from `origin/main`)
- **Scope**: `repo-AgentsCommander` only; Claude-only auto-resume path (codex/gemini paths intentionally untouched, see §7).

Line numbers in this plan were verified against the current tip of `fix/82-no-continue-on-fresh-session` (aligned with `origin/main` at `96860c0`). Dev can apply offsets 1:1.

---

## 1. Problem restatement

`create_session_inner` auto-injects `--continue` into a Claude session's argv when **all** of:

1. Agent is a Claude variant (`is_claude == true`).
2. `skip_auto_resume == false`.
3. `~/.claude/projects/<mangle_cwd_for_claude(cwd)>/` `is_dir()` returns `true`.

The mangled name is a deterministic function of the CWD path (`session/session.rs:8-18`). When a workgroup replica is torn down and re-created at the same path, the new replica's CWD mangles to the **same** projects-dir name. The dir survives the teardown (Claude Code never cleans it). On first launch of the new replica:

- `is_dir()` → `true` (ghost dir from prior lifetime).
- `--continue` is injected.
- Claude rejects it: `No conversation found to continue`.

The dir-existence test is **necessary but not sufficient** — it does not prove there is a conversation belonging to *this* AC instance worth resuming. The signal we already have for that is the existence of a `PersistedSession` entry in `sessions.toml` (or, equivalently, a live `SessionManager` record at the same CWD).

Six call sites pass `skip_auto_resume`; four of them currently send `false` from a context that is semantically "fresh create". Only one (the startup-restore loop) sends `false` from a context that is semantically "restore" — the case `--continue` was designed for.

---

## 2. Recommended fix and rationale

**Recommendation: Option 1 — invert the default at fresh-create call sites — refined with a small carry-through at the wake spawn-fallback.**

The bool stays. Its meaning stays. We change the values at the four "fresh create" call sites (CLI/UI/root-agent/session-request) from `false` to `true`, leave the startup-restore call site at `false`, and at the wake spawn-fallback (`mailbox.rs:638`) compute a per-call value based on whether `find_active_session` matched anything.

### 2.1 Why option 1 over the alternatives

| Option | Verdict | Reasoning |
|---|---|---|
| **1. Invert default at fresh-create call sites** (chosen) | ✅ | Aligns the call to the actual semantic intent at every call site. The information needed to decide "fresh vs restore" is **already** present at every call site (it's literally what each command represents). No new state, no new heuristic. Smallest blast radius — six explicit value flips and one carried boolean. |
| 2. Strengthen the heuristic (mtime + cwd-match in `.jsonl`) | ❌ | Replaces an unsound check with a less-unsound one. Still inferring "is there a conversation worth resuming" from filesystem residue. Mtime is touched by backups/AV/sync tools. `.jsonl` cwd field can drift if the user moved the dir. We already have a real signal (`PersistedSession`/`SessionManager` records) — burying it under a heuristic is a regression in clarity. Useful as a **belt-and-suspenders** layer for #55 but not as the primary fix for #82. |
| 3. Marker file in replica dir | ❌ | Adds a second persistent state. `sessions.toml`'s `PersistedSession` entry already serves as a marker — it's exactly "AC believes a session exists at this CWD". Adding a second source of truth invites desync. Also: the marker would have to be deleted on replica teardown, and the replica teardown path is owned by the user / external tooling, so the marker would leak just like the projects dir leaks today. |
| **4. Typed enum `ResumeIntent::{Fresh, Restore, Wake}`** (rejected) | ❌ | Cleaner API but bigger blast radius (fn signature, all six call sites, all tests). The boolean already encodes the same information; the only thing we lose is grep-ability. Defer to a future cleanup if a third value ever appears. |

### 2.2 Why this fix doesn't make #40 worse

Issue #40 is the *opposite* failure mode (`--continue` not applied when the user wanted it). The only call site that applies `--continue` for a restored session today is `lib.rs:594`, and this plan **leaves that line at `false`**. Whatever Claude-internal binding problem #40 represents, this fix cannot regress it: the input to that codepath is unchanged.

If anything, the cleaner separation makes #40 easier to reason about — the failure surface is now isolated to "lib.rs:594 fired `--continue` and Claude rejected it" rather than mixing with "fresh create fired `--continue` and Claude rejected it".

### 2.3 Why this fix is compatible with #55

Issue #55 (TTL-based filter on stale auto-resume) layers cleanly on top: it would add a freshness check inside the auto-inject block at `commands/session.rs:357-382`, gated by `!skip_auto_resume`. After this plan lands, that block runs only for restore + qualifying wake call sites — exactly the surface where a TTL filter makes sense. No conflict.

---

## 3. Files to touch

| File | Change |
|---|---|
| `src-tauri/src/web/commands.rs` | 1 line: `false` → `true` at the `skip_auto_resume` arg |
| `src-tauri/src/commands/session.rs` | 2 lines: `false` → `true` at the `skip_auto_resume` arg in `create_session` (Tauri cmd) and `create_root_agent_session`; documentation comment update on `create_session_inner` |
| `src-tauri/src/lib.rs` | 1 comment update on the restore call site (clarify that this is the *only* sanctioned `false`) |
| `src-tauri/src/phone/mailbox.rs` | 2 changes: `process_session_requests` line 1801 → `true`; `deliver_wake` spawn-fallback (line 638) → carry `had_prior_session` and pass `!had_prior_session` |
| `src-tauri/src/commands/session.rs` (tests) | Add unit tests for the auto-inject decision (extracted into a pure helper) and for the wake path's `had_prior_session` carry-through |

No changes to:
- `create_session_inner` signature (the `skip_auto_resume: bool` param stays).
- `effective_restart_skip_auto_resume` and the restart path (already correct: defaults to `true`, opt-in `Some(false)` for deferred-wake).
- Codex/Gemini auto-resume paths at `commands/session.rs:384-398` (they share the same `!skip_auto_resume` gate, so they ride the same correctness improvement automatically — see §7 note).
- `~/.claude/projects/` lifecycle (we don't manage that dir; it's owned by Claude Code).
- `mangle_cwd_for_claude`.

---

## 4. Exact code changes

### 4.1 `src-tauri/src/web/commands.rs` line 80

**Before** (lines 68-82):
```rust
            let info = crate::commands::session::create_session_inner(
                &state.app_handle,
                &state.session_mgr,
                &state.pty_mgr,
                shell,
                shell_args,
                cwd,
                session_name,
                agent_id,
                None,  // agent_label (auto-detected)
                false, // skip_tooling_save
                Vec::new(), // git_repos
                false, // skip_auto_resume
            )
            .await?;
```

**After** — change line 80 only:
```rust
                true, // skip_auto_resume = true → fresh create, no `--continue` injection
```

Rationale: this path is the CLI `create_session` command. Always a fresh user-initiated create. No prior-conversation context exists from AC's perspective.

### 4.2 `src-tauri/src/commands/session.rs` line 633 — Tauri `create_session` command

**Before** (lines 621-635):
```rust
    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        cwd.clone(),
        session_name,
        agent_id,
        agent_label,
        false, // persist tooling
        git_repos.unwrap_or_default(),
        false, // skip_auto_resume
    )
    .await?;
```

**After** — change line 633 only:
```rust
        true, // skip_auto_resume = true → fresh create, no `--continue` injection
```

Rationale: invoked by the sidebar UI's "new session" button and equivalents. Always a fresh user-initiated create.

### 4.3 `src-tauri/src/commands/session.rs` line 1216 — `create_root_agent_session`

**Before** (lines 1204-1218):
```rust
    let info = create_session_inner(
        &app,
        session_mgr.inner(),
        pty_mgr.inner(),
        shell,
        shell_args,
        root_agent_path.clone(),
        Some("Root Agent".to_string()),
        agent_id,
        agent_label,
        false,
        Vec::new(),
        false, // skip_auto_resume
    )
    .await?;
```

**After** — change line 1216 only:
```rust
        true, // skip_auto_resume = true → fresh create, no `--continue` injection
```

Rationale: `create_root_agent_session` is invoked once per app instance to create the root agent session at `{exe_dir}/.{binary_name}/ac-root-agent`. The function already has an early-return that **reuses an existing session at that path** (line 1154-1164), so the `create_session_inner` call only fires when no prior live session exists. Treat as fresh create.

Note: at app restart, the root agent's session is restored via the normal `lib.rs` restore loop (sessions.toml entry), so resume-on-restart is unaffected.

### 4.4 `src-tauri/src/phone/mailbox.rs` line 1801 — `process_session_requests`

**Before** (lines 1789-1803):
```rust
            match crate::commands::session::create_session_inner(
                app,
                session_mgr.inner(),
                pty_mgr.inner(),
                request.shell.clone(),
                request.shell_args.clone(),
                request.cwd.clone(),
                Some(request.session_name.clone()),
                Some(request.agent_id.clone()),
                None,  // No agent label — auto-detected from shell
                false, // Persist tooling
                Vec::new(), // git_repos
                false, // skip_auto_resume
            )
```

**After** — change line 1801 only:
```rust
                true, // skip_auto_resume = true → CLI session-request is a fresh create
```

Rationale: `process_session_requests` consumes JSON request files dropped by the CLI's `create-agent` flow (`src-tauri/src/cli/create_agent.rs:53` `SessionRequest`). These are external fresh-create calls, not restores.

### 4.5 `src-tauri/src/phone/mailbox.rs` lines 535-641 — `deliver_wake` spawn-fallback

This is the one call site where the right value depends on runtime state. Two cases must be distinguished:

- **(a) Wake-from-known-state** — `find_active_session` matched a session record at this CWD (typically a deferred non-coord with `Exited(0)` status). A real prior conversation may exist; `--continue` is desired. → `skip_auto_resume = false`.
- **(b) Wake-from-cold** — `find_active_session` returned `None`. AC has no record of any session at this CWD. The `~/.claude/projects/` dir, if present, is a ghost from another instance/lifetime. → `skip_auto_resume = true`.

**Before** (lines 535-641) — abridged for context, only the relevant control-flow lines reproduced:
```rust
    async fn deliver_wake(
        &self,
        app: &tauri::AppHandle,
        msg: &OutboxMessage,
    ) -> Result<(), String> {
        if let Some(session_id) = self.find_active_session(app, &msg.to).await {
            let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
            let mgr = session_mgr.read().await;
            let sessions = mgr.list_sessions().await;
            let session = sessions.iter().find(|s| s.id == session_id.to_string());

            if let Some(s) = session {
                // ... existing match wake_action_for(&s.status) ...
                //     WakeAction::Inject  → return inject_into_pty(...).
                //     WakeAction::RespawnExited → destroy_session_inner(...) and fall through.
            } else {
                // session_id not in list_sessions — fall through.
                drop(mgr);
            }
        }

        // ── No active session (or only Exited) — spawn a persistent one ──
        log::info!(
            "[mailbox] wake: no active session for '{}', spawning persistent session",
            msg.to
        );
        // ...
        let info = crate::commands::session::create_session_inner(
            app,
            session_mgr.inner(),
            pty_mgr.inner(),
            shell,
            shell_args,
            cwd,
            Some(session_name),
            agent_id,
            agent_label,
            false,              // skip_tooling_save = false → persist lastCodingAgent
            Vec::new(),         // git_repos
            false,              // skip_auto_resume = false → allow provider auto-resume
        )
        .await
```

**After** — introduce a `had_prior_session` flag at the top of the function, set it to `true` whenever `find_active_session` returns `Some`, and use `!had_prior_session` for `skip_auto_resume`:

1. **Insert** at line 534, immediately after the function signature opens:
   ```rust
           // True iff `find_active_session` matched a SessionManager record
           // at this CWD. Distinguishes wake-from-known-state (resume desired)
           // from wake-from-cold (fresh spawn, no `--continue`). See issue #82.
           let mut had_prior_session = false;
   ```

2. **Modify** line 535 to set the flag:
   ```rust
           if let Some(session_id) = self.find_active_session(app, &msg.to).await {
               had_prior_session = true;
               // ... rest of block unchanged ...
   ```

3. **Modify** line 638 (inside the spawn-fallback `create_session_inner` call):
   ```rust
               !had_prior_session, // skip_auto_resume — true on cold wake, false on respawn-exited
   ```

   (Replaces the existing `false, // skip_auto_resume = false → allow provider auto-resume` line.)

   Update the inline comment on `false` to `false (was true on cold path before #82 fix)` is unnecessary — the new comment above is enough.

#### 4.5.a Ordering note (re-entry safety)

`destroy_session_inner` is invoked synchronously inside the `RespawnExited` branch *before* the spawn-fallback runs. After destroy, `find_active_session` would return `None` if called again. **Do not** re-query `find_active_session` after destroy — the `had_prior_session` flag set at the start of the function captures the pre-destroy state, which is what we want.

### 4.6 `src-tauri/src/lib.rs` line 594 — restore call site (no behavior change)

**Before** (line 594):
```rust
                            false, // skip_auto_resume
```

**After** — comment-only update for clarity:
```rust
                            false, // skip_auto_resume = false → restore path; allow `--continue`
```

This call site is the **only** sanctioned `false` in the codebase after this fix. Calling out the intent explicitly will keep future contributors from copy-pasting it into a fresh-create context.

### 4.7 `src-tauri/src/commands/session.rs:255-261` — `create_session_inner` doc comment

**Before** (lines 255-262):
```rust
/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (`claude --continue`, `codex resume --last`) when appropriate.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
/// If `skip_auto_resume` is true, suppresses provider-specific auto-resume injection (used by
/// restart_session to ensure a fresh conversation even when prior history exists).
pub async fn create_session_inner(
```

**After** — replace the `skip_auto_resume` paragraph with a more discriminating description:
```rust
/// Core session creation logic shared by the Tauri command and the restore path.
/// Creates a session record, spawns a PTY, and emits the session_created event.
/// Auto-detects agent from shell command if not provided, and auto-injects provider-specific
/// resume flags (`claude --continue`, `codex resume --last`, `gemini --resume latest`)
/// when appropriate.
/// If `skip_tooling_save` is true, skips writing to the repo's config.json (for temp sessions).
///
/// `skip_auto_resume` controls provider auto-resume injection:
/// - `true` (default for fresh creates): suppress all provider auto-resume. Use this whenever
///   the call represents a "new" session — UI/CLI/root-agent create, mailbox wake-from-cold,
///   `restart_session` with default semantics. The `~/.claude/projects/` dir is NOT a
///   reliable signal of a resumable conversation in these contexts (issue #82).
/// - `false`: allow provider auto-resume. Use ONLY for paths that are restoring a session
///   AC already knows about: the startup-restore loop in `lib.rs`, the wake-from-known-state
///   branch in `mailbox::deliver_wake`, and `restart_session` when its caller passes
///   `Some(false)` (the deferred-coordinator wake path).
pub async fn create_session_inner(
```

### 4.8 Extract `should_inject_continue` for unit testability

The current auto-inject block at `commands/session.rs:344-382` is gated by three runtime conditions and one filesystem check, making it un-testable end-to-end without mocking `dirs::home_dir()`. Extract the **pure decision** into a free function so it can be unit-tested independently of the filesystem call.

**Insert before line 262** (before `pub async fn create_session_inner`):

```rust
/// Decide whether to auto-inject `--continue` for a Claude session.
/// Pure function: no filesystem access. Caller is responsible for resolving
/// `claude_project_exists` (typically `~/.claude/projects/<mangled-cwd>/.is_dir()`).
///
/// Returns `true` only when ALL of:
///   - the session is a Claude variant
///   - the caller has not requested skip
///   - the projects dir exists on disk
///   - the configured argv does not already contain `--continue` / `-c`
fn should_inject_continue(
    is_claude: bool,
    skip_auto_resume: bool,
    claude_project_exists: bool,
    full_cmd: &str,
) -> bool {
    if !is_claude || skip_auto_resume || !claude_project_exists {
        return false;
    }
    let already_has_continue = full_cmd.split_whitespace().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower == "-c"
    });
    !already_has_continue
}
```

**Replace** the body of the auto-inject block at lines 344-382 to call the helper:

```rust
    // Auto-inject --continue for Claude agents when AC has reason to believe a prior
    // conversation exists for this session (issue #82: `is_dir()` alone is unsound).
    let claude_project_exists = {
        if let Some(home) = dirs::home_dir() {
            let mangled = crate::session::session::mangle_cwd_for_claude(&cwd);
            home.join(".claude")
                .join("projects")
                .join(&mangled)
                .is_dir()
        } else {
            false
        }
    };
    if should_inject_continue(is_claude, skip_auto_resume, claude_project_exists, &full_cmd) {
        if let Some(ref aid) = agent_id {
            if executable_basename(&shell) == "cmd" {
                if let Some(last) = shell_args.last_mut() {
                    if executable_basename(last) == "claude"
                        || last.to_lowercase().contains("claude")
                    {
                        *last = format!("{} --continue", last);
                        log::info!("Auto-injected --continue for agent '{}' (prior conversation exists, cmd path)", aid);
                    }
                }
            } else {
                shell_args.push("--continue".to_string());
                log::info!(
                    "Auto-injected --continue for agent '{}' (prior conversation exists)",
                    aid
                );
            }
        }
    }
```

This is a refactor with **zero behavior change**: same conditions, same log lines, same writes to `shell_args`.

---

## 5. Behavior matrix — six call sites

| # | Call site | `skip_auto_resume` (before) | `skip_auto_resume` (after) | Why |
|---|---|---|---|---|
| 1 | `web/commands.rs:80` (CLI `create_session`) | `false` | **`true`** | Fresh user-initiated create. No prior AC state. Bug today. |
| 2 | `commands/session.rs:633` (Tauri `create_session`) | `false` | **`true`** | Fresh user-initiated create from sidebar UI. Bug today. |
| 3 | `commands/session.rs:1216` (`create_root_agent_session`) | `false` | **`true`** | Fresh root-agent create. Path-reuse already short-circuits at line 1154; this call only fires for true creates. |
| 4 | `phone/mailbox.rs:638` (`deliver_wake` spawn-fallback) | `false` | **`!had_prior_session`** | Two sub-cases. Wake-from-known-state (Exited record matched): `false`, allow `--continue`. Wake-from-cold (no match): `true`. |
| 5 | `phone/mailbox.rs:1801` (`process_session_requests`) | `false` | **`true`** | CLI `create-agent` flow drops a `SessionRequest` JSON. Fresh create. |
| 6 | `lib.rs:594` (startup restore loop) | `false` | `false` (unchanged) | Restoring a `PersistedSession` from `sessions.toml`. The intended path for `--continue`. Comment updated for posterity. |
| (Aux) | `commands/session.rs:844` (`restart_session`) | `effective_restart_skip_auto_resume(...)` | unchanged | Already correct: defaults to `true` (fresh restart), opts in to `false` only via explicit `Some(false)` from the deferred-wake UI path. |

The `restart_session` row is included for completeness — no change.

---

## 6. Interaction with related issues

### 6.1 Issue #40 — restored sessions do not resume previous Claude Code context

**Will this fix make #40 worse? No.**

`#40` is observed on the `lib.rs:594` path: a session is persisted, app restarts, the session is recreated, `--continue` is injected, but Claude does not actually pick up the prior conversation. Whatever the root cause inside Claude (jsonl binding, recent-conversation cursor, etc.), it is **gated on `--continue` actually being passed** — which is exactly the input this plan preserves at `lib.rs:594`. No regression possible.

**Does this fix help close #40?** Tangentially. After this plan, if a user reports `No conversation found to continue`, the failure surface is narrower:
- If it appears on a fresh WG / fresh session: this fix resolves it (the bug it was — #82).
- If it still appears on app-restart restore: it is genuinely #40, with a clear repro (only the restore path injects `--continue`).

That separation is valuable for whoever picks up #40 next.

### 6.2 Issue #55 — TTL-based filter on stale auto-resume

**Compatible.** #55 proposes a freshness gate (don't auto-resume sessions older than N hours). After this plan, the auto-inject block is the natural home for that gate: it runs only on the restore + wake-from-known-state surfaces, exactly where staleness applies. The `should_inject_continue` helper extracted in §4.8 gives #55 a single-function home for the new condition. **No conflict, modest setup**.

### 6.3 Issue #65 — StatusBar misses dynamic spawn flags

Out of scope. #65 is a display issue (StatusBar reads configured args from `sessions.toml` instead of effective args). Independent codepath, independent fix.

---

## 7. Out-of-scope (deliberately untouched)

1. **Codex auto-resume** at `commands/session.rs:384-390` and **Gemini auto-resume** at `commands/session.rs:392-398`. These also gate on `!skip_auto_resume`, so they ride along with the `false → true` flips at the four fresh-create call sites and inherit the same correctness improvement for free. **No code change** in those blocks. Tech-lead's brief asked to fix only the Claude path; the gating bool happens to be shared, so the fresh-create call sites also stop injecting codex/gemini resume — which is **the same desired behavior** by symmetry. Flag this for dev-rust to confirm.
2. **`mangle_cwd_for_claude` and the JSONL watcher.** The mangling is shared with `pty/jsonl_watcher.rs`; changing its semantics would have ripple effects far beyond #82. The fix doesn't need to touch it.
3. **`~/.claude/projects/` cleanup.** AC does not own that directory and should not delete from it. The "ghost dir" is a feature of Claude Code's data model; we work with it, not against it.
4. **Time-based / mtime / cwd-match heuristic.** Belongs in #55, not here.
5. **`skip_auto_resume` rename or enum migration.** Style improvement, not a bugfix.
6. **`failed_recoverable` wake handling.** If a `PersistedSession` failed to restore at startup (lib.rs:602) and a peer later wakes that agent, `find_active_session` will not match (no SessionManager record) and we'll treat it as wake-from-cold. The user loses `--continue` for that one wake. Trade-off is acceptable: failed_recoverable is rare, transient, and a fresh session is recoverable on the user's next interaction. Documented as an open question (§9).
7. **Frontend changes.** None needed. `skip_auto_resume` is backend-only; `restart_session` already has a typed frontend caller for the deferred-wake `Some(false)` opt-in.

---

## 8. Test plan

### 8.1 Unit tests (`src-tauri/src/commands/session.rs`)

Add to the existing `mod tests` (lines 1262+):

1. **`should_inject_continue_returns_false_when_not_claude`** — `is_claude=false`, all else `true` → `false`.
2. **`should_inject_continue_returns_false_when_skip_requested`** — `skip_auto_resume=true`, all else permissive → `false`.
3. **`should_inject_continue_returns_false_when_dir_missing`** — `claude_project_exists=false` → `false`.
4. **`should_inject_continue_returns_false_when_continue_already_in_full_cmd`** — full_cmd contains `--continue` → `false`. Cover both lowercase and uppercase, and the short form `-c`.
5. **`should_inject_continue_returns_true_for_canonical_resume_case`** — `is_claude=true`, `skip_auto_resume=false`, `claude_project_exists=true`, no continue in argv → `true`.

### 8.2 Wake path test (mailbox.rs)

The wake spawn-fallback can be unit-tested by extracting the boolean computation into a one-liner — but `had_prior_session` is just `find_active_session(...).is_some()`. A full unit test would need a `SessionManager` fixture; this is consistent with how the rest of `mailbox.rs` is tested today (mostly via the `wake_action_for` style of pure helpers).

If dev-rust judges the wiring trivial enough, leave it covered by the manual repro in §8.4. Otherwise extract a thin helper:

```rust
fn wake_spawn_skip_auto_resume(had_prior_session: bool) -> bool {
    !had_prior_session
}
```
…with two trivial assertions. The 3-line indirection is barely justifiable; dev-rust's call.

### 8.3 Restart path tests (already present)

Existing tests at `commands/session.rs:1530-1550` cover `effective_restart_skip_auto_resume`. No additions needed.

### 8.4 Manual repro

Run all four scenarios after build. Tester is the user (the `repo-` workspace is on Windows; ConPTY is the live target).

#### A. Fresh-WG repro (the bug)

1. Confirm `~/.claude/projects/C--Users-maria-0-repos-agentscommander--ac-new-wg-1-dev-team---agent-tech-lead/` exists on disk (it does, today, from a prior lifetime).
2. Tear down WG-1 and recreate it (`agentscommander_mb` workgroup tooling — the user knows the exact CLI).
3. Spawn the tech-lead agent's Claude session.
4. **Expected**: terminal shows the Claude welcome / fresh prompt. **No** `No conversation found to continue` error. The effective argv (visible via StatusBar after #65, or via logs `[session] FINAL resolved: ...`) does **not** contain `--continue`.

#### B. App-restart restore (regression check for #40)

1. Open AC, spawn at least one Claude session, let Claude attach to a real conversation, type one line and let it respond (so a `.jsonl` exists with content).
2. Quit AC normally.
3. Re-launch AC.
4. **Expected**: the restored session re-attaches via `--continue`, conversation is visible. (If Claude rejects the resume, that's #40 — log the symptom and route there; not a regression of #82.)

#### C. Wake-from-deferred (startOnlyCoordinators=true)

1. Set `startOnlyCoordinators=true` in settings.
2. Restart AC. Confirm non-coordinator sessions are listed but `Exited(0)`.
3. From a coordinator agent, send a wake message (`agentscommander_mb send … --mode wake`) to a non-coord.
4. **Expected**: non-coord session re-spawns. If the non-coord had a prior `.jsonl`, `--continue` should be in argv. Conversation resumes.

#### D. Wake-from-cold

1. Choose an agent FQN that has no entry in `sessions.toml` and no live session (e.g., a brand-new agent that was added to a team after the last AC startup).
2. Send a wake message to that FQN.
3. **Expected**: session spawns fresh; `--continue` is **not** in argv; no `No conversation found to continue` even if a ghost `~/.claude/projects/` dir for that path exists.

#### E. Side-effect check on Codex/Gemini fresh creates (§7 #1)

1. With Codex configured as the agent, spawn a fresh session via the UI button.
2. **Expected**: argv does **not** contain `resume --last`. Codex starts a fresh session.
3. Same for Gemini → no `--resume latest` in argv on fresh create.

The change is "free" (same `!skip_auto_resume` gate), but tester should confirm it matches the desired behavior. If the user wants codex/gemini fresh-create to *keep* injecting resume (i.e. their previous behavior was actually correct for them), that's a follow-up — not a #82 regression.

---

## 9. Open questions for dev-rust / grinch

1. **Codex/Gemini side-effect (§7 #1).** The four fresh-create call sites currently inject codex `resume --last` / gemini `--resume latest` whenever the underlying agent runtime supports it, with no equivalent `is_dir()`-style gate. Flipping to `skip_auto_resume=true` at fresh-create call sites also suppresses those. Is that the desired semantic? The user explicitly said #82 only impacts Claude, but symmetry makes it likely codex/gemini also benefited the wrong way. Confirm intent before merging.

2. **Wake-from-cold and `failed_recoverable` (§7 #6).** When startup restore fails for a session and that agent is later woken, this plan treats the wake as "cold" (no `--continue`). Acceptable, or should mailbox also consult `sessions.toml` (or a SessionManager "recently-failed" set) before deciding? If yes, this is a non-trivial scope expansion — propose deferring to a follow-up issue.

   **Status (rounds 2-3 update per grinch G3/G2.1 / dev-rust E3+F1):** This question is moot for the spawn-fallback path. PTY-spawn failure inside `create_session_inner` (lines 457-461) does not call `mgr.mark_exited`, so an orphan `SessionManager` record from a `failed_recoverable` lifecycle stays at status `Running`, not `Exited(_)`. `wake_action_for(Running) = Inject`, which routes through `inject_into_pty` (line 553) and never reaches the spawn-fallback where `spawn_with_resume` is read. Therefore the failed_recoverable case neither (a) erroneously injects `--continue` via the spawn-fallback nor (b) needs the additional `sessions.toml` lookup originally proposed here. The orphan-record problem (inject into a dead PTY) is a pre-existing latent bug tracked separately as §7 #8; the in-place teardown problem (silent wake delivery failure) is tracked as §7 #10 (added in R3.5). Neither lifecycle is in #82's scope.

3. **`should_inject_continue` location.** §4.8 puts it as a free function in `commands/session.rs`. Alternative: move to `pty/inject.rs` next to other inject helpers. Pure function, low blast radius either way — dev-rust's call.

4. **Test extraction (§8.2).** Worth the 3-line `wake_spawn_skip_auto_resume` helper, or just rely on §8.4.D manual repro for that branch?

5. **Comment churn.** §4.6 (lib.rs:594) and §4.7 (`create_session_inner` doc) are comment-only. Acceptable in this PR or split out? My read: keep them, the value is durable rationale at the only `false` call site after the fix.

6. **Branch state.** Tech-lead reports the branch was created clean from `origin/main`. No commits yet on `fix/82-no-continue-on-fresh-session` at the time this plan was written. If grinch's review adds new requirements, they should land as additional commits on the same branch — not a re-base.

---

## 10. Done state

- Six callsite values reflect the matrix in §5.
- `should_inject_continue` extracted; existing auto-inject block calls it.
- New unit tests added in `commands/session.rs` for `should_inject_continue`.
- `mailbox::deliver_wake` carries `had_prior_session` through to the spawn-fallback.
- Comment updates at `commands/session.rs:255-261` and `lib.rs:594` reflect the new policy.
- Manual repros A–E pass.
- Branch `fix/82-no-continue-on-fresh-session` is ready for review / testing.

The plan stops here.

---

## Dev-rust enrichment (round 1)

**Reviewer:** dev-rust (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Verified against:** branch tip = `origin/main` @ `96860c0`, no commits on `fix/82-no-continue-on-fresh-session` yet.

### D1. Line-number / path verification

All seven `create_session_inner` call sites cross-referenced against the current branch tip. Result: every line number in the plan matches the source. Specifically:

| Plan reference | Verified at | Status |
|---|---|---|
| `web/commands.rs:80` (`false, // skip_auto_resume`) | line 80 | ✅ exact |
| `commands/session.rs:633` (Tauri `create_session`) | line 633 | ✅ exact |
| `commands/session.rs:1216` (`create_root_agent_session`) | line 1216 | ✅ exact |
| `commands/session.rs:255-262` (doc comment) | lines 255-261 | ✅ exact (trailing line is the `pub async fn` at 262) |
| `commands/session.rs:344-382` (auto-inject block) | lines 344-382 | ✅ exact |
| `commands/session.rs:384-398` (codex/gemini blocks) | lines 384-398 | ✅ exact |
| `commands/session.rs:844` (`restart_session` aux row) | line 844 | ✅ exact |
| `lib.rs:594` (restore loop) | line 594 | ✅ exact |
| `mailbox.rs:535-641` (`deliver_wake`) | lines 530-641 | ✅ exact (function signature opens at 530, body at 535) |
| `mailbox.rs:1801` (`process_session_requests`) | line 1801 | ✅ exact |
| `commands/session.rs:1530-1550` (existing tests) | lines 1530-1550 | ✅ exact (test mod ends at 1551) |
| TS `RestartSessionOptions.skipAutoResume` | `src/shared/ipc.ts:35-45` | ✅ already plumbed; no FE work |
| `inject_codex_resume`, `inject_gemini_resume` (no `is_dir()` gate) | lines 200-252, 149-198 | ✅ confirmed shared `!skip_auto_resume` gate (relevant for §9.1) |

No mismatches. Dev can apply all line numbers 1:1.

I also enumerated **all** `create_session_inner` callers in `src-tauri/src/`:
1. `web/commands.rs:68` (CLI) — covered §4.1
2. `lib.rs:582` (restore) — covered §4.6
3. `mailbox.rs:626` (wake spawn) — covered §4.5
4. `mailbox.rs:1789` (session-request) — covered §4.4
5. `commands/session.rs:621` (Tauri `create_session`) — covered §4.2
6. `commands/session.rs:832` (`restart_session`) — covered §5 aux
7. `commands/session.rs:1204` (root agent) — covered §4.3

Seven call sites; all accounted for in §5. ✅

### D2. Clarifications on §4.5 (the `had_prior_session` carry-through)

The text "Insert at line 534, immediately after the function signature opens" reads slightly ambiguously. The current source layout is:

```
530: async fn deliver_wake(
531:     &self,
532:     app: &tauri::AppHandle,
533:     msg: &OutboxMessage,
534: ) -> Result<(), String> {
535:     if let Some(session_id) = self.find_active_session(app, &msg.to).await {
```

The new `let mut had_prior_session = false;` (with its leading comment) goes **between** lines 534 and 535 — i.e., it becomes the first body statement, pushing the existing line 535 down. Phrase as: "Insert between the opening `{` of the function (current line 534) and the first body statement (current line 535)."

Behavior of `find_active_session` worth flagging for grinch:

- `find_active_session` returns `Some(Uuid)` even when **all matching sessions are `Exited`** — see `mailbox.rs:1056-1067`. It sorts by status (Active < Idle < Exited) and returns the best match unconditionally if any match exists.
- This means `had_prior_session` is `true` for the `RespawnExited` branch (correct: there IS a prior conversation we want to resume).
- It is also `true` for the `Some(s) if list_sessions doesn't contain it` race-fallthrough at lines 575-581 (which falls into the spawn-fallback). That's also correct: AC believed a session existed at this CWD a moment ago; treating it as wake-from-known-state is the desired bias.
- It is `false` only when zero sessions match the FQN — true wake-from-cold. ✅

The §4.5.a ordering note is correct as written. No changes needed.

### D3. Codex/Gemini side-effect (§7 #1, §9.1) — strengthening the case for symmetric

The plan acknowledges the symmetric side-effect but soft-pedals it ("rides along ... the same correctness improvement automatically"). I'd argue the symmetric fix is **more** correct for codex/gemini than for Claude, and the asymmetric scope is the wrong default. Reasoning:

1. **Claude's `--continue` is CWD-scoped.** Claude binds to a `~/.claude/projects/<mangled-cwd>/` directory; the failure mode is a user-visible error (`No conversation found to continue`) but at least the *scope* of the failed resume is "this CWD."

2. **Codex's `resume --last` is GLOBAL.** Reading `inject_codex_resume` (`commands/session.rs:200-252`) and the codex CLI semantics: `resume --last` attaches to whatever the codex CLI's own "most recently used conversation" pointer says. That pointer is **not CWD-scoped**; it survives across terminal sessions, across CWDs, across AC restarts. A fresh-create call site injecting `resume --last` can therefore attach a brand-new AC session to a totally unrelated conversation from any other shell the user ran codex in.

3. **Gemini's `--resume latest` is similar.** Same global-pointer model.

4. **No `is_dir()` guard exists for codex/gemini today.** Look at the auto-inject blocks at `commands/session.rs:384-398`: the only gate is `!skip_auto_resume`. There is no equivalent of the Claude projects-dir check. So today, every fresh CLI/UI/root-agent codex/gemini create injects resume — a latent footgun the symmetric fix closes.

5. **The opt-in path for users who really want codex/gemini fresh-create resume is already there.** Configure the agent command as `codex resume --last` directly. Then `inject_codex_resume` correctly returns `false` (already-present check at line 204) and the user-explicit choice is preserved.

**My stance on §9.1:** strongly recommend symmetric. The architect's framing already leans this way; my recommendation is to commit to it without per-provider gating even if the user's first reaction is "Claude only." If the user insists on Claude-only, the cleanest fallback is to add an orthogonal `is_fresh_create: bool` parameter to `create_session_inner` and gate ONLY the Claude block on `!is_fresh_create && !skip_auto_resume`. But that's two flags doing nearly the same thing — anti-pattern. Push back first.

### D4. Edge cases the test plan misses

Add to §8.1:

6. **`should_inject_continue_returns_false_when_uppercase_continue_present`** — `full_cmd = "claude --CONTINUE"` → `false`. Verifies the `.to_lowercase()` branch.
7. **`should_inject_continue_returns_false_when_short_form_present`** — `full_cmd = "claude -c"` → `false`. Verifies the `-c` branch.
8. **`should_inject_continue_returns_false_when_continue_embedded_in_cmd_wrapper`** — `full_cmd = "cmd /C claude --continue"` → `false`. Verifies that token-level scan, not arg-index scan, is what we want (matches the existing behavior the helper extracts from).

Note for tests 6-8: the existing block at `commands/session.rs:359-362` already does these via `full_cmd.split_whitespace()` + `.to_lowercase()`. The tests are not asking for new behavior; they are asking for a **regression fence** so that anyone refactoring the helper later does not silently lose case-insensitivity or short-form detection.

Also add to §8.1:

9. **`should_inject_continue_returns_true_when_full_cmd_has_unrelated_continue_substring`** — `full_cmd = "claude --continued-mode something"` → `true`. (i.e., we match on whole tokens, not substrings.) The `==` comparison in the helper makes this trivially true; the test ensures we don't accidentally introduce a `.contains("--continue")` regression. Edge-case-y but the cost is one assertion.

### D5. Wake-path test extraction (§9.4) — recommend including the helper

My vote: include the 3-line helper. Rationale:

- The cost is trivial (3 lines + 2 assertions).
- It pins the semantic to a unit test: anyone who later "simplifies" `!had_prior_session` to `had_prior_session` (an easy off-by-one) will fail a test instead of regressing the bug we're fixing.
- It documents intent at a scannable layer (helper name + doc) rather than embedding the inversion in a `create_session_inner` call argument.

Concrete addition for §8.2:

```rust
/// `skip_auto_resume` value used by `deliver_wake`'s spawn-fallback.
/// `true` (skip resume) on cold wake — no SessionManager record at this CWD;
/// `false` (allow resume) when a prior session matched, including Exited ones
/// that triggered RespawnExited (the prior conversation may exist on disk).
fn wake_spawn_skip_auto_resume(had_prior_session: bool) -> bool {
    !had_prior_session
}
```

Tests (place in `mailbox.rs`'s existing `mod tests` if it exists, otherwise add one):

```rust
#[test]
fn wake_spawn_skip_auto_resume_is_true_on_cold() {
    assert!(super::wake_spawn_skip_auto_resume(false));
}

#[test]
fn wake_spawn_skip_auto_resume_is_false_when_prior_matched() {
    assert!(!super::wake_spawn_skip_auto_resume(true));
}
```

Then `deliver_wake` calls `wake_spawn_skip_auto_resume(had_prior_session)` instead of `!had_prior_session` inline. Cost: 5 lines of code + 6 lines of test. Net benefit: durable test fence on the inversion.

### D6. `should_inject_continue` location (§9.3)

**Keep it in `commands/session.rs`.** Reasoning:

- The function is consumed exactly once, by the auto-inject block in `create_session_inner` (same file).
- `pty/inject.rs` is about post-spawn PTY-level text injection (writing bytes to stdin after the session is up). `should_inject_continue` is about pre-spawn argv assembly (deciding what to put into shell_args before the PTY is even spawned). Different layer, different concern.
- Co-location reduces the cognitive cost of reading the auto-inject block: helper + caller in the same file = one scroll.

### D7. Test imports update — easy-to-miss detail

§8.1 says "add to existing `mod tests`". The current import line at `commands/session.rs:1264` is:

```rust
use super::{inject_codex_resume, resolve_actual_agent};
```

After the fix, it must be:

```rust
use super::{inject_codex_resume, resolve_actual_agent, should_inject_continue};
```

Easy to forget — flagging explicitly.

### D8. Doc comment wording nit (§4.7)

The new doc reads:

> `skip_auto_resume` controls provider auto-resume injection:
> - `true` (default for fresh creates): suppress all provider auto-resume.

There's no "default" mechanism — every caller passes a value explicitly. Suggest tightening to:

> - `true` — suppress all provider auto-resume. **Use this for any "fresh create" call site** (UI/CLI/root-agent create, mailbox wake-from-cold, `restart_session` with default semantics from `effective_restart_skip_auto_resume`).
> - `false` — allow provider auto-resume. **Use this only for paths restoring a session AC already knows about** (the startup-restore loop in `lib.rs`, the wake-from-known-state branch in `mailbox::deliver_wake`, and `restart_session` when its caller passes `Some(false)`).

Wording-only; same semantic.

### D9. Add a debug log on the wake-path branch decision

Inside the `deliver_wake` spawn-fallback, just before the `create_session_inner` call (around current line 626), add:

```rust
log::debug!(
    "[mailbox] wake-spawn: had_prior_session={}, skip_auto_resume={}",
    had_prior_session, !had_prior_session
);
```

Cost: 4 lines. Benefit: when a future user reports "wake didn't continue my conversation" or "wake spuriously injected --continue", the answer is already in the log. Without it, the only signal is the `Auto-injected --continue` info-level log fired downstream — which is too far from the decision point to attribute.

This is consistent with the existing logging style in `deliver_wake` (already several `log::info!` and `log::warn!` calls around the wake decision).

### D10. Pre-existing user-config edge case (out-of-scope but worth noting)

A user can configure their agent command directly as `claude --continue` in settings. After this fix:

- Fresh create: `skip_auto_resume = true` → `should_inject_continue` returns `false` (skip suppressed) → BUT shell_args already has `--continue` from user config → Claude still receives `--continue` → fails on a fresh CWD with `No conversation found to continue`.

The fix doesn't help this case (and isn't trying to). It's user-error, not a bug. Documented here so we don't accidentally "fix" it later by stripping user-supplied `--continue` — that would be a feature, not a bugfix, and would surprise users who deliberately configured it.

Out of scope for #82. No action needed; flagging for grinch in case they raise it.

### D11. Comment churn (§9.5)

**Keep it.** §4.6 (lib.rs:594) and §4.7 (`create_session_inner` doc) are exactly the kind of comment updates that pay for themselves the next time someone copy-pastes the line. Trivial diff. Bundle in this PR.

### D12. Branch state confirmation

Verified: branch `fix/82-no-continue-on-fresh-session` is up to date with `origin/main`, zero commits ahead, plan file is the only untracked path. No rebase concerns. ✅

### D13. Summary of dev-rust stances on §9 open questions

| Q# | Question | Stance |
|---|---|---|
| 9.1 | Codex/Gemini side-effect — keep symmetric or scope to Claude? | **Symmetric.** Stronger than the architect's lean — see D3. Per-provider gating would be an anti-pattern. |
| 9.2 | `failed_recoverable` wake handling — acceptable trade-off? | **Acceptable.** Defer to a follow-up issue if user complains. Scope expansion not justified by frequency. |
| 9.3 | `should_inject_continue` location — `commands/session.rs` or `pty/inject.rs`? | **`commands/session.rs`.** See D6. |
| 9.4 | Worth a `wake_spawn_skip_auto_resume` helper? | **Yes.** See D5. 3 lines of code, durable inversion test. |
| 9.5 | Comment churn (§4.6, §4.7) — keep or split? | **Keep in this PR.** See D11. |
| 9.6 | Branch state — clean? | **Confirmed clean.** See D12. |

### D14. Items that need architect input (not dev-rust/grinch alone)

- **§9.1 user go/no-go on symmetric vs. Claude-only.** Tech-lead has flagged with user. We block on the answer for the doc-comment wording in §4.7 (which currently reads as if codex/gemini are also covered). Until then, all enrichments above assume symmetric.

### D15. Items dev-rust + grinch can resolve directly

- D2 wording (between lines 534/535).
- D3 codex/gemini side-effect framing in §7 / §9.1 (inside the symmetric scope).
- D4 additional unit tests.
- D5 helper extraction for the wake path.
- D6 helper location.
- D7 test imports.
- D8 doc comment wording.
- D9 debug log.
- D10 user-config edge case (no-action documentation).
- D11 comment churn.

End of dev-rust enrichment.

---

## Grinch adversarial review (round 1)

**Reviewer:** dev-rust-grinch (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Verified against:** branch tip = `origin/main` @ `96860c0`, no commits on `fix/82-no-continue-on-fresh-session` yet.
**Scope assumption (per tech-lead brief):** symmetric (codex/gemini follow same fresh/restore split as Claude) until user decides §9.1.

I tried to break this plan. I read the seven `create_session_inner` call sites, the `find_active_session` ranking logic, the existing `strip_auto_injected_args`, the `MailboxPoller::poll`/`poll_session_requests` driver, and the auto-inject block at `commands/session.rs:344-398`. I cross-referenced architect §1-§10 and dev-rust D1-D15.

The plan's core logic is sound and I confirm the seven-caller enumeration. The §6.1 #40 non-regression claim holds (the `lib.rs:594` input is unchanged). The §4.5.a ordering reasoning is correct given the structure of `deliver_wake`. But I have findings the plan should answer before implementation.

### G1. (MEDIUM) In-place WG teardown without AC restart re-triggers #82 via the wake-Exited path

**What.** §4.5 sets `had_prior_session = true` whenever `find_active_session` matches *any* session record at the FQN's CWD, including `Exited(non-zero)` records. There is a real lifecycle the plan does not address: user tears down a WG dir while AC stays running, then recreates it at the same path, then a peer sends a wake to that agent.

**Why.**
- Pre-teardown: live Claude session for agent-X. `~/.claude/projects/<mangled-cwd-X>/` is real.
- Teardown (filesystem `rm -rf` of WG dir, AC stays running): the PTY dies because cwd vanishes. The shell process exits with a non-zero code (ENOENT-class, OS-dependent). `mark_exited` records the code. The session record stays in `SessionManager`.
- Recreate WG dir at same path: new agent identity at same FQN. `mangle_cwd_for_claude` yields the same projects dir name. The pre-existing `.jsonl` files in `~/.claude/projects/<mangled-cwd-X>/` are now stale relative to the new WG.
- Wake arrives for FQN-X. `find_active_session` matches the stale `Exited(non-zero)` record (lines 1056-1067 sort returns it). `wake_action_for` → `RespawnExited`. `destroy_session_inner` runs. Spawn-fallback fires with `had_prior_session = true` → `skip_auto_resume = false` → `should_inject_continue` returns `true` (is_claude=true, projects-dir is_dir=true, no continue in argv). **`--continue` is injected, Claude rejects with `No conversation found to continue` — the exact bug #82.**

The user's reported repro (fresh WG without prior AC state) is correctly fixed by §4.1/§4.2/§4.4/§4.5/§5 matrix. But the in-place-teardown variant — which the user could trivially hit while iterating on WG layouts — silently regresses through the wake spawn-fallback. The plan's §7 #6 only addresses `failed_recoverable` (startup restore), not in-place teardown.

**Why this is not catastrophically wrong:** the deferred-non-coord wake path *also* matches `Exited(0)` and *does* want `--continue`. So we cannot just say "Exited match ⇒ wake-from-cold". The signals overlap.

**Fix.** One of:
1. **Cheap, partial:** treat only `Exited(0)` (deferred-non-coord clean state) as "known state"; treat `Exited(non-zero)` as "cold" — promote `had_prior_session` from `bool` to `enum {Cold, KnownState}` and decide based on status code. Catches the in-place-teardown case (PTY-died-from-vanished-cwd is non-zero) without regressing deferred-non-coord. Trade-off: a session that exited non-zero for a *recoverable* reason (e.g., agent crash, user typing `exit 1`) loses `--continue` on next wake. Probably acceptable.
2. **Acknowledge as known limitation:** add a §9 entry "in-place WG teardown without AC restart will re-inject `--continue` against the ghost projects dir; deferred to a follow-up issue (require AC restart between teardown and recreate)." Lower implementation cost; user-visible foot-gun.
3. **Defer with the architect:** punt the decision (1 vs 2) to user/architect.

I do not consider this HIGH because the plan does close the most common repro. But it is observable and the user has explicitly hit this lifecycle. Pick option 1 or 2 before merging.

### G2. (MEDIUM) `should_inject_continue` does not catch `--continue=value` form

**What.** §4.8's helper checks `lower == "--continue"`. GNU long-option convention allows `--continue=value`. The token `--continue=foo` does not equal `--continue`, so `already_has_continue` is `false` and the helper re-injects `--continue`. Resulting argv: `claude --continue=foo --continue` — at minimum noisy, at worst Claude flags it as conflicting.

**Why.** This is pre-existing (the original `commands/session.rs:359-362` block has the same gap), so it is not a regression. But §4.8 is the natural seam to fix it: a one-line predicate change costs nothing. Tech-lead's pressure point #2 explicitly asked about `--continue=value`. Punting it now means re-opening this file later for the same edit.

**Fix.** Change the helper predicate to:
```rust
let already_has_continue = full_cmd.split_whitespace().any(|t| {
    let lower = t.to_lowercase();
    lower == "--continue" || lower.starts_with("--continue=") || lower == "-c"
});
```
And add to §8.1:
- `should_inject_continue_returns_false_when_continue_with_value_present` — `full_cmd = "claude --continue=foo"` → `false`.

If the team prefers to keep §82 surgical, document the gap in §9 and bundle the fix with #55. Either way, do not silently inherit it.

### G3. (LOW) §7 #6 / §9.2 `failed_recoverable` analysis is incomplete — orphan SessionManager records can exist after restore failure

**What.** Plan §7 #6 claims: "If a `PersistedSession` failed to restore at startup (lib.rs:602) and a peer later wakes that agent, `find_active_session` will not match (no SessionManager record)." This is only true when restoration fails *before* `mgr.create_session` (line 293-305 in `create_session_inner`). The PTY-spawn step at `commands/session.rs:457-461` is **after** the SessionManager record is created and has **no cleanup on failure** (unlike the context-materialization path at lines 401-419 which explicitly calls `destroy_session(id)`).

**Why.** When PTY spawn fails during restore (shell binary missing, ConPTY failure, etc.), the session record stays in SessionManager as an orphan. `find_active_session` matches → `had_prior_session = true` → `--continue` is injected on subsequent wake. The plan's analysis says "wake-from-cold (no `--continue`)"; reality may be wake-from-known-state (`--continue` injected).

This is a **pre-existing bug in `create_session_inner`** (orphan records on PTY-spawn failure), not introduced by #82's fix. But the plan's §9.2 reasoning relies on the orphan not existing, which is sometimes false. The user's experience with this case will not match the plan's documented expectation.

**Fix.** Two options:
1. Update §9.2 to acknowledge: "the analysis above assumes no orphan SessionManager records. If `create_session_inner` fails after `mgr.create_session` but before returning Ok (e.g., PTY spawn failure), an orphan record persists; subsequent wake will treat it as known-state."
2. Track this as a follow-up to fix `create_session_inner`'s error-path cleanup. Not required for #82.

I would do (1) for honesty and let (2) be a separate cleanup.

### G4. (LOW) Test for "skip beats projects-dir" is implicit, not asserted

**What.** §8.1 test #2 (`should_inject_continue_returns_false_when_skip_requested`) sets `skip_auto_resume=true` and "all else permissive". The helper's early-return order is `if !is_claude || skip_auto_resume || !claude_project_exists`. As long as the test sets `is_claude=true` AND `claude_project_exists=true`, it locks in the early-return. Confirm the test fixture body sets both — the §8.1 prose says "all else permissive" which is ambiguous.

**Fix.** Make the test body explicit: name it `should_inject_continue_returns_false_when_skip_overrides_existing_dir`. Set `is_claude=true`, `claude_project_exists=true`, `skip_auto_resume=true`, `full_cmd="claude"`. Assert `false`. Otherwise a future refactor that re-orders the early-return clauses (e.g., to `!claude_project_exists || !is_claude || skip_auto_resume`) might still pass test #2 by accident if the fixture skipped one of the three conditions.

### G5. (LOW) `-c` shadowing in mixed-tool compound commands

**What.** The helper's "already has continue" check fires on `-c` as well as `--continue`. Codex uses `-c key=value` for config (`commands/session.rs:45-67`). If a user has a compound command `cmd /K codex -c model_reasoning_effort=high && claude`, then `is_claude` is `true` (claude basename is in cmd_basenames), `full_cmd` contains `-c` from codex's tokens, helper returns `false`, and `--continue` is **not** injected for Claude.

**Why.** Pre-existing in the original code (lines 359-362). Plan extracts the helper without changing semantics. Realistic? Probably not — most users do not chain codex and claude in one wrapper. But the plan extracts and tests this function, and the test plan's "regression fence" framing in D4 #6-#9 invites covering this too.

**Fix.** Either:
- Restrict `-c` detection to: only count it as `--continue`-shorthand when no codex/gemini basename precedes it in `cmd_basenames`. Adds non-trivial complexity.
- Document the interaction in the helper's doc comment. Cheap.

I would document and defer. Not a #82 blocker.

### G6. (LOW) §4.7 doc-comment line range is mislabeled

**What.** §4.7's heading says "lines 255-274". Read of the actual source: doc comment is lines 255-261, function signature opens at 262, signature body extends through 274 (closing `) -> Result<...> {`). Calling 255-274 the "doc comment" is misleading — the bottom half is the signature.

**Fix.** Reword to "§4.7 `commands/session.rs:255-261` — `create_session_inner` doc comment". Cosmetic; do not block.

### G7. (LOW) Future-refactor hazard on §4.5.a is documented but not test-fenced

**What.** §4.5.a's reasoning ("do not re-query `find_active_session` after destroy") is correct but lives only in plan prose. A future refactor that, e.g., adds `let had_prior_session = self.find_active_session(...).await.is_some()` at line 626 (post-destroy) would silently regress the RespawnExited path back to `had_prior_session = false` → `--continue` lost on deferred-non-coord wake. dev-rust D5's `wake_spawn_skip_auto_resume(bool)` helper test catches the inversion of the boolean but does **not** catch a re-query that flips the boolean's *source*.

**Fix.** Either:
- Add a comment at the proposed flag declaration site: `// MUST be set BEFORE destroy_session_inner runs — see plan §4.5.a. Re-querying find_active_session after destroy will return None for RespawnExited and silently regress #82.`
- Or write an integration-style test (heavy: needs a `MailboxPoller` fixture). Not justifiable for a 1-line invariant.

I would do the comment. Cheap, durable.

### G8. (informational) D3 codex/gemini "global resume" claim verified against AC code only

**What.** Dev-rust D3 asserts codex `resume --last` is global, not CWD-scoped. I read `inject_codex_resume` (`commands/session.rs:200-252`) and confirm AC's code does no CWD-scoping for codex/gemini. So D3's claim is *consistent with AC's code*. I cannot independently verify the codex CLI's own semantics from inside this repo (would require running `codex resume --last` and observing).

**Why.** The §9.1 user decision rests partly on this claim. If the user pushes back ("codex IS CWD-scoped"), a quick external verification (running `codex resume --last` in two distinct CWDs, observing whether they pick up the same conversation) would settle it.

**Fix.** None for the plan; flag for the user-decision conversation.

### G9. (informational) Concurrency surface — sequential mailbox poll mitigates wake-vs-wake races, but UI commands run on a separate task

**What.** `MailboxPoller::poll` (mailbox.rs:124) and `poll_session_requests` (line 1740, called from line 259) run on a single background tokio task — sequential. Within a poll cycle, `for path in entries` is sequential. So multiple wakes for the same FQN do not race against each other in the spawn-fallback. Good.

But Tauri commands (`create_session`, `destroy_session`, `restart_session`) run on separate tokio tasks. A user clicking "destroy" in the UI mid-wake could destroy a session between `find_active_session` (line 535) and the second `mgr.read().await` (line 537). Result: the inner `if let Some(s) = session` (line 541) goes to the `else` branch, and the function falls through to spawn-fallback with `had_prior_session = true`. The session vanished mid-flight; the spawn-fallback resurrects it with `--continue`.

**Why.** Pre-existing race. The plan's bias (treat `had_prior_session=true` even when the session vanished mid-call) means the user's destroy intent is partially overridden by a concurrent wake. Whether this is desired is a UX decision, not a #82 correctness issue. Worth noting in §7 as out-of-scope.

**Fix.** None for #82. Flag for awareness.

### G10. (informational) Verified all seven `create_session_inner` callers

Independent grep (`Grep`/grep-ripgrep equivalent) yields:
- `lib.rs:582` (restore) ✓
- `phone/mailbox.rs:626` (wake) ✓
- `phone/mailbox.rs:1789` (session-request) ✓
- `web/commands.rs:68` (CLI) ✓
- `commands/session.rs:621` (Tauri create_session) ✓
- `commands/session.rs:832` (restart_session via `effective_restart_skip_auto_resume`) ✓
- `commands/session.rs:1204` (root agent) ✓

Plus the definition at `commands/session.rs:262`. §5 matrix is complete.

### G11. (informational) §6.1 #40 non-regression claim verified

The only call site that injects `--continue` for a restored session is `lib.rs:594`, kept at `false`. After the plan, no other call site injects `--continue` on the restore semantic. The input to whatever Claude-internal binding issue causes #40 is unchanged. ✓ No regression possible.

### G12. (informational) §4.5.a ordering note verified

`deliver_wake` structure (mailbox.rs:530-641): if `find_active_session` returns `Some` (line 535), `had_prior_session` is set true *before* any branch. The two fall-through paths to spawn-fallback are (a) `WakeAction::RespawnExited` after `destroy_session_inner`, and (b) inner `else` when `list_sessions` does not find the session_id. Both paths preserve `had_prior_session=true` from the start of the function. ✓ Correct as written.

---

## Grinch Verdict: REQUEST CHANGES

Severity counts: **0 HIGH, 2 MEDIUM, 5 LOW (G3-G7), 5 INFORMATIONAL (G8-G12)**.

The plan correctly fixes the user's primary repro. The two MEDIUM findings are real:
- **G1** (in-place WG teardown via wake-Exited path) — the bug the user reported can re-surface through a slightly different lifecycle that the plan does not handle. Pick fix option 1 (Exited(0) vs Exited(non-zero) discrimination) or option 2 (document as known limitation) before implementation.
- **G2** (`--continue=value` form) — pre-existing gap, but §4.8 is the moment to fix it. One-line predicate change plus one test. Skipping it now is technical debt for a future #55-adjacent PR.

Does **not** invalidate prior sections. §5 matrix, §6.1 #40 analysis, and §4.5.a ordering are correct. Dev-rust D1-D15 are mostly accurate; D3's "global resume" claim cannot be independently verified inside AC code (G8) and §9.2 needs a small accuracy correction (G3).

**Items needing architect input (not dev-rust+grinch resolvable):**
- **G1 fix selection** (option 1 vs 2). Option 1 changes the data flow; option 2 changes the docs. Architect should choose the trade-off, since user-visible behavior differs.
- **G2 scope** — fix in this PR or defer to #55? Architect's call on PR scope.

**Items dev-rust + grinch can resolve directly:**
- G3 wording fix in §9.2.
- G4 explicit test fixture.
- G5 doc comment in helper.
- G6 line-range correction in §4.7 heading.
- G7 placement comment in §4.5.

End of grinch review.

---

## Architect round 2 (post-grinch)

**Reviewer:** architect (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Reading:** dev-rust D1–D15 (accepted in full unless flagged below) + grinch G1–G12.

### R2.1 G1 decision — option (a), Exited(0) vs Exited(non-zero) discrimination

**Pick: option (a).** Treat only `Exited(0)` as known-state in the wake-Exited branch; treat `Exited(non-zero)` as cold.

#### Rationale

Grinch's repro is real: in-place WG teardown without AC restart leaves an `Exited(non-zero)` SessionManager record, which `find_active_session` surfaces, which the round-1 plan would have promoted to `had_prior_session=true`, which would re-inject `--continue` against the ghost projects dir — exactly bug #82, just via a different lifecycle. Option (b) ships a known foot-gun on a lifecycle the user already exercises (iterating on WG layouts is normal authoring); option (c) re-opens the seam after the same review cycle. Option (a) costs a one-line predicate, fits the existing carry-through mechanism, and is the principled fix.

The grinch counter-case ("agent crashed / `exit 1` left valid conversation behind, option (a) loses `--continue` for it") is real but acceptable because:

1. **Asymmetric severity.** False-cold (lose `--continue` after a crash) is a *soft* regression — the user gets a fresh conversation when they wanted a continue. They can recover via the restart-button path that calls `restart_session(skip_auto_resume=Some(false))`. False-known-state (inject `--continue` after a teardown) is a *hard* error — Claude rejects with `No conversation found to continue` and the user cannot recover without manually clearing the ghost dir.
2. **Frequency asymmetry.** In-place WG teardown is the user's stated dev-loop; agent-crash-with-recoverable-conversation is rare in practice (Claude/Codex/Gemini themselves rarely produce non-zero exits without taking the conversation down with them).
3. **Convention.** Treating non-zero exit codes as evidence of an unhealthy state is the orthodox interpretation of POSIX exit codes. Hijacking `Exited(0)` as "clean exit, prior conversation likely intact" is the well-established mental model in the rest of `mailbox.rs` (`wake_action_for` already groups all `Exited(_)` together, but this plan adds a finer-grained read of the inner code).

I considered the "drop SessionManager records when CWD content state changes" alternative tech-lead floated. Rejected: it requires either filesystem watching (heavy, platform-specific, error-prone on Windows) or a CWD-fingerprint stored at session-creation time (new persistent state, exactly the complexity option-3-marker-file was rejected for in §2.1). Same objection applies.

I also considered making the discrimination crash-vs-deferred *explicit* via a new `SessionStatus::Deferred` variant rather than overloading `Exited(0)`. Tempting but out of scope: it would touch the persistence layer, the sidebar UI's status-rendering, the existing `wake_action_for` test matrix, and the `startOnlyCoordinators` branch in `lib.rs`. Bigger blast radius than #82 warrants. If a future refactor wants the explicit variant, this plan does not block it — `matches!(s.status, SessionStatus::Exited(0))` becomes `matches!(s.status, SessionStatus::Deferred)` with one substitution.

User-lifecycle confirmation (tech-lead is asking) does **not** affect this decision: option (a) covers both the with-AC-restart and the without-AC-restart variants. The user's answer informs priority, not correctness. Ship option (a) regardless.

### R2.2 G2 decision — fix in this PR

**Pick: fix in this PR.** Add `lower.starts_with("--continue=")` to the predicate, plus one unit test.

#### Rationale

§4.8 already extracts the predicate; the marginal cost is one disjunct + one test. Punting drags the same file back into the next PR for a one-line edit, and the gap is *literally* the case the tech-lead pressure-tested first. There is no scope-discipline argument for deferring a one-line fix on the same predicate we are already touching.

The original `commands/session.rs:359-362` already has the gap, so technically this is a pre-existing-bug fix bundled with the refactor — exactly the kind of tiny adjacent improvement the §2.1 "minimal blast radius" principle does *not* preclude (no surrounding cleanup, no design churn, just one missing OR-clause in a predicate that we are extracting). Acceptable.

### R2.3 Updated §4.5 (`deliver_wake` spawn-fallback) — supersedes round-1 §4.5

**Variable rename and re-scoping.** The round-1 plan introduced `had_prior_session` and set it to `true` whenever `find_active_session` matched. After G1, the variable's *semantic* changes — only `Exited(0)` matches count as known-state — and its computation site moves from "top of function" to "inside the RespawnExited branch."

Rename to `spawn_with_resume` to make the new semantic readable (positive form, no double-negative). Drop the inversion at the call site (`!had_prior_session` → just `!spawn_with_resume`, or keep the dev-rust-D5 helper with renamed argument — see R2.7).

**Replaces** the round-1 §4.5 step list with the following (steps 1–3):

1. **Insert** between current line 534 and current line 535 — the first body statement in `deliver_wake`. Comment text MUST anchor the §4.5.a invariant per grinch G7:

   ```rust
           // Whether the spawn-fallback should allow provider auto-resume.
           // Default false (cold wake — no SessionManager record at this CWD,
           // OR record exists but exited abnormally). Promoted to true only by
           // the RespawnExited branch when status is Exited(0) — the deferred-
           // non-coord clean-exit case, the one signal AC has that there is a
           // resumable prior conversation belonging to THIS app instance.
           //
           // MUST NOT be re-derived after `destroy_session_inner` runs: post-
           // destroy, `find_active_session` returns None and the inversion
           // would silently regress #82 (deferred-non-coord wake loses
           // --continue). Set the flag inside the pre-destroy match arm only.
           // See plan §4.5.a and #82 G7.
           let mut spawn_with_resume = false;
   ```

2. **Modify** the existing `match wake_action_for(&s.status)` block (current lines 548-573 — the body of `if let Some(s) = session`). Replace the existing `WakeAction::RespawnExited` arm:

   **Before** (current lines 555-573):
   ```rust
                       WakeAction::RespawnExited => {
                           log::info!(
                               "[mailbox] wake: session {} is Exited, destroying before respawn",
                               session_id
                           );
                           // Drop read lock before destroy call — release promptly
                           // (destroy acquires its own read lock).
                           drop(mgr);
                           if let Err(e) =
                               crate::commands::session::destroy_session_inner(app, session_id).await
                           {
                               log::error!(
                                   "[mailbox] wake: failed to destroy exited session {}: {}",
                                   session_id,
                                   e
                               );
                           }
                           // Fall through to spawn-persistent.
                       }
   ```

   **After**:
   ```rust
                       WakeAction::RespawnExited => {
                           // Only Exited(0) is treated as a known-state prior session.
                           // Non-zero exits (cwd vanished from in-place teardown, agent
                           // crash, OOM, ENOENT-class shell exits) signal that the
                           // ~/.claude/projects/ contents may be stale relative to the
                           // session AC will spawn next. See #82 G1.
                           spawn_with_resume = matches!(s.status, SessionStatus::Exited(0));
                           log::info!(
                               "[mailbox] wake: session {} is Exited (status={:?}), spawn_with_resume={}, destroying before respawn",
                               session_id,
                               s.status,
                               spawn_with_resume
                           );
                           drop(mgr);
                           if let Err(e) =
                               crate::commands::session::destroy_session_inner(app, session_id).await
                           {
                               log::error!(
                                   "[mailbox] wake: failed to destroy exited session {}: {}",
                                   session_id,
                                   e
                               );
                           }
                           // Fall through to spawn-persistent.
                       }
   ```

   The augmented log line subsumes dev-rust's D9 debug-log request — same information, info-level rather than debug-level (dev-rust's framing was "future user reports …"; an info log is more useful for that than a debug log, and we already log at info elsewhere in this function).

3. **Inner `else` (race fallthrough at current lines 575-581)**: leave `spawn_with_resume` at its initial `false`. No code change in this arm; just augment the comment to be explicit about the bias:

   ```rust
                   } else {
                       // session_id was returned by find_active_session but vanished
                       // from list_sessions before we read it — only possible if a
                       // concurrent destroy ran between the two awaits. Bias: treat
                       // as cold (spawn_with_resume stays false). See #82 G9.
                       log::warn!(
                           "[mailbox] wake: session {} not in list_sessions",
                           session_id
                       );
                       drop(mgr);
                   }
   ```

4. **Modify** the `create_session_inner` call inside the spawn-fallback (current line 638). Replaces the round-1 step 3:

   ```rust
               !spawn_with_resume, // skip_auto_resume — see spawn_with_resume comment above
   ```

   Or equivalently, via the dev-rust D5 helper renamed in R2.7:

   ```rust
               wake_spawn_skip_auto_resume(spawn_with_resume),
   ```

#### 4.5.a Ordering note (updated for R2.3)

The round-1 §4.5.a wording is correct in spirit but no longer applies literally — `spawn_with_resume` is now set inside the RespawnExited arm, not at function entry. The replacement invariant: **`spawn_with_resume` MUST be set inside the pre-destroy match arm, never re-derived from `find_active_session` after `destroy_session_inner` runs.** The doc comment at the variable declaration (R2.3 step 1) anchors this. Grinch G7 satisfied.

#### find_active_session no longer dictates the resume bit

D2's clarification (round 1) noted that `find_active_session` returns `Some` for any matching session including all Exited variants. After R2.3, that fact is no longer load-bearing for `spawn_with_resume` — the discriminator is now the inner `s.status` read inside RespawnExited, not the outer match. D2's comments remain accurate as documentation of `find_active_session` behavior; no further change needed.

### R2.4 Updated §4.8 — `should_inject_continue` predicate (per G2)

**Replaces** the predicate in the round-1 §4.8 helper body:

```rust
fn should_inject_continue(
    is_claude: bool,
    skip_auto_resume: bool,
    claude_project_exists: bool,
    full_cmd: &str,
) -> bool {
    if !is_claude || skip_auto_resume || !claude_project_exists {
        return false;
    }
    let already_has_continue = full_cmd.split_whitespace().any(|t| {
        let lower = t.to_lowercase();
        lower == "--continue" || lower.starts_with("--continue=") || lower == "-c"
    });
    !already_has_continue
}
```

The `lower.starts_with("--continue=")` clause closes the GNU long-option `--continue=value` blind spot per grinch G2.

Per grinch G5 (LOW), the helper's doc comment must call out the `-c` overlap with codex's `-c key=value` config option:

```rust
/// Decide whether to auto-inject `--continue` for a Claude session.
/// Pure function: no filesystem access. Caller is responsible for resolving
/// `claude_project_exists` (typically `~/.claude/projects/<mangled-cwd>/.is_dir()`).
///
/// Returns `true` only when ALL of:
///   - the session is a Claude variant
///   - the caller has not requested skip
///   - the projects dir exists on disk
///   - the configured argv does not already contain `--continue`,
///     `--continue=<value>`, or `-c`
///
/// Note: `-c` is also Codex's short form for `--config` (e.g.,
/// `codex -c key=value`). In compound commands that mix `codex` and `claude`
/// (e.g., `cmd /K codex -c k=v && claude`), the `-c` from codex's tokens will
/// suppress Claude's `--continue` injection. Pre-existing behavior; documented
/// here so refactors do not silently lose it.
fn should_inject_continue(
```

### R2.5 Updated §5 (behavior matrix) — wake row

| # | Call site | `skip_auto_resume` (before) | `skip_auto_resume` (after, **R2**) | Why |
|---|---|---|---|---|
| 4 | `phone/mailbox.rs:638` (`deliver_wake` spawn-fallback) | `false` | **`!spawn_with_resume`**, where `spawn_with_resume = matches!(s.status, SessionStatus::Exited(0))` inside the RespawnExited arm only; otherwise `false` | Three sub-cases. (i) `find_active_session` → None: cold wake → `true`. (ii) Inject branch (Active/Running/Idle): early return, irrelevant. (iii) RespawnExited: `Exited(0)` → known state (`false`); `Exited(non-zero)` → cold (`true`); inner-`else` race → cold (`true`). |

Other rows in §5 are unchanged. The R2 row supersedes the round-1 row 4.

### R2.6 Updated §7 (out-of-scope)

**Add** to the round-1 §7 list:

8. **Orphan SessionManager records on PTY-spawn failure (grinch G3).** The path at `commands/session.rs:457-461` (PTY spawn) creates an `mgr.create_session` record at lines 293-305 *before* spawning the PTY, with no cleanup if the spawn fails. After this plan, a PTY-spawn-failure during restore could leave an `Exited`-status orphan that round-2 §4.5 may misclassify (e.g., if the orphan is `Exited(0)` because no `mark_exited` ran and the default status is treated as `Exited(0)` by some intermediate read — though my read of the code is the orphan stays in `Running` or whatever the `status` field defaulted to, which would not match the `Exited(0)` predicate at all). Either way: this is a pre-existing latent bug in `create_session_inner`, not introduced by #82. Recommend tracking as a separate cleanup follow-up issue (proposed title: "create_session_inner: clean up SessionManager record on post-mgr-create failure paths"). No action in this PR.

9. **UI-destroy-mid-wake race (grinch G9).** The Tauri commands run on separate tokio tasks from `MailboxPoller`. A concurrent destroy between `find_active_session` and the inner `list_sessions` read can land in the inner-`else` race fallthrough. After R2.3 step 3, that path is correctly biased to cold (`spawn_with_resume=false`). Whether the destroy intent should "win" over the wake intent is a UX call orthogonal to #82 correctness — flag for awareness, no action.

### R2.7 Updated §8 (test plan)

#### §8.1 additions (per G2)

Add after the round-1 §8.1 test #5:

10. **`should_inject_continue_returns_false_when_continue_with_value_present`** — `is_claude=true`, `skip_auto_resume=false`, `claude_project_exists=true`, `full_cmd="claude --continue=somevalue"` → `false`. (Verifies the `--continue=` long-option-with-value form per grinch G2.)

Per grinch G4, **strengthen** the existing round-1 §8.1 test #2 (`should_inject_continue_returns_false_when_skip_requested`):

- **Rename** to `should_inject_continue_returns_false_when_skip_overrides_existing_dir`.
- **Body** must be explicit, not "all else permissive": `is_claude=true`, `claude_project_exists=true`, `skip_auto_resume=true`, `full_cmd="claude"`. Assert `false`. This locks the predicate against future refactors that re-order the early-return clauses.

#### §8.2 helper rename (per R2.3)

The dev-rust D5 helper stays, with renamed argument to match the new variable semantics:

```rust
/// `skip_auto_resume` value used by `deliver_wake`'s spawn-fallback.
/// `true` (skip resume) when the wake is cold — either no SessionManager record
/// at this CWD, or the matched record exited with non-zero status (abnormal
/// termination — cwd vanished from in-place WG teardown, agent crash, OOM).
/// `false` (allow resume) only when the matched record exited cleanly
/// (`Exited(0)` — the deferred-non-coord case where a resumable prior
/// conversation likely exists).
fn wake_spawn_skip_auto_resume(spawn_with_resume: bool) -> bool {
    !spawn_with_resume
}
```

Tests (replace round-1 §8.2 names):
```rust
#[test]
fn wake_spawn_skip_auto_resume_skips_when_cold() {
    assert!(super::wake_spawn_skip_auto_resume(false));
}

#[test]
fn wake_spawn_skip_auto_resume_allows_when_known_state() {
    assert!(!super::wake_spawn_skip_auto_resume(true));
}
```

The 3-line helper now also documents *why* the inversion exists (cwd-vanished case, etc.), which is harder to embed at the call site. Net benefit goes up after G1.

#### §8.4 manual repro additions

**[ROUND-3 RETRACTION (R3.6 / R3.9 #1):** the §8.4.D2 manual repro that originally lived here has been dropped. Per grinch G2.1 + architect R3.1, the precondition (session moves to `Exited(non-zero)`) is unproducible against current code — `mark_exited` has one caller (`lib.rs:561`) with literal `0`, and the PTY manager's read loop never surfaces a child exit code. R3 reverts to round-1's manual repros A–E (no D2). Body removed for clarity.**

### R2.8 Items I accept from grinch but defer to dev-rust round 2

These are wording / placement / test-fixture nits already triaged by tech-lead; documenting acceptance for the record so dev-rust round 2 doesn't re-litigate:

- **G3** — §9.2 wording correction (orphan SessionManager records on post-mgr-create failure). Accept; dev-rust round-2 to update §9.2 prose. R2.6 #8 above also references it for the §7 out-of-scope list.
- **G4** — explicit test fixture in §8.1 test #2. Already incorporated in R2.7.
- **G5** — `-c` codex overlap doc comment. Already incorporated in R2.4 helper doc.
- **G6** — §4.7 line-range fix (255-261 not 255-274). Accept; dev-rust round-2 to correct heading.
- **G7** — anchor §4.5.a invariant via comment at the variable declaration. Already incorporated in R2.3 step 1.

### R2.9 Items I reject from grinch

**None.** All G1–G12 findings either accepted, incorporated, or correctly framed as informational/out-of-scope by grinch themselves.

### R2.10 Items needing user input vs team-resolvable

**Needs user input:**
- §9.1 codex/gemini symmetric scope. Tech-lead has flagged with user. Architect + dev-rust both lean symmetric; the §4.7 doc comment in R2.4 still reads as if symmetric is in force. If user pushes back to Claude-only, the cleanest follow-up is a separate `is_fresh_create: bool` orthogonal flag on `create_session_inner` gating only the Claude block — but that's two flags doing nearly the same thing (anti-pattern, dev-rust D3). My recommendation: commit to symmetric.

- **User lifecycle confirmation** (which #82 lifecycle the user actually triggered: with-AC-restart vs without-AC-restart-via-in-place-teardown). Independent of fix correctness — option (a) covers both. Useful for setting test priority and PR-narrative emphasis.

**Resolvable inside the team:**
- Everything else. Round-2 is enrichment territory.

### R2.11 Done state (R2 supersedes round-1 §10)

- Six callsite values reflect the matrix in §5 + R2.5.
- `should_inject_continue` extracted; predicate covers `--continue`, `--continue=<value>`, `-c` (R2.4).
- `mailbox::deliver_wake` carries `spawn_with_resume`, set inside the RespawnExited arm only, true iff `Exited(0)` (R2.3). Helper `wake_spawn_skip_auto_resume` extracted (R2.7) with rename and test fence.
- Comment updates at `commands/session.rs:255-261` (R2.4 helper doc + round-1 fn doc), `lib.rs:594` (round-1 §4.6), and the new variable declaration in `deliver_wake` (R2.3 step 1).
- New unit tests: §8.1 #1-#10 (incl. G2 `--continue=value` test), §8.2 wake-spawn helper tests.
- Manual repros A–E from round 1 plus §8.4.D2 (in-place WG teardown).
- Branch `fix/82-no-continue-on-fresh-session` ready for dev-rust round-2 enrichment, then grinch round-2 review.

The plan stops here.

End of architect round 2.

---

## Dev-rust enrichment (round 2)

**Reviewer:** dev-rust (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Reading:** grinch G1–G12 + architect R2.1–R2.11 + my own round-1 D1–D15.
**Source state:** branch tip = `origin/main` @ `96860c0`, no commits on `fix/82-no-continue-on-fresh-session` yet (the plan file is the only untracked path).

### E1. Line-number / path verification of R2 references

R2 introduces several new code references. All verified against the current branch tip:

| R2 reference | Verified location | Status |
|---|---|---|
| `mailbox.rs:548` (`match wake_action_for(&s.status)`) | line 548 | ✅ exact |
| `mailbox.rs:555-573` (`WakeAction::RespawnExited` arm) | lines 555-573 | ✅ exact |
| `mailbox.rs:575-581` (inner-`else` race fallthrough) | lines 575-581 | ✅ exact |
| `SessionStatus::Exited(i32)` at `session/session.rs` | line 98 (variant `Exited(i32)` inside `pub enum SessionStatus`) | ✅ exact — pattern `matches!(s.status, SessionStatus::Exited(0))` is well-formed |
| `SessionManager::create_session` initial status | `manager.rs:50` (`status: SessionStatus::Running`) | ✅ exact — orphan from PTY-spawn failure stays `Running`, not `Exited(_)` |
| Existing `mod tests` in `mailbox.rs` | lines 1870-1871 (`use super::*;`) | ✅ exact — `wake_spawn_skip_auto_resume` tests fit there, no new imports needed |
| `commands/session.rs:457-461` (PTY spawn, no error-path cleanup) | lines 457-461 | ✅ exact — confirms grinch G3 / R2.6 #8 |
| `commands/session.rs:401-419` (context-mat with `destroy_session` cleanup) | lines 401-419 (cleanup at line 413) | ✅ exact — counter-example to PTY-spawn path |
| `commands/session.rs:1264` (test imports) | line 1264 | ✅ exact — still needs `should_inject_continue` added |

No mismatches in R2's references. R2.3 / R2.4 / R2.7 patches can be applied 1:1.

### E2. G6 line-range fix — applied

Done in this round: `### 4.7 ... :255-274` → `:255-261` (heading at line 289). Also corrected the cross-reference in §10 Done state (line 543 → reads `:255-261` now). The architect's R2.11 already used the corrected range.

### E3. G3 fix — §9.2 wording updated, with rationale for *why it's now moot*

Done in this round: §9.2 (line 525) now carries a "Status (round-2 update)" note explaining that R2.3's `Exited(0)`-only discriminator incidentally protects the failed_recoverable lifecycle from the spawn-fallback altogether. The reasoning chain:

1. PTY-spawn failure does not call `mark_exited` (verified at `commands/session.rs:457-461` vs the context-mat cleanup at `commands/session.rs:413`).
2. Orphan `SessionManager` records therefore stay at `SessionStatus::Running` (verified at `manager.rs:50`).
3. `wake_action_for(Running) = Inject` (verified at `mailbox.rs:79-81`); the function early-returns via `inject_into_pty` at line 553.
4. The spawn-fallback (lines 626-641) where `spawn_with_resume` is read is therefore never reached for orphan records.
5. The orphan-PTY-write failure mode (separately surfaced by grinch G3) is a pre-existing latent bug, properly tracked as §7 #8 (architect-added in R2.6) for follow-up cleanup. Out of #82's scope.

The §9.2 note now references §7 #8 as the canonical tracking entry. ✅

### E4. D7 update — explicit test-import scope

Round-1 D7 said: "add `should_inject_continue` to the `use super::{...}` line at `commands/session.rs:1264`."

After R2.7 introduces the `wake_spawn_skip_auto_resume` helper in `mailbox.rs`, **no analogous change is needed in `mailbox.rs`'s test mod** — the existing `mod tests { use super::*; }` at `mailbox.rs:1870-1871` imports everything. Only `commands/session.rs:1264` needs the explicit addition:

```rust
use super::{inject_codex_resume, resolve_actual_agent, should_inject_continue};
```

Clarifying for grinch in case the round-1 D7 wording read as "two import sites need updating."

### E5. D2 status — preserved by R2.3 explicit incorporation

R2.3 already states: "D2's comments remain accurate as documentation of `find_active_session` behavior; no further change needed." The behavioral fact (find_active_session returns Some for any status, including all Exited variants) is no longer load-bearing for `spawn_with_resume`, but the documentation D2 captured remains accurate. No round-2 action required on D2.

### E6. D8 status — STILL applies for §4.7 prose (R2 didn't touch it)

R2.4 updated **§4.8** (the `should_inject_continue` helper doc) per G5. R2 did NOT update **§4.7** (the `create_session_inner` function doc) — only G6's heading-level line-range got corrected. So my round-1 D8 wording suggestion remains relevant for §4.7's body text.

The round-1 §4.7 doc still says "true (default for fresh creates)" — phrasing that implies a default mechanism that does not exist. The function signature still has `skip_auto_resume: bool` (no `Option`, no `unwrap_or_default`). Every caller passes a value. Suggested rewording (re-affirming D8):

> `skip_auto_resume` controls provider auto-resume injection:
> - `true` — suppress all provider auto-resume. **Use this for any "fresh create" call site** (UI/CLI/root-agent create, mailbox wake-from-cold, mailbox wake-from-Exited-non-zero per §4.5, `restart_session` with default semantics from `effective_restart_skip_auto_resume`).
> - `false` — allow provider auto-resume. **Use this only for paths restoring a session AC already knows about** (the startup-restore loop in `lib.rs`, the wake-from-Exited(0) branch in `mailbox::deliver_wake` per R2.3, and `restart_session` when its caller passes `Some(false)`).

Note the bullets now name the post-R2 wake semantic explicitly ("wake-from-cold" / "wake-from-Exited-non-zero" / "wake-from-Exited(0)") — more precise than my round-1 phrasing, which conflated "wake-from-cold" with "wake-from-known-state."

dev-rust + grinch can resolve this directly during implementation; not architect-blocking.

### E7-E9 [ROUND-3 RETRACTION]

**[E7 / E8 / E9 dropped per round-3 R3.6 / R3.9 #2.]** Grinch G2.1 + architect R3.1 chose option 1 (replace R2.3's `matches!(s.status, SessionStatus::Exited(0))` discriminator with a constant `true` inside the `RespawnExited` arm), making:
- E7's `wake_resume_for_exited` helper moot (the discriminator it pinned no longer exists).
- E8's `§8.4.D2` step-2 verification nit moot (the manual repro is dropped per R3.9 #1).
- E9's concurrency cross-check on the matches!() moot (the matches!() is replaced by a constant). The underlying observation about `s.status` being a stable read into a local Vec snapshot remains true and is now documented in grinch G2.8.

Original E7-E9 bodies removed for clarity. The drops are tracked in dev-rust round-3 enrichment §F2.

### E10. Codex/Gemini side-effect (§9.1) status — still pending user

Architect's R2.10 confirms all three of us lean symmetric, user not yet replied. R2.4's helper doc and R2.5's matrix are written assuming symmetric. If the user pushes back, a separate revision pass on §4.7 (and possibly the codex/gemini blocks at `commands/session.rs:384-398`) would be needed; my round-1 D3 anti-pattern argument stands.

No round-2 action required from dev-rust. Continue assuming symmetric.

### E11. Round-2 stances on the open R2 deltas

| Item | Stance |
|---|---|
| R2.1 G1 → option (a), Exited(0) discrimination | **Endorse.** Asymmetric severity argument is correct: false-cold = soft regression, false-known-state = hard error. Convention-aligned (POSIX exit codes). |
| R2.2 G2 → fix in this PR | **Endorse.** One-line predicate, one test. Punting drags the same file back. |
| R2.3 variable rename `had_prior_session` → `spawn_with_resume` | **Endorse.** Positive-form name avoids the double-negative at the call site. The R2.3 step 1 anchor comment satisfies G7. |
| R2.4 `--continue=` predicate + `-c` codex overlap doc (G5) | **Endorse.** |
| R2.5 §5 matrix — wake row updated | **Endorse.** |
| R2.6 §7 expansion (#8 orphan, #9 UI race) | **Endorse.** Both are accurate framings. |
| R2.7 test additions + helper rename | **Endorse**, augmented by E7 (additional helper for the Exited(0) discriminator). |
| R2.8 acceptance of G3-G7 as triaged | **Endorse**, applied G3 + G6 in this round; E7 layers on top of G7's anchor comment with a unit-test fence. |
| R2.9 nothing rejected | **Endorse.** |
| R2.10 user-input items | **Endorse.** No new items. |
| R2.11 Done state | **Endorse**, with E7's helper added if accepted. |

### E12. Summary — items that materially change the plan

Two:

1. **§9.2 wording fix (E3 / G3).** Applied. The §9.2 question is now annotated as moot under R2.3.
2. **§4.7 heading line-range fix (E2 / G6).** Applied. Also synced §10's cross-reference.

One **proposal** (recommend, not applied):

3. **`wake_resume_for_exited` helper + tests (E7).** Pin the Exited(0) discriminator semantic. 3 + 12 lines. Defer to grinch round-2 if they want to redirect.

Two **clarifications** (no plan-text change required):

4. **D7 scope clarification (E4).** Test imports are needed only in `commands/session.rs`'s test mod, not `mailbox.rs`'s.
5. **§4.7 doc-comment text (E6 / D8).** Round-1 D8 still applies; R2 only fixed the heading.

One **manual-repro nit** (recommend a 1-sentence addition):

6. **§8.4.D2 step 2 — exit-code verification (E8).** Add log-line guidance so a tester doesn't get confused by "Exited" sidebar text.

### E13. Items needing user/architect input vs team-resolvable

**Needs user input:**
- §9.1 codex/gemini scope (pending — unchanged from round 1).

**Needs architect input:**
- E7 acceptance (helper for Exited(0) discriminator). I lean strongly include; architect can override if it's seen as over-test-fencing.

**Resolvable inside dev-rust + grinch round 2:**
- E2, E3 (already applied).
- E4, E5, E6 (status updates, no plan-text changes).
- E8 (manual-repro text clarification — cheap).

### E14. Verdict

Plan is technically correct after R2. With E2 + E3 applied (this round), the plan is **ready for grinch round 2**. E7 is the one substantive recommendation; everything else is housekeeping. No new architect blockers introduced.

End of dev-rust round-2 enrichment.

---

## Grinch adversarial review (round 2)

**Reviewer:** dev-rust-grinch (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Reading:** architect R2.1-R2.11 + dev-rust E1-E14 + my own G1-G12.
**Source state:** branch tip = `origin/main` @ `96860c0`, no commits on `fix/82-no-continue-on-fresh-session` yet.
**Scope assumption:** symmetric (per tech-lead's standing instruction).

I tried to break the round-2 plan, including the new R2 deltas. The architect addressed my round-1 MEDIUMs faithfully and the rename + scope-move + predicate change are mechanically correct. **But** I found something my round-1 review missed entirely, and which neither dev-rust E1-E14 nor architect R2.1-R2.11 caught: **the production state space the round-2 fix is designed for does not exist.**

### G2.1. (HIGH — NEW) `Exited(non-zero)` is unreachable in production code; R2.1/R2.3/R2.5/§8.4.D2 are predicated on a state that cannot occur

**What.** I exhaustively traced every transition into `SessionStatus::Exited(_)` in the codebase. There is exactly one writer:

- `mgr.mark_exited(id, code)` at `session/manager.rs:174-186` is the **only** code path that sets `s.status = SessionStatus::Exited(code)` (line 184).
- `mark_exited` has **exactly one caller**: `lib.rs:561`, with the integer literal `0` (deferred-non-coord at startup).
- The PTY manager's `spawn` (`pty/manager.rs:277-456`) holds the child as `_child: child` (line 363) — intentionally unused — and the read loop (lines 382-453) `break`s on EOF (line 386) or read error (line 450) without ever calling `mark_exited` or surfacing the child's exit code. Thread terminates; `PtyInstance` stays in `self.ptys`; session status stays whatever it was (Running/Active/Idle).
- No `Default` impl, no `From<...>` constructor, no deserialization path produces non-zero `Exited`. `PersistedSession.status` (`config/sessions_persistence.rs:48-49`) is documented as "only present in live snapshots, ignored on restore"; restored sessions enter via `create_session_inner` at `Running` (`manager.rs:50`), then optionally `mark_exited(0)` for deferred-non-coord.

**The only reachable `Exited` variant in the entire production code is `Exited(0)`.** `Exited(1)`, `Exited(-1)`, `Exited(255)` exist only inside the test module at `mailbox.rs:1897` and `:1901`.

**Why this matters.**

- **The "in-place WG teardown" repro from my round-1 G1 does not actually traverse the wake-spawn-fallback path.** When the cwd is `rm -rf`'d while AC stays running, the PTY's read loop hits EOF and breaks, but the session's `status` field stays at `Running` / `Active` / `Idle` (whichever it was before). A subsequent wake hits `find_active_session` → matches → inner `find` returns `Some(s)` with non-Exited status → `wake_action_for(s.status) = Inject` → `inject_into_pty` writes to a dead PTY (likely silent failure on Windows, broken-pipe on POSIX). **The spawn-fallback is never reached.** No `--continue` injection occurs because no fresh session is spawned. The wake silently fails to deliver — a separate bug, but **not** the #82 ghost-dir regression.

- **R2.1's "asymmetric severity" rationale evaluates a non-existent code path.** The argument was: false-cold (lose `--continue` after agent crash) is soft, false-known-state (inject `--continue` after teardown) is hard. Both branches presuppose `Exited(non-zero)` is producible. It is not.

- **R2.3 step 2's discrimination `matches!(s.status, SessionStatus::Exited(0))` evaluates the same way as `matches!(s.status, SessionStatus::Exited(_))` for every reachable input.** The only Exited variant that reaches this match arm is `Exited(0)`; the integer-literal narrowing is semantically inert. R2.3 is functionally equivalent to "any RespawnExited match → resume" — which is the round-1 plan's behavior re-written in a positive-form variable name.

- **§8.4.D2 manual repro CANNOT be performed against current code.** Step 2 says "Confirm in the sidebar that the session moves to Exited(non-zero)." There is no production code path that produces Exited(non-zero). A tester following these steps will see the session stay at Running/Active/Idle in the sidebar, conclude the precondition is not met, and report the test as un-runnable. Dev-rust's E8 ("the sidebar shows 'Exited' not 'Exited(N)' — point at log line") describes a cleanup that papers over this: the log line at `mailbox.rs:543` would only print `Exited(0)`, never `Exited(N>0)`, because the underlying status field never holds a non-zero code. E8 is solving a confusion that doesn't end at a useful answer.

**Why we all missed this.**

I missed it in round-1 because I assumed PTY death surfaces an exit code (it does in `tokio::process::Child::wait()`, in `std::process::Child::wait()`, and in most wrappers — but `portable-pty`'s child handle is held as `_child` and never `.wait()`-ed). Architect R2.1 inherited my framing. Dev-rust E1 verified the *syntax* of `matches!(_, Exited(0))` against `session.rs:98` but not the *semantics* — it confirmed the pattern is well-formed without checking whether the producing code path exists.

E1's verification matrix was thorough on call-site line numbers but did not include a "where does Exited(non-zero) actually come from?" check. That gap is the load-bearing one.

**Failing input / scenario.**

A tester following §8.4.D2 step 2: `rm -rf` the WG dir, observe sidebar. Sidebar shows `Running` (or `Active`/`Idle`). Step 4: send wake. The wake routes to `Inject`, calls `inject_into_pty`, writes to the dead PTY's master writer, observes either a silent buffered write or a broken-pipe error. **No spawn-fallback fires. No `--continue` is injected. The G1 fix is not exercised.** The tester reports "I can't reproduce the precondition" and the team has no way to verify the discrimination behaves correctly under the only state that supposedly drives it.

**Suggested fix — pick one.**

1. **Simplify R2.3 to match the reachable state space and acknowledge the limit.** Replace `matches!(s.status, SessionStatus::Exited(0))` with `true` (the only path that gets here is the deferred-non-coord clean-exit case, status `Exited(0)` by construction). The variable becomes a constant within the arm — eliminate it. Keep the comment explaining the design assumption: "Today, only deferred-non-coord (`mark_exited(0)` at lib.rs:561) reaches the RespawnExited arm; if PTY exit detection is ever added, this comment is the seam to revisit." Drop §8.4.D2 entirely. **Cost:** undoes part of R2's complexity. **Benefit:** plan reflects the code that exists, no impossible repros, the behavioral guarantee for the reachable state is unchanged.

2. **Keep R2.3 as forward-compat scaffolding for a future PTY-exit-detection PR, but explicitly document that the discrimination is dormant today.** Add a §7 entry "8a. The Exited(0)-vs-non-zero discrimination in R2.3 is forward-compatible scaffolding only. Today, no code path produces `Exited(non-zero)` (verified `mark_exited` has one caller, `lib.rs:561`, with literal `0`; the PTY manager's read loop breaks without surfacing exit codes). If/when a future PR adds child-exit detection (`portable_pty::Child::wait()` or equivalent) and calls `mark_exited(id, code)` with the real code, this discrimination becomes load-bearing automatically." Convert §8.4.D2 to a future-PR test case ("blocked on PTY exit detection — reactivate when that lands"). **Cost:** keeps the variable + comment. **Benefit:** signals intent to future readers and the next PR (PTY-exit-detection) inherits a documented seam.

3. **Add PTY exit detection in this PR.** Out of scope for #82; bigger blast radius (touches `pty/manager.rs`, the read-loop thread, the broken-pipe / EOF distinction, status-event emission to the frontend, sidebar rendering, Telegram bridge teardown, etc.). I would not bundle this with #82.

I lean strongly **option 1**. R2's added complexity — variable rename, in-arm computation, augmented log line, §8.4.D2 manual repro, E7's regression-fence helper — was scoped against a misread of the code. Stripping the discrimination back to "any RespawnExited → resume" matches the round-1 semantic without re-introducing anything I attacked in G1 (because G1 itself was based on the same misread).

Option 2 is acceptable if the team wants to telegraph the seam to whoever picks up PTY exit detection.

This is the only HIGH I found in round 2.

### G2.2. (LOW) Round-1 G1 was wrong — retracting

For the record: my round-1 G1 ("in-place WG teardown without AC restart re-triggers #82 via wake-spawn-fallback") was incorrect. It was predicated on the same false premise as R2.1: that `Exited(non-zero)` is producible from PTY death. It is not. The actual lifecycle (cwd vanishes, session stays at Running/Active/Idle, wake routes to Inject) does not regress #82.

I retract round-1 G1 as a finding. The round-1 plan was correct as written for the reachable state space; my G1 prompted R2.1, which over-corrected for an unreachable state. The architect's response to G1 was epistemically faithful given the framing I provided — the gap is in my round-1 verification, not in R2's reasoning.

This does not invalidate the rest of round-1: G2 (`--continue=value` predicate fix), G3-G7 wording corrections, G8-G12 informational items remain accurate.

### G2.3. (LOW) E7's `wake_resume_for_exited` helper third test (`#[should_panic]`) is meaningless and should be dropped

**What.** E7 proposes:

```rust
#[test]
#[should_panic(expected = "non-Exited status")]
fn wake_resume_for_exited_non_exited_is_caller_error() {
    let _ = wake_resume_for_exited(&SessionStatus::Running);
    panic!("non-Exited status");
}
```

The helper itself does **not** panic on non-Exited input — it returns `false` (the `matches!` returns false for `Running`). The test then unconditionally calls `panic!("non-Exited status")`. Cargo passes it because of `#[should_panic(expected = ...)]`. This test asserts that the test panics *because the test panics*. It tests nothing about `wake_resume_for_exited`.

**Why.** Dev-rust E7 itself flags this: "The third test (`#[should_panic]`) is debatable; it's a contract-documentation pattern more than a unit test. I'd drop it if grinch flags it as cute." Flagging.

**Fix.** Drop the third test. The "callers MUST gate via `wake_action_for(...)`" contract belongs in the helper's doc comment, not a fake test. If the contract is worth enforcing at runtime, the helper should `debug_assert!(matches!(status, SessionStatus::Exited(_)))` and the test should verify the assertion fires under non-Exited input. Either way, the current `panic!("non-Exited status")` body is wrong.

(Note: if G2.1 option 1 is adopted, the entire helper goes away and this finding is moot.)

### G2.4. (informational) E6 / round-1 D8 doc-comment phrasing fix is correct

Dev-rust E6 is right: §4.7's body still reads "(default for fresh creates)" which implies a non-existent default mechanism. Every caller passes a value explicitly; the function signature is `skip_auto_resume: bool` (no Option, no Default). Dev-rust's tightening — "**Use this for any 'fresh create' call site** (UI/CLI/root-agent create, **mailbox wake-from-cold, mailbox wake-from-Exited-non-zero per §4.5**, …)" — is genuinely better.

**Caveat under G2.1:** if option 1 is adopted, the "wake-from-Exited-non-zero" bullet is also a phantom; rephrase to just "mailbox wake-from-cold (no SessionManager record at this CWD)." If option 2 is adopted, "wake-from-Exited-non-zero (forward-compat, dormant in current code)" is honest.

Cosmetic; not a blocker independent of G2.1.

### G2.5. (informational) Tech-lead's pressure-point #2 edge cases are mostly moot under G2.1

For completeness, my analysis of the architect's `Exited(0)` discrimination edge cases assuming the discrimination is real:

- **User /exit after a session of work → Exited(0)**: under R2.3, `spawn_with_resume=true` → `--continue` injected. ✓ semantically correct. (But this only happens if `mark_exited(0)` is called; today only the deferred-non-coord path does that. A user who types `/exit` does NOT trigger `mark_exited`, so the session stays at Running/Active/Idle. **Same G2.1 issue.**)
- **Claude crashes mid-session, PTY translates to exit 0**: cannot happen today. PTY exit codes are not surfaced.
- **`claude --version` one-shot → exit 0 → resume undesired**: even if this *were* reachable, `should_inject_continue` returns false because `claude --version` doesn't create a `~/.claude/projects/<mangled>/` dir. The `claude_project_exists` gate covers this. ✓
- **`--continue=value` predicate**: Whether or not Claude actually accepts `--continue=foo`, the predicate addition is harmless (passes through unchanged when the form isn't used; deduplicates correctly when it is). Approve R2.2/R2.4.

### G2.6. (informational) R2.3 control-flow paths verified

Despite G2.1's framing critique, R2.3's mechanical correctness holds. Verified each fall-through to spawn-fallback reads the correct `spawn_with_resume`:

1. `find_active_session` returns `None` → outer if-let does not enter → spawn-fallback reads default `false`. ✓
2. Outer matches, inner `find` returns `Some(s)`, `WakeAction::Inject` → returns from function via `inject_into_pty` (line 553); spawn-fallback unreached.
3. Outer matches, inner `find` returns `Some(s)`, `WakeAction::RespawnExited` → flag set inside the arm BEFORE `drop(mgr)` and BEFORE `destroy_session_inner` → spawn-fallback reads the in-arm value. ✓
4. Outer matches, inner `find` returns `None` (race fallthrough) → flag stays default `false` → spawn-fallback reads `false`. ✓

The `s` borrow in step 3 is a local `&SessionInfo` from the `Vec<SessionInfo>` snapshot returned by `mgr.list_sessions().await` — owned by the function stack frame, independent of the `mgr` guard, so reading `s.status` after `drop(mgr)` would also be safe (E9 confirms; the read happens before drop anyway).

§4.5.a invariant ("MUST be set inside pre-destroy match arm") satisfied by R2.3 step 1's anchoring comment + step 2's placement. G7 fully addressed.

### G2.7. (informational) Predicate change R2.2/R2.4 verified harmless-or-better

`lower.starts_with("--continue=")` adds correct handling for the GNU long-option-with-value form. If the form is never used, no behavior change. If it ever appears, `--continue=value` followed by `--continue` injection is suppressed (correct). G2 fully addressed.

### G2.8. (informational) Concurrency cross-check on R2.3 step 2

E9's analysis is correct: `s` is a borrow into the local `Vec<SessionInfo>` snapshot. Concurrent mutation of `SessionManager` cannot retroactively modify `s.status`. Read is stable. ✓

### G2.9. (informational) Round-2 endorsements

For all items where my finding is informational only:
- R2.2 / R2.4 predicate fix: ✓ approve.
- R2.5 matrix row 4 update: mechanically correct for reachable inputs (only Exited(0) leg fires); inaccurate for the Exited(non-zero) leg per G2.1.
- R2.6 §7 expansion (#8 orphan, #9 UI race): ✓ approve. Both framings are accurate.
- R2.7 helper rename + tests: helper itself is fine; E7's third test is bogus per G2.3.
- R2.8 acceptance of G3-G7: ✓ all incorporated correctly.
- R2.10 user-input items: nothing to add.

### G2.10. (informational) Round-2 retractions / corrections

- **Retract round-1 G1** per G2.2.
- **Retract round-1 G3 framing partly:** the orphan-record path I described (PTY-spawn failure leaves Running, not Exited) is correct, and the pre-existing bug (no cleanup on error path at `commands/session.rs:457-461`) is real. But the PHRASING I used implied that orphan records could surface as Exited and surprise the spawn-fallback. They cannot, both because the spawn-fallback doesn't fire for non-Exited and because no path produces Exited(non-zero) anyway. Dev-rust E3 captured this correctly; my round-1 framing was sloppy.

---

## Grinch Verdict (round 2): REQUEST CHANGES

Severity counts: **1 HIGH (G2.1), 0 MEDIUM, 2 LOW (G2.2 retraction, G2.3 bogus test), 6 INFORMATIONAL**.

The single HIGH (G2.1) is a NEW finding none of architect R2 / dev-rust E / my round-1 G1-G12 caught: the production state space the round-2 fix is designed for does not exist. R2's response to my round-1 G1 — option (a) Exited(0) discrimination — is mechanically correct but semantically inert against the current code. The §8.4.D2 manual repro cannot be performed.

Per Role.md Rule 5 (entering round 2 of iterate phase): a NEW HIGH that none of us have addressed requires round 3.

**My round-1 G1 was the original misread.** I introduced the false premise; architect R2 and dev-rust E faithfully built on it. The retraction is mine to make. The simplest path forward is to revert R2's discrimination back to "any RespawnExited match → resume" (option 1 in G2.1) and remove §8.4.D2 — i.e., land round-1's plan with G2/G4-G7 corrections and the renames from R2.3 step 1, but without the integer-literal narrowing. The behavior change is zero; the plan complexity drops.

Option 2 (forward-compat scaffolding with explicit dormant-discrimination documentation) is also acceptable if the team prefers to telegraph the seam.

**Items needing architect input (round 3 trigger):**
- **G2.1 fix selection (option 1 vs 2 vs 3).** Option 3 (add PTY exit detection in this PR) is out of scope; pick 1 or 2.
- **G2.4 cascade**: §4.7 doc-comment wording depends on G2.1's resolution.

**Items dev-rust + grinch can resolve directly (assuming any G2.1 option lands):**
- G2.3: drop E7's third test. (Or drop the helper entirely if G2.1 option 1.)
- §8.4.D2 wording: rewrite or remove per G2.1 outcome.

**Items where I retract or correct round-1:**
- G1 — retracted (G2.2).
- G3 framing partly retracted (G2.10).

End of grinch round-2 review.

---

## Architect round 3 (post-grinch-r2)

**Reviewer:** architect (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Reading:** grinch G2.1–G2.10 (HIGH G2.1 + retraction of round-1 G1) + dev-rust E1–E14.
**Source state:** unchanged — branch tip = `origin/main` @ `96860c0`.

### R3.1 G2.1 decision — option 1, simplify R2.3 to match the reachable state space

**Pick: option 1.** Replace R2.3's `matches!(s.status, SessionStatus::Exited(0))` discrimination with a constant `true` inside the `RespawnExited` arm. Drop §8.4.D2. Drop dev-rust E7's `wake_resume_for_exited` helper. Keep the `spawn_with_resume` variable and its R2.3-step-1 anchor comment (G7 still relevant; the variable is still load-bearing for the cold-vs-known split).

**Rationale.** Tech-lead's three reasons are correct and I adopt them:

1. **The round-1 plan was already correct for the reachable state space.** Grinch's round-2 trace is conclusive: `mark_exited` has one caller, `lib.rs:561`, with literal `0`. The PTY manager's read loop never surfaces a child exit code (`portable-pty`'s `_child` is held but never `.wait()`-ed). Every `Exited(non-zero)` reference in the production code is non-existent. R2.1's "asymmetric severity" rationale evaluated a non-existent code path.

2. **Dormant code is a maintenance hazard.** A future contributor reading R2.3's `matches!(s.status, SessionStatus::Exited(0))` will reason from a false model: they will assume the discrimination matters today and design around it. Worst case, they "simplify" the matches!() to `Exited(_)` thinking they are removing dead code, leaving the comment that referenced "non-zero exits become cold" stale. Net cognitive cost > zero, behavioral benefit = zero today.

3. **Future PTY-exit-detection PR is the right time, with full context.** When a PR adds `portable_pty::Child::wait()` and routes the exit code into `mark_exited`, it will need to handle the broken-pipe vs EOF distinction, the read-loop thread teardown, status-event emission to the frontend sidebar rendering, and the Telegram bridge teardown — and at that point the discrimination semantics can be designed against the actual exit-code distribution. Pre-emptively scaffolding here is speculative.

**Why not option 2 (forward-compat scaffolding).** Tech-lead explicitly weighed option 2 against (b) and (c). I agree: a §7 entry "the discrimination is dormant today, will reactivate when PTY exit detection lands" telegraphs the seam, but at the cost of dormant code that lies about what it does. Option 2 is genuine if and only if someone is *currently working on* PTY exit detection in a parallel PR. They are not. Defer.

**Why not option 3 (add PTY exit detection in this PR).** Out of scope. Touches `pty/manager.rs`, frontend sidebar, Telegram bridge teardown. #82 is a one-bool fix at the call sites; option 3 would balloon into a session-lifecycle refactor.

User-lifecycle question (still open): under option 1, the "with-AC-restart" lifecycle is the only one #82 closes. The "without-AC-restart" lifecycle (in-place WG teardown) is a separate failure mode that this PR does not address — see R3.4 for the §7 reframing.

### R3.2 Updated §4.5 — supersedes R2.3 (smallest delta from R2)

The variable `spawn_with_resume` stays. Its declaration at the top of `deliver_wake` (R2.3 step 1) stays — the anchor comment per G7 is still valuable because the variable is still load-bearing for the cold-vs-known split, and the "MUST NOT be re-derived after destroy" invariant is still real (a future refactor that hoists `find_active_session()` post-destroy would still flip the bool incorrectly).

**Replace** R2.3 step 1's anchor comment with a slightly relaxed version that drops the Exited(0) framing:

```rust
        // Whether the spawn-fallback should allow provider auto-resume.
        // Default false: cold wake — either no SessionManager record at this
        // CWD, or the matched record vanished from list_sessions before we
        // could read it (concurrent destroy). Promoted to true only inside
        // the RespawnExited match arm below.
        //
        // MUST NOT be re-derived after `destroy_session_inner` runs: post-
        // destroy, `find_active_session` returns None and the value would
        // silently flip, regressing the deferred-non-coord wake by losing
        // `--continue`. Set the flag inside the pre-destroy match arm only.
        // See plan §4.5.a / round-1 G7.
        let mut spawn_with_resume = false;
```

**Replace** the R2.3 step 2 `RespawnExited` arm body with a constant assignment + a "today vs future" comment that documents the seam Grinch G2.1 surfaced:

```rust
                    WakeAction::RespawnExited => {
                        // Today the only writer of `Exited(_)` is `mark_exited`,
                        // and its sole caller (`lib.rs:561`, deferred-non-coord at
                        // startup) passes literal `0`. Any RespawnExited match is
                        // therefore a known-state prior session worth resuming.
                        //
                        // If a future PR adds PTY exit-code surfacing
                        // (`portable_pty::Child::wait()` + `mark_exited(id, real_code)`),
                        // this is the seam to revisit — non-zero exits should likely
                        // become cold (cwd-vanished from in-place teardown, agent
                        // crash, OOM). See plan round-3 R3.1.
                        spawn_with_resume = true;
                        log::info!(
                            "[mailbox] wake: session {} is Exited (status={:?}), destroying before respawn",
                            session_id,
                            s.status
                        );
                        drop(mgr);
                        if let Err(e) =
                            crate::commands::session::destroy_session_inner(app, session_id).await
                        {
                            log::error!(
                                "[mailbox] wake: failed to destroy exited session {}: {}",
                                session_id,
                                e
                            );
                        }
                        // Fall through to spawn-persistent.
                    }
```

The `status={:?}` in the log line stays — it costs nothing and gives whoever investigates an actual exit code if `mark_exited` ever starts being called with non-zero. The R2.3 step-2 `spawn_with_resume={}` field in the log goes away (constant in this arm; redundant).

**Race fallthrough (inner `else`) — keep the R2.3 step 3 comment as written.** `spawn_with_resume` stays at its initial `false`, biasing the race toward cold. The R2 framing of this branch is unchanged under option 1.

**Spawn-fallback call site — keep the R2.3 step 4 substitution.** Either the inline `!spawn_with_resume` or the helper-form `wake_spawn_skip_auto_resume(spawn_with_resume)` (dev-rust D5/E11). Both are accepted under option 1; dev-rust round 3's call.

#### 4.5.a Ordering note (updated for R3.2)

The R2.3.a wording stays accurate: `spawn_with_resume` is set inside the pre-destroy match arm (now to a constant `true` rather than to the matches!() result), never re-derived from `find_active_session` after `destroy_session_inner` runs. R3.2's relaxed anchor comment at the variable declaration covers this. G7 still satisfied.

### R3.3 Updated §4.7 — doc comment phantom fix (G2.4 / E6 / D8 cascade)

R2 only updated §4.8's helper doc, not §4.7's `create_session_inner` function doc. Round-1 §4.7's body still reads "(default for fresh creates)" — a phrase that implies a default mechanism that does not exist (the function signature is `skip_auto_resume: bool`, not `Option<bool>`; every caller passes a value).

**Replace** the round-1 §4.7 doc-comment body (the bullets after "`skip_auto_resume` controls provider auto-resume injection:") with the following — explicit "fresh create" call sites listed, no phantom "wake-from-Exited-non-zero" reference per option 1:

```rust
/// `skip_auto_resume` controls provider auto-resume injection:
/// - `true` — suppress all provider auto-resume. Use this for any "fresh
///   create" call site (UI/CLI/root-agent create, mailbox wake-from-cold
///   meaning no SessionManager record at this CWD, `restart_session` with
///   default semantics from `effective_restart_skip_auto_resume`).
/// - `false` — allow provider auto-resume. Use this only for paths restoring
///   a session AC already knows about (the startup-restore loop in `lib.rs`,
///   the wake-from-known-state branch in `mailbox::deliver_wake` — any
///   `RespawnExited` match, today driven exclusively by deferred-non-coord
///   `Exited(0)` records — and `restart_session` when its caller passes
///   `Some(false)`).
```

The "today driven exclusively by deferred-non-coord `Exited(0)` records" parenthetical documents the reachable state space without scaffolding code for an unreachable one. If a future PR adds PTY exit-code surfacing, that parenthetical is the seam to update at the same time as the `deliver_wake` arm.

### R3.4 Updated §5 — supersedes R2.5 (matrix row 4)

| # | Call site | `skip_auto_resume` (before) | `skip_auto_resume` (after, **R3**) | Why |
|---|---|---|---|---|
| 4 | `phone/mailbox.rs:638` (`deliver_wake` spawn-fallback) | `false` | **`!spawn_with_resume`**, where `spawn_with_resume` defaults to `false` and is set to constant `true` inside the `RespawnExited` match arm only | Three sub-cases. (i) `find_active_session` → None: cold wake → `true`. (ii) Inject branch (Active/Running/Idle): early return, irrelevant. (iii) RespawnExited: any Exited match → known-state → `false` (today only `Exited(0)` reaches this arm; the integer-literal narrowing R2.3 added is semantically inert against current code per #82 G2.1, deferred to a future PTY-exit-detection PR). Inner-`else` race → cold (`spawn_with_resume` default carries). |

Other rows in §5 are unchanged.

### R3.5 Updated §7 — supersedes R2.6 (#10 added; #8/#9 unchanged)

R2.6 §7 #8 (orphan SessionManager records on PTY-spawn failure) stays as written — dev-rust E3 verified the orphan path stays at `Running`, not `Exited`, which means the spawn-fallback path is never reached for orphans (they'd hit Inject and write to a dead PTY). The framing is accurate; no #82 regression.

R2.6 §7 #9 (UI-destroy mid-wake race) stays as written.

**Add** §7 #10 (replaces the now-incorrect "R2.3 closed the in-place WG teardown lifecycle" framing the round-2 plan implied):

10. **In-place WG teardown without AC restart.** When a user `rm -rf`'s a WG dir while AC stays running, the agent's PTY's read loop hits EOF and the read thread terminates, but the session's `status` field stays at `Running`/`Active`/`Idle` because no code path surfaces the child's exit code (`portable-pty`'s `_child` is held as `_child` and never `.wait()`-ed; verified in grinch G2.1). Subsequent wake events route through `WakeAction::Inject` → `inject_into_pty` writes to a dead PTY master writer. On Windows ConPTY this is typically a silent buffered write (the bytes go nowhere); on POSIX it produces a broken-pipe error. **The wake silently fails to deliver.** This is a separate failure mode from the original #82 ghost-dir bug — different lifecycle, different symptom, different fix. It would be addressed by a future PTY-exit-detection PR (which would surface the child exit code via `mark_exited(id, code)`, transition the session to `Exited(code)`, and route subsequent wakes through `RespawnExited` with the code-aware discrimination R2.3 originally proposed). Out of #82 scope. Tracking: open as a separate issue if the user reports the silent-wake symptom.

§9.2 wording (round-2 dev-rust E3 already updated it with the "Status (round-2 update)" annotation) — stays accurate. The "R2.3 closes the in-place teardown lifecycle" implication that may have been read into §7 #8 / §9.2 is removed by R3.5 #10's explicit reframing.

### R3.6 Updated §8 — supersedes R2.7 (drop §8.4.D2 + drop E7 helper + drop E7 tests)

**Drop §8.4.D2 entirely** (the "in-place WG teardown without AC restart" repro). Per grinch G2.1, the precondition (session moves to `Exited(non-zero)`) is unproducible against current code. A tester following these steps would see the session stay at `Running` and report the test as un-runnable. Removing the test is more honest than dev-rust E8's "verify via log line" cleanup, which only papers over the unreachable state.

**Drop E7's `wake_resume_for_exited` helper and its three tests.** Under option 1, the discriminator the helper pinned (`matches!(_, Exited(0))`) is replaced by a constant `true`. The helper is moot. Grinch G2.3's bogus `#[should_panic]` test concern is moot by the same token.

**Keep** R2.7's other test additions:
- §8.1 #10 (`should_inject_continue_returns_false_when_continue_with_value_present`) — still valid, R2.4 predicate change is unaffected by G2.1.
- §8.1 G4 strengthening (rename test #2 to `should_inject_continue_returns_false_when_skip_overrides_existing_dir`, explicit fixture) — still valid.
- §8.2 `wake_spawn_skip_auto_resume(spawn_with_resume)` helper — still valid as a regression fence on the `!spawn_with_resume` inversion. Dev-rust D5/E11 endorsements still apply.

**Manual repro list** is now §8.4 A–E plus the symmetric-scope check from round 1, no D2. The dropped D2 was an addition R2 introduced; reverting it returns the manual-repro count to round-1's set.

### R3.7 Items I accept from grinch round 2

- **G2.1** — addressed via R3.1 / R3.2.
- **G2.2** — round-1 G1 retraction acknowledged. R3.5 #10 reframes the in-place teardown lifecycle correctly.
- **G2.3** — moot under option 1 (E7 helper dropped).
- **G2.4** — addressed via R3.3.
- **G2.5–G2.10** — informational; verified consistent with R3.

### R3.8 Items I reject from grinch round 2

**None.** All findings either incorporated, accepted-and-deferred, or correctly framed as informational by grinch themselves.

### R3.9 Items dev-rust round 3 should apply (no architect review needed)

These are mechanical edits inherited from R2 + R3, queued for dev-rust round-3 enrichment to apply directly:

1. **Drop §8.4.D2** entirely (R3.6).
2. **Drop the E7 helper text and its three tests** from the round-2 dev-rust enrichment (R3.6).
3. **Replace R2.3 step 1's anchor comment** per R3.2 (relaxed wording).
4. **Replace R2.3 step 2's `RespawnExited` arm body** per R3.2 (constant `true`, `today vs future` comment).
5. **Replace R2.5's matrix row 4** per R3.4.
6. **Update §7** per R3.5 (add #10, drop any "R2.3 closed it" implication if present anywhere else).
7. **Update §4.7 doc-comment body** per R3.3 (drop "wake-from-Exited-non-zero" reference).
8. Carry forward all other R2 edits unchanged: §4.8 predicate fix (R2.4), §8.1 #10 test (R2.7), §8.2 helper (R2.7), §7 #8/#9 (R2.6), §4.7 line-range fix (R2.6/E2), §9.2 E3 wording (already applied).

### R3.10 Items still needing user input (unchanged from R2.10)

- **§9.1 codex/gemini symmetric scope.** All four of us (architect, dev-rust, grinch, tech-lead) lean symmetric. User pinged twice, no answer yet. Doc comments and tests are written assuming symmetric. Continues to be non-blocking.
- **User lifecycle confirmation** — now demoted to informational only. R3.5 #10 documents the in-place teardown lifecycle as a separate out-of-scope failure mode; user's answer informs PR-narrative emphasis but does not change the fix.

### R3.11 Done state (R3 supersedes R2.11)

- Six callsite values reflect the matrix in R3.4. Only one path (`mailbox.rs:638` spawn-fallback) carries a runtime-computed value (`!spawn_with_resume`); all others are constant.
- `should_inject_continue` extracted; predicate covers `--continue`, `--continue=<value>`, `-c` (R2.4).
- `mailbox::deliver_wake` carries `spawn_with_resume`, defaulting to `false` and set to constant `true` inside the `RespawnExited` arm (R3.2). No discriminator on `s.status`. Helper `wake_spawn_skip_auto_resume(spawn_with_resume)` extracted (R2.7) for the inversion test fence; the `wake_resume_for_exited` discriminator helper (E7) is dropped.
- Comment updates: `commands/session.rs:255-261` (R3.3), the new variable declaration in `deliver_wake` (R3.2), the `RespawnExited` arm body (R3.2), `lib.rs:594` (round-1 §4.6), and `should_inject_continue`'s doc (R2.4).
- New unit tests: `should_inject_continue` #1–#10 (incl. G2 `--continue=value` test, G4 explicit fixture), `wake_spawn_skip_auto_resume` two tests (R2.7).
- Manual repros A–E from round 1 (no D2). Symmetric-scope check from round-1 §8.4.E retained pending §9.1 user decision.
- Branch `fix/82-no-continue-on-fresh-session` ready for dev-rust round-3 enrichment, then grinch round-3 review.

The plan stops here.

End of architect round 3.

---

## Dev-rust enrichment (round 3)

**Reviewer:** dev-rust (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Reading:** grinch G2.1–G2.10 + architect R3.1–R3.11 + my own round-2 E-points (with E7/E8/E9 retracted) + tech-lead's round-3 brief.
**Source state:** unchanged — branch tip = `origin/main` @ `96860c0`, no commits on `fix/82-no-continue-on-fresh-session` yet (the plan file is still the only untracked path).
**Convergence target:** make this the last plan-iteration round. Per Role.md Rule 5, minority opinion loses if no consensus emerges after grinch round 3.

### F1. Concurrence with R3 — option 1 is correct

Endorse R3.1's option 1 in full. Three independent reasons, all already covered in the architect's rationale:

1. **The round-1 plan was already correct for the reachable state space.** Grinch G2.1's exhaustive trace of `mark_exited` writers (one caller, `lib.rs:561`, literal `0`) plus the PTY read-loop's `_child` non-waiting (`pty/manager.rs:363, 382-453`) is conclusive. R2's discrimination was scaffolding for an unreachable state.
2. **Dormant code is a maintenance hazard.** A future contributor seeing `matches!(s.status, SessionStatus::Exited(0))` reasons from a false model. Best-case outcome: confusion. Worst-case: a "simplification" to `Exited(_)` that breaks a then-active discrimination if PTY-exit detection lands later, with the comment chain rotted out by then.
3. **The future PTY-exit-detection PR is the right time, with full context.** When a PR adds `portable_pty::Child::wait()` and routes the exit code into `mark_exited`, it will need to handle broken-pipe vs EOF, read-loop teardown, status-event emission to the frontend sidebar, and Telegram bridge teardown. Pre-emptively scaffolding here without that surrounding context is speculative.

I also retract my round-2 E11 endorsement of "R2.1 G1 → option (a)". Grinch G2.2 retracted his round-1 G1; R2.1 inherited the false framing; R3.1 properly steps back. My E11 row 1 is now superseded (the option-(a) decision is gone). Same for my E11 row 7 ("R2.7 helper rename" — I endorsed the rename, which still stands; the augmentation-via-E7 part of that row is the part that's gone).

### F2. Mechanical edits applied this round (per R3.9)

Status of the seven items in R3.9, with what I did this round:

| R3.9 # | Item | Action this round |
|---|---|---|
| 1 | Drop §8.4.D2 entirely | **Applied.** R2.7's `#### §8.4 manual repro additions` body replaced with a one-paragraph retraction marker. |
| 2 | Drop E7 helper text + 3 tests | **Applied.** My round-2 E7 + E8 + E9 (all three were predicated on the dropped discriminator) replaced with a `### E7-E9 [ROUND-3 RETRACTION]` marker that points at this round. |
| 3 | Replace R2.3 step 1's anchor comment | **Implementation-time.** R3.2 already supersedes R2.3 in the plan structure; the canonical anchor comment text is in R3.2 step 1 ("Default false: cold wake — either no SessionManager record at this CWD, or the matched record vanished from list_sessions before we could read it"). Implementer applies R3.2's text, not R2.3's. |
| 4 | Replace R2.3 step 2's RespawnExited arm body | **Implementation-time.** Same: R3.2 step 2 is canonical. The implementer writes `spawn_with_resume = true;` plus the "today vs future" comment from R3.2, not R2.3's `matches!()` form. |
| 5 | Replace R2.5's matrix row 4 | **Implementation-time.** R3.4 IS the new row 4. Implementer references R3.4 when applying §5. |
| 6 | Update §7 (add #10) | **Implementation-time.** R3.5 introduces #10. Implementer treats §7 as R2.6 #8/#9 + R3.5 #10. |
| 7 | Update §4.7 doc-comment body | **Implementation-time.** R3.3 IS the new doc-comment body. Implementer applies R3.3, not the round-1 §4.7 body or my round-2 E6 wording (which still mentioned the now-phantom "wake-from-Exited-non-zero" branch). |
| 8 | Carry forward R2 edits unchanged | **Inherent.** §4.8 (R2.4), §8.1 #10 (R2.7), §8.2 helper (R2.7), §7 #8/#9 (R2.6), §4.7 line range (E2/G6), §9.2 wording (E3 — re-edited this round to drop the now-stale R2.3 framing while preserving the reasoning chain). |

For items 3-7: I deliberately did **not** physically rewrite R2.3 / R2.5 / R2.6 / round-1 §4.7 body in place. The plan's round-by-round structure encodes the supersession via R3.X explicit "supersedes R2.Y" markings. Editing R2.X in place would lose the audit trail of how the team converged. This is consistent with tech-lead's "smallest delta posture" — only items that are factually wrong (E7 helper, §8.4.D2 repro) were physically deleted; supersession via "supersedes" markings handles the rest.

If grinch round 3 prefers physical rewrites of the architect-authored R2.X sections, I can apply them; flagging the choice for that round.

### F3. §9.2 wording re-edited this round (cascade from R3.1)

My round-2 E3 update to §9.2 referenced "R2.3's `Exited(0)`-only discriminator". Under R3, that discriminator no longer exists. The reasoning chain (orphan stays at `Running` → `wake_action_for(Running) = Inject` → spawn-fallback never reached) is still valid, but the framing was anachronistic.

This round: replaced the "R2.3's `Exited(0)`-only discriminator" phrasing with neutral wording that holds under both R2 and R3. Also added the §7 #10 cross-reference (in-place teardown lifecycle, separately tracked) for completeness.

### F4. §7 #10 PTY-write claim — verified against pty/manager.rs

R3.5 #10 claims:

> Subsequent wake events route through `WakeAction::Inject` → `inject_into_pty` writes to a dead PTY master writer. On Windows ConPTY this is typically a silent buffered write (the bytes go nowhere); on POSIX it produces a broken-pipe error.

Verified against `pty/manager.rs`:

- Line 17: `_child: Box<dyn portable_pty::Child + Send + Sync>` — child held, never `.wait()`-ed.
- Line 363: instantiation as `_child: child` (intentionally `_`-prefixed, no waiter task).
- Lines 382-453: read-loop terminates on `Ok(0)` EOF (line 386) or `Err(_)` (line 450) without `mark_exited` and without surfacing the child's exit code.
- Lines 458-473: `write()` calls `writer.write_all(data)` + `writer.flush()`, both errors propagated as `AppError::PtyError(e.to_string())`.

`inject_into_pty` (`mailbox.rs:694+`) receives the PtyError via `mgr.write(...)?` and returns `Err(format!("PTY write failed: ..."))`. The error propagates up through `process_message`; the message gets moved to `rejected/` and the wake originator does **not** see a synchronous failure response (mailbox is fire-and-forget).

The architect's "silent on Windows / broken-pipe on POSIX" framing is a claim about the underlying portable-pty / OS behavior, which I cannot independently verify from inside the AC repo (would require running the lifecycle on each OS and observing). What I **can** confirm:

- AC's write path correctly converts the OS-level error (whatever it is) to an `AppError::PtyError`.
- The error is logged via `log::error!` at the mailbox layer.
- The user-visible signal of failure is the rejected-message file plus a log line. Neither propagates as a UI-level notification today.

**Caveat to flag for grinch:** the OS-specific framing in R3.5 #10 ("typically a silent buffered write" on Windows) is plausible but not verified from within this repo. If grinch wants stronger evidence, an external test (run AC on Windows, `rm -rf` cwd of a live agent, observe write behavior on next wake) would settle it. Not blocking; framing is consistent with what's commonly known about ConPTY write semantics. Just calling out the verification limit for honesty.

### F5. Cross-check of round-1/2 references against R3 simplification

For each prior-round reference that touched the dropped discriminator or §8.4.D2 manual repro, my disposition:

| Prior-round reference | Touches dropped state? | Disposition |
|---|---|---|
| D1 line-number table | No | Unchanged. |
| D2 (find_active_session note) | No | R2.3 explicitly preserves it; R3 unchanged. |
| D3 (codex/gemini global-resume) | No | Unchanged. |
| D4-D5 (test additions) | No | Carried forward unchanged via R3.6. |
| D6 (helper location) | No | Unchanged. |
| D7 (test imports) | No | Still applies for `commands/session.rs:1264`. |
| D8 (§4.7 doc rewrite) | Marginally — round-1 D8 didn't mention the dropped state; my round-2 E6 expansion **did** mention "wake-from-Exited-non-zero". R3.3 is the canonical doc body, which drops the phantom. | E6 left as historical record (not factually wrong as a R2-era proposal); R3.3 supersedes. Implementer uses R3.3. |
| D9 (debug log on wake) | Marginally — subsumed by R2.3's augmented info-level log. R3.2's log line keeps `status={:?}` formatting, which still surfaces the exit code in the future PR's eventual scenario. | Unchanged; subsumption holds under R3.2. |
| D10 (user-config edge case) | No | Unchanged. |
| D11-D15 | No | Unchanged. |
| E1 line-number table row about session.rs:98 | Marginally — the `matches!(s.status, SessionStatus::Exited(0))` "is well-formed" verification is no longer load-bearing. The variant `Exited(i32)` still exists. | Row left as-is (factually correct); not load-bearing under R3 and the row's reader can see this from context (R3.2's body has no matches!()). |
| E2 (G6 line-range) | No | Applied in round 2; unchanged. |
| E3 (G3 §9.2) | Yes — referenced R2.3's discriminator | **Re-edited this round (F3).** |
| E4 (D7 scope) | No | Unchanged. |
| E5 (D2 status) | Marginally — references R2.3's "D2 still accurate" note. R3.2 inherits that statement. | Unchanged; the underlying claim still holds. |
| E6 (D8 prose) | Yes — mentioned "wake-from-Exited-non-zero" | E6 left as historical record. R3.3 is the canonical implementation. |
| E7-E9 | Yes — entirely predicated on the discriminator | **Dropped this round (replaced with retraction marker).** |
| E10 (§9.1 user pending) | No | Unchanged. |
| E11 (R2 endorsements) | Partly — endorsed R2.1 option (a) and "augmented by E7" | Left as historical record. F1 explicitly retracts the R2.1 row and the E7-augmentation portion of the R2.7 row. |
| E12-E13 (summaries) | Partly — proposed E7 helper | Left as historical record. F2 documents the R3.9 disposition. |
| E14 (round-2 verdict) | Partly — referenced E7 as "the one substantive recommendation" | Left as historical record. F8 (this round's verdict) supersedes. |

No factually-wrong content remains in prior-round dev-rust sections after this round's edits.

### F6. Items the team may still want to discuss

These are **not** blockers; flagging in case grinch round 3 wants to redirect:

1. **`spawn_with_resume` could become unconditional in the RespawnExited arm.** Under R3.2 step 2, the arm assigns `spawn_with_resume = true;` unconditionally. The variable's only "knob" is then the cold-vs-known distinction at the function-entry level (default `false`, set to `true` inside RespawnExited). This is fine, but a reader could legitimately ask: "why not just `spawn_with_resume = false` at fn-entry, `true` in RespawnExited, and skip the arm-internal assignment by initializing inside the arm?"

   **Answer:** The arm-internal write keeps the value live across the `drop(mgr)` and `destroy_session_inner(...)` calls (they happen inside the arm). Initializing the variable later (e.g., in the spawn-fallback) loses the pre-destroy state, exactly the §4.5.a invariant. R3.2's structure is correct; the apparent redundancy is the cost of the "set before destroy, read after destroy" pattern. Anchor comment at the variable declaration documents this. ✅

2. **`wake_spawn_skip_auto_resume` helper still earns its keep under R3.** Even though the discriminator is gone, the inversion at the call site (`!spawn_with_resume`) is still real. The helper documents the inversion semantic, the test fences the inversion. Cost stays at 3 + 6 lines. R3.6 keeps it; I endorse keeping it. (This row of E11 is unchanged under R3.)

3. **§4.7 doc-comment "today driven exclusively by deferred-non-coord `Exited(0)` records" parenthetical.** R3.3's body includes a parenthetical that names the today-only-reachable subcase. This is the right call — it documents the dormant seam without creating dormant code, AND it gives whoever picks up PTY-exit-detection a textual breadcrumb to update at the same time. Slight nit: "any `RespawnExited` match" might read confusingly to a reader who hasn't internalized the AC `wake_action_for` mapping. Possible (very minor) clarification: prepend "in `mailbox::deliver_wake`'s `WakeAction::RespawnExited` arm — today driven …". Not blocking.

### F7. Open user/architect items (unchanged from R2.10 / R3.10)

- **§9.1 codex/gemini symmetric scope.** All four of us lean symmetric. User pinged twice, no answer. Doc comments and tests assume symmetric. Continues to be non-blocking for grinch round 3.
- **User lifecycle confirmation.** R3.10 demoted this to informational. R3.5 #10 documents the in-place teardown lifecycle as a separate out-of-scope failure mode. User's answer informs PR-narrative emphasis only.

### F8. Round-3 verdict

**Plan is technically correct after R3.** The mechanical edits R3.9 #1-#2 are applied to the plan file (drops); items #3-#7 are encoded as supersession via the round-by-round structure (R3.2 supersedes R2.3, etc.); item #8 carries forward unchanged.

No new HIGH findings. No MEDIUM findings. No LOW findings beyond the very-minor F6 #3 wording nit which is grinch's call.

**Ready for grinch round 3.** Convergence target met from my side: this is the last round of plan-iteration I have substantive input for. If grinch round 3 finds nothing, the plan can move to implementation.

End of dev-rust round-3 enrichment.

---

## Grinch adversarial review (round 3)

**Reviewer:** dev-rust-grinch (wg-1-dev-team)
**Date:** 2026-04-25 (UTC)
**Reading:** architect R3.1-R3.11 + dev-rust F1-F8 + my own G2.1-G2.10 (with G2.2 round-1 G1 retraction).
**Source state:** unchanged — branch tip = `origin/main` @ `96860c0`, no commits on `fix/82-no-continue-on-fresh-session` yet.
**Scope assumption:** symmetric (per tech-lead's standing instruction; user pinged twice, no answer).

I tried to break round 3. The architect's option-1 simplification is correct, the variable rename + scope-move + constant substitution is mechanically clean, the retraction-marker approach is acceptable, and §9.2's re-edit is genuinely neutral. I have one LOW wording-precision finding on §7 #10 (out-of-scope failure mode), one F6 #3 cosmetic disposition, and several explicit endorsements. **No new HIGH. No new MEDIUM.**

### G3.1. (LOW) §7 #10 framing of the in-place-teardown silent-wake symptom is asymmetric across platforms

**What.** R3.5 #10 says:

> Subsequent wake events route through `WakeAction::Inject` → `inject_into_pty` writes to a dead PTY master writer. On Windows ConPTY this is typically a silent buffered write (the bytes go nowhere); on POSIX it produces a broken-pipe error. **The wake silently fails to deliver.**

The framing collapses two distinct user-visible behaviors into "silently fails":

- **Windows path** (silent buffered write): `mgr.write(...)` returns `Ok(())`. `inject_into_pty` returns `Ok(())`. `deliver_wake` returns `Ok(())`. `process_message` calls `move_to_delivered(...)`. **The mailbox layer records the wake as successfully delivered.** The sender's outbox file moves to `delivered/`. From the sender's POV the wake succeeded; the recipient never received it. This is "false-positive delivery" — strictly worse than a visible failure.
- **POSIX path** (broken-pipe): `writer.write_all(data)?` propagates `AppError::PtyError` → `inject_into_pty` returns `Err` → `deliver_wake` returns `Err` → `process_message` propagates → `poll` increments `attempt_count` and retries. Verified at `mailbox.rs:184-194` with `MAX_DELIVERY_ATTEMPTS = 10` (`mailbox.rs:23`) and `poll_interval = 3s` (`mailbox.rs:94`): up to 10 attempts × 3s = ~30s of retries before the message is permanently rejected. Each retry logs `log::warn!` at line 197-209. The user sees nothing in the UI; the file lingers in the outbox for half a minute, then moves to `rejected/`.

**Why it matters.** A future reader of §7 #10 trying to triage a "wake didn't arrive" report will reach for the wrong hypothesis. The Windows symptom is "appears delivered, isn't" — much harder to diagnose than the framing implies. The POSIX symptom is "30 seconds of retries with warn-level logs" — also not "silent" in the literal sense. The architect's framing is defensible at a high level (no UI surface either way), but the asymmetry matters for whoever opens the follow-up issue.

Dev-rust F4 already flagged the verification limit ("the OS-specific framing... is plausible but not verified from within this repo") but did not call out the Windows false-success-delivery vs POSIX retry-then-reject distinction.

**Failing input / scenario.** A future user reports "I sent a wake message; my agent says it never arrived." A maintainer reads §7 #10, sees "silently fails to deliver," and asks the user "did you check if the message went to rejected/?" — which is the right question on POSIX but the WRONG question on Windows (the message will be in delivered/, not rejected/). Wasted triage cycle.

**Suggested fix.** Tighten R3.5 #10 to explicit per-platform symptoms:

> Subsequent wake events route through `WakeAction::Inject` → `inject_into_pty` writes to a dead PTY master writer. **Two distinct symptoms by platform** (out of scope for #82, framing for whoever opens the follow-up issue):
> - **Windows ConPTY:** the master writer's `write_all` typically succeeds (bytes buffer into the now-orphaned ConPTY pipe). `inject_into_pty` returns `Ok(())`, the mailbox layer records the wake as delivered, the sender's outbox file moves to `delivered/`. **The wake appears successful but the agent never receives the message.**
> - **POSIX:** `write_all` returns a broken-pipe error. `inject_into_pty` propagates `AppError::PtyError`, `deliver_wake` returns `Err`, the poller retries up to `MAX_DELIVERY_ATTEMPTS = 10` times at `poll_interval = 3s` (~30s total) before moving the message to `rejected/`. Each retry logs `log::warn!` at `mailbox.rs:197-209`.
>
> Either way, no UI-level notification surfaces to the user. The Windows path is harder to diagnose (no error logs, false-positive delivery) and is the more concerning of the two symptoms.

This is **out-of-scope description quality**, not a fix. R3.5 #10 is in §7 (deliberately untouched / out-of-scope), so this LOW does not block the PR. Suggest applying as a follow-up wording polish or leaving for whoever opens the follow-up issue. **Not blocking round 3.**

**Note on dev-rust F4's verification limit:** F4 was honest about not being able to verify the Windows-vs-POSIX OS-layer behavior from inside the repo. The above suggested wording captures what the AC-side code does deterministically (`write_all` → `Ok` vs `Err`; `MAX_DELIVERY_ATTEMPTS`; `move_to_delivered` vs `move_to_rejected`) without overcommitting on the OS-layer claim — the only OS-layer claim is "write to a dead ConPTY master typically succeeds" / "write to a dead POSIX pty master returns EPIPE", both of which match documented `portable-pty` behavior. If the team wants strict verification, the external test F4 proposed (Windows + POSIX manual repro) is the way; not blocking for #82.

### G3.2. (informational) R3.2 simplification — control flow verified clean

Verified each fall-through path to spawn-fallback under R3.2:

1. `find_active_session` returns `None` → outer `if let Some` doesn't enter → spawn-fallback reads `spawn_with_resume = false` (default). ✓
2. Outer matches, inner `find` returns `Some(s)`, `WakeAction::Inject` → returns from function via `inject_into_pty` (line 553); spawn-fallback unreached.
3. Outer matches, inner `find` returns `Some(s)`, `WakeAction::RespawnExited` → arm body sets `spawn_with_resume = true;` BEFORE `drop(mgr)` and BEFORE `destroy_session_inner`. Falls through to spawn-fallback, which reads `true`. ✓
4. Outer matches, inner `find` returns `None` (concurrent destroy race) → else branch logs warn and `drop(mgr)`. `spawn_with_resume` stays at default `false`. Falls through. Spawn-fallback reads `false`. ✓

No dead code. No dangling references to `matches!(s.status, SessionStatus::Exited(0))` in canonical R3 sections — only in retracted/historical R2.3 (clearly marked as superseded by R3.2). The variable rename `had_prior_session` → `spawn_with_resume` is consistently applied across R3.2/R3.4/R3.6/R3.11. R2.3 step 3 (race fallthrough comment) is preserved unchanged under R3.2 — the inner-`else` branch's logic is unaffected by the discriminator drop.

R3.2 step 1 anchor comment continues to satisfy G7's "MUST NOT be re-derived after destroy" invariant. The relaxed phrasing — "set to true only inside the RespawnExited match arm" — is still accurate; the post-destroy re-derivation hazard is still real (a refactor that adds `spawn_with_resume = self.find_active_session(...).await.is_some()` after destroy would silently flip true to false for the deferred-non-coord wake).

R3.2's mechanical correctness is clean.

### G3.3. (informational) §7 #10 — verified what can be verified inside the repo

I verified the AC-side handling against `pty/manager.rs` and `phone/mailbox.rs`:

- **Read loop** (`pty/manager.rs:382-453`): breaks on `Ok(0)` EOF (line 386) or `Err(_)` read failure (line 450) without calling `mark_exited` or surfacing the child's exit code. Thread terminates; `PtyInstance` stays in `self.ptys` keyed by session UUID. Confirms G2.1's exhaustive trace.
- **Child handle**: held as `_child: child` at line 363 (the `_` prefix is conventional Rust for "intentionally unused"). No `child.wait()` or `child.try_wait()` anywhere in the spawn or destroy paths. Confirmed via grep on `try_wait|wait\(\)|child\..*exit` over `src-tauri/src/pty/`.
- **Write path** (`pty/manager.rs:458-473`): both `writer.write_all(data)` and `writer.flush()` are propagated as `AppError::PtyError(e.to_string())`. No panic, no swallow.
- **Mailbox retry policy** (`mailbox.rs:18-23, 88, 184-225`): `RetryState { attempt_count }` per outbox file, `MAX_DELIVERY_ATTEMPTS = 10`, `poll_interval = 3s` (`mailbox.rs:94`). Permanent-error class includes `ERR_UNRESOLVABLE_AGENT` (line 24) which is fast-fail; PTY-write failures are NOT in that class, so they go through the full retry loop.

No panic risk, no hang risk in AC's code. The OS-layer claim ("silent buffered write on Windows / broken-pipe on POSIX") is consistent with documented `portable-pty` and ConPTY/PTY semantics, but I cannot independently verify it without running the lifecycle on each platform — same limit dev-rust F4 acknowledged. Concur with F4's verification disclaimer.

The architect's framing is accurate at a high level; my G3.1 finding refines it to be platform-explicit so future triage doesn't waste cycles on the wrong hypothesis.

### G3.4. (informational) Retraction-marker approach — endorse

Dev-rust F2-F3 chose retraction markers over physical deletion of architect-authored R2 prose. I endorse this for three reasons:

1. **Clarity for the implementer.** The marker explicitly says "[ROUND-3 RETRACTION (R3.6 / R3.9 #1): the §8.4.D2 manual repro that originally lived here has been dropped. ... Body removed for clarity." An implementer reading the plan top-to-bottom hits the marker at line 1212 and at line 1325 before reaching anything that would depend on the dropped content. R3.6 also explicitly says "Drop §8.4.D2 entirely" so the redundant signal is in the canonical section too.
2. **Audit trail value is real here.** This is a multi-round iteration where round-2 produced a NEW HIGH (G2.1) that none of us caught in round 1. Future readers (incl. whoever picks up the PTY-exit-detection follow-up issue) will benefit from being able to trace HOW the team converged: the failed discrimination, the retraction, the simplification. Physical deletion would erase that.
3. **Architect's "drop" intent is preserved.** R3.9 #1-#2 say "drop §8.4.D2 entirely" / "drop the E7 helper text and its three tests" — the *effect* is dropping (no implementer acts on it). Dev-rust's retraction markers achieve the effect; the precise editorial form is dev-rust's editorial call to make. F2's reasoning is sound.

If tech-lead or architect explicitly prefers physical rewrites for any reason (e.g., file-size or grepability), that's a follow-up editorial pass; not blocking round 3.

**Verification:** I grepped for stale references to dropped content. Findings:
- `Exited(non-zero)` references: appear in G2.1 (the finding itself), retracted R2.3 (clearly under "supersedes" hierarchy), R3.5 #10 (out-of-scope reframing), and F4 (verification note). None are load-bearing under R3.
- `matches!(s.status, SessionStatus::Exited(0))` references: appear in retracted R2.3 only. No references in canonical R3 sections.
- `§8.4.D2` references: appear in the retraction marker (line 1212), in R3.6 ("drop §8.4.D2 entirely"), in F2/F5 disposition tables, and in G2.1 (the original finding). All are clearly historical or canonical-drop. No implementer-targeted "perform this manual repro" references survive.
- `wake_resume_for_exited` (E7 helper): appears in retracted E7-E9 marker (line 1325-onward), F1 retraction, F5 row, G2.3, R3.6 ("drop the E7 helper text and its three tests"). All clearly historical or canonical-drop.

The cascade is clean. Dev-rust F5's disposition table is accurate.

### G3.5. (informational) §9.2 re-edit verified neutral

The re-edited §9.2 (line 525-527 with the "Status (rounds 2-3 update per grinch G3/G2.1 / dev-rust E3+F1)" annotation) reads:

> This question is moot for the spawn-fallback path. PTY-spawn failure inside `create_session_inner` (lines 457-461) does not call `mgr.mark_exited`, so an orphan `SessionManager` record from a `failed_recoverable` lifecycle stays at status `Running`, not `Exited(_)`. `wake_action_for(Running) = Inject`, which routes through `inject_into_pty` (line 553) and never reaches the spawn-fallback where `spawn_with_resume` is read. ...

Verified neutral against both R2 and R3 framings:
- Doesn't reference the dropped `Exited(0)` discriminator.
- References §7 #8 (orphan tracking, R2.6 #8 unchanged) and §7 #10 (in-place teardown, R3.5 added).
- The reasoning chain (PTY-spawn fail → no mark_exited → orphan stays Running → wake_action_for=Inject → spawn-fallback unreached) is independent of whether R2.3's discrimination or R3.2's constant-true is in force — under both, the spawn-fallback branch is avoided for orphans.
- The variable name `spawn_with_resume` is the rename that R3 inherits from R2.3 step 1 (kept under R3); §9.2 is consistent.

F3's claim of neutrality is accurate.

### G3.6. (informational) F6 #3 cosmetic nit — skip

Dev-rust F6 #3 floated a possible refinement to R3.3's doc-comment phrase "any `RespawnExited` match" — prepending "in `mailbox::deliver_wake`'s `WakeAction::RespawnExited` arm". I propose **skipping** the refinement: R3.3's existing wording already names the code site (`mailbox::deliver_wake`) one bullet earlier ("the wake-from-known-state branch in `mailbox::deliver_wake`"), so the proposed prepend is redundant. Current text reads cleanly without the addition.

Not blocking either way; dev-rust correctly flagged it as cosmetic.

### G3.7. (informational) R3 endorsements

For the record:

| R3 item | Endorsement |
|---|---|
| R3.1 — option 1 (drop discrimination, keep variable + rename) | **Endorse.** Three reasons: round-1 was already correct for reachable states; dormant code is a maintenance hazard; future PTY-exit-detection PR is the right time. (Restating tech-lead's three reasons from the round-3 brief.) |
| R3.2 — `spawn_with_resume = true;` constant inside RespawnExited arm | **Endorse.** Mechanically correct, simpler than R2.3, anchor comment preserved per G7. |
| R3.3 — §4.7 doc-comment body fix (drops "(default for fresh creates)" phrase, drops phantom "wake-from-Exited-non-zero") | **Endorse.** Dev-rust D8/E6's wording correction applied correctly under R3 framing. |
| R3.4 — §5 matrix row 4 simplification | **Endorse.** Reads correctly; the "today only `Exited(0)` reaches this arm" parenthetical is honest about the reachable state space without scaffolding code. |
| R3.5 — §7 #10 (in-place teardown lifecycle) | **Endorse with G3.1 caveat** on the per-platform symptom precision. |
| R3.6 — drop §8.4.D2, drop E7 helper + tests; keep §8.1 #10, R2.7 G4 strengthening, §8.2 helper | **Endorse.** All retentions are still relevant under R3; all drops are correctly identified as moot. |
| R3.7 — accept G2.1-G2.10 | **Endorse.** Round-2 findings correctly incorporated. |
| R3.8 — reject nothing | **Endorse.** No grinch round-2 finding survives that should be rejected. |
| R3.9 — mechanical edits queue | **Endorse with retraction-marker approach** per G3.4. |
| R3.10 — user-input items | **Endorse.** §9.1 still pending but non-blocking; user lifecycle confirmation now informational. |
| R3.11 — Done state | **Endorse.** Accurate summary of canonical R3 plan content. |

### G3.8. (informational) F1-F8 endorsements

| F item | Endorsement |
|---|---|
| F1 — endorses R3.1 option 1, retracts E11 row 1 | **Endorse.** Three independent reasons; retraction is correct. |
| F2 — mechanical edits via retraction markers | **Endorse** per G3.4. |
| F3 — §9.2 re-edit neutral | **Endorse** per G3.5. |
| F4 — §7 #10 PTY-write claim verified within repo limits | **Endorse**, augmented by G3.1's per-platform precision and G3.3's confirmation. |
| F5 — disposition table | **Endorse** per G3.4 cascade verification. |
| F6 #1 — `spawn_with_resume` arm-internal write rationale | **Endorse.** The "set before destroy, read after destroy" pattern is correct; F6 #1's "apparent redundancy is the cost" framing is right. |
| F6 #2 — keep `wake_spawn_skip_auto_resume` helper under R3 | **Endorse.** Inversion is still real; helper still fences against accidental polarity flip. R3.6 keeps it. |
| F6 #3 — cosmetic doc nit | **Skip** per G3.6. |
| F7 — open user/architect items unchanged | **Endorse.** §9.1 / user lifecycle disposition is unchanged. |
| F8 — round-3 verdict | **Endorse.** Plan technically correct after R3. |

### G3.9. (informational) Round-3 retractions / corrections from grinch

None. My round-2 retractions (G2.2 round-1 G1; G2.10 round-1 G3 framing) stand. No round-3 retractions needed.

### G3.10. (informational) Anything new the team has missed AGAIN

I tried hard to find a new HIGH or MEDIUM. Areas I attacked:

- **Restart_session(Some(false)) interaction with ghost projects dir.** Could a deferred-coordinator wake via `restart_session` re-trigger #82 if the WG was torn down + recreated between AC restarts? Yes, but this is the same case as `lib.rs:594` restore-with-ghost-dir, which the plan explicitly defers via §6.1 to issue #40. Not a new finding; the plan correctly punts the restore-with-ghost-dir surface to #40.
- **Future-refactor risk on R3.2 step 1 anchor comment.** Could a future contributor remove `spawn_with_resume` as "unused"? The variable is initialized to false, set to true in only one branch, read at line 638 as `!spawn_with_resume`. A "simplification" that hoists the spawn-fallback into the RespawnExited arm would obviate the variable but break the cold-wake path (find_active_session=None → spawn-fallback unreached → cold wake never spawns). The anchor comment doesn't explicitly warn against the hoist. Speculative future-refactor; not a current bug.
- **`should_inject_continue` test fixture explicitness.** Round-1 tests #1, #3, #4, #5 and dev-rust D4 #6-#9, R2.7 #10 say "all else permissive" or similar. Only test #2 has explicit-fixture per G4. Implementer should make ALL fixtures explicit. Implementation detail; not a plan flaw.
- **`-c` interaction with codex.** Pre-existing; documented in R2.4 helper doc. ✓
- **Wake_action_for variant additions.** A future variant added to `WakeAction` enum would default `spawn_with_resume` to false (not match any of the two existing arms). Speculative; not a current bug.
- **PTY write hang possibility.** Verified `write_all` doesn't panic; theoretical hang is OS-layer, not AC-layer. Mailbox retry policy (10 × 3s) eventually rejects. Captured in G3.1.

Nothing new to escalate. The plan is internally consistent under R3 + F.

---

## Grinch Verdict (round 3): APPROVE

Severity counts: **0 HIGH, 0 MEDIUM, 1 LOW (G3.1 §7 #10 platform-precision wording), 9 INFORMATIONAL**.

The plan is technically correct. R3.1's option-1 simplification correctly drops the discriminator that G2.1 surfaced as scaffolding for an unreachable state. R3.2's `spawn_with_resume = true;` constant inside the RespawnExited arm is mechanically clean and the anchor comment preserves the §4.5.a/G7 invariant. R3.3's §4.7 doc-comment body fix correctly removes the phantom "wake-from-Exited-non-zero" reference and the "(default for fresh creates)" phrasing. R3.4's §5 matrix row 4 is honest about the reachable state space. R3.5 #10's in-place-teardown reframing is out-of-scope and correctly captured (with G3.1's per-platform precision refinement available as a follow-up wording polish). R3.6's drops of §8.4.D2 and the E7 helper are correctly identified as moot under option 1. The remaining R2 + dev-rust enrichment items (R2.4 predicate, R2.6 §7 #8/#9, R2.7 §8.1 #10 / G4 strengthening / §8.2 helper, R3.3 doc body, R3.5 #10) are all preserved correctly.

The retraction-marker approach (F2/F3) is editorially sound — the audit trail is genuinely valuable for a multi-round iteration where round-2 produced a NEW HIGH that none of us caught in round 1, and a future PTY-exit-detection PR will benefit from being able to trace how the team converged.

**The single LOW (G3.1) is wording-precision in an out-of-scope §7 entry. It does not block the PR. Suggest applying the per-platform tightening as a follow-up wording polish or leaving for whoever opens the silent-wake follow-up issue.**

Per Role.md "approval is never a courtesy": I tried to find a way to make this plan fail, including:
- Re-checking R3.2's control flow against the §4.5.a invariant. Clean.
- Re-grepping for stale references to dropped content. Clean.
- Cross-checking the §9.2 re-edit against both R2 and R3 framings. Neutral.
- Probing for new edge cases not covered (restart_session+ghost dir, future refactor risks, test fixture gaps, write hang). All either covered, deferred to existing issues, or speculative future-refactor risks.

I genuinely could not find a way to make this plan fail. The fix correctly closes the user's reported #82 lifecycle for the reachable state space; the deferred lifecycles (#40 restore-with-ghost-dir, R3.5 #10 in-place teardown silent wake, R2.6 #8 orphan PTY-spawn-failure, R2.6 #9 UI race) are correctly identified as out-of-scope and individually tracked.

**Verdict: APPROVE.**

**Items needing user input** (unchanged from R3.10 / R2.10):
- §9.1 codex/gemini symmetric scope (pinged twice, no answer; all four of us lean symmetric, plan written assuming symmetric).

**Items resolvable in-team** (post-approval, optional):
- G3.1 §7 #10 per-platform wording polish (follow-up wording PR if anyone notices).
- F6 #3 cosmetic doc nit (skip per G3.6).

**Items that will need a future PR** (out of #82 scope, tracked by §7 entries):
- §7 #8: orphan SessionManager records on PTY-spawn failure (R2.6).
- §7 #9: UI-destroy mid-wake race (R2.6).
- §7 #10: in-place WG teardown silent-wake symptom (R3.5; G3.1 precision refinement).
- PTY exit-code surfacing (`portable_pty::Child::wait()` + `mark_exited(id, real_code)`) — would reactivate the discrimination R2.3 originally proposed; correct implementation timing per R3.1 reason 3.

End of grinch round-3 review.
