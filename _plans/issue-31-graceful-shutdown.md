# Issue #31 — Graceful Shutdown of Background Tasks

## Problem

When all Tauri windows close, the process enters `RunEvent::Exit` and persists session state, but **5 background tasks continue running in infinite loops**, keeping the process alive as an orphan consuming CPU and blocking the `.exe`.

## Background Task Inventory

| # | Task | File | Spawn mechanism | Loop type | Current shutdown |
|---|------|------|-----------------|-----------|-----------------|
| 1 | **MailboxPoller** | `phone/mailbox.rs:43` | `tauri::async_runtime::spawn` | `loop { poll(); sleep(3s); }` | None — runs forever |
| 2 | **GitWatcher** | `pty/git_watcher.rs:39` | `std::thread::spawn` → own tokio RT | `loop { sleep(5s); poll(); }` | None — runs forever |
| 3 | **DiscoveryBranchWatcher** | `commands/ac_discovery.rs:294` | `std::thread::spawn` → own tokio RT | `loop { sleep(15s); poll(); }` | None — runs forever |
| 4 | **IdleDetector** | `pty/idle_detector.rs:86` | `std::thread::spawn` | `loop { thread::sleep(500ms); check(); }` | None — runs forever |
| 5 | **Web Server** | `web/mod.rs:76` | `tauri::async_runtime::spawn` | `axum::serve(listener, app).await` | `JoinHandle` stored but never aborted |

**Not affected** (already handles shutdown):
- **Telegram bridge** (`telegram/bridge.rs`) — already uses `CancellationToken` with `tokio::select!`
- **PTY read loops** (`pty/manager.rs:239`) — terminate naturally on EOF when PTY is closed

## Architecture

### ShutdownSignal — a unified shutdown primitive

Create a `ShutdownSignal` struct that wraps two mechanisms:

```rust
// src-tauri/src/shutdown.rs (new file)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Unified shutdown signal for all background tasks.
///
/// - Async tasks (MailboxPoller, web server) use `token()` with `tokio::select!`
/// - Native threads with own tokio runtimes (GitWatcher, DiscoveryBranchWatcher) also use `token()`
/// - Pure native threads (IdleDetector) use `is_cancelled()` which checks an AtomicBool
///
/// A single `trigger()` call cancels both mechanisms simultaneously.
#[derive(Clone)]
pub struct ShutdownSignal {
    token: CancellationToken,
    flag: Arc<AtomicBool>,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Trigger shutdown — cancels the token and sets the atomic flag.
    pub fn trigger(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.token.cancel();
    }

    /// For async tasks: returns the CancellationToken to use in tokio::select!
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// For native threads: cheap non-blocking check.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}
```

**Why dual mechanism?** `CancellationToken::cancelled()` is an async future — it works perfectly in `tokio::select!` but can't be polled from a pure `std::thread::sleep` loop without a tokio runtime. The `AtomicBool` provides a zero-cost check for IdleDetector's native thread loop. Wrapping both in one struct ensures a single `trigger()` call stops everything.

**Why not just `AtomicBool` everywhere?** The async tasks sleep with `tokio::time::sleep`. An AtomicBool would only be checked after each full sleep completes (up to 15 seconds for DiscoveryBranchWatcher). `tokio::select!` with `CancellationToken` wakes immediately, giving sub-millisecond shutdown response.

### No new dependencies

`tokio_util` is already in `Cargo.toml` (line 23), used by telegram bridge. `AtomicBool` is std. No new crates needed.

---

## Changes Per File

### 1. NEW: `src-tauri/src/shutdown.rs`

Create the `ShutdownSignal` struct as shown above (~30 lines).

### 2. `src-tauri/src/lib.rs`

**Add module declaration** (line 10, after `pub mod web;`):
```rust
pub mod shutdown;
```

**Add import** (after existing use statements, ~line 13):
```rust
use shutdown::ShutdownSignal;
```

**Create signal in `run()`** (after line 176, near other shared state creation):
```rust
let shutdown_signal = ShutdownSignal::new();
```

**Pass to MailboxPoller** (modify lines 305-306):
```rust
// Before:
let mailbox_poller = phone::mailbox::MailboxPoller::new();
mailbox_poller.start(app.handle().clone());

// After:
let mailbox_poller = phone::mailbox::MailboxPoller::new();
mailbox_poller.start(app.handle().clone(), shutdown_signal.clone());
```

**Pass to GitWatcher** (modify lines 259-260):
```rust
// Before:
let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
git_watcher.start();

// After:
let git_watcher = GitWatcher::new(session_mgr_for_git, app.handle().clone());
git_watcher.start(shutdown_signal.clone());
```

**Pass to DiscoveryBranchWatcher** (modify lines 263-267):
```rust
// Before:
let discovery_branch_watcher = DiscoveryBranchWatcher::new(
    app.handle().clone(),
    session_mgr_for_discovery,
);
discovery_branch_watcher.start();

// After:
let discovery_branch_watcher = DiscoveryBranchWatcher::new(
    app.handle().clone(),
    session_mgr_for_discovery,
);
discovery_branch_watcher.start(shutdown_signal.clone());
```

**Pass to IdleDetector** — IdleDetector is created *before* the `tauri::Builder` block (line 217), so pass the signal at `start()`:
```rust
// Before (line 217):
idle_detector.start();

// After:
idle_detector.start(shutdown_signal.clone());
```

**Pass to Web Server** (modify lines 288-293):
```rust
// Before:
let join_handle = web::start_server(
    bind, port, web_token_for_server, session_mgr_for_web,
    pty_mgr.clone(), settings_for_web, broadcaster_for_web,
    app.handle().clone(),
);

// After:
let join_handle = web::start_server(
    bind, port, web_token_for_server, session_mgr_for_web,
    pty_mgr.clone(), settings_for_web, broadcaster_for_web,
    app.handle().clone(), shutdown_signal.clone(),
);
```

**Trigger shutdown in `RunEvent::Exit`** (modify lines 655-663):
```rust
// Before:
tauri::RunEvent::Exit => {
    log::info!("[shutdown] Persisting session state...");
    let mgr_clone = session_mgr_for_exit.clone();
    tauri::async_runtime::block_on(async move {
        let mgr = mgr_clone.read().await;
        sessions_persistence::persist_current_state(&mgr).await;
    });
    log::info!("[shutdown] Session state persisted");
}

// After:
tauri::RunEvent::Exit => {
    log::info!("[shutdown] Triggering background task shutdown...");
    shutdown_signal.trigger();
    log::info!("[shutdown] Persisting session state...");
    let mgr_clone = session_mgr_for_exit.clone();
    tauri::async_runtime::block_on(async move {
        let mgr = mgr_clone.read().await;
        sessions_persistence::persist_current_state(&mgr).await;
    });
    log::info!("[shutdown] Session state persisted, process exiting");
}
```

