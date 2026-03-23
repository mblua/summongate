# PLAN: Git Branch Polling per Session

## Goal

Display the current git branch next to each session name in the sidebar. This lets users identify which worktree/branch an agent (e.g., Claude Code) is working in.

## Approach

Poll `git rev-parse --abbrev-ref HEAD` in each session's `working_directory` every 5 seconds. When the branch changes (or is first detected), update the session and notify the frontend.

**Scope:** MVP uses the session's initial `working_directory` (set at spawn). Dynamic CWD tracking (OSC 7 or Windows API) is a future enhancement.

---

## Backend Changes

### 1. Add `git_branch` to Session struct

**File:** `src-tauri/src/session/session.rs`

- Add `git_branch: Option<String>` to `Session`
- Add `git_branch: Option<String>` to `SessionInfo` (serialized as `gitBranch`)
- Update `SessionInfo::from()` to include the new field

### 2. Add methods to SessionManager

**File:** `src-tauri/src/session/manager.rs`

```rust
pub async fn set_git_branch(&self, id: Uuid, branch: Option<String>) {
    let mut sessions = self.sessions.write().await;
    if let Some(s) = sessions.get_mut(&id) {
        s.git_branch = branch;
    }
}

pub async fn get_sessions_directories(&self) -> Vec<(Uuid, String)> {
    let sessions = self.sessions.read().await;
    sessions.iter().map(|(id, s)| (*id, s.working_directory.clone())).collect()
}
```

### 3. Create GitWatcher module

**File:** `src-tauri/src/pty/git_watcher.rs` (new)

**Key design decisions (from review):**
- Holds `Arc<tokio::sync::RwLock<SessionManager>>` (same Arc from `.manage()`)
- Receives `AppHandle` directly in `new()` (not OnceLock)
- Uses `tokio::task::spawn` (not `std::thread::spawn`) because it needs `.await` for SessionManager
- Uses `tokio::process::Command` (async) for git subprocess calls
- Wrapped in `Arc` so PtyManager can call `remove_session`

```rust
pub struct GitWatcher {
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    app_handle: AppHandle,
    poll_interval: Duration,  // 5 seconds
    cache: tokio::sync::Mutex<HashMap<Uuid, Option<String>>>,
}
```

**Logic:**
1. `tokio::task::spawn` a loop that runs every 5 seconds
2. Gets all sessions with their working directories via `session_manager.read().await`
3. For each session, runs `git rev-parse --abbrev-ref HEAD` via `tokio::process::Command`
   - Non-zero exit (not a git repo): `None`
   - Returns "HEAD" (detached): treat as `None`
   - Normal branch name: `Some(branch)`
4. Compares result with cached value
5. If changed:
   - Updates SessionManager via `set_git_branch()`
   - Emits `session_git_branch` event with `{ sessionId, branch }`
   - Updates cache
6. Cleanup: remove entries from cache when sessions are destroyed

**Exposed methods:**
- `new(session_manager, app_handle, poll_interval) -> Arc<Self>`
- `start(self: &Arc<Self>)` - spawn the tokio task
- `remove_session(&self, id)` - clean up cache entry

### 4. Wire GitWatcher into app lifecycle

**File:** `src-tauri/src/lib.rs`

- Clone the `Arc<tokio::sync::RwLock<SessionManager>>` before passing to `GitWatcher::new()`
- Create `Arc<GitWatcher>` alongside IdleDetector
- Pass `Arc<GitWatcher>` to PtyManager
- Call `git_watcher.start()` inside `setup()`

**File:** `src-tauri/src/pty/manager.rs`

- Add `git_watcher: Arc<GitWatcher>` field
- On `kill()`: call `git_watcher.remove_session(id)`

### 5. Export module

**File:** `src-tauri/src/pty/mod.rs`

- Add `pub mod git_watcher;`

---

## Frontend Changes

### 6. Update Session type

**File:** `src/shared/types.ts`

- Add `gitBranch: string | null` to `Session` interface

### 7. Add event listener

**File:** `src/shared/ipc.ts`

- Add `onSessionGitBranch(callback)` listener for `session_git_branch` event

### 8. Update sessions store

**File:** `src/sidebar/stores/sessions.ts`

- Add `setGitBranch(sessionId, branch)` method
- Initialize `gitBranch: null` when adding sessions

### 9. Subscribe to event in sidebar

**File:** `src/sidebar/App.tsx`

- Add `onSessionGitBranch()` listener in `onMount`
- Call `sessionsStore.setGitBranch()` on event

### 10. Display branch in SessionItem

**File:** `src/sidebar/components/SessionItem.tsx`

- Show `gitBranch` next to session name or shell type
- Style: muted text, monospace, with a branch icon or `on:` prefix
- Example rendering: `Session 1  on feature/my-branch`
- If `null`, show nothing (not a git repo)

---

## Files Modified (summary)

| File | Change |
|------|--------|
| `src-tauri/src/session/session.rs` | Add `git_branch` field |
| `src-tauri/src/session/manager.rs` | Add setter + directory getter |
| `src-tauri/src/pty/git_watcher.rs` | **NEW** - polling logic |
| `src-tauri/src/pty/mod.rs` | Export `git_watcher` |
| `src-tauri/src/lib.rs` | Wire GitWatcher into app |
| `src-tauri/src/pty/manager.rs` | Add GitWatcher ref, cleanup on kill |
| `src/shared/types.ts` | Add `gitBranch` to Session |
| `src/shared/ipc.ts` | Add event listener |
| `src/sidebar/stores/sessions.ts` | Add store method |
| `src/sidebar/App.tsx` | Subscribe to event |
| `src/sidebar/components/SessionItem.tsx` | Render branch |

## Non-goals (future)

- Dynamic CWD tracking (OSC 7 or Windows process introspection)
- Git status indicators (dirty/clean/ahead/behind)
- Branch switching from the sidebar
- Tracking nested git repos if user `cd`s into a different project
