# fix/session-status-idle

## Problem Statement

Session status indicators in the sidebar show yellow/blue (active/running) when they should show green (idle/waiting). Sessions appear to get activated but never transition back to idle state.

## Root Cause

**Race condition in `IdleDetector::start()` watcher thread** (`src-tauri/src/pty/idle_detector.rs`).

`Instant::now()` was captured on line 86 BEFORE the `activity` mutex was acquired on line 87. A PTY read thread calling `record_activity_with_bytes` could insert a timestamp *newer* than `now` between those two lines. When the watcher then called `now.duration_since(last_seen)` with `last_seen > now`, Rust's `Instant::duration_since()` panicked ("supplied instant is later than self"), silently killing the watcher thread. After that, no idle detection ever ran again.

Secondary issue: the `on_idle` callback was called while holding both `activity` and `idle_set` locks, causing unnecessary contention that blocked PTY read threads during Tauri event emission and transcript writes.

## Fix

1. Moved `Instant::now()` inside the lock scope (after acquiring both mutexes), guaranteeing all `last_seen` values are <= `now`.
2. Collect newly-idle session IDs into a `Vec` under the lock, release both locks, THEN call `on_idle` callbacks outside the lock scope.

## Files Changed

- `src-tauri/src/pty/idle_detector.rs` — Fixed watcher loop: `now` after locks, callbacks outside locks
- `src-tauri/src/session/manager.rs` — Added `[session-state]` logging on mark_idle, mark_busy, mark_exited, switch_session

## Validation

- `cargo check` passes
- Code review via feature-dev:code-reviewer