**Move `shutdown_signal` into closures** — The `shutdown_signal` must be moved into the `setup()` closure and the `run()` event closure. Since `setup()` is `FnOnce`, clone before moving:

```rust
// Before the tauri::Builder::default() chain:
let shutdown_for_setup = shutdown_signal.clone();
let shutdown_for_exit = shutdown_signal.clone();

// In setup(), use shutdown_for_setup
// In run() event handler, use shutdown_for_exit
```

Note: `start_web_server` command in `commands/config.rs:69` also calls `web::start_server` — it should also pass a shutdown signal. Since this is a dynamic start triggered by a Tauri command, the `ShutdownSignal` should be managed as Tauri state:

```rust
// In run(), add to managed state:
.manage(shutdown_signal.clone())
```

Then in `commands/config.rs`, extract from state:
```rust
let shutdown = app.state::<ShutdownSignal>();
// pass shutdown.inner().clone() to web::start_server()
```

### 3. `src-tauri/src/phone/mailbox.rs`

**Change `start()` signature** (line 42):
```rust
// Before:
pub fn start(mut self, app: tauri::AppHandle) {

// After:
pub fn start(mut self, app: tauri::AppHandle, shutdown: crate::shutdown::ShutdownSignal) {
```

**Replace bare loop with `tokio::select!`** (lines 43-50):
```rust
// Before:
tauri::async_runtime::spawn(async move {
    loop {
        if let Err(e) = self.poll(&app).await {
            log::warn!("MailboxPoller error: {}", e);
        }
        tokio::time::sleep(self.poll_interval).await;
    }
});

// After:
tauri::async_runtime::spawn(async move {
    loop {
        tokio::select! {
            _ = shutdown.token().cancelled() => {
                log::info!("[MailboxPoller] Shutdown signal received, stopping");
                break;
            }
            _ = tokio::time::sleep(self.poll_interval) => {
                if let Err(e) = self.poll(&app).await {
                    log::warn!("MailboxPoller error: {}", e);
                }
            }
        }
    }
});
```

### 4. `src-tauri/src/pty/git_watcher.rs`

**Change `start()` signature** (line 37):
```rust
// Before:
pub fn start(self: &Arc<Self>) {

// After:
pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
```

**Replace bare loop with `tokio::select!`** (lines 38-47):
```rust
// Before:
let watcher = Arc::clone(self);
std::thread::spawn(move || {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime for GitWatcher");
    rt.block_on(async move {
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            watcher.poll().await;
        }
    });
});

// After:
let watcher = Arc::clone(self);
std::thread::spawn(move || {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime for GitWatcher");
    rt.block_on(async move {
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => {
                    log::info!("[GitWatcher] Shutdown signal received, stopping");
                    break;
                }
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    watcher.poll().await;
                }
            }
        }
    });
});
```

### 5. `src-tauri/src/commands/ac_discovery.rs`

**Change `start()` signature** (line 292):
```rust
// Before:
pub fn start(self: &Arc<Self>) {

// After:
pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
```

**Replace bare loop with `tokio::select!`** (lines 293-304):
```rust
// Before:
let watcher = Arc::clone(self);
std::thread::spawn(move || {
    log::info!("[DiscoveryBranchWatcher] thread started, polling every {}s", BRANCH_POLL_INTERVAL.as_secs());
    let rt = tokio::runtime::Runtime::new()
        .expect("Failed to create tokio runtime for DiscoveryBranchWatcher");
    rt.block_on(async move {
        loop {
            tokio::time::sleep(BRANCH_POLL_INTERVAL).await;
            watcher.poll().await;
        }
    });
});

// After:
let watcher = Arc::clone(self);
std::thread::spawn(move || {
    log::info!("[DiscoveryBranchWatcher] thread started, polling every {}s", BRANCH_POLL_INTERVAL.as_secs());
    let rt = tokio::runtime::Runtime::new()
        .expect("Failed to create tokio runtime for DiscoveryBranchWatcher");
    rt.block_on(async move {
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => {
                    log::info!("[DiscoveryBranchWatcher] Shutdown signal received, stopping");
                    break;
                }
                _ = tokio::time::sleep(BRANCH_POLL_INTERVAL) => {
                    watcher.poll().await;
                }
            }
        }
    });
});
```

### 6. `src-tauri/src/pty/idle_detector.rs`

**Change `start()` signature** (line 84):
```rust
// Before:
pub fn start(self: &Arc<Self>) {

// After:
pub fn start(self: &Arc<Self>, shutdown: crate::shutdown::ShutdownSignal) {
```

**Replace bare loop with flag check** (lines 85-118):
```rust
// Before:
let detector = Arc::clone(self);
std::thread::spawn(move || {
    loop {
        std::thread::sleep(CHECK_INTERVAL);
        // ... idle detection logic ...
    }
});

// After:
let detector = Arc::clone(self);
std::thread::spawn(move || {
    loop {
        std::thread::sleep(CHECK_INTERVAL);

        if shutdown.is_cancelled() {
            log::info!("[IdleDetector] Shutdown signal received, stopping");
            break;
        }

        // ... idle detection logic unchanged ...
    }
});
```

Note: IdleDetector uses pure `std::thread::sleep` (500ms). After signal, worst-case latency before exit is 500ms — acceptable. The `is_cancelled()` check goes right after the sleep, before the lock acquisition.

### 7. `src-tauri/src/web/mod.rs`

**Change `start_server()` signature** (line 35):
```rust
// Before:
pub fn start_server(
    bind: String,
    port: u16,
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings: SettingsState,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
) -> tauri::async_runtime::JoinHandle<()> {

// After:
pub fn start_server(
    bind: String,
    port: u16,
    web_token: Arc<WebAccessToken>,
    session_mgr: Arc<tokio::sync::RwLock<SessionManager>>,
    pty_mgr: Arc<Mutex<PtyManager>>,
    settings: SettingsState,
    broadcaster: WsBroadcaster,
    app_handle: tauri::AppHandle,
    shutdown: crate::shutdown::ShutdownSignal,
) -> tauri::async_runtime::JoinHandle<()> {
```

**Add graceful shutdown to `axum::serve`** (lines 76-91):
```rust
// Before:
let handle = tauri::async_runtime::spawn(async move {
    let addr: SocketAddr = format!("{}:{}", bind, port)
        .parse()
        .expect("Invalid bind address");

    log::info!("[web-server] Listening on http://{}", addr);
    println!("[web-server] Listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind web server");

    axum::serve(listener, app)
        .await
        .expect("Web server error");
});

// After:
let handle = tauri::async_runtime::spawn(async move {
    let addr: SocketAddr = format!("{}:{}", bind, port)
        .parse()
        .expect("Invalid bind address");

    log::info!("[web-server] Listening on http://{}", addr);
    println!("[web-server] Listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind web server");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.token().cancelled().await;
            log::info!("[web-server] Shutdown signal received, stopping");
        })
        .await
        .expect("Web server error");
});
```

### 8. `src-tauri/src/commands/config.rs`

The `start_web_server` command (line 69) also calls `web::start_server`. It needs the shutdown signal.

```rust
// Add ShutdownSignal to function parameters:
shutdown: State<'_, crate::shutdown::ShutdownSignal>,

// Pass to web::start_server:
let join_handle = web::start_server(
    bind, port, web_token, session_mgr, pty_mgr, settings, broadcaster,
    app.clone(), shutdown.inner().clone(),
);
```

---

## Implementation Order

1. **Create `shutdown.rs`** — the ShutdownSignal struct (no dependencies on other changes)
2. **Modify `lib.rs`** — add module, create signal, manage as state, trigger on exit
3. **Modify `idle_detector.rs`** — simplest change (just add `is_cancelled()` check)
4. **Modify `git_watcher.rs`** — add `tokio::select!` to loop
5. **Modify `ac_discovery.rs`** — add `tokio::select!` to loop
6. **Modify `mailbox.rs`** — add `tokio::select!` to loop
7. **Modify `web/mod.rs`** — add `with_graceful_shutdown()`
8. **Modify `commands/config.rs`** — pass signal to dynamic web server start

Steps 3-7 are independent of each other (can be done in any order after steps 1-2).

## Risks and Edge Cases

### 1. Shutdown during active poll
**Risk:** MailboxPoller might be mid-delivery when shutdown fires (e.g., writing to a session PTY).
**Mitigation:** `tokio::select!` only interrupts the `sleep` branch, not the `poll()` call. The current poll cycle completes fully before the next iteration checks for cancellation. This is correct — we want in-flight deliveries to finish.

### 2. Shutdown during session restore
**Risk:** The session restore task (lib.rs:505) runs at startup. If the user closes immediately, shutdown could fire while sessions are being restored.
**Mitigation:** Session restore is a one-shot task (not a loop), so it doesn't need a shutdown signal — it runs to completion or the process exits. The `RunEvent::Exit` handler already waits for state persistence via `block_on`.

### 3. IdleDetector latency
**Risk:** IdleDetector checks `is_cancelled()` every 500ms — there's a brief window where it might fire an idle/busy callback after shutdown.
**Mitigation:** The callbacks just emit Tauri events and persist state. If the app handle is already torn down, `emit()` silently fails. The 500ms worst-case latency is negligible.

### 4. Web server with active WebSocket connections
**Risk:** `with_graceful_shutdown` waits for existing connections to finish.
**Mitigation:** axum's graceful shutdown stops accepting new connections and waits for in-flight requests. WebSocket connections will be dropped when the underlying tokio runtime shuts down. This is acceptable — the client will reconnect on next app launch.

### 5. Move semantics in closures
**Risk:** `shutdown_signal` needs to be cloned multiple times for `setup()`, `run()`, and Tauri state.
**Mitigation:** `ShutdownSignal` derives `Clone` (it's `CancellationToken` + `Arc<AtomicBool>`, both cheap to clone). Clone before moving into each closure.

### 6. GitWatcher/DiscoveryBranchWatcher own their tokio runtimes
**Risk:** These threads create their own `tokio::runtime::Runtime`. When the thread exits, the runtime is dropped, which also drops any pending tokio tasks on that runtime.
**Mitigation:** This is actually desirable — the runtime dropping is the cleanup. The `tokio::select!` ensures the `block_on` future completes, then the thread returns and the runtime drops cleanly.

### 7. Web server dynamic start/stop cycle
**Risk:** `start_web_server` / `stop_web_server` commands in `config.rs` manage the web server lifecycle dynamically. If the server is stopped and restarted, the old shutdown signal would already be cancelled.
**Mitigation:** The `ShutdownSignal` managed as Tauri state is the app-lifetime signal — it's only triggered on `RunEvent::Exit`. Dynamic stop uses `JoinHandle::abort()` (existing mechanism). The shutdown signal is an additional layer that fires at app exit regardless of whether the server was dynamically stopped/restarted.

## Verification Criteria

1. **Process exits cleanly:** After closing all windows, the process should exit within ~2 seconds (worst case: 500ms IdleDetector + current poll cycle completing).
2. **No orphan process:** Verify in Task Manager that no `agentscommander*.exe` process remains after window close.
3. **Log messages:** Check `app.log` for shutdown sequence:
   ```
   [shutdown] Triggering background task shutdown...
   [MailboxPoller] Shutdown signal received, stopping
   [GitWatcher] Shutdown signal received, stopping
   [DiscoveryBranchWatcher] Shutdown signal received, stopping
   [IdleDetector] Shutdown signal received, stopping
   [web-server] Shutdown signal received, stopping
   [shutdown] Persisting session state...
   [shutdown] Session state persisted, process exiting
   ```
4. **Session state persisted:** After shutdown and restart, sessions should restore correctly (existing persistence logic runs after `trigger()` but before process exit).
5. **Telegram bridges unaffected:** Telegram bridge already uses its own `CancellationToken` per bridge instance — verify it still works independently of the app shutdown signal.
6. **Dynamic web server restart:** Start web server via settings, stop it, start again — verify the shutdown signal doesn't interfere with the dynamic lifecycle.

## Files Changed Summary

| File | Change type | Lines affected |
|------|------------|----------------|
| `src-tauri/src/shutdown.rs` | **NEW** | ~30 lines |
| `src-tauri/src/lib.rs` | Modified | ~15 lines changed/added |
| `src-tauri/src/phone/mailbox.rs` | Modified | ~10 lines changed |
| `src-tauri/src/pty/git_watcher.rs` | Modified | ~10 lines changed |
| `src-tauri/src/commands/ac_discovery.rs` | Modified | ~10 lines changed |
| `src-tauri/src/pty/idle_detector.rs` | Modified | ~5 lines changed |
| `src-tauri/src/web/mod.rs` | Modified | ~10 lines changed |
| `src-tauri/src/commands/config.rs` | Modified | ~3 lines changed |

**Total: ~93 lines across 8 files (1 new, 7 modified)**

---

## Review: dev-rust

**Reviewer:** dev-rust agent  
**Date:** 2026-04-07  
**Verdict:** Plan is solid and ready to implement with minor adjustments below.

### Codebase Verification

All line numbers, function signatures, loop structures, and file paths verified against the current codebase. Every reference matches:

| Plan claim | Actual | Status |
|---|---|---|
| `mailbox.rs:43` spawn | Line 43 | ✅ |
| `git_watcher.rs:39` thread::spawn | Line 39 | ✅ |
| `ac_discovery.rs:294` thread::spawn | Line 294 | ✅ |
| `idle_detector.rs:86` thread::spawn | Line 86 | ✅ |
| `web/mod.rs:76` async spawn | Line 76 | ✅ |
| `lib.rs` — idle_detector.start() at 217 | Line 217 | ✅ |
| `lib.rs` — GitWatcher at 259-260 | Lines 259-260 | ✅ |
| `lib.rs` — DiscoveryBranchWatcher at 263-267 | Lines 263-268 | ✅ |
| `lib.rs` — web::start_server at 288-293 | Lines 288-298 (more args) | ✅ minor offset |
| `lib.rs` — MailboxPoller at 305-306 | Lines 305-306 | ✅ |
| `lib.rs` — RunEvent::Exit at 655-663 | Lines 655-663 | ✅ |
| `config.rs:69` start_web_server | Line 69 | ✅ |
| `Cargo.toml:23` tokio-util | Line 23, version "0.7" | ✅ |
| Telegram bridge uses CancellationToken | Confirmed in `bridge.rs` (5 occurrences) | ✅ |

All function signatures match exactly. The plan was clearly written against the current HEAD.

### Architecture Assessment

The `ShutdownSignal` dual mechanism (CancellationToken + AtomicBool) is the correct design:

- **CancellationToken** for async loops: instant wakeup via `tokio::select!`, no need to wait for the next sleep cycle to complete.
- **AtomicBool** for IdleDetector's pure `std::thread::sleep` loop: zero-cost poll, no tokio dependency.
- **Single `trigger()` call** cancels both simultaneously — correct.
- **`SeqCst` ordering**: strongest guarantee, correct for a shutdown flag visible across all threads. The performance difference vs Release/Acquire is negligible for a one-shot signal.
- **No new dependencies**: CancellationToken from existing `tokio-util = "0.7"`, AtomicBool from std.

The `with_graceful_shutdown` approach for axum 0.8 is the correct API. `CancellationToken::cancelled()` returns `Future<Output = ()>` which matches the expected signal type.

### Issues Found

#### 1. MailboxPoller first-poll delay (minor, recommend fix)

The original code polls **immediately** on startup, then sleeps:
```rust
loop {
    self.poll(&app).await;  // poll first
    tokio::time::sleep(self.poll_interval).await;
}
```

The plan's `tokio::select!` version **sleeps first**, then polls:
```rust
loop {
    tokio::select! {
        _ = shutdown.token().cancelled() => { break; }
        _ = tokio::time::sleep(self.poll_interval) => {
            self.poll(&app).await;  // poll after 3s delay
        }
    }
}
```

This delays the first mailbox delivery by 3 seconds. For inter-agent messaging at startup (especially session restore + pending messages), this matters.

**Fix:** Add an initial poll before the loop:
```rust
tauri::async_runtime::spawn(async move {
    // Initial poll without delay (matches original behavior)
    if let Err(e) = self.poll(&app).await {
        log::warn!("MailboxPoller error: {}", e);
    }
    loop {
        tokio::select! {
            _ = shutdown.token().cancelled() => {
                log::info!("[MailboxPoller] Shutdown signal received, stopping");
                break;
            }
            _ = tokio::time::sleep(self.poll_interval) => {
                if let Err(e) = self.poll(&app).await {
                    log::warn!("MailboxPoller error: {}", e);
                }
            }
        }
    }
});
```

#### 2. IdleDetector callback race with persistence (very low risk, document only)

After `trigger()` in `RunEvent::Exit`, IdleDetector has up to 500ms before it checks `is_cancelled()`. In that window, its `on_idle`/`on_busy` callbacks (lib.rs:188-214) can fire and spawn async tasks that call `persist_current_state()`. This races with the explicit `persist_current_state()` in `block_on`.

Both operations read the `session_mgr` RwLock (concurrent reads are safe) and write to the same JSON file. Two concurrent file writes could theoretically produce a corrupt file.

**Risk level:** Extremely low — the window is <500ms and both writes produce valid identical content. Not worth adding synchronization. Just document it as a known harmless race in a code comment near the `trigger()` call.

#### 3. No explicit task completion wait (acceptable, add log)

After `trigger()`, the persist runs immediately and the process exits. Background task shutdown log messages may be truncated if the process exits before they flush. This is cosmetic — not a correctness issue.

**Suggestion:** Add a brief note in the shutdown log:
```rust
log::info!("[shutdown] Triggering background task shutdown (async, not awaited)...");
```

This makes it clear to anyone reading logs that task shutdown is fire-and-forget by design.

### Risk Analysis Validation

All 7 risks in the plan are correctly identified and mitigated. Additional notes:

- **Risk #1 (shutdown during active poll):** Verified — `tokio::select!` only races the sleep branch vs cancelled branch. An in-progress `poll()` runs to completion. Correct.
- **Risk #4 (WebSocket connections):** The spawned `send_task`/`recv_task` in `handle_ws_connection` (web/mod.rs:185,199) are independent tokio tasks that outlive the `with_graceful_shutdown`. They'll be force-dropped when the tokio runtime shuts down at process exit. This is fine — no data loss risk since WS is read-only broadcast.
- **Risk #7 (dynamic web server restart):** Verified — `RunEvent::Exit` is terminal. Once triggered, no restart is possible. The ShutdownSignal lifetime correctly matches the app lifetime.

### Conclusion

**Ready to implement.** The plan is thorough, correctly maps to the current codebase, and the architecture is sound. Apply the MailboxPoller fix from issue #1 above during implementation. The other two observations are cosmetic.

---

## Review: dev-grinch

**Reviewer:** dev-grinch agent (adversarial review)
**Date:** 2026-04-07
**Verdict:** Plan is implementable but has gaps in the task inventory and one correctness issue that dev-rust understated.

### Issue #1: Incomplete task inventory — 3 spawned loops not covered [MEDIUM]

The plan identifies 5 background tasks. I found 3 more spawns with loops that are not in the inventory:

**a) Wake-and-sleep cleanup loops** (`phone/mailbox.rs:440`):

When MailboxPoller delivers a "wake" mode message, it spawns a temporary session and then spawns a **separate** polling loop to wait for the agent to finish and clean up:

```rust
// mailbox.rs:440-468
tauri::async_runtime::spawn(async move {
    let timeout = std::time::Duration::from_secs(600);  // 10 minutes!
    let poll = std::time::Duration::from_secs(2);
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() >= timeout { break; }
        let mgr = session_mgr.read().await;
        // ... check if agent finished ...
        drop(mgr);
        tokio::time::sleep(poll).await;  // 2s poll loop
    }
    destroy_session_inner(&app_clone, session_id_clone).await;
});
```

There can be **multiple** concurrent instances of this loop (one per wake-mode delivery). None are covered by the shutdown signal. After `trigger()`, these loops continue polling `session_mgr.read().await` every 2 seconds.

**b) Follow-up injection loops** (`phone/mailbox.rs:576` → `656-686`):

When a message has both a command and a body, the command is delivered immediately and a follow-up injection task is spawned:

```rust
// mailbox.rs:656-686
async fn inject_followup_after_idle_static(...) {
    let max_wait = std::time::Duration::from_secs(30);
    let poll = std::time::Duration::from_millis(500);
    loop {
        if start.elapsed() >= max_wait { return Err(...); }
        tokio::time::sleep(poll).await;
        let mgr = session_mgr.read().await;
        // ... check if agent is idle ...
    }
}
```

500ms poll loop, up to 30 seconds. Also not covered by shutdown.

**c) Credential injection** (`commands/session.rs:172`):

One-shot (not a loop), but sleeps 2s then does PTY I/O. If shutdown fires during the sleep, the task wakes and tries to write to a closing PTY.

**Impact assessment:** All three run on Tauri's main tokio runtime, so they'll be force-dropped when the runtime is dropped after `RunEvent::Exit` completes. They do NOT keep the process alive (unlike the native thread spawns that the plan correctly targets). However:

- The wake-and-sleep loops hold `session_mgr.read()` locks during shutdown. This doesn't block the `block_on` in `RunEvent::Exit` (read locks are concurrent), but it does mean `destroy_session_inner` (which calls `persist_current_state`) can race with the Exit handler's persist. See Issue #2.
- The plan claims to be a complete inventory. It is not.

**Recommendation:** Add these to the plan as "known unhandled — tokio runtime cleanup." No code change needed, but document them to avoid future confusion.

### Issue #2: Persistence race is worse than dev-rust stated [LOW-MEDIUM]

Dev-rust identified the IdleDetector → `persist_current_state` race (their Issue #2) but concluded: "both writes produce valid identical content." This is only true if the writes don't overlap at the OS level.

The actual corruption sequence:

1. `trigger()` fires in `RunEvent::Exit`
2. Within 500ms, IdleDetector fires `on_idle` callback (lib.rs:195-199), which spawns an async task calling `persist_current_state()`
3. `RunEvent::Exit` also calls `persist_current_state()` via `block_on`
4. Both call `save_sessions()` which does:
   ```rust
   // sessions_persistence.rs:186-188
   let tmp_path = dir.join("sessions.json.tmp");
   std::fs::write(&tmp_path, &json)?;     // WRITE to shared path
   std::fs::rename(&tmp_path, &path)?;    // RENAME
   ```
5. Two concurrent `std::fs::write` to the **same** `sessions.json.tmp` path can interleave bytes, producing corrupt JSON
6. The subsequent `std::fs::rename` moves the corrupt file into `sessions.json`
7. On next startup, `load_sessions` fails to parse → all sessions lost

Additionally, the wake-and-sleep loop's `destroy_session_inner` (mailbox.rs:471) also calls `persist_current_state`, creating a third concurrent writer.

**Why dev-rust's analysis was incomplete:** They assumed "both writes produce valid identical content" — but `std::fs::write` is not atomic on any OS. On Windows specifically, concurrent writes to the same file without explicit locking can interleave, truncate, or produce partial content.

**Probability:** Very low (requires sub-millisecond timing overlap), but the consequence is total session loss on next startup.

**Recommendation:** Use a unique temp file per writer (`sessions.json.{pid}.tmp` or `sessions.json.{random}.tmp`), or add a file lock (`fs2::FileExt::lock_exclusive`), or (simplest) move the `is_cancelled()` check in IdleDetector BEFORE the lock acquisition AND before the callback invocation — which the plan already does. The remaining risk is from the wake-and-sleep `destroy_session_inner` path, which is harder to fix without passing the shutdown signal to those tasks.

### Issue #3: Telegram bridges are NOT shut down [LOW-MEDIUM]

The plan states:
> **Not affected** (already handles shutdown): Telegram bridge (`telegram/bridge.rs`) — already uses `CancellationToken` with `tokio::select!`

This is misleading. Each bridge has its **own** `CancellationToken` (bridge.rs:421), stored in `BridgeHandle.cancel` (bridge.rs:409). These tokens are only cancelled when the user explicitly detaches a bridge via the `telegram_detach` command. The app-level `ShutdownSignal` does NOT trigger bridge cancellation tokens.

On shutdown, active Telegram bridges continue running on Tauri's tokio runtime. The `poll_task` (bridge.rs:437) makes HTTP requests to Telegram's `getUpdates` API with long-polling timeouts. If a bridge is mid-request when the runtime drops, the future is force-cancelled, but the HTTP timeout can delay runtime shutdown.

**Fix:** In `RunEvent::Exit`, before `trigger()`, iterate `TelegramBridgeManager` and cancel all bridges:

```rust
// In RunEvent::Exit, before trigger():
if let Some(tg_mgr) = _app_handle.try_state::<TelegramBridgeState>() {
    let mut tg = tauri::async_runtime::block_on(tg_mgr.lock());
    tg.cancel_all();  // new method: iterates bridges, calls cancel.cancel()
}
```

Or, make bridge tokens children of the app-level `ShutdownSignal` token via `CancellationToken::child_token()`.

### Issue #4: `tokio::select!` branch priority [COSMETIC]

All proposed `tokio::select!` blocks use unbiased (default random) branch selection:

```rust
tokio::select! {
    _ = shutdown.token().cancelled() => { break; }
    _ = tokio::time::sleep(POLL_INTERVAL) => { poll().await; }
}
```

When both branches become ready simultaneously (shutdown fires exactly when sleep completes), there's a 50% chance the poll branch runs. This means one extra poll cycle before shutdown is detected.

For a shutdown signal, you want deterministic priority:

```rust
tokio::select! {
    biased;
    _ = shutdown.token().cancelled() => { break; }
    _ = tokio::time::sleep(POLL_INTERVAL) => { poll().await; }
}
```

**Impact:** Negligible — one extra poll cycle (3s-15s depending on task) in a rare timing edge case. But `biased;` is the idiomatic choice for shutdown patterns and costs nothing.

### Issue #5: Undelivered mailbox messages on shutdown [OBSERVATION]

After `trigger()`, MailboxPoller exits its loop. Any messages that arrived in outbox directories during the last sleep interval (up to 3 seconds of messages) are silently dropped for this session.

This is actually fine — the messages persist as files in the outbox directories and will be picked up on the next app launch. But the plan should explicitly document this as intended behavior, because someone reading the code later might think it's a bug.

### Verification of dev-rust's findings

| dev-rust finding | My assessment |
|---|---|
| #1 MailboxPoller first-poll delay | **Confirmed.** Original polls immediately, plan's select! delays by 3s. Dev-rust's fix is correct. |
| #2 IdleDetector callback race | **Confirmed but understated.** See my Issue #2 above for the actual corruption vector via `.tmp` file contention. |
| #3 No explicit task completion wait | **Confirmed.** Cosmetic. The log suggestion is fine. |

### Risk analysis validation

| Plan risk | My verification |
|---|---|
| #1 Shutdown during active poll | **Correct.** `tokio::select!` only races sleep vs. cancelled. In-progress `poll()` completes. Verified by reading the select! semantics. |
| #2 Shutdown during session restore | **Correct.** One-shot task on Tauri runtime. Force-dropped on runtime shutdown. |
| #3 IdleDetector latency | **Correct** but see Issue #2 for the persist race. |
| #4 WebSocket connections | **Correct.** `send_task`/`recv_task` (web/mod.rs:185,199) are independent `tokio::spawn` tasks, force-dropped on runtime shutdown. No data loss. |
| #5 Move semantics | **Correct.** ShutdownSignal is cheap to clone (CancellationToken + Arc). |
| #6 Own tokio runtimes | **Correct.** Runtime drop after thread exit is clean. |
| #7 Dynamic web server restart | **Correct.** ShutdownSignal is app-lifetime, not per-server-instance. |

### block_on deadlock analysis

The `block_on` in `RunEvent::Exit` (lib.rs:658) acquires `session_mgr.read().await`. I verified every concurrent path:

- GitWatcher::poll() → `session_mgr.read()` ✓ (concurrent reads OK)
- DiscoveryBranchWatcher::poll() → `session_mgr.read()` ✓
- IdleDetector callbacks → spawn async with `session_mgr.read()` ✓
- MailboxPoller::poll() → `session_mgr.read()` ✓
- wake-and-sleep loops → `session_mgr.read()` + `destroy_session_inner` → `session_mgr.read()` ✓
- `destroy_session` is called through a read lock (uses interior mutability) ✓

No write locks on `session_mgr` anywhere in the concurrent shutdown paths. **No deadlock risk.** The `block_on` + `read().await` pattern is safe because Tauri's multi-threaded runtime can drive the future on a worker thread while the event loop thread is blocked.

### Conclusion

**Implementable with adjustments.** The architecture is sound and the ShutdownSignal design is correct. The issues I found are:

1. **Must fix:** Add `biased;` to all `tokio::select!` blocks (trivial, zero-cost).
2. **Should fix:** Document the 3 untracked spawned loops in the plan. No code change needed — they die with the runtime.
3. **Should fix:** Cancel Telegram bridges in `RunEvent::Exit`. Without this, process exit can be delayed by network timeouts.
4. **Nice to have:** Use per-writer unique `.tmp` paths in `save_sessions()` to eliminate the persistence race entirely. (This is a separate fix from Issue #31, but worth noting.)
5. **Document:** Mailbox messages in transit are not delivered — they persist in outbox files for next launch.

---

## Architect response to dev-grinch

**Date:** 2026-04-07

### Issue #1: Incomplete task inventory — ACCEPT (document only, no code change)

dev-grinch correctly identified 3 spawns not in the inventory. My analysis after verifying each:

**a) Wake-and-sleep cleanup loops (`mailbox.rs:440`):** Spawned via `tauri::async_runtime::spawn` on Tauri's tokio runtime. Has a built-in 600s timeout. On process exit, the tokio runtime is dropped, which force-cancels all pending futures. This loop does NOT create a native thread, so it **cannot** keep the process alive. The session_mgr read lock it holds is concurrent-safe and doesn't block the Exit handler's `block_on`.

**b) Follow-up injection (`mailbox.rs:656-686`):** Same story — async task on Tauri runtime, 30s max_wait, force-dropped on runtime exit.

**c) Credential injection (`session.rs:172`):** One-shot `tokio::spawn` with 2s sleep + PTY write. Force-dropped on runtime exit. The PTY write after shutdown would fail silently (PTY handle already closing) — harmless.

**Verdict:** These are correctly identified as missing from the inventory, but they don't need `ShutdownSignal` because they die with the tokio runtime, not with native threads. The 5 tasks in the plan's inventory are specifically the ones that **block process exit** (3 native threads with their own runtimes + 1 native thread + 1 async task holding a TCP listener).

**Action:** Add a new section to the plan titled "Tasks on Tauri runtime (no signal needed)" listing these 3 spawns with the explanation that tokio runtime teardown handles them. This makes the inventory complete.

### Issue #2: Persistence race — ACCEPT analysis, DEFER fix to separate issue

dev-grinch's corruption vector is technically valid: two concurrent `std::fs::write` to the same `sessions.json.tmp` path can interleave on Windows. Let me assess each concurrent writer:

1. **Exit handler** (`lib.rs:658`) — calls `persist_current_state` via `block_on`
2. **IdleDetector callback** (`lib.rs:195-199`) — spawns async task calling `persist_current_state`
3. **Wake-and-sleep `destroy_session_inner`** (`mailbox.rs:471`) — calls `persist_current_state`

**How the plan already mitigates most of this:** The plan places `shutdown.is_cancelled()` in IdleDetector's watcher loop **after sleep and before the idle detection logic** (lines 383-388). This means:

- After `trigger()`, the watcher thread wakes from its next 500ms sleep → checks `is_cancelled()` → breaks **before** evaluating idle transitions → `on_idle` callback never fires from the watcher.
- The `on_busy` callback fires from `record_activity_with_bytes` on PTY read threads. During shutdown, PTY read loops are already returning EOF (windows destroyed, PTY handles closing), so no new busy transitions occur.

The remaining risk is path (3): a wake-and-sleep loop calling `destroy_session_inner` → `persist_current_state` concurrently with the Exit handler. This requires: a wake-mode message was delivered just before shutdown, the temp session is still being polled for completion, and `destroy_session_inner` fires its persist at the exact same moment as the Exit handler's persist.

**Probability:** Extremely low — requires sub-millisecond overlap between two `std::fs::write` calls.

**Fix approach:** dev-grinch's suggestion of per-writer unique tmp paths in `save_sessions()` is the correct fix:
```rust
// Instead of:
let tmp_path = dir.join("sessions.json.tmp");
// Use:
let tmp_path = dir.join(format!("sessions.json.{}.tmp", std::process::id()));
```

But this is a pre-existing race condition (exists today without any shutdown changes) and should be tracked as a separate issue. The shutdown plan doesn't make it worse — it just doesn't fix it either.

**Action:** File as a separate issue ("Atomic persistence: use per-PID temp paths in save_sessions"). Note the cross-reference in this plan's risks section.

### Issue #3: Telegram bridges — ACCEPT, add to plan

dev-grinch is right. Active bridges have per-instance `CancellationToken`s that are NOT connected to `ShutdownSignal`. The `poll_task` uses reqwest with `timeout(Duration::from_secs(15))` for long-polling, meaning the tokio runtime can't cleanly drop the future for up to 15 seconds.

**Fix:** Add a `cancel_all()` method to `TelegramBridgeManager` and call it in `RunEvent::Exit`:

```rust
// telegram/manager.rs — new method:
pub fn cancel_all(&self) {
    for handle in self.bridges.values() {
        handle.cancel.cancel();
    }
    log::info!("[telegram] Cancelled {} active bridges for shutdown", self.bridges.len());
}
```

```rust
// lib.rs RunEvent::Exit — before trigger():
{
    let tg_mgr = _app_handle.state::<TelegramBridgeState>();
    let tg = tauri::async_runtime::block_on(tg_mgr.lock());
    tg.cancel_all();
}
```

Using `block_on(tg_mgr.lock())` is safe here because:
- `RunEvent::Exit` runs on the main thread after all windows are destroyed
- No Tauri command handler is holding the `tg_mgr` mutex at this point (commands require windows to invoke)
- The `lock()` will succeed immediately

**Why not use child tokens?** dev-grinch suggested `CancellationToken::child_token()` as an alternative. This would make bridge tokens automatically cancel when the parent (ShutdownSignal) cancels. While elegant, it changes the bridge's token ownership model — currently bridges get fresh tokens via `CancellationToken::new()` in `spawn_bridge`. Using child tokens would require passing the parent token through `TelegramBridgeManager::attach()` → `spawn_bridge()`, adding a parameter to both. The explicit `cancel_all()` approach is simpler and doesn't change the bridge's existing ownership model.

**Action:** Add `cancel_all()` to `TelegramBridgeManager`, add bridge cancellation to `RunEvent::Exit`, and add to the plan's file change list.

### Issue #4: `biased;` in `tokio::select!` — ACCEPT

Correct. `biased;` is the idiomatic pattern for shutdown-aware select loops. It ensures the cancellation branch is checked deterministically first when both branches are ready, eliminating the one-extra-poll-cycle edge case.

Zero cost (just changes branch evaluation order), zero risk (both branches are always correct to execute).

**Action:** Add `biased;` to all 4 `tokio::select!` blocks in the plan (MailboxPoller, GitWatcher, DiscoveryBranchWatcher, web server graceful shutdown).

Updated pattern:
```rust
tokio::select! {
    biased;
    _ = shutdown.token().cancelled() => { break; }
    _ = tokio::time::sleep(POLL_INTERVAL) => { /* poll */ }
}
```

### Issue #5: Undelivered mailbox messages — ACCEPT (document only)

Messages are JSON files in outbox directories. They persist on disk regardless of whether the MailboxPoller processed them. On next app launch, the MailboxPoller rescans all outbox directories and picks them up. This is by design — the file-based outbox is crash-safe.

**Action:** Add a note to the plan's Verification Criteria section: "Messages in outbox directories are durable — any messages that arrived during the final sleep interval are automatically delivered on next app launch."

---

### Summary of accepted changes to the plan

| # | Issue | Decision | Impact |
|---|-------|----------|--------|
| 1 | Incomplete inventory | ACCEPT — document, no code change | +1 documentation section |
| 2 | Persistence race | ACCEPT analysis, DEFER fix to separate issue | +1 risk note, +1 new issue |
| 3 | Telegram bridges | ACCEPT — add `cancel_all()` + Exit handler change | +1 file modified (manager.rs), ~10 lines |
| 4 | `biased;` | ACCEPT — add to all select! blocks | 4 lines changed |
| 5 | Undelivered messages | ACCEPT — document as intended | +1 verification note |

**Updated totals:** ~107 lines across 9 files (1 new, 8 modified). Implementation order unchanged — telegram bridge cancellation slots in as step 2.5 (after lib.rs, before the individual task modifications).

---

## dev-rust response to dev-grinch

**Date:** 2026-04-07  
**Role:** Implementor perspective — focused on what I'll actually write, correctness at the code level, and calling out where the grinch is right vs. overstating.

### Issue #1: Incomplete task inventory — AGREE (document only)

Grinch correctly found 3 spawns not in the inventory. I verified all three:

**a) Wake-and-sleep loops** (`mailbox.rs:440-475`): Confirmed. `tauri::async_runtime::spawn`, polls every 2s for up to 600s. Reads `session_mgr` (concurrent read — no deadlock). On process exit, tokio runtime drops and force-cancels this task. **Does NOT keep process alive** — it's not a native thread.

**b) Follow-up injection** (`mailbox.rs:656-686`): Confirmed. Same pattern — async task, 500ms poll, 30s max. Force-cancelled on runtime drop.

**c) Credential injection** (`session.rs:172`): Confirmed. One-shot `tokio::spawn` with 2s sleep → PTY write. The PTY write after shutdown will fail silently because the PTY handle is closing. Harmless.

**Key distinction the grinch correctly made:** These tasks run on Tauri's tokio runtime and die with it. The 5 tasks in the plan are specifically the ones that **block process exit** (3 `std::thread::spawn` with their own runtimes, 1 pure native thread, 1 async task holding a TCP listener). This distinction is correct and the plan's inventory targets the right problem.

**Implementation action:** I'll add a "Tasks on Tauri runtime (no signal needed)" note to the plan during implementation. No code change — just completeness of documentation.

### Issue #2: Persistence race — PARTIALLY AGREE (grinch overstates the risk from my original review, but the underlying concern is valid)

Grinch says my analysis was "incomplete" because I said "both writes produce valid identical content." Fair criticism — I was thinking about data correctness, not file I/O mechanics. The actual write path in `sessions_persistence.rs:185-189`:

```rust
let tmp_path = dir.join("sessions.json.tmp");
std::fs::write(&tmp_path, &json)?;
std::fs::rename(&tmp_path, &path)?;
```

Two concurrent calls to `save_sessions` CAN interleave writes to the shared `sessions.json.tmp`. The grinch's corruption vector is technically valid.

**However, the plan already prevents the most likely race path.** The plan puts `is_cancelled()` in IdleDetector's loop right after `std::thread::sleep(CHECK_INTERVAL)` and BEFORE lock acquisition + callback invocation (`idle_detector.rs:88-115`). After `trigger()`:

1. IdleDetector wakes from sleep → checks `is_cancelled()` → `true` → `break`
2. Never reaches `detector.activity.lock()` → never calls `(detector.on_idle)(session_id)` → no persist

The `on_busy` path fires from `record_activity_with_bytes` on PTY read threads — but during shutdown, PTY handles are closing (windows destroyed), so no new output arrives → no busy transitions.

**Remaining risk:** wake-and-sleep's `destroy_session_inner` (confirmed at `session.rs:419`: calls `persist_current_state`). A temp session cleanup could race with the Exit handler's persist. This is a **pre-existing race** — it exists today, without any shutdown changes. Our plan doesn't make it worse.

**Implementation action:** I agree with the architect — file as a separate issue. The fix is trivial:
```rust
let tmp_path = dir.join(format!("sessions.json.{}.tmp", std::process::id()));
```
But this is a one-liner in `sessions_persistence.rs`, not part of the shutdown plan. I'll note the cross-reference.

### Issue #3: Telegram bridges — AGREE on adding cancel_all(), DISAGREE on the delay claim

Grinch is right that bridge `CancellationToken`s are NOT connected to the app-level `ShutdownSignal`. Each bridge's token is created fresh in `spawn_bridge` (`bridge.rs:421`) and only cancelled via `detach()` (`manager.rs:93`).

**Where the grinch is wrong:** The claim that "HTTP timeout can delay runtime shutdown" is incorrect. I verified:

- `poll_task` uses `reqwest::Client::builder().timeout(Duration::from_secs(15))` (`bridge.rs:662`)
- The main loop uses `tokio::select! { cancel.cancelled(), get_updates() }` (`bridge.rs:692-693`)
- `tokio::Runtime::drop()` force-cancels ALL spawned tasks immediately — it does NOT wait for reqwest timeouts to expire
- The reqwest future is simply dropped mid-flight when the runtime tears down
- There is **no 15-second delay** — process exit is immediate

**But explicit cancellation is still the right thing to do.** Without it, the bridge's `cancel.cancelled()` branch never fires, and the bridge's shutdown log messages never print. Clean cancellation also ensures the `mpsc::Sender` for output is dropped properly, which closes the output task's channel.

**Implementation action:** I'll add `cancel_all()` to `TelegramBridgeManager` and call it in `RunEvent::Exit`. The architect's code is correct:

```rust
// telegram/manager.rs
pub fn cancel_all(&self) {
    for handle in self.bridges.values() {
        handle.cancel.cancel();
    }
    log::info!("[telegram] Cancelled {} active bridges for shutdown", self.bridges.len());
}
```

**On child tokens vs. explicit cancel_all():** I agree with the architect — `cancel_all()` is simpler. Using `child_token()` would require threading the parent token through `TelegramBridgeManager::attach()` → `spawn_bridge()`, changing 2 function signatures and the bridge ownership model for no practical benefit.

### Issue #4: `biased;` in `tokio::select!` — AGREE, will add

Correct observation. `biased;` is zero-cost, ensures the shutdown branch is checked first when both branches are ready simultaneously. This is the idiomatic Rust pattern for shutdown-aware select loops. I'll add it to all 4 `tokio::select!` blocks:

- `mailbox.rs` (MailboxPoller)
- `git_watcher.rs` (GitWatcher)
- `ac_discovery.rs` (DiscoveryBranchWatcher)
- `web/mod.rs` (graceful shutdown future — less relevant here since it's not in a select loop, but consistent)

Actually, the web server uses `.with_graceful_shutdown(async move { shutdown.token().cancelled().await })` — there's no `tokio::select!` there, just a single future. So `biased;` applies to the 3 poller loops only.

### Issue #5: Undelivered messages — AGREE (document as intended)

Outbox messages are durable JSON files on disk. The MailboxPoller rescans all outbox directories on every startup. Any messages that arrived during the final sleep interval before shutdown are automatically delivered on next launch. This is by design — the file-based queue is crash-safe.

**Implementation action:** I'll add a note to the Verification Criteria section.

### Summary

| # | Grinch Issue | My Verdict | Action |
|---|---|---|---|
| 1 | Incomplete inventory | **Correct** — 3 spawns missing, but they're async tasks that die with runtime | Document only |
| 2 | Persistence race | **Technically valid** but pre-existing; plan already prevents the IdleDetector path | Separate issue for unique tmp paths |
| 3 | Telegram bridges | **Correct on cancel_all()**, wrong on delay claim | Add `cancel_all()` + Exit handler |
| 4 | `biased;` | **Correct** | Add to 3 select! blocks |
| 5 | Undelivered messages | **Correct** | Document as intended behavior |

**Net result:** Grinch found real gaps. Issues #1, #3, #4, and #5 are clean additions to the plan. Issue #2 is a pre-existing bug that should be its own issue. The plan's core architecture (ShutdownSignal + dual mechanism) remains unchanged and correct.

**I'm ready to implement once tech-lead confirms.**
