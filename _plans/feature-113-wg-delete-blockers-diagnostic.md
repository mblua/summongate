# Plan — feature/113: Workgroup-delete blockers diagnostic

**Issue:** https://github.com/mblua/AgentsCommander/issues/113
**Branch:** `feature/113-wg-delete-blockers-diagnostic`
**Phase:** Full Features
**Status:** Architect draft (Step 2). Awaiting dev-rust enrichment (Step 3).

---

## 1. Requirement

When `delete_workgroup` calls `std::fs::remove_dir_all(&wg_dir)` and Windows returns
**"The process cannot access the file because it is being used by another process. (os error 32)"**,
AC must replace the raw error with a structured report identifying:

1. **AC-internal sessions** with `working_directory` inside the workgroup tree.
2. **External processes** (any PID, including AC's own children) holding open handles
   on files inside the workgroup tree, via the **Windows Restart Manager API**.

The report is delivered to the frontend via a new `BLOCKERS:` sentinel-prefixed string,
parallel to the existing `DIRTY_REPOS:` mechanism, and rendered as a list in the existing
delete-workgroup modal (no force-delete option — the user has to free the resource).

The diagnostic runs **only after** the actual `remove_dir_all` fails with os error 32.
It is NOT a preflight check. All other paths (`.ac-new` missing, name validation,
`DIRTY_REPOS:`, success) are untouched.

---

## 2. Resolved design questions

### 2.1 Restart Manager integration → **direct FFI via `windows-sys`**

`windows-sys = "0.59"` is already in `Cargo.toml` under
`[target.'cfg(windows)'.dependencies]` (line 33–34). We extend its feature set with
`Win32_System_RestartManager`. Trade-offs:

| Option | Verdict | Reason |
|---|---|---|
| **`windows-sys` FFI** ✅ | Chosen | Zero new deps; consistent with existing `#[cfg(windows)]` blocks (e.g. `pty/manager.rs`); no maintenance risk from third-party crates. |
| `restart_manager` crate | Rejected | Adds a low-traffic transitive dep for ~5 functions we'd call directly anyway. |
| Shell-out to `handle.exe` | Rejected | Sysinternals tool is not bundled with Windows; we'd ship a dependency the user must install. |

### 2.2 AC watcher detection → **scoped OUT of v1**

Investigated each long-lived watcher:

| Watcher | File | Holds handles inside WG? |
|---|---|---|
| `GitWatcher` | `pty/git_watcher.rs` | NO. Derives polled paths from `SessionManager::get_sessions_repos()` per tick — once a session is removed, the watcher stops touching its repos. Spawned `git rev-parse` children are <2s. |
| `DiscoveryBranchWatcher` | `commands/ac_discovery.rs:234` | NO directly. Keeps `replicas: HashMap<String, Vec<ReplicaBranchEntry>>` but only spawns `tokio::process::Command::new("git").current_dir(repo_dir)` with a 2-s timeout. Children show up via Restart Manager if they happen to be running at diagnostic time. |
| `JSONL watcher` | `telegram/jsonl_watcher.rs` | NO. Reads from `~/.claude/projects/<mangled-cwd>/`, NOT inside the workgroup. |
| `ResponseWatcher` | `pty/manager.rs:25` | NO. In-memory marker buffer only. |

**Conclusion:** AC sessions cover everything that AC itself owns long-term inside the
workgroup. Restart Manager catches every other PID (including transient watcher children,
coding-agent subprocesses, user-spawned cmd/explorer windows, etc.). Adding watcher
introspection in v1 would mean teaching `DiscoveryBranchWatcher` a `paths_under(prefix)`
method without changing the user-visible answer set. Defer.

**Documented limitation in the plan**: if a `git rev-parse` child spawned by a watcher is
the *only* blocker and Restart Manager misses it (e.g. process exited between snapshot
calls), the user's retry will succeed — the watcher poll is bounded at 2 s.

### 2.3 Module placement → **dedicated module**

New file: `src-tauri/src/commands/wg_delete_diagnostic.rs`. `entity_creation.rs::delete_workgroup`
keeps only the os-error-32 detection + sentinel formatting. Rationale:
- testable (no `AppHandle` / `SessionManager` mocking pollution),
- reusable for a hypothetical future preflight (out of scope here, but cheap to keep open),
- keeps `entity_creation.rs` (1500 lines) from growing further.

### 2.4 Sentinel + payload → **`BLOCKERS:` prefix + JSON**

Mirrors `DIRTY_REPOS:`. JSON serialised after the prefix:

```json
{
  "workgroup": "wg-7-dev-team",
  "platform": "windows",
  "diagnosticAvailable": true,
  "rawOsError": "The process cannot access the file because it is being used by another process. (os error 32)",
  "sessions": [
    { "sessionId": "abc-123-...", "agentName": "agentscommander:wg-7-dev-team/architect", "cwd": "C:\\Users\\maria\\0_repos\\...\\wg-7-dev-team\\__agent_architect" }
  ],
  "processes": [
    { "pid": 12345, "name": "git.exe", "files": ["C:\\...\\wg-7-dev-team\\repo-foo\\.git\\index.lock"] }
  ]
}
```

- Non-Windows builds set `diagnosticAvailable: false`, leave `sessions` / `processes` empty,
  and pass the raw OS error through `rawOsError`. Frontend shows a "diagnostic not available
  on this platform" message.
- An empty `processes` list on Windows when `diagnosticAvailable: true` is legitimate
  (Restart Manager found no external blockers; only AC sessions are at fault). Frontend
  must handle this case.

### 2.5 Dev split

| Task | Owner |
|---|---|
| New `wg_delete_diagnostic.rs` module (Restart Manager FFI + AC session scan + payload struct + `Cargo.toml` feature flag) | **dev-rust** |
| Modify `entity_creation.rs::delete_workgroup` (os error 32 detection, sentinel formatting) | **dev-rust** |
| Add TS mirror types in `src/shared/types.ts` | **dev-rust** (small, keeps the contract together with the producer) |
| `ProjectPanel.tsx` modal + signal + sentinel parsing + reset | **dev-webpage-ui** |

### 2.6 Tauri command registration

**Not needed.** The diagnostic is a synchronous Rust call inside `delete_workgroup`,
folded into the existing `Result<(), String>` return type via the new sentinel.
No change to `lib.rs:758` registration block.

---

## 3. Affected files

| File | Change |
|---|---|
| `src-tauri/Cargo.toml` | Add `Win32_System_RestartManager` + `Win32_System_ProcessStatus` features to `windows-sys` |
| `src-tauri/src/commands/wg_delete_diagnostic.rs` | **NEW** module: payload structs + `diagnose_blockers()` + Restart Manager FFI helpers |
| `src-tauri/src/commands/mod.rs` | Add `pub mod wg_delete_diagnostic;` |
| `src-tauri/src/commands/entity_creation.rs` | Modify `delete_workgroup` (lines 763–808) to call diagnostic on os error 32 |
| `src/shared/types.ts` | Add `BlockerReport`, `BlockerSession`, `BlockerProcess` types |
| `src/sidebar/components/ProjectPanel.tsx` | Add `wgBlockers` signal + parser + UI block; reset in `closeWgDeleteModal` |

`lib.rs`, the IPC layer, and the typed wrapper in `src/shared/ipc.ts` are unchanged
(the `deleteWorkgroup` Promise still rejects with the prefixed string).

---

## 4. Detailed change spec

### 4.1 `src-tauri/Cargo.toml` — add Restart Manager feature

**Current** (lines 33–34):

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_System_Console", "Win32_Foundation", "Win32_System_Threading"] }
```

**Replace with:**

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
    "Win32_System_Console",
    "Win32_Foundation",
    "Win32_System_Threading",
    "Win32_System_RestartManager",
] }
```

**Dev-rust deviation (Step 3) — drop `Win32_System_ProcessStatus`.** The architect's
draft added `Win32_System_ProcessStatus` for `K32GetModuleBaseNameW` (PID → exe-name
fallback). Verified against `windows-sys 0.59.0`
(`.cargo/registry/src/.../windows-sys-0.59.0/src/Windows/Win32/System/Threading/mod.rs:255`):
`QueryFullProcessImageNameW` already lives in the already-enabled
`Win32_System_Threading` feature and gets the same job done with the lighter
`PROCESS_QUERY_LIMITED_INFORMATION` access right (vs. `PROCESS_QUERY_INFORMATION
| PROCESS_VM_READ` required by `K32GetModuleBaseNameW`). Net effect: one fewer
feature flag, lighter privilege ask, identical capability. The exe-name fallback
in §4.2 uses `QueryFullProcessImageNameW`.

### 4.2 `src-tauri/src/commands/wg_delete_diagnostic.rs` — NEW module

Full skeleton — dev-rust will fill in the FFI body in Step 3. Public API and types are
locked down here.

```rust
//! Diagnostic for `delete_workgroup` failures caused by file-in-use (Windows os error 32).
//!
//! Two scans:
//!   1. AC-internal sessions whose `working_directory` lives inside the workgroup tree.
//!   2. External processes holding handles on files inside the workgroup tree, via the
//!      Windows Restart Manager API (RmStartSession / RmRegisterResources / RmGetList).
//!
//! Pure helpers — no Tauri commands. Invoked from `entity_creation::delete_workgroup`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::session::manager::SessionManager;

/// Round-2 (G.6 / C.10): lock the producer with an enum so the four-variant
/// promise can't drift. Lowercase JSON keeps wire-compat with the prior `String`
/// shape and matches the TS literal union in `src/shared/types.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Linux,
    Macos,
    Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerReport {
    pub workgroup: String,
    pub platform: Platform,
    pub diagnostic_available: bool,
    pub raw_os_error: String,
    pub sessions: Vec<BlockerSession>,
    pub processes: Vec<BlockerProcess>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerSession {
    pub session_id: String,
    /// FQN like "wg-7-dev-team/architect" if derivable, otherwise the raw cwd basename.
    pub agent_name: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerProcess {
    pub pid: u32,
    /// Executable file name (e.g. "git.exe", "node.exe"). Best-effort.
    pub name: String,
    /// Sample of paths inside the workgroup that this process holds. Capped at MAX_FILES_PER_PROCESS.
    pub files: Vec<String>,
}

const MAX_FILES_PER_PROCESS: usize = 5;
const MAX_FILES_TO_PROBE: usize = 200;

/// Top-level diagnostic. Always returns a `BlockerReport`; on non-Windows the body is empty
/// and `diagnostic_available = false`.
pub async fn diagnose_blockers(
    wg_dir: &Path,
    workgroup_name: &str,
    raw_os_error: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> BlockerReport {
    let canonical_wg = canonicalize_for_compare(wg_dir);

    let sessions = scan_ac_sessions(&canonical_wg, session_mgr).await;

    // Dev-rust enrichment (Step 3): the Windows scan is fully synchronous (FFI +
    // file-tree walk). We expect ~1 s wall-time post-failure; running it on the
    // current Tokio worker would block that worker for the duration. Hand it off
    // via `spawn_blocking` so other async work (PTY reads, IPC events) keeps
    // ticking. JoinError is treated as a scan failure and falls through to the
    // `diagnostic_available = false` path — matches the FFI-error branch.
    #[cfg(windows)]
    let (processes, diagnostic_available) = {
        let wg_for_scan = canonical_wg.clone();
        match tokio::task::spawn_blocking(move || scan_external_processes_windows(&wg_for_scan)).await {
            Ok(Ok(p)) => (p, true),
            Ok(Err(e)) => {
                log::warn!("[wg_delete_diagnostic] Restart Manager scan failed: {}", e);
                (Vec::new(), false)
            }
            Err(join_err) => {
                log::warn!("[wg_delete_diagnostic] Restart Manager scan task panicked: {}", join_err);
                (Vec::new(), false)
            }
        }
    };

    #[cfg(not(windows))]
    let (processes, diagnostic_available) = (Vec::<BlockerProcess>::new(), false);

    BlockerReport {
        workgroup: workgroup_name.to_string(),
        platform: detect_platform(),
        diagnostic_available,
        raw_os_error: raw_os_error.to_string(),
        sessions,
        processes,
    }
}

fn detect_platform() -> Platform {
    if cfg!(windows) { Platform::Windows }
    else if cfg!(target_os = "linux") { Platform::Linux }
    else if cfg!(target_os = "macos") { Platform::Macos }
    else { Platform::Other }
}

/// Return wg_dir canonicalised, with the Windows extended-length prefixes stripped
/// for shape parity with `entity_creation.rs:903`. Round-2 fix (G.3.2): also handle
/// the UNC variant — `canonicalize` returns `\\?\UNC\server\share\…` for a network
/// path, and a blind `\\?\` strip leaves `UNC\server\share\…` (malformed). Strip
/// `\\?\UNC\` → `\\` first, then `\\?\` → `` (order matters because `\\?\UNC\`
/// starts with `\\?\`).
fn canonicalize_for_compare(p: &Path) -> PathBuf {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = canon.to_string_lossy();
    let stripped = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    };
    PathBuf::from(stripped)
}

/// Walk the workgroup tree breadth-first and collect up to MAX_FILES_TO_PROBE absolute
/// file paths to feed RmRegisterResources. Hot files (lock-prone metadata) are always
/// taken first so the budget can't be exhausted on a single `.git/objects/` subtree.
///
/// Round-2 fix (G.2.6): the prior DFS variant could exhaust the 200-file cap inside one
/// repo's `.git/objects/pack/` and never visit a sibling repo. With BFS the queue fans
/// out level-by-level, so every top-level child (each `repo-*` dir, each `__agent_*`
/// dir) is visited before the budget is consumed. Hot-file prioritisation provides a
/// further safety margin: anything matching `is_hot_lock_candidate` is always taken
/// before any cold file, so the typical Windows blocker (a `.lock` file) is registered
/// even if the cold pool would have drowned it.
///
/// Skips dirs we can't read; never follows symlinks.
#[cfg(windows)]
fn collect_files_to_probe(wg_dir: &Path) -> Vec<PathBuf> {
    use std::collections::VecDeque;

    /// Files commonly held by long-running processes inside an AC workgroup:
    /// any `*.lock` (git index.lock, ORIG_HEAD.lock, packfile locks), plus the
    /// lock-prone metadata files inside any `.git/` subtree.
    fn is_hot_lock_candidate(p: &Path) -> bool {
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(s) => s,
            None => return false,
        };
        if name.ends_with(".lock") {
            return true;
        }
        let in_git = p
            .ancestors()
            .any(|a| a.file_name().and_then(|n| n.to_str()) == Some(".git"));
        in_git
            && matches!(
                name,
                "index" | "HEAD" | "ORIG_HEAD" | "FETCH_HEAD" | "MERGE_HEAD" | "packed-refs"
            )
    }

    /// Soft ceiling on total walk size — once we've inventoried 4× the probe
    /// budget, we have plenty to choose from. Avoids walking gigabytes of
    /// `.git/objects/` when the WG is unusually large.
    const WALK_SOFT_CEILING: usize = MAX_FILES_TO_PROBE * 4;

    let mut hot: Vec<PathBuf> = Vec::new();
    let mut cold: Vec<PathBuf> = Vec::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(wg_dir.to_path_buf());

    while let Some(dir) = queue.pop_front() {
        if hot.len() + cold.len() >= WALK_SOFT_CEILING {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                queue.push_back(path);
            } else if ft.is_file() {
                if is_hot_lock_candidate(&path) {
                    hot.push(path);
                } else {
                    cold.push(path);
                }
            }
        }
    }

    // Hot files always win. Cold fills the remainder.
    let mut out = hot;
    out.truncate(MAX_FILES_TO_PROBE);
    let remaining = MAX_FILES_TO_PROBE - out.len();
    out.extend(cold.into_iter().take(remaining));
    out
}

/// Scan SessionManager for sessions whose working_directory lives under wg_dir.
async fn scan_ac_sessions(
    canonical_wg: &Path,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> Vec<BlockerSession> {
    let mgr = session_mgr.read().await;
    let sessions = mgr.list_sessions().await;
    drop(mgr);

    sessions
        .into_iter()
        .filter_map(|s| {
            let cwd_canon = canonicalize_for_compare(Path::new(&s.working_directory));
            if !cwd_canon.starts_with(canonical_wg) {
                return None;
            }
            // Reuse the same FQN derivation the rest of the codebase uses.
            // Round-2 fix (G.3.1): the `if agent_name.is_empty()` fallback was
            // unreachable — `agent_fqn_from_path` only returns "" for empty input,
            // and `Session::working_directory` is never empty (sessions can't spawn
            // without a CWD). Use the FQN directly.
            let agent_name = crate::config::teams::agent_fqn_from_path(&s.working_directory);
            Some(BlockerSession {
                session_id: s.id,
                agent_name,
                cwd: s.working_directory,
            })
        })
        .collect()
}

/// Two-pass Restart Manager scan:
///
/// 1. **Bulk pass** — open one RM session, register all probed files at once,
///    call `RmGetList` to learn which PIDs hold any handles. Cheap (one session).
/// 2. **Per-file attribution pass** — RM doesn't tell us *which* file each PID
///    held. Iterate files; for each, open a fresh session, register only that
///    file, and accumulate (file → matching PID) entries. Cap each PID at
///    `MAX_FILES_PER_PROCESS` (5) and short-circuit once every blocker PID is
///    saturated — bounds the work tightly even for 200-file probes.
///
/// FFI surface (verified against
/// `windows-sys-0.59.0/src/Windows/Win32/System/RestartManager/mod.rs`):
///   RmStartSession(*mut u32 sessionHandle, 0, PWSTR sessionKey) -> WIN32_ERROR
///   RmRegisterResources(handle, nFiles, *const PCWSTR, 0, null, 0, null) -> WIN32_ERROR
///   RmGetList(handle, *mut u32 needed, *mut u32 have, *mut RM_PROCESS_INFO,
///             *mut u32 rebootReasons) -> WIN32_ERROR
///   RmEndSession(handle) -> WIN32_ERROR
///
/// `RM_PROCESS_INFO` is `#[repr(C)]` with `strAppName: [u16; 256]` (inline NUL-
/// terminated UTF-16, NOT a pointer) and `Process: RM_UNIQUE_PROCESS` carrying
/// `dwProcessId: u32` and `ProcessStartTime: FILETIME`.
///
/// Calling pattern for `RmGetList`: first probe with `have = 0` to learn
/// `needed`, then allocate and retry. May return `ERROR_MORE_DATA` (234) again
/// if processes appeared between calls — bounded retry.
///
/// PID name resolution: prefer `RM_PROCESS_INFO::strAppName`; fall back to
/// `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, ...)` + `QueryFullProcessImageNameW`
/// when `strAppName` is empty (common for non-GUI processes).
#[cfg(windows)]
fn scan_external_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
    use std::collections::{HashMap, HashSet};
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
        RM_PROCESS_INFO,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const ERROR_SUCCESS: u32 = 0;
    const ERROR_MORE_DATA: u32 = 234;
    /// `RmGetList` can keep returning `ERROR_MORE_DATA` if the process list grows between
    /// probe and read. Three retries is the standard Microsoft sample bound.
    const MAX_GETLIST_RETRIES: usize = 3;

    fn to_wide_nul(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// RAII guard for an RM session — guarantees `RmEndSession` runs even on early
    /// return / panic. Leaking a session persists across the AC process lifetime
    /// and burns Restart Manager's per-process session quota (default 64).
    struct RmSession(u32);
    impl Drop for RmSession {
        fn drop(&mut self) {
            unsafe {
                let _ = RmEndSession(self.0);
            }
        }
    }

    fn rm_start() -> Result<RmSession, String> {
        let mut handle: u32 = 0;
        // `strSessionKey` is PWSTR (mutable). Must hold CCH_RM_SESSION_KEY+1 wide chars.
        let mut key: Vec<u16> = vec![0u16; (CCH_RM_SESSION_KEY as usize) + 1];
        let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
        if rc != ERROR_SUCCESS {
            return Err(format!("RmStartSession failed: WIN32_ERROR={}", rc));
        }
        Ok(RmSession(handle))
    }

    /// `wide_files` must outlive `ptrs` — keep both alive through the FFI call.
    fn rm_register(handle: u32, wide_files: &[Vec<u16>]) -> Result<(), String> {
        if wide_files.is_empty() {
            return Ok(());
        }
        let ptrs: Vec<*const u16> = wide_files.iter().map(|w| w.as_ptr()).collect();
        let rc = unsafe {
            RmRegisterResources(
                handle,
                ptrs.len() as u32,
                ptrs.as_ptr(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(format!("RmRegisterResources failed: WIN32_ERROR={}", rc));
        }
        Ok(())
    }

    fn rm_get_list(handle: u32) -> Result<Vec<RM_PROCESS_INFO>, String> {
        let mut needed: u32 = 0;
        let mut have: u32 = 0;
        let mut reasons: u32 = 0;

        // Probe: zero buffer, learn `needed`. RM returns SUCCESS with needed=0 when
        // there are no blockers — distinct from MORE_DATA.
        let rc = unsafe {
            RmGetList(
                handle,
                &mut needed,
                &mut have,
                std::ptr::null_mut(),
                &mut reasons,
            )
        };
        if rc == ERROR_SUCCESS {
            return Ok(Vec::new());
        }
        if rc != ERROR_MORE_DATA {
            return Err(format!("RmGetList probe failed: WIN32_ERROR={}", rc));
        }

        for _ in 0..MAX_GETLIST_RETRIES {
            let mut buf: Vec<RM_PROCESS_INFO> = Vec::with_capacity(needed as usize);
            have = needed;
            let rc = unsafe {
                RmGetList(handle, &mut needed, &mut have, buf.as_mut_ptr(), &mut reasons)
            };
            if rc == ERROR_SUCCESS {
                // Round-2 fix (G.2.5): defensive cap. If RM ever wrote `have > needed`
                // (RM bug, hostile race, or kernel quirk), `set_len` on a Vec with
                // `capacity == needed` and `len > capacity` is immediate UB. The
                // Microsoft docs say this can't happen; one `min` removes the
                // soundness hole entirely. The `debug_assert!` surfaces a violation
                // in dev builds without paying for it in release.
                debug_assert!(
                    (have as usize) <= (needed as usize),
                    "RmGetList wrote {} entries into a buffer sized for {}",
                    have,
                    needed
                );
                let actual = (have as usize).min(needed as usize);
                // SAFETY: capacity is `needed` (from Vec::with_capacity above), and
                // `actual ≤ needed` by the `min` cap. RM wrote `actual` valid
                // `RM_PROCESS_INFO`s into the buffer.
                unsafe { buf.set_len(actual); }
                return Ok(buf);
            }
            if rc == ERROR_MORE_DATA {
                continue; // grow on next iteration
            }
            return Err(format!("RmGetList read failed: WIN32_ERROR={}", rc));
        }
        Err("RmGetList: ERROR_MORE_DATA persisted past retry budget".into())
    }

    /// `RM_PROCESS_INFO::strAppName` is a fixed `[u16; 256]` array, NUL-terminated.
    fn process_info_app_name(info: &RM_PROCESS_INFO) -> String {
        let arr = &info.strAppName;
        let nul = arr.iter().position(|&c| c == 0).unwrap_or(arr.len());
        String::from_utf16_lossy(&arr[..nul])
    }

    /// Fallback PID → exe basename via `QueryFullProcessImageNameW`. Empty on failure.
    fn pid_to_exe_basename(pid: u32) -> String {
        // `OpenProcess` returns NULL on failure (e.g. cross-session, permission denied,
        // process exited). Treat all failure modes the same — return empty and let the
        // caller fall back to a numeric label.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return String::new();
            }
            // Round-2 fix (G.3.4): MAX_PATH=260 is too tight on Windows 10/11 with
            // long-path support enabled — some toolchains have exe paths >260 chars.
            // QueryFullProcessImageNameW returns ERROR_INSUFFICIENT_BUFFER for those
            // and we'd silently degrade to "pid {N}". 1024 wide chars (2 KB stack)
            // covers all practical exe paths.
            let mut buf: [u16; 1024] = [0; 1024];
            let mut size: u32 = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
            let _ = CloseHandle(h);
            if ok == 0 || size == 0 {
                return String::new();
            }
            let path = String::from_utf16_lossy(&buf[..size as usize]);
            path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
        }
    }

    /// FILETIME equality — tolerates PID recycling between bulk and per-file passes.
    fn same_start(
        a: &windows_sys::Win32::Foundation::FILETIME,
        b: &windows_sys::Win32::Foundation::FILETIME,
    ) -> bool {
        a.dwLowDateTime == b.dwLowDateTime && a.dwHighDateTime == b.dwHighDateTime
    }

    // ── Phase 0: collect files to probe ──────────────────────────────────────
    let files = collect_files_to_probe(wg_dir);
    if files.is_empty() {
        return Ok(Vec::new());
    }
    // Defensive: drop files that vanished between collection and the FFI call.
    // `RmRegisterResources` aborts the batch on `ERROR_FILE_NOT_FOUND`.
    let alive: Vec<&PathBuf> = files.iter().filter(|p| p.exists()).collect();
    if alive.is_empty() {
        return Ok(Vec::new());
    }
    let wide_files: Vec<Vec<u16>> = alive
        .iter()
        .map(|p| to_wide_nul(&p.to_string_lossy()))
        .collect();

    // ── Phase 1: bulk pass — get the set of blocker PIDs ─────────────────────
    //
    // Round-2 fix (G.2.3): the original "one session, one register-all" variant
    // aborted the entire diagnostic if *any* single file rejected
    // RmRegisterResources (e.g. ERROR_ACCESS_DENIED on a permission-restricted
    // path, ERROR_FILE_NOT_FOUND on a vanished path, ERROR_INVALID_PARAMETER on
    // a weird reparse point). Binary-search-with-fallback isolates the bad file
    // to a leaf and skips just that one. RmRegisterResources REPLACES the
    // resource set per call (not append), so each successful sub-batch gets its
    // own session — bounded by O(log N + bad_files) sessions, well under the
    // RM quota of 64.
    fn collect_blockers_tolerant(
        wide: &[Vec<u16>],
        original_paths: &[&PathBuf], // parallel slice for log messages only
    ) -> Vec<RM_PROCESS_INFO> {
        if wide.is_empty() {
            return Vec::new();
        }
        let session = match rm_start() {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[wg_delete_diagnostic] bulk-pass RmStartSession failed (giving up on this sub-batch of {}): {}",
                    wide.len(),
                    e
                );
                return Vec::new();
            }
        };
        match rm_register(session.0, wide) {
            Ok(()) => rm_get_list(session.0).unwrap_or_else(|e| {
                log::warn!(
                    "[wg_delete_diagnostic] bulk-pass RmGetList failed for sub-batch of {}: {}",
                    wide.len(),
                    e
                );
                Vec::new()
            }),
            Err(e) if wide.len() == 1 => {
                log::warn!(
                    "[wg_delete_diagnostic] skipping unregisterable file '{}': {}",
                    original_paths[0].display(),
                    e
                );
                Vec::new()
            }
            Err(_) => {
                drop(session); // release before recursing
                let mid = wide.len() / 2;
                let mut left =
                    collect_blockers_tolerant(&wide[..mid], &original_paths[..mid]);
                let right =
                    collect_blockers_tolerant(&wide[mid..], &original_paths[mid..]);
                left.extend(right);
                left
            }
        }
        // session Drop fires here (or already fired on the recursion path)
    }

    let bulk_list: Vec<RM_PROCESS_INFO> =
        collect_blockers_tolerant(&wide_files, &alive);

    if bulk_list.is_empty() {
        // Distinguishes "diagnostic ran cleanly, nothing held the lock" from
        // "diagnostic blew up." `diagnostic_available` stays true at the call
        // site (we returned `Ok`); frontend renders the "No blockers identified"
        // copy. Acceptable — even if individual sub-batches choked, the union
        // of registered files is still a meaningful negative result.
        return Ok(Vec::new());
    }

    // Seed the result map with one entry per unique PID. Resolve the executable
    // name now (prefer `strAppName`, fall back to `QueryFullProcessImageNameW`).
    let mut by_pid: HashMap<u32, BlockerProcess> = HashMap::new();
    for info in &bulk_list {
        let pid = info.Process.dwProcessId;
        if pid == 0 {
            // System Idle / kernel — never actionable, never a real blocker.
            continue;
        }
        let name = {
            let app = process_info_app_name(info);
            if app.is_empty() {
                let exe = pid_to_exe_basename(pid);
                if exe.is_empty() { format!("pid {}", pid) } else { exe }
            } else {
                app
            }
        };
        by_pid.entry(pid).or_insert(BlockerProcess {
            pid,
            name,
            files: Vec::new(),
        });
    }

    // ── Phase 2: per-file attribution ────────────────────────────────────────
    let target_pids: HashSet<u32> = by_pid.keys().copied().collect();
    for (path, wide) in alive.iter().zip(wide_files.iter()) {
        if by_pid.values().all(|p| p.files.len() >= MAX_FILES_PER_PROCESS) {
            break; // every blocker has its quota — further probing wastes RM sessions
        }
        if !path.exists() {
            continue; // raced again; skip
        }
        let single = std::slice::from_ref(wide);

        let session = match rm_start() {
            Ok(s) => s,
            Err(e) => {
                log::warn!(
                    "[wg_delete_diagnostic] per-file RmStartSession failed for {}: {}",
                    path.display(),
                    e
                );
                continue;
            }
        };
        if rm_register(session.0, single).is_err() {
            continue; // file vanished mid-scan or RM rejected it; non-fatal
        }
        let list = match rm_get_list(session.0) {
            Ok(l) => l,
            Err(_) => continue,
        };
        drop(session); // explicit RmEndSession before next iteration

        for info in &list {
            let pid = info.Process.dwProcessId;
            if !target_pids.contains(&pid) {
                continue;
            }
            // PID-recycle defence: the bulk pass saw a different process if the
            // ProcessStartTime FILETIME differs.
            if let Some(b) = bulk_list.iter().find(|b| b.Process.dwProcessId == pid) {
                if !same_start(&b.Process.ProcessStartTime, &info.Process.ProcessStartTime) {
                    continue;
                }
            }
            if let Some(entry) = by_pid.get_mut(&pid) {
                if entry.files.len() < MAX_FILES_PER_PROCESS {
                    entry.files.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    Ok(by_pid.into_values().collect())
}
```

**Notes for the implementer (locked down by Step 3):**

1. **Helper `to_wide_nul`** is defined inline — keep it inside the function so it
   doesn't pollute the module namespace.
2. **`RmSession` Drop guard** is the only path to `RmEndSession`. Never call
   `RmEndSession` manually — relying on Drop covers panic, early return, and
   `?`-propagation uniformly.
3. **`collect_files_to_probe` is called twice** (architect intent) — once via the
   `let files = ...` line. The defensive `path.exists()` filter runs *immediately
   before each FFI call* and again inside the per-file loop, since a file can
   vanish at any moment.
4. **PID 0 is filtered.** RM sometimes lists "System Idle Process" — never the
   real blocker; pollutes the report.
5. **`String::from_utf16_lossy`** is the right choice for `strAppName` —
   `from_utf16` rejects invalid surrogates and we don't want to fail the whole
   scan on one weird app name.
6. **Restart Manager session quota** (default 64 per process). Using the
   per-file-pass strategy with `MAX_FILES_TO_PROBE = 200` means up to 201 sessions
   over the diagnostic's lifetime — but the early-exit caps the per-file pass at
   ~`MAX_FILES_PER_PROCESS × distinct_blocker_pids` sessions in practice. With
   typical ≤3 blockers this is ~15 sessions, well under quota. Each session is
   `RmEndSession`'d before the next one opens (Drop runs at scope exit), so quota
   never accumulates.
7. **`RmRegisterResources` aborts the entire batch on `ERROR_FILE_NOT_FOUND`** —
   not just the one bad file. The `path.exists()` filter is therefore mandatory,
   not defensive-belt-and-suspenders. If a file races between the filter and the
   FFI call, the session simply returns no PIDs for that probe — non-fatal,
   logged as warn, scan continues.

### 4.3 `src-tauri/src/commands/mod.rs` — register new module

**Add line after line 11 (`pub mod window;`):**

```rust
pub mod wg_delete_diagnostic;
```

### 4.4 `src-tauri/src/commands/entity_creation.rs::delete_workgroup` — wire diagnostic

**Current** (lines 799–800):

```rust
    std::fs::remove_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to delete workgroup directory: {}", e))?;
```

**Replace with:**

```rust
    if let Err(e) = std::fs::remove_dir_all(&wg_dir) {
        // Detect Windows os error 32 (file in use). On other OSes / other error kinds,
        // fall through to the legacy raw-error string so existing UX is unchanged.
        let raw = e.to_string();
        if is_file_in_use_error(&e) {
            log::info!(
                "[entity_creation] delete_workgroup: file-in-use detected for '{}', running blocker diagnostic",
                workgroup_name
            );
            let report = crate::commands::wg_delete_diagnostic::diagnose_blockers(
                &wg_dir,
                &workgroup_name,
                &raw, // raw OS error verbatim — see Dev-rust additions C.1
                session_mgr.inner(),
            )
            .await;
            let json = serde_json::to_string(&report).map_err(|se| {
                format!("Failed to serialize blocker report: {}; original error: {}", se, raw)
            })?;
            return Err(format!("BLOCKERS:{}", json));
        }
        return Err(format!("Failed to delete workgroup directory: {}", raw));
    }
```

**Add this helper** in the "Internal helpers" section (after line 1344, end of
`check_workgroup_repos_dirty`):

```rust
/// True iff `e` represents the Windows "file in use" error (os error 32, ERROR_SHARING_VIOLATION).
/// On non-Windows always returns false (Linux / macOS produce different error codes for
/// "directory not empty due to open file" and we don't run the diagnostic there).
fn is_file_in_use_error(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        // Win32 ERROR_SHARING_VIOLATION = 32. raw_os_error returns the Win32 code on Windows.
        return e.raw_os_error() == Some(32);
    }
    #[cfg(not(windows))]
    {
        let _ = e;
        false
    }
}
```

**Note on placement:** keep the dirty-repos check (lines 783–797) UNCHANGED. The diagnostic
fires only after `remove_dir_all` itself fails — `DIRTY_REPOS:` keeps its current short-circuit.

### 4.5 `src/shared/types.ts` — TS mirror

**Append at end of file:**

```ts
// ---------------------------------------------------------------------------
// Workgroup-delete blocker report (BLOCKERS: sentinel payload)
// Mirrors src-tauri/src/commands/wg_delete_diagnostic.rs structs.
// ---------------------------------------------------------------------------

export interface BlockerSession {
  sessionId: string;
  agentName: string;
  cwd: string;
}

export interface BlockerProcess {
  pid: number;
  name: string;
  files: string[];
}

export interface BlockerReport {
  workgroup: string;
  platform: "windows" | "linux" | "macos" | "other";
  diagnosticAvailable: boolean;
  rawOsError: string;
  sessions: BlockerSession[];
  processes: BlockerProcess[];
}
```

### 4.6 `src/sidebar/components/ProjectPanel.tsx` — modal + parser

**4.6.a — import update (line 3):**

```ts
import type { AcWorkgroup, AcAgentReplica, AcTeam, Session, TelegramBotConfig, BlockerReport } from "../../shared/types";
```

**4.6.b — add signals after line 209 (`const [wgDirtyRepos, ...]`):**

```tsx
const [wgBlockers, setWgBlockers] = createSignal<BlockerReport | null>(null);
// Tracks an in-flight Retry from the blockers modal. Distinct from
// wgDeleteInProgress so the bottom "Delete" button's spinner state
// doesn't flicker when Retry is clicked.
const [wgRetryInProgress, setWgRetryInProgress] = createSignal(false);
// Round-2 fix (G.2.1): the original Delete click may have used force=true
// (dirty-repos confirm flow). On Retry from BLOCKERS, we MUST replay that
// force value, otherwise the backend re-runs the dirty-repo check and
// surfaces DIRTY_REPOS: again — making the user re-type the WG name they
// already confirmed. Captured in §4.6.d at click time.
const [wgLastForceUsed, setWgLastForceUsed] = createSignal(false);
// Round-2 fix (G.2.7): generation counter for the in-flight retry. If the
// user closes the modal mid-retry (or opens a different WG's modal in the
// same project iteration scope), `closeWgDeleteModal` bumps `retryGen` and
// the awaiting retry's post-await branch sees `myGen !== retryGen` and
// bails. Plain `let` (not a signal) — never read reactively.
let retryGen = 0;
```

**4.6.c — extend `closeWgDeleteModal` (lines 222–228):**

```tsx
const closeWgDeleteModal = () => {
  setWgDeleteError("");
  setWgDirtyRepos(false);
  setWgConfirmText("");
  setWgDeleteInProgress(false);
  setWgBlockers(null);              // NEW
  setWgRetryInProgress(false);      // NEW
  setWgLastForceUsed(false);        // NEW (round-2, G.2.1)
  retryGen++;                       // NEW (round-2, G.2.7) — orphans any in-flight retry
  setDeletingWg(null);
};
```

**4.6.d — extend the Delete click handler (lines 1318–1344):**

Two edits: (1) capture the force-state into the signal before awaiting, so a
later Retry can replay it (round-2, G.2.1); (2) extend the catch block with the
`BLOCKERS:` branch.

**Edit 1:** right after the existing `const forceDelete = wgDirtyRepos();` (line
~1324), add:
```tsx
setWgLastForceUsed(forceDelete);   // round-2 (G.2.1) — replayed by retryWgDelete
```

**Edit 2:** insert a sentinel branch BEFORE the `DIRTY_REPOS:` branch in the
existing catch block (order matters only for clarity — the prefixes are disjoint):

```tsx
} catch (e: any) {
  console.error("delete_workgroup failed:", e);
  const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete workgroup";
  // BLOCKERS: sentinel — render structured blocker list, no force-delete option.
  if (msg.startsWith("BLOCKERS:")) {
    try {
      const report = JSON.parse(msg.slice("BLOCKERS:".length)) as BlockerReport;
      setWgBlockers(report);
      // Round-2 fix (G.2.2): if the user reached delete_workgroup via the
      // dirty-repos confirm flow, the DIRTY_REPOS UI is still rendered behind
      // the BLOCKERS banner. Clear it so the modal body shows exactly one banner.
      setWgDirtyRepos(false);
      setWgConfirmText("");
      setWgDeleteError("");
      setWgDeleteInProgress(false);
      return;
    } catch (parseErr) {
      // Round-2 fix (G.2.4): on parse failure, do NOT fall through to
      // setWgDeleteError(msg) — `msg` is the full unparsed `"BLOCKERS:{json}"`
      // string and would render as ugly red debug text in the modal. Show a
      // sanitised fallback instead.
      console.error("Failed to parse BLOCKERS: payload:", parseErr);
      setWgDeleteError("Workgroup is locked, but the blocker report could not be parsed. Try again.");
      setWgDeleteInProgress(false);
      return;
    }
  }
  // DIRTY_REPOS: sentinel prefix — switch to force-confirm mode
  if (!forceDelete && msg.startsWith("DIRTY_REPOS:")) {
    setWgDeleteError(msg.slice("DIRTY_REPOS:".length));
    setWgDirtyRepos(true);
    setWgConfirmText("");
    setWgDeleteInProgress(false);
    return;
  }
  setWgDeleteError(msg);
  setWgDeleteInProgress(false);
  return;
}
```

**4.6.e — add the blocker list UI inside the modal body** (insert after the existing
`<Show when={wgDirtyRepos()}>` block, before the closing `</div>` of `new-agent-form`,
around line 1309):

```tsx
<Show when={wgBlockers()}>
  {(() => {
    const r = wgBlockers()!;
    return (
      <div style={{
        "background": "var(--danger, #c0392b)",
        "color": "#fff",
        "padding": "10px 12px",
        "border-radius": "6px",
        "margin-top": "10px",
        "font-size": "12px",
        "line-height": "1.5",
      }}>
        <strong>Cannot delete:</strong> the workgroup is locked by the following:
        <Show when={r.sessions.length > 0}>
          <div style={{ "margin-top": "6px" }}><strong>AC sessions</strong></div>
          <ul style={{ margin: "4px 0 6px 16px", padding: "0" }}>
            <For each={r.sessions}>
              {(s) => <li>{s.agentName} <span style={{ opacity: 0.75 }}>({s.cwd})</span></li>}
            </For>
          </ul>
        </Show>
        <Show when={r.processes.length > 0}>
          <div style={{ "margin-top": "6px" }}><strong>External processes</strong></div>
          <ul style={{ margin: "4px 0 6px 16px", padding: "0" }}>
            <For each={r.processes}>
              {(p) => (
                <li>
                  {p.name} (PID {p.pid})
                  <Show when={p.files.length > 0}>
                    <ul style={{ margin: "2px 0 0 16px", padding: "0", "font-size": "11px", opacity: 0.85 }}>
                      <For each={p.files}>{(f) => <li>{f}</li>}</For>
                    </ul>
                  </Show>
                </li>
              )}
            </For>
          </ul>
        </Show>
        <Show when={!r.diagnosticAvailable}>
          <div style={{ "margin-top": "6px", opacity: 0.85 }}>
            Diagnostic not available on this platform. Raw error: <code>{r.rawOsError}</code>
          </div>
        </Show>
        <Show when={r.diagnosticAvailable && r.sessions.length === 0 && r.processes.length === 0}>
          <div style={{ "margin-top": "6px", opacity: 0.85 }}>
            No blockers identified. The lock may be transient — try again in a moment.
            Raw error: <code>{r.rawOsError}</code>
          </div>
        </Show>
        <div style={{ "margin-top": "8px" }}>
          Close the listed sessions / quit the listed processes, then click <strong>Retry</strong> below.
        </div>
        <div style={{ "margin-top": "10px", display: "flex", "justify-content": "flex-end" }}>
          <button
            class="new-agent-create-btn"
            style={{ "background": "#fff", "color": "var(--danger, #c0392b)", "min-width": "84px" }}
            disabled={wgRetryInProgress() || wgDeleteInProgress()}
            onClick={retryWgDelete}
          >
            {wgRetryInProgress() ? "Retrying…" : "Retry"}
          </button>
        </div>
      </div>
    );
  })()}
</Show>
```

The Retry button mirrors the danger-banner palette (white background, red text)
so it stays clearly attached to the blocker block — visually distinct from the
modal-footer `Cancel` / `Delete` pair, which are disabled while `wgBlockers()`
is non-null (per §4.6.f).

**4.6.f — disable the Delete button while blockers are shown** (line 1318):

```tsx
disabled={
  wgDeleteInProgress()
  || activeReplicas().length > 0
  || (wgDirtyRepos() && wgConfirmText() !== deletingWg()!.name)
  || wgBlockers() !== null    // NEW: force user to retry only after closing blockers
}
```

Rationale: the user must dismiss/retry explicitly, not accidentally re-fire while the
list is still on screen. The Cancel button (line 1312) still works.

**4.6.g — `retryWgDelete` handler (NEW; Step 3 user-override addition).**

User confirmed in the Step-3 brief: NO kill button (info-only modal), but ADD an inline
Retry button so the user doesn't have to close+reopen the modal between attempts. The
button (rendered inside the blocker banner per §4.6.e) wires to this handler, which
mirrors the existing Delete-button onClick logic but treats results differently:

- **Success** → close modal, reload project (silent, matches the existing successful-
  delete UX — see "no toast" rationale below).
- **Still-`BLOCKERS:`** → refresh `wgBlockers` in place. Modal contents update; user
  is invited to close more stuff and click Retry again. No transient flash, no toast.
- **`DIRTY_REPOS:`** → blockers cleared, dirty-repos UI takes over the same modal —
  matches the first-attempt sentinel ordering. (Practically unreachable when the
  retry replays force=true per G.2.1 fix below — the backend skips the dirty-repo
  check. Still possible if the user un-staged the prior force-confirm by closing
  and reopening; handle defensively.)
- **Other error** → blockers cleared, generic error rendered via `wgDeleteError`. Modal
  stays open with the error text in the same body.

**Round-2 fixes folded into the handler below (`retryWgDelete`):**

- **G.2.1** — `force` is now read from `wgLastForceUsed()` (captured by the
  Delete onClick at §4.6.d), not hardcoded to `false`. Preserves the user's
  prior force-confirm so a BLOCKERS-after-dirty flow doesn't make them re-type
  the WG name.
- **G.2.2** — every BLOCKERS branch (success refresh, parse-fail) clears
  `wgDirtyRepos` and `wgConfirmText` so two banners can never stack.
- **G.2.4** — parse-failure shows a sanitised message, never the raw
  `BLOCKERS:{json}` payload.
- **G.2.7** — `myGen = ++retryGen` snapshot at start; every post-await write
  is guarded with `if (myGen !== retryGen) return;`. `closeWgDeleteModal`
  bumps `retryGen` to orphan in-flight retries.

Insert as a top-level component-scope function next to the other modal handlers
(after `closeWgDeleteModal`, ~line 229):

```tsx
const retryWgDelete = async () => {
  if (wgRetryInProgress()) return;
  const wg = deletingWg();
  if (!wg) return;
  // Find the project the same way the existing Delete button does — via the
  // closure capture of `proj` in the rendered modal scope. (`retryWgDelete` is
  // declared *inside* the project-scoped render block, mirroring the existing
  // delete handler at line 1319; see §4.6.e for the call-site placement.)
  setWgRetryInProgress(true);

  // Round-2 fix (G.2.7): capture a generation snapshot. If the user closes the
  // modal (or fires another flow) mid-await, `closeWgDeleteModal` bumps
  // `retryGen`, and our post-await guard `myGen !== retryGen` aborts every
  // state write below. Prevents banner-resurrection on a closed modal and
  // cross-WG signal contamination.
  const myGen = ++retryGen;

  // Round-2 fix (G.2.1): replay the force-state of the originating Delete
  // click. If the user reached BLOCKERS via the dirty-repos confirm flow,
  // they passed `force=true`; passing `false` here would re-trigger the
  // dirty-repos check and defeat the confirm they already gave.
  const force = wgLastForceUsed();

  try {
    await EntityAPI.deleteWorkgroup(proj.path, wg.name, force);
    if (myGen !== retryGen) return;        // canceled mid-flight (G.2.7)
    await projectStore.reloadProject(proj.path);
    if (myGen !== retryGen) return;
    closeWgDeleteModal();
  } catch (e: any) {
    if (myGen !== retryGen) return;        // canceled mid-flight (G.2.7)
    const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete workgroup";
    if (msg.startsWith("BLOCKERS:")) {
      try {
        const report = JSON.parse(msg.slice("BLOCKERS:".length)) as BlockerReport;
        setWgBlockers(report);     // refresh in place; banner re-renders
        // Round-2 fix (G.2.2): orthogonal-state reset on every BLOCKERS hop.
        setWgDirtyRepos(false);
        setWgConfirmText("");
        setWgDeleteError("");
        setWgRetryInProgress(false);
        return;
      } catch (parseErr) {
        // Round-2 fix (G.2.4): sanitised fallback copy, never the raw payload.
        console.error("Failed to parse BLOCKERS: payload on retry:", parseErr);
        setWgBlockers(null);
        setWgDeleteError("Workgroup is still locked, but the blocker report could not be parsed. Try again.");
        setWgRetryInProgress(false);
        return;
      }
    }
    if (msg.startsWith("DIRTY_REPOS:")) {
      setWgBlockers(null);
      setWgDeleteError(msg.slice("DIRTY_REPOS:".length));
      setWgDirtyRepos(true);
      setWgConfirmText("");
      setWgRetryInProgress(false);
      return;
    }
    setWgBlockers(null);
    setWgDeleteError(msg);
    setWgRetryInProgress(false);
  }
};
```

**Placement note:** because the existing modal closes over `proj` (the current project
in the iterator at the modal-render scope), `retryWgDelete` must be declared inside
the same scope. Concretely: declare it at the same indentation as the existing
`closeWgDeleteModal` near line 229, *but only render the modal/portal that references
it inside the project-iteration block*. If the existing modal lives outside that scope
(verify against the actual file when implementing), promote `retryWgDelete` into a
component-scoped helper that takes `wg` and `proj` as args.

**No toast on success — deliberate.** Searched the codebase for a global toast
primitive; the only `showToast` is local to `ActionBar.tsx` (file-scoped helper,
not exported). Adding a global toast just for this success path would be scope
creep. The existing successful-delete UX is silent (close + project reload) —
Retry mirrors that exactly. If a global toast lands in a future change, the hook
point is right after `closeWgDeleteModal()` in the success branch above.

---

## 5. Dependencies

- **Crate dependency change** (Cargo.toml only): two extra `windows-sys` features
  (`Win32_System_RestartManager`, `Win32_System_ProcessStatus`). NO new crates.
- **No new Tauri command.** No `lib.rs` registration change.
- **No new Tauri Event.** No frontend listener wiring change.

---

## 6. Notes / edge cases for dev-rust and dev-webpage-ui

### 6.1 OS-error gating

The diagnostic triggers on `raw_os_error()` matching any of:
`ERROR_SHARING_VIOLATION (32)`, `ERROR_LOCK_VIOLATION (33)`, `ERROR_USER_MAPPED_FILE (1224)`.

**Rationale for expansion (post-Step 6b /feature-dev):** VSCode and other IDEs hold files
via memory-mapped I/O, which Windows surfaces as `ERROR_USER_MAPPED_FILE (1224)` — not
`ERROR_SHARING_VIOLATION (32)`. The user's motivating real-world case is exactly that
scenario; shipping with `error == 32` only would risk the diagnostic never firing in the
most common case. `ERROR_LOCK_VIOLATION (33)` added for completeness — same blocker
semantics (a sibling holds the file), different driver path.

**Out of gate (intentional):** `ERROR_ACCESS_DENIED (5)`, `ERROR_FILE_NOT_FOUND (2)`,
`ERROR_DIR_NOT_EMPTY (145)`. Those are separate failure modes — not file-in-use cases —
and keep the legacy raw-string error path. A negative unit test in `wg_delete_diagnostic::tests`
locks this in (`is_file_in_use_error_rejects_unrelated_errors_on_windows`).

### 6.2 Path comparison

`Session::working_directory` is stored as the user typed/derived it (may be relative-shaped
or contain mixed separators). The scan canonicalises both sides via `std::fs::canonicalize`
and strips the `\\?\` UNC prefix — same shape used at `entity_creation.rs:903` and
`ac_discovery.rs:294`. Use `PathBuf::starts_with`, NOT string `starts_with` — case
sensitivity and separator normalisation differ.

### 6.3 Race: session destroyed between failure and scan

Acceptable. The list is best-effort — if a session is gone by the time we read
`SessionManager`, it can't be the blocker, so omitting it is correct.

### 6.4 Restart Manager limitations

- RM only sees handles opened via standard Win32 APIs. NTFS file-system filter drivers
  (some AV software) can hide handles. Acceptable in v1.
- RM's `RM_PROCESS_INFO::strAppName` is sometimes empty for non-GUI processes. Fall back
  to `K32GetModuleBaseNameW(OpenProcess(pid))` — see skeleton notes 4.2.
- RM session creation requires no special privileges, but the *target process* enumeration
  may exclude processes running as a different user (e.g., a Windows service started as
  SYSTEM). Document in the modal copy if this is a real-world hit; not v1 work.

### 6.5 Performance

`collect_files_to_probe` walks the workgroup tree with a 200-file cap. For typical
workgroups (a few `repo-*` clones + agent dirs), this is well under that. No async
needed — runs in tens of milliseconds. The diagnostic is one-shot (post-failure),
not on a hot path.

### 6.6 What dev MUST NOT do

- Do NOT add a preflight check on the happy path. The diagnostic only runs after
  `remove_dir_all` fails with os error 32.
- Do NOT change the `DIRTY_REPOS:` branch order or shape — frontend regressions there
  break the existing dirty-repo flow.
- Do NOT add a "force delete" button to the blockers UI. The user has to free the
  resource; we do not silently retry past a sharing violation.
- Do NOT introduce a new Tauri command — the diagnostic rides on the existing
  `delete_workgroup` `Result<(), String>`.
- Do NOT add `anyhow`, `notify`, or any crate not already in `Cargo.toml`. Restart
  Manager goes through `windows-sys` (already a dep).
- Do NOT canonicalise paths returned by Restart Manager before showing them — preserve
  the OS-reported casing/shape so the user can recognise their own path.

---

## Dev-rust additions (Step 3)

This section captures everything dev-rust changed, deviated from, or added on top of
the architect's draft. Treat as a Step-2 → Step-3 diff log.

### A. Verification pass — every reference confirmed against current code

Cross-checked the architect's plan against the working tree at
`feature/113-wg-delete-blockers-diagnostic`:

| Reference in plan | Actual location | Status |
|---|---|---|
| `Cargo.toml` lines 33–34 (`windows-sys` block) | `src-tauri/Cargo.toml:33-34` | ✓ matches verbatim |
| `entity_creation.rs` lines 763–808 (`delete_workgroup`) | same | ✓ |
| `entity_creation.rs` lines 799–800 (`remove_dir_all` + `map_err`) | same | ✓ |
| `entity_creation.rs:903` (UNC-prefix strip in `build_session_repo`) | same | ✓ |
| `entity_creation.rs:1344` (end of `check_workgroup_repos_dirty`) | same — fn starts line 1258, closes line 1344 | ✓ |
| `commands/mod.rs:11` (`pub mod window;`) | same — file is exactly 11 lines | ✓ |
| `lib.rs:758` (`commands::entity_creation::delete_workgroup,`) | same | ✓ |
| `ProjectPanel.tsx:3` (import line) | same | ✓ |
| `ProjectPanel.tsx:209` (`wgDirtyRepos` signal) | same | ✓ |
| `ProjectPanel.tsx:222–228` (`closeWgDeleteModal`) | same | ✓ |
| `ProjectPanel.tsx:1309` (`</Show>` for dirty-repos block) | same — `</Show>` at 1309, `</div>` of `new-agent-form` at 1310 | ✓ |
| `ProjectPanel.tsx:1312` (Cancel button) | same | ✓ |
| `ProjectPanel.tsx:1318` (Delete `disabled` prop) | same | ✓ |
| `ProjectPanel.tsx:1328–1342` (catch block) | same | ✓ |
| `SessionInfo` shape (id: String, working_directory: String) | `src-tauri/src/session/session.rs:147-...` | ✓ |
| `SessionManager::list_sessions() -> Vec<SessionInfo>` | `src-tauri/src/session/manager.rs:164` | ✓ |
| `crate::config::teams::agent_fqn_from_path(&str) -> String` | `src-tauri/src/config/teams.rs:62` | ✓ |
| `pty/git_watcher.rs` (GitWatcher) | exists | ✓ |
| `commands/ac_discovery.rs:234` (DiscoveryBranchWatcher) | exists | ✓ |
| `pty/manager.rs:25` (ResponseWatcher) | exists | ✓ |

No mismatches found. The plan's anchors are accurate.

### B. Deviations from architect's draft

**B.1 — drop `Win32_System_ProcessStatus` feature.** See §4.1 deviation note. Saves
one feature flag; uses `QueryFullProcessImageNameW` (already in the enabled
`Win32_System_Threading`) with the lighter `PROCESS_QUERY_LIMITED_INFORMATION`
access right.

**B.2 — wrap the Windows scan in `tokio::task::spawn_blocking`.** See §4.2
`diagnose_blockers` body. The architect's skeleton called the FFI synchronously on
the calling Tokio task. The full FFI loop is ~1 s wall-time post-failure (200-file
probe + per-file attribution); blocking a Tokio worker for that span starves
PTY reads and IPC events. `spawn_blocking` moves it onto the dedicated blocking
pool. JoinError is treated as scan failure (degrades to `diagnostic_available =
false`), matching the FFI-error branch.

**B.3 — full FFI body for `scan_external_processes_windows`.** See §4.2. Replaces
`todo!()` with the two-pass strategy (bulk + per-file) and the supporting helpers.
Strategy choice and rationale documented inline in the doc comment and in the
"Notes for the implementer" block.

**B.4 — Retry button instead of close-and-reopen flow.** Step-3 user override (per
tech-lead brief): the modal is info-only (no kill button), but adds an inline
Retry button so the user re-fires `delete_workgroup` without losing the modal.
Spec'd in §4.6.b/c/e/g. Behaviour matrix:

| Retry result | UX |
|---|---|
| `Ok(())` | Close modal, reload project (silent — matches existing successful-delete UX) |
| Still `BLOCKERS:` | Refresh `wgBlockers` in place; banner re-renders with new list; no toast/flash |
| `DIRTY_REPOS:` | Clear blockers UI; switch same modal to dirty-repos confirm flow |
| Other error | Clear blockers UI; render generic error in same modal body |

### C. New issues spotted during read-through

**C.1 — `raw_os_error` field stripped of editorial prefix (applied inline in §4.4).**
The architect's draft passed `&format!("Failed to delete workgroup directory: {}",
raw)` into the `raw_os_error` field. The field is named `rawOsError` and is
consumed *structurally* by the frontend — `Raw error: <code>{r.rawOsError}</code>`.
The "Failed to…" prefix made the field name a lie and rendered as duplicated
text on the modal. Edited the wiring in §4.4 to pass `&raw` verbatim; the
prefix is preserved on the *legacy non-os-error-32 fallback path* since that
string is rendered as-is by the modal's generic error block.

Also added a `log::info!` at the top of the BLOCKERS branch (per §C.9) so the
diagnostic firing leaves a trail in the AC log.

**C.2 — `RmRegisterResources` aborts on `ERROR_FILE_NOT_FOUND`.** The architect's
note 4.2.4 said "add a defensive `Path::exists()` filter". Strengthened in §4.2:
the filter is *mandatory*, not defensive — without it, a single vanished file
breaks the whole bulk pass. Implemented in the FFI body's Phase 0.

**C.3 — RM session quota.** Default 64 sessions per process. The per-file
attribution pass could theoretically open up to `MAX_FILES_TO_PROBE = 200`
sessions sequentially; each is `RmEndSession`'d via Drop before the next opens, so
quota does not accumulate. The early-exit (every blocker PID saturated at 5
files) in practice caps live work at ~15 sessions. Documented in §4.2 note 6.

**C.4 — PID 0 filter.** Restart Manager occasionally surfaces "System Idle Process"
(PID 0). It is never the real blocker and adds noise to the report. Filtered in
the bulk-pass loop. Documented in §4.2 note 4.

**C.5 — `String::from_utf16_lossy` for `strAppName`.** RM-returned WCHAR arrays can
contain unpaired surrogates (rare, but possible if a process's window title is
mangled). `from_utf16` would `Err` and we'd fall back to PID resolution
unnecessarily; `from_utf16_lossy` keeps the partial name with replacement chars.
Documented in §4.2 note 5.

**C.6 — clippy lints to watch in implementation:**
- `clippy::ptr_as_ptr` / `clippy::cast_possible_truncation` on `ptrs.len() as u32`
  — bounded by `MAX_FILES_TO_PROBE = 200`, well under `u32::MAX`. Inline
  `#[allow]` not needed but acceptable if clippy gets noisy.
- `clippy::needless_collect` may fire on the `let ptrs: Vec<*const u16> =
  wide_files.iter().map(|w| w.as_ptr()).collect();` line. False positive — the
  collected Vec must outlive the FFI call so the pointers stay valid. Add
  `#[allow(clippy::needless_collect)]` only if the lint actually fires.
- `clippy::too_many_arguments` on `delete_workgroup` — already triggered for
  adjacent functions; pattern is `#[allow(clippy::too_many_arguments)]` per
  `update_team` at line 812.

**C.7 — `tokio::task::spawn_blocking` Send bound.** The closure captures
`canonical_wg: PathBuf` by `move`. `PathBuf: Send + 'static` ✓. The
`SessionManager` reference is *not* captured by the closure — the AC-session
scan completes before `spawn_blocking` is called. So Send/Sync issues from
`tauri::State<>` (which is `!Send` in some configurations) do not propagate
into the blocking task. Verified by inspection of the §4.2 `diagnose_blockers`
body order.

**C.8 — Frontend `proj` capture for `retryWgDelete`.** The existing modal
(`ProjectPanel.tsx:1255–1351`) renders inside an iterator over projects, so
`proj` is in lexical scope. `retryWgDelete` must be declared in the same scope
or take `proj` + `wg` as parameters. §4.6.g specifies the inline-declaration
approach with a fallback if the actual scope differs at implementation time.
dev-webpage-ui to verify when applying §4.6.

**C.9 — Logging discipline.** Per Role.md the diagnostic should `log::info!` on
fire and `log::warn!` on FFI sub-failures. The fire-time info log is added at
the *call site* in §4.4 (knows the workgroup name, fires before the scan
starts). Sub-failure warns are inline in the §4.2 FFI body. Recommended
addition during implementation: a closing `log::info!` in `diagnose_blockers`
right before the return, summarising counts:
`"diagnostic done: {} sessions, {} processes, available={}"`.

**C.10 — TS mirror types: `BlockerReport.platform`.** The Rust side serializes
`platform` as `String` ("windows" | "linux" | "macos" | "other"). The TS
mirror narrows it to a literal union. If the Rust side ever emits a value
outside that union (e.g. someone adds "freebsd"), TS clients silently fail
the type check at use sites. Either widen the TS type to `string`, or add
a Rust-side enum to lock the producer. Recommend: leave TS as the literal
union (cheap to update if the producer adds variants; gives precision today).
Flagged for grinch.

### D. Things the architect locked that we are NOT changing

- Sentinel format (`BLOCKERS:` + JSON) — matches `DIRTY_REPOS:` precedent.
- Module placement (dedicated `wg_delete_diagnostic.rs`).
- Public struct names (`BlockerReport`, `BlockerSession`, `BlockerProcess`).
- Field naming (camelCase via serde `rename_all`).
- Diagnostic firing only on Windows os error 32, never as a preflight.
- Out-of-scope items (§8) — kill button, preflight, watcher introspection,
  cross-platform diagnostic.

---

## 7. Test plan

### Rust unit tests

All in a `#[cfg(test)] mod tests { ... }` block at the bottom of
`src-tauri/src/commands/wg_delete_diagnostic.rs`. Pattern matches existing test
modules in the crate (e.g. `config/teams.rs:713`).

**7.1 — `is_file_in_use_error` recognises os error 32 on Windows, no-ops elsewhere.**

```rust
#[cfg(windows)]
#[test]
fn is_file_in_use_error_matches_sharing_violation_on_windows() {
    use crate::commands::entity_creation::is_file_in_use_error;
    let e = std::io::Error::from_raw_os_error(32);
    assert!(is_file_in_use_error(&e), "os error 32 must match");

    let other = std::io::Error::from_raw_os_error(2); // ERROR_FILE_NOT_FOUND
    assert!(!is_file_in_use_error(&other), "non-32 must not match");
}

#[cfg(not(windows))]
#[test]
fn is_file_in_use_error_no_op_on_non_windows() {
    use crate::commands::entity_creation::is_file_in_use_error;
    let e = std::io::Error::from_raw_os_error(32);
    assert!(!is_file_in_use_error(&e), "non-Windows must always return false");
}
```

`is_file_in_use_error` is currently private in `entity_creation.rs` per §4.4.
Either expose it as `pub(crate)` (preferred — keeps the helper next to the call
site) or move the test inside `entity_creation.rs`'s own test module. dev-rust
to pick during implementation; either works.

**7.2 — `canonicalize_for_compare` strips the UNC prefix.**

```rust
#[test]
fn canonicalize_for_compare_strips_unc_prefix() {
    let tmp = std::env::temp_dir();
    let canon_via_helper = canonicalize_for_compare(&tmp);
    let canon_raw = std::fs::canonicalize(&tmp).unwrap();
    let raw_str = canon_raw.to_string_lossy();
    let expected_str = raw_str.strip_prefix(r"\\?\").unwrap_or(&raw_str);
    assert_eq!(canon_via_helper.to_string_lossy(), expected_str);
}
```

`canonicalize_for_compare` is private in `wg_delete_diagnostic.rs` — the test
lives in the same module so visibility is not a concern.

**7.3 — `scan_ac_sessions` filters by canonical prefix.**

Synthetic-input shape: build a `SessionManager`, create two sessions —
one with `working_directory` inside a temp dir, one outside — and assert the
returned `Vec<BlockerSession>` contains exactly the inside one.

```rust
#[tokio::test]
async fn scan_ac_sessions_filters_by_canonical_prefix() {
    use crate::session::manager::SessionManager;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let tmp = tempfile::tempdir().expect("tempdir");
    let inside = tmp.path().join("inside");
    let outside = std::env::temp_dir().join("blockers-test-outside");
    std::fs::create_dir_all(&inside).ok();
    std::fs::create_dir_all(&outside).ok();

    let mgr = SessionManager::new();
    let _ = mgr.create_session(
        "powershell.exe".into(), vec![],
        inside.to_string_lossy().to_string(),
        None, None, vec![], false,
    ).await;
    let _ = mgr.create_session(
        "powershell.exe".into(), vec![],
        outside.to_string_lossy().to_string(),
        None, None, vec![], false,
    ).await;
    let mgr = Arc::new(RwLock::new(mgr));

    let canonical_wg = canonicalize_for_compare(tmp.path());
    let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

    assert_eq!(blockers.len(), 1);
    assert!(blockers[0].cwd.contains("inside"));

    // cleanup
    let _ = std::fs::remove_dir_all(&outside);
}
```

Adds `tempfile` as a `[dev-dependencies]` entry in `Cargo.toml` if not already
present. (Quick check at implementation time — it's a common dev dep; if the
crate doesn't have it, dev-rust adds it under `[dev-dependencies]` only.)

**7.4 — `BlockerReport` JSON shape matches the TS mirror.**

No snapshot crate in the project (verified — only `#[test]` style throughout).
Test by serialising a hand-rolled `BlockerReport` and asserting field names
+ shapes via `serde_json::Value` round-trip.

```rust
#[test]
fn blocker_report_serializes_with_camelcase_fields() {
    let report = BlockerReport {
        workgroup: "wg-7-dev-team".into(),
        platform: "windows".into(),
        diagnostic_available: true,
        raw_os_error: "...".into(),
        sessions: vec![BlockerSession {
            session_id: "abc".into(),
            agent_name: "wg-7-dev-team/architect".into(),
            cwd: r"C:\foo".into(),
        }],
        processes: vec![BlockerProcess {
            pid: 42,
            name: "git.exe".into(),
            files: vec![r"C:\foo\bar".into()],
        }],
    };
    let json: serde_json::Value = serde_json::to_value(&report).unwrap();
    // Top-level fields
    for k in &["workgroup", "platform", "diagnosticAvailable", "rawOsError", "sessions", "processes"] {
        assert!(json.get(*k).is_some(), "missing field: {}", k);
    }
    // BlockerSession fields
    let s = &json["sessions"][0];
    for k in &["sessionId", "agentName", "cwd"] {
        assert!(s.get(*k).is_some(), "missing session field: {}", k);
    }
    // BlockerProcess fields
    let p = &json["processes"][0];
    for k in &["pid", "name", "files"] {
        assert!(p.get(*k).is_some(), "missing process field: {}", k);
    }
    // Snake-case must NOT appear at the wire boundary.
    for k in &["diagnostic_available", "raw_os_error", "session_id", "agent_name"] {
        assert!(json.get(*k).is_none(), "leaked snake_case field: {}", k);
    }
}
```

This catches the most likely regression: a future struct edit that drops
`#[serde(rename_all = "camelCase")]` and ships a snake_case JSON to the
frontend, silently breaking the TS parse.

### Rust integration / smoke tests (Windows-only, manual)

**7.5 — Notepad-blocks-delete scenario** (full happy-Retry path):

Pre-conditions: clean working tree on `feature/113-wg-delete-blockers-diagnostic`,
`cargo build` succeeded, `npm run tauri dev` running.

Steps:
1. Create a workgroup in any test project (right-click team → Create Workgroup).
2. Open the WG's `BRIEF.md` (or any file inside the WG tree) in Notepad.
3. Right-click the WG in the sidebar → Delete Workgroup → confirm.
4. **Expected**: blockers modal appears. Lists at least `notepad.exe` with
   the file path. No force-delete confirmation appears (different from
   DIRTY_REPOS path). Bottom Cancel/Delete buttons disabled (Delete grayed
   per §4.6.f).
5. Close Notepad.
6. Click the inline **Retry** button in the banner.
7. **Expected**: modal closes, project reloads, WG is gone from sidebar.

Variants to run:
- **7.5.a — multiple blockers:** open the same file in two editors (Notepad +
  VS Code). Expect both PIDs listed.
- **7.5.b — Retry while still blocked:** at step 6, click Retry *without*
  closing Notepad. Expect modal to refresh in place (banner re-renders, no
  flash, no toast). Then close Notepad, click Retry again, expect success.
- **7.5.c — Retry into DIRTY_REPOS:** between two Retry attempts, modify a
  file in a `repo-*` clone inside the WG (introduces dirty-repos state).
  Click Retry. Expect blockers UI to clear and dirty-repos confirm to appear
  in the same modal. (Edge case — addresses §C in Dev-rust additions.)
- **7.5.d — AC session as blocker:** in a fresh WG, spawn an agent session
  via the sidebar (so the session's PTY child holds open handles inside the
  WG). Try to delete. Expect the `sessions[]` list in the modal to contain
  the agent's FQN. Note: the legacy "active replicas" check at line 1272
  may also fire and pre-block — verify ordering. The `BLOCKERS:` path runs
  *only* if `remove_dir_all` actually fails, so if the legacy active-replicas
  check already covers this case, it short-circuits before our diagnostic.
  Document which one wins.

**7.6 — Diagnostic OFF on non-Windows (smoke).** Build on Linux or macOS,
trigger any path that would call `diagnose_blockers` (delete a WG with files
in use is hard to reproduce non-Windows since `remove_dir_all` semantics
differ; instead, `cargo test` should compile cleanly with the `#[cfg(not(windows))]`
fallback returning `diagnostic_available = false`). Pure compile-time test —
the goal is "doesn't break the non-Windows build."

### Frontend tests (manual)

**7.7 — `BLOCKERS:` payload renders.** Mock the Tauri command response with:
```json
{
  "workgroup": "wg-test",
  "platform": "windows",
  "diagnosticAvailable": true,
  "rawOsError": "The process cannot access the file because it is being used by another process. (os error 32)",
  "sessions": [{ "sessionId": "x", "agentName": "wg-test/alice", "cwd": "C:\\\\wg-test\\\\__agent_alice" }],
  "processes": [
    { "pid": 12345, "name": "git.exe", "files": ["C:\\\\wg-test\\\\repo-foo\\\\.git\\\\index.lock"] },
    { "pid": 67890, "name": "notepad.exe", "files": ["C:\\\\wg-test\\\\BRIEF.md"] }
  ]
}
```
Confirm: AC sessions list renders, processes list renders with files indented,
Retry button visible inside banner, footer Cancel works (closes modal cleanly,
all state reset), footer Delete is disabled.

**7.8 — Retry happy path.** With the BLOCKERS modal open from §7.7, mock the
next `delete_workgroup` call to resolve `Ok(())`. Click Retry. Confirm: modal
closes, no toast, `wgRetryInProgress` cleared. (Verify by inspecting the
component's signals via dev tools.)

**7.9 — Retry still-blocked.** With the BLOCKERS modal open, mock the next call
to reject with a *different* `BLOCKERS:` payload (e.g. only the `notepad.exe`
process — the user closed `git.exe`). Click Retry. Confirm: banner re-renders
with the new (shorter) list; `wgRetryInProgress` clears; no flash, no toast.

**7.10 — Retry → DIRTY_REPOS.** With the BLOCKERS modal open, mock the next call
to reject with `DIRTY_REPOS:...`. Click Retry. Confirm: blockers UI disappears,
dirty-repos confirm input appears in the same modal.

**7.11 — Retry → other error.** With the BLOCKERS modal open, mock the next call
to reject with a non-sentinel string. Click Retry. Confirm: blockers UI
disappears, generic error rendered via `wgDeleteError`, modal stays open.

**7.12 — `diagnosticAvailable: false`.** Mock with `diagnosticAvailable: false`,
empty sessions/processes, and a non-empty `rawOsError`. Confirm: platform
fallback message renders, no AC-sessions or processes lists, Retry still
visible (works the same — can succeed if the underlying lock cleared).

### Frontend regression tests (manual)

**7.13 — `DIRTY_REPOS:` flow unchanged.** Without any `BLOCKERS:` involvement,
trigger the existing dirty-repos path. Confirm: force-confirm input still
appears, typing the WG name enables the Delete button, force-delete still
succeeds.

**7.14 — Successful first-attempt delete unchanged.** Delete a clean WG with
no blockers. Confirm: modal closes, project reloads, no blocker banner ever
flashes.

**7.15 — Active-replicas pre-block unchanged.** Delete a WG with an active
session running. Confirm: the existing "Cannot delete: the following sessions
are still active" banner appears (same as today), Delete stays disabled, no
attempt is made to call `delete_workgroup` and no `BLOCKERS:` path fires.

### Round-2 additions to §7 (covers G.4.1–G.4.9)

**7.16 — BLOCKERS-from-dirty-confirm flow (covers G.4.1, G.2.1, G.2.2).** Manual
on Windows. Make a `repo-*/.git/index` dirty inside the WG (touch a tracked
file). Open Notepad on `WG/BRIEF.md`. Click Delete → DIRTY_REPOS confirm
appears → type the WG name → click Delete (force=true). Expected: BLOCKERS
modal appears with `notepad.exe`; the dirty-repos confirm input is GONE (only
the BLOCKERS banner is visible). Close Notepad, click Retry. Expected: WG
deletes successfully without re-prompting for the WG name (force was replayed).

**7.17 — `BLOCKERS:` JSON-parse failure shows sanitized fallback (covers G.4.2,
G.2.4).** Frontend mock: have the mocked `delete_workgroup` reject with
`"BLOCKERS:not-valid-json"`. Trigger via the Delete click. Expected: modal
shows "Workgroup is locked, but the blocker report could not be parsed. Try
again." (NOT the raw `BLOCKERS:not-valid-json` string). `wgDeleteInProgress`
clears. Repeat for the Retry path with the same mocked response — Retry
parse-failure shows "Workgroup is **still** locked, but the blocker report
could not be parsed. Try again."

**7.18 — Bulk-pass partial-failure tolerance (covers G.4.3, G.2.3).** Manual
on Windows. Inside a workgroup, create a file and remove read permission for
the current user (`icacls path /deny %USERNAME%:R`). Open Notepad on a
sibling file. Click Delete. Expected: BLOCKERS modal lists `notepad.exe`
with the sibling file (the unreadable file is skipped via the binary-search-
fallback in `collect_blockers_tolerant`, surfaced as a `log::warn!` line in
the AC log). Restore permissions afterward.

**7.19 — Deeply-nested-WG file selection (covers G.4.4, G.2.6).** Manual
on Windows. Create a WG with two `repo-*` clones; let the first repo accumulate
a large `.git/objects/` tree (e.g. clone a sizeable history). In the *second*
repo, hold `repo-bar/.git/index.lock` via PowerShell:
```powershell
$f = [System.IO.File]::Open("...repo-bar\.git\index.lock", "Create", "Read", "Read")
```
Click Delete. Expected: BLOCKERS modal lists `powershell.exe` with the
`index.lock` path. (With the round-2 BFS-with-hot-priority change, a
`.lock` file in any sibling repo is always probed even when the first
repo's git tree is huge.) Release with `$f.Close()` after the test.

**7.20 — `set_len` precondition documented (covers G.4.5, G.2.5).**
Code-review verification: confirm the `debug_assert!((have as usize) <=
(needed as usize), …)` line lives directly above `unsafe { buf.set_len(actual);
}` in `rm_get_list`. No runtime test is needed (mocking `RmGetList` would
be heavy); the assert runs in dev-build CI and the cap-by-min ensures
soundness in release. Add a code comment if not already present:
`// SAFETY: actual ≤ needed = capacity; see G.2.5 fix in §4.2`.

**7.21 — Retry-during-cancel race (covers G.4.6, G.2.7).** Frontend mock.
Open BLOCKERS modal. Click Retry; the mock has a 200 ms delay before
resolving. While the await is pending, click Cancel. Then let the mock
resolve with another `BLOCKERS:{json}` payload. Expected: the modal stays
closed, no banner re-appears, no `setWgBlockers` call lands (verifiable via
spy on the setter or by inspection of the rendered DOM 50 ms after
resolution). Repeat for the success-resolve case (also expected: no
re-render on a closed modal).

**7.22 — Sentinel collision invariant (covers G.4.7).** Pure-Rust unit test
(append to the same `#[cfg(test)] mod tests` block):
```rust
#[test]
fn workgroup_names_cannot_collide_with_sentinels() {
    use crate::commands::entity_creation::validate_existing_name;
    // Both prefixes contain ':' (BLOCKERS:) or '_' and ':' (DIRTY_REPOS:),
    // neither of which is in the validator's alphanumeric+'-' whitelist.
    assert!(validate_existing_name("BLOCKERS:foo", "Workgroup").is_err());
    assert!(validate_existing_name("DIRTY_REPOS:foo", "Workgroup").is_err());
    // Bare-prefix-without-colon would be alphanumeric and pass the validator,
    // but a bare prefix isn't a sentinel hit (frontend uses startsWith with the colon).
    assert!(validate_existing_name("BLOCKERS", "Workgroup").is_ok());
    assert!(validate_existing_name("DIRTY-REPOS", "Workgroup").is_ok());
}
```
This locks the invariant: WG names can never accidentally produce a
`BLOCKERS:` or `DIRTY_REPOS:` substring at message-prefix position.
`validate_existing_name` is at `entity_creation.rs:116`; verified
character-set covers exactly `is_ascii_alphanumeric() || c == '-'`.

**7.23 — `canonicalize_for_compare` UNC strip (covers G.4.8, G.3.2).** Pure-
Rust unit test, no FS dependency:
```rust
#[test]
fn canonicalize_for_compare_strips_unc_prefix() {
    // Synthesise the canonical-shape strings without touching the FS.
    let unc_canon = PathBuf::from(r"\\?\UNC\server\share\proj");
    let drive_canon = PathBuf::from(r"\\?\C:\Users\me\proj");
    let no_prefix = PathBuf::from(r"C:\Users\me\proj");
    // Exercise the prefix-strip logic via a helper that mirrors `canonicalize_for_compare`
    // minus the `std::fs::canonicalize` call (factor out into a private `strip_long_prefix`).
    assert_eq!(strip_long_prefix(&unc_canon).to_string_lossy(), r"\\server\share\proj");
    assert_eq!(strip_long_prefix(&drive_canon).to_string_lossy(), r"C:\Users\me\proj");
    assert_eq!(strip_long_prefix(&no_prefix).to_string_lossy(), r"C:\Users\me\proj");
}
```
Implementation note: dev-rust to refactor the prefix-strip half of
`canonicalize_for_compare` into a private `strip_long_prefix(&Path) -> PathBuf`
helper so the test can hit it without disk I/O.

**7.24 — Drop-on-panic for `RmSession` (covers G.4.9).** Accept-by-inspection
per grinch's "minimal version is fine." A unit test that constructs a
fake handle and panics inside the scope is heavy for the value (would need
to replace `RmEndSession` with a thread-local-counter mock). Document the
correctness in §4.2 note 2 (already done) and add an inline `// G.4.9:
RmSession::Drop is the only path to RmEndSession; covers panic, ?, return.`
comment at the `impl Drop for RmSession` site.

---

## 8. Out of scope (do not add to v1)

- Killing offending processes from AC.
- Preflight blocker check.
- AC watcher introspection (see §2.2).
- Cross-platform diagnostic (Linux/macOS uses different mechanisms — `lsof`, `fuser`).
- Auto-retry deletion after a delay.
- Telemetry / logging of blocker reports for analytics.

---

## 9. Hand-off

Reply path back to tech-lead:
`repo-AgentsCommander/_plans/feature-113-wg-delete-blockers-diagnostic.md`

Step 3 owner (dev-rust) needs to:
- Implement the `scan_external_processes_windows` body (FFI loop) per §4.2 skeleton.
- Add the unit tests in §7.
- Verify the manual Windows scenario in §7.4.

Step 3 owner (dev-webpage-ui) needs to:
- Apply §4.6 modifications.
- Verify the mocked manual scenarios in §7 (frontend section).

If either dev hits a question that requires re-design, message the architect rather
than diverging from this plan.

---

## Grinch additions (Step 4)

Verification pass: every plan anchor in §A re-confirmed independently
(`entity_creation.rs:799-800`, `:1344`, `:903`; `commands/mod.rs` 11 lines;
`session/manager.rs:164`; `session/session.rs:147`; `config/teams.rs:62`;
`lib.rs:758`; `ProjectPanel.tsx:3`, `:209`, `:222-228`, `:1255-1351`,
`:1318`, `:1328-1342`). FFI signatures in §4.2 cross-checked against
`windows-sys-0.59.0/src/Windows/Win32/System/RestartManager/mod.rs` and
`...Foundation/mod.rs` and `...System/Threading/mod.rs` — all match
(`RmStartSession`, `RmRegisterResources`, `RmGetList`, `RmEndSession`,
`OpenProcess`, `QueryFullProcessImageNameW`, `RM_PROCESS_INFO`,
`RM_UNIQUE_PROCESS`, `FILETIME { dwLowDateTime, dwHighDateTime }`,
`HANDLE = *mut c_void`).

Verdict up front: **no critical / data-loss / crash issues found.** Several
likely bugs and risky design choices. Plan needs another iteration before
implementation, but the architecture is sound — no rewrites, only spec
amendments.

### G.1 — Critical issues

None. The FFI body is conservative, the Drop guard is correct on every
`?`/`continue`/panic path, the bulk-then-per-file strategy is sound, and the
sentinel format is collision-free.

### G.2 — Likely bugs

**G.2.1 — Retry forces a re-confirmation when the original click was a dirty-repos
force-delete.** §4.6.g hardcodes `force=false` in `retryWgDelete`, with the
justification "BLOCKERS is orthogonal to dirty-repos; never auto-force." That
reasoning is wrong for the case where the user *already* typed the WG name
to confirm dirty-repos and clicked Delete (so `force=true` was passed to the
*original* call). The original call bypassed the dirty-repo check, then
`remove_dir_all` failed with os-error 32 and surfaced BLOCKERS:. On Retry,
the handler passes `force=false` → backend re-runs the dirty-repo check →
returns `DIRTY_REPOS:` → §4.6.g's DIRTY_REPOS branch fires → `setWgConfirmText("")`
→ user must re-type the WG name even though they confirmed seconds earlier.

**Why it matters:** concrete UX regression, easy to hit (any user who has
ever force-deleted a workgroup and run into a BLOCKERS state).

**Fix:** `retryWgDelete` should preserve the force-state from the originating
Delete click. Either capture it into a new signal `wgLastForceUsed` set in
the §4.6.d Delete onClick at the same point `forceDelete` is computed, or
re-read `wgDirtyRepos()` at retry click-time:
```tsx
const force = wgDirtyRepos() && wgConfirmText() === wg.name;
await EntityAPI.deleteWorkgroup(proj.path, wg.name, force);
```
Either approach works; the signal-capture is cleaner because `wgConfirmText`
may already have been cleared.

**G.2.2 — BLOCKERS payload from a dirty-repos confirm flow leaves the dirty-repos
confirm UI rendered behind it.** §4.6.d's BLOCKERS branch sets `setWgBlockers(report)`
and clears `wgDeleteError`, but does NOT clear `wgDirtyRepos` or `wgConfirmText`.
If the user reached `delete_workgroup` via the dirty-repos confirm flow
(`wgDirtyRepos()` was true at click time), the BLOCKERS banner appears on top
of the still-rendered "type the WG name" input box (it's gated on
`<Show when={wgDirtyRepos()}>`, line 1294). Two danger banners stacked,
visually confusing.

**Why it matters:** the §4.6.d catch path can fire from any state; it must
fully reset orthogonal UI state. Hits in any flow where a force-delete
trips into a sharing violation.

**Fix:** in §4.6.d's BLOCKERS branch and §4.6.g's BLOCKERS branch, also call:
```tsx
setWgDirtyRepos(false);
setWgConfirmText("");
```
This ensures the modal's body has exactly one banner (BLOCKERS) at any time.

**G.2.3 — Bulk-pass aborts the entire diagnostic on any single
`RmRegisterResources` failure.** §4.2 propagates errors from the bulk
`rm_register` via `?` (line `rm_register(session.0, &wide_files)?;`). RM
aborts the batch on `ERROR_FILE_NOT_FOUND`, but also on
`ERROR_ACCESS_DENIED`, `ERROR_INVALID_PARAMETER`, etc. for any *one* file in
the batch. A single funky path (e.g. an NTFS reparse point, a permission-
restricted file) causes the whole bulk pass to fail → caller hits the
`Err(_)` branch → frontend shows `diagnosticAvailable: false` → user sees
"Diagnostic not available on this platform" copy (§4.6.e), which is wrong:
the platform is Windows; the diagnostic just choked on one file.

**Why it matters:** a single bad path blanks out the entire diagnostic. WGs
are typically full of git internals, lock files, and OS-managed metadata —
plenty of opportunity for one to be in a weird state.

**Fix (any one):**
1. (Cheapest, recommended) Soften the bulk-pass error: on `ERROR_*` from
   `rm_register`, attempt one retry with the half of `wide_files` that
   excludes the problematic file (binary search, or just retry with each
   half — bounded recursion). Reasonable ceiling: log a warn and proceed
   with whatever subset registers cleanly.
2. Or distinguish `diagnostic_available = true` (Windows but FFI choked)
   from the non-Windows fallback, and surface a different copy in §4.6.e
   ("Diagnostic could not run; raw error: …"). Plan currently conflates
   both into the same `diagnosticAvailable: false` branch.
3. Or split the bulk pass into batches of N files, accept partial failures.

**G.2.4 — `BlockerReport` JSON parse failure on the frontend leaves the user staring
at a raw `BLOCKERS:{json}` string in the modal.** §4.6.d's `try { … } catch
(parseErr) { … fall through to the raw-error path below }` does
`setWgDeleteError(msg)` where `msg` is the *full* unparsed `"BLOCKERS:…"`
string. The user sees the raw payload in red text.

**Why it matters:** any Rust-side struct change that drifts the JSON shape
(e.g., a future field added without serde defaults) ships an unparseable
payload. The plan's §7.4 test guards the wire boundary, but a legitimate
JSON-parse failure (also possible if the backend ever serializes a control
character in `rawOsError`) drops the user into a debug-grade error display.

**Fix:** in the parse-failure branch, sanitize the prefix:
```tsx
} catch (parseErr) {
  console.error("Failed to parse BLOCKERS: payload:", parseErr);
  setWgDeleteError("Workgroup is locked, but the blocker report could not be parsed. Try again.");
  setWgDeleteInProgress(false);
  return;
}
```
Same in §4.6.g's retry handler parse-failure branch.

**G.2.5 — `RmGetList` `set_len` trusts the API to never write past `pnProcInfo`
input.** §4.2 has:
```rust
have = needed;
let rc = unsafe { RmGetList(handle, &mut needed, &mut have, buf.as_mut_ptr(), &mut reasons) };
if rc == ERROR_SUCCESS {
    unsafe { buf.set_len(have as usize); }
```
If RM ever wrote `have > needed` (bug or hostile race condition), `set_len`
on a `Vec` with `capacity == needed` and `length > capacity` is **immediate
UB**. The Microsoft docs say RM writes "the number of structures filled in"
and that should never exceed input, but defensive coding cheaply avoids the
UB:
```rust
let actual = (have as usize).min(needed as usize);
unsafe { buf.set_len(actual); }
```
**Fix:** add the `min` cap before `set_len`. One line, removes the soundness
hole entirely.

**G.2.6 — `collect_files_to_probe` is depth-first with no breadth fairness;
a deeply nested WG with the blocker file outside the first 200 DFS-visited
nodes will get an empty diagnostic.** Stack-based DFS visits one branch fully
before moving to the next. For a typical WG layout
(`__agent_*/`, `repo-*/.git/objects/...`), DFS could exhaust the 200 cap
inside `repo-foo/.git/objects/pack/` before ever touching `repo-bar/`. If
the actual blocker is `repo-bar/.git/index.lock`, the diagnostic returns an
empty processes list — frontend shows "No blockers identified. The lock may
be transient — try again in a moment." (§4.6.e), misleading the user.

**Why it matters:** real Windows blockers concentrate in `.git/` lock files
(git index.lock, ORIG_HEAD.lock, etc.) which are ALWAYS the same depth
across all repos. DFS that fills the budget on one repo's git internals
misses every other repo entirely.

**Fix:** switch to BFS, or seed the probe with known-likely targets first
(any `*.lock` file at any depth, plus `.git/index`, plus `__agent_*/CLAUDE.md`,
plus the WG root files), THEN fall back to DFS for the remainder. A simple
implementation: walk the tree once collecting ALL paths up to a higher cap
(say 5000) cheaply (`std::fs::read_dir` is fast), then take the first 200
prioritising matches against a known-blocker-heuristic list.

Not blocking, but probability of "empty diagnostic with active notepad
holding repo-foo/BRIEF.md" is non-trivial.

**G.2.7 — Race: modal closed while a retry is in-flight.** If the user clicks
Cancel during `retryWgDelete`'s `await EntityAPI.deleteWorkgroup(...)`,
`closeWgDeleteModal` runs and resets `deletingWg=null`, `wgBlockers=null`,
etc. The async retry then settles and runs its post-await branches —
including the success branch's `closeWgDeleteModal()` (idempotent, OK) or
the BLOCKERS branch's `setWgBlockers(report)` (resurrects the modal banner
on a now-closed modal). Worse: if the user has by now opened a *different*
WG's delete modal (same project iteration scope, same signals), the stale
retry's `setWgBlockers` will overwrite the new modal's state.

**Why it matters:** the project-iteration scope shares signals. Any
in-flight retry whose closure references stale `wg`/`proj` can step on a
fresh modal interaction.

**Fix:** capture a generation counter at retry start, abort post-await
state writes if the counter has changed:
```tsx
const gen = ++retryGenRef;
try {
  await EntityAPI.deleteWorkgroup(...);
  if (gen !== retryGenRef) return;
  ...
}
```
Or guard each `setWgBlockers`/`setWgDeleteError` post-await with
`if (deletingWg() !== wg) return;`.

### G.3 — Risky design choices

**G.3.1 — Dead `agent_name.is_empty()` branch in `scan_ac_sessions`.**
`agent_fqn_from_path` only returns an empty string if the input path is
itself empty (verified at `config/teams.rs:25-39`). `Session::working_directory`
is never an empty string in practice — sessions can't spawn without a CWD.
The branch:
```rust
agent_name: if agent_name.is_empty() {
    cwd_canon.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string()
} else { agent_name },
```
is unreachable defensive code that adds a maintenance burden. Recommend
trimming to just `agent_name: agent_name`. Minor — keep if dev-rust prefers
defensive style, but document.

Also note the architect's example payload (§2.4) shows
`"agentName": "wg-7-dev-team/architect"` — but `agent_fqn_from_path` for a
real WG replica returns `"<project>:wg-7-dev-team/architect"` (with project
prefix and colon, see `config/teams.rs:62-78`). The example is cosmetically
incorrect but the test §7.4 doesn't depend on it. **Fix:** update the §2.4
example to `"agentName": "myproject:wg-7-dev-team/architect"` so future
readers don't get misled.

**G.3.2 — UNC-prefix strip is too aggressive on real network paths.**
`canonicalize_for_compare` strips `\\?\` blindly. For a UNC path
`\\server\share\proj\.ac-new\wg-…`, `canonicalize` returns
`\\?\UNC\server\share\…`; stripping `\\?\` leaves `UNC\server\share\…`,
which is a *malformed* path. Self-consistent for prefix comparison (both
sides get malformed identically), but cosmetically broken in the JSON `cwd`
output the user sees.

**Why it matters:** AC has at least one user (the developer) who works in
network-mounted dirs in CI environments. The `cwd` field in the modal would
render as `UNC\server\…\__agent_x` — confusing.

**Fix:** strip both `\\?\UNC\` (replacing with `\\`) and `\\?\` (replacing
with empty), in that order. There's an existing helper at
`entity_creation.rs:903` doing the same `\\?\` strip — fix both, or add a
shared `path_for_display` helper to a util module. Acceptable to keep this
out of v1 if dev-rust adds a doc comment to `canonicalize_for_compare`
flagging UNC as a known imperfection.

**G.3.3 — No cancellation propagation to `spawn_blocking`.** §B.2 hands the
~1 s FFI scan off to `spawn_blocking`. If the calling Tokio task is
cancelled (e.g., user closes the AC window mid-delete), the blocking task
keeps running to completion. Two consequences:
1. CPU/RM-session quota is held for ~1 s after cancellation.
2. The `scan_external_processes_windows` writes to no shared state, so
   nothing leaks — but it's wasted work.

Not a bug; documenting because §B.2's "JoinError is treated as scan failure"
phrasing implies the join completes. It does, but cancellation behaviour
isn't mentioned. Acceptable in v1 — the diagnostic is bounded — but worth a
one-line acknowledgement in §B.2.

**G.3.4 — `pid_to_exe_basename` 260-char buffer.** Plan uses
`buf: [u16; 260]` for `QueryFullProcessImageNameW`. On Windows 10/11 with
long-path support, exe paths can exceed 260 chars (rare for system tools,
common for some user-installed dev tools in deeply nested toolchains). The
API returns `ERROR_INSUFFICIENT_BUFFER` and the plan correctly degrades to
`format!("pid {}", pid)`. Not a bug, but worth bumping the buffer to
something like 1024 or 4096 — the cost is 2-8 KB stack per call, called at
most once per blocker PID.

**G.3.5 — `RM_PROCESS_INFO` size.** `RM_PROCESS_INFO` is ~580 bytes
(`[u16; 256]` + `[u16; 64]` + smaller fields). With `Vec::with_capacity(needed)`
in the retry loop and `needed` typically 1–10, allocation is trivial. But
worst case (a host with 10k blocker processes — pathological but possible)
is ~5.8 MB on the heap, allocated and freed each retry iteration. Bounded
by `MAX_GETLIST_RETRIES = 3`. Not a real concern; flagging only because the
struct is bigger than I expected and the retry-allocates-fresh pattern is
worth understanding.

### G.4 — Test gaps

§7's test plan is solid for happy paths and basic regressions. Gaps:

**G.4.1 — No coverage of the BLOCKERS-from-dirty-confirm flow** (G.2.1, G.2.2).
Add a frontend manual test: trigger DIRTY_REPOS, type WG name, click Delete,
and have a process holding a file inside the WG. Expected: BLOCKERS modal
appears, dirty-repos input gone, retry re-uses force=true. Currently
nothing in §7.5 / §7.7-7.12 catches this combined state.

**G.4.2 — No JSON-parse-failure test.** The `try { JSON.parse } catch`
path in §4.6.d and §4.6.g is untested. Add a frontend mock test that
returns `BLOCKERS:not-valid-json` and assert the user-facing fallback
message (post-fix per G.2.4).

**G.4.3 — No bulk-pass error-injection test.** Add a Windows-manual test:
make a file inside the WG unreadable to the user (icacls deny read), trigger
delete, hold a file with notepad. Expect BLOCKERS to surface what RM can
see — currently the plan's bulk-pass would propagate the
`ERROR_ACCESS_DENIED`/`ERROR_FILE_NOT_FOUND` and blank the diagnostic.
Catches G.2.3.

**G.4.4 — No deeply-nested-WG test.** Add a manual scenario: create a WG
with a deep `repo-*/.git/objects/...` tree, hold a lock file in a sibling
repo (`repo-bar/.git/index.lock`) via a script. Confirm whether the
diagnostic includes `repo-bar`. Catches G.2.6 — if the test fails, the
file-selection strategy needs to change.

**G.4.5 — No `set_len` UB regression test.** Hard to test (would need to
mock `RmGetList`), but at minimum add a code comment / debug_assert at the
`set_len` site documenting the precondition `have <= needed`. Catches
G.2.5 if a future RM update or kernel quirk violates the contract.

**G.4.6 — No retry-during-cancel test.** Add a frontend mock test: open
BLOCKERS modal, click Retry, then click Cancel before the mocked delete
resolves. Mock then resolves with another BLOCKERS:. Assert no banner
re-appears, no setWgBlockers writes after close. Catches G.2.7.

**G.4.7 — Sentinel collision regression.** Add a unit test asserting that
`validate_existing_name` rejects strings starting with "BLOCKERS:" or
"DIRTY_REPOS:", or document in a comment that the validator's existing
character whitelist already prevents these prefixes (verify by reading
`validate_existing_name`). Currently not stated in §6 either way.

**G.4.8 — `canonicalize_for_compare` UNC path test.** §7.2 covers the
common case (`std::env::temp_dir()` — usually `C:\Users\…`). Add a test
that exercises a UNC-style path, even if synthesised — catches G.3.2.

**G.4.9 — No Drop-on-panic test.** RAII guard correctness across panic
boundaries is asserted in §4.2 note 2 but never tested. Add a
`#[should_panic]` test that triggers a panic inside the per-file loop
scope (e.g., via a helper that deliberately panics) and uses a
`thread_local!` counter to confirm `RmEndSession` (or a stand-in) ran.
Architecturally heavy for the value; minimal version: a
`debug_assert_eq!(session_count_at_drop, …)` style internal counter is
fine. Or, accept that the Drop guard is correct by inspection and move on.

### G.5 — Approval / disapproval of dev-rust deviations B.1–B.4

| Deviation | Verdict | Rationale |
|---|---|---|
| **B.1** drop `Win32_System_ProcessStatus`, use `QueryFullProcessImageNameW` | **APPROVE** | One fewer feature flag, lighter privilege ask (`PROCESS_QUERY_LIMITED_INFORMATION` works cross-session vs `PROCESS_QUERY_INFORMATION` requiring same-user / debug priv), identical capability for our use case (basename only). Verified via `windows-sys-0.59.0/src/Windows/Win32/System/Threading/mod.rs:255`. |
| **B.2** wrap Windows scan in `tokio::task::spawn_blocking` | **APPROVE** | Architect's synchronous variant would block a Tokio worker for ~1 s, starving PTY reads and IPC events on a live AC instance. `spawn_blocking` is the canonical fix. JoinError → `diagnostic_available = false` is a clean degradation. (Add the cancellation acknowledgement per G.3.3.) |
| **B.3** full FFI body (bulk + per-file two-pass) | **APPROVE WITH AMENDMENTS** | Two-pass strategy is correct. Drop guard is correct. Per-file early-exit is correct. Pending fixes from G.2.3 (bulk-pass softening), G.2.5 (`set_len` cap), G.2.6 (file-selection breadth). FFI signatures cross-checked — all match windows-sys 0.59. |
| **B.4** Retry button instead of close-and-reopen | **APPROVE WITH AMENDMENTS** | UX intent is right (sticky modal with re-fire). Pending fixes from G.2.1 (force-state preservation), G.2.2 (orthogonal-state reset), G.2.4 (parse-failure copy), G.2.7 (cancel-while-retry race). All four are localised to §4.6.d and §4.6.g — no design pivot needed. |

### G.6 — C.10 verdict (TS literal-union vs Rust String for `platform`)

**Recommendation: keep TS as the literal union `"windows" | "linux" | "macos" | "other"`. Lock the Rust producer with an enum.**

Rationale:
- The producer is `detect_platform()` (§4.2), which is a 4-arm `if cfg!(...)`
  chain returning `&'static str`. Literal-union on the consumer side gives
  free type-safety today. The architect's draft and dev-rust's enrichment
  agree on the four variants; there's no churn risk.
- Widening TS to `string` loses precision at every call site for no benefit:
  switch-statements lose exhaustiveness checks, dead-code detection fails.
- Dev-rust's "easy to update later" rationale assumes someone notices.
  History says they don't — schema drift between `serde` and TS lasts
  across multiple releases in similar codebases.
- Locking the Rust side with `#[derive(Serialize)] enum Platform { Windows, Linux, Macos, Other }`
  with `#[serde(rename_all = "lowercase")]` is two extra lines and
  guarantees the producer can't drift. If a future contributor adds
  `Freebsd`, the TS type fails to compile and forces the conversation —
  exactly what we want.

Concrete spec change for §4.2:
```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows, Linux, Macos, Other,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerReport {
    pub workgroup: String,
    pub platform: Platform,           // was: String
    pub diagnostic_available: bool,
    ...
}
```
And `detect_platform()` returns `Platform` instead of `&'static str`. Test
§7.4 needs a one-line update to assert the lowercase string in JSON.

If dev-rust prefers to keep `String` (lighter touch), at minimum add a unit
test that asserts `detect_platform()` returns one of the four allowed
strings — the schema lock is a defensive belt the cost of an enum is
trivially below.

### G.7 — Summary

- **0 critical issues.** Plan is structurally sound.
- **7 likely bugs (G.2.1–G.2.7)** — all fixable with localised spec edits;
  none require redesign.
- **5 risky design choices (G.3.1–G.3.5)** — most acceptable in v1 with
  doc-comments; G.3.2 (UNC strip) deserves a follow-up.
- **9 test gaps (G.4.1–G.4.9)** — most are 1-line additions to §7.
- **B.1–B.4 deviations: APPROVE** (B.3, B.4 with amendments per G.2).
- **C.10 verdict:** keep TS literal union, lock Rust with an enum.

Verdict: **needs another iteration (Step 5 consensus pass).** Most of G.2
is one-paragraph per fix; B.3/B.4 amendments are the bulk of the work.
Once G.2.1–G.2.7 land in the spec, the plan is implementation-ready.

---

## Dev-rust round-2 additions (Step 5 — round 2)

Round 2 of max 3. Per Role.md Rule 5, on round 3 the minority opinion loses,
so this round-2 pass aims for full convergence with grinch — every G.x finding
gets an explicit Accept / Reject / Defer + rationale + pointer to the spec
edit that addresses it.

### Decision table

Legend: ✓ Accept (fixed inline), ◯ Reject (with rationale), ➤ Defer (with rationale).

| Finding | Decision | Spec edit lives in | Notes |
|---|:--:|---|---|
| **G.2.1** Retry hardcodes force=false | ✓ | §4.6.b (new `wgLastForceUsed` signal), §4.6.c (clear in close), §4.6.d edit 1 (capture at click), §4.6.g (replay via `wgLastForceUsed()`) | Replays the originating force-state. Resolves the BLOCKERS-after-dirty-confirm UX bug. |
| **G.2.2** BLOCKERS payload leaves dirty-repos UI rendered | ✓ | §4.6.d catch BLOCKERS branch, §4.6.g BLOCKERS branch | Both branches now `setWgDirtyRepos(false)` and `setWgConfirmText("")`. Single-banner invariant holds. |
| **G.2.3** Bulk-pass aborts on any single bad file | ✓ | §4.2 — new `collect_blockers_tolerant` (binary-search-fallback inside `scan_external_processes_windows`) | Bad files are isolated to size-1 leaves and skipped with `log::warn!`. Bounded at O(log N + bad_files) RM sessions. |
| **G.2.4** JSON parse failure shows raw `BLOCKERS:{json}` | ✓ | §4.6.d catch parse-fail branch, §4.6.g retry parse-fail branch | Sanitised fallback strings; retry-path string explicitly says "still locked" to distinguish first-attempt vs retry. |
| **G.2.5** `set_len` UB hole | ✓ | §4.2 `rm_get_list` SUCCESS branch | `debug_assert!(have ≤ needed)` + `let actual = have.min(needed); buf.set_len(actual);`. One line of belt-and-suspenders. |
| **G.2.6** DFS exhausts file budget on one repo's `.git/objects/` | ✓ | §4.2 `collect_files_to_probe` rewrite | BFS via `VecDeque` + hot-priority for `*.lock` and lock-prone git metadata (`index`, `HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `MERGE_HEAD`, `packed-refs`). 4× soft-ceiling on the walk. |
| **G.2.7** Race: modal closed mid-retry | ✓ | §4.6.b (new `retryGen` counter), §4.6.c (bump on close), §4.6.g (`myGen = ++retryGen` snapshot + 4 post-await guards) | Plain `let` counter (not a signal — never read reactively). Every `setWg*` after an await is gated. |
| **G.3.1** Dead `agent_name.is_empty()` branch + cosmetic example agentName | ✓ | §4.2 `scan_ac_sessions` (branch trimmed); §2.4 example payload (project-prefixed FQN) | Branch was unreachable; trimmed with a comment for archaeology. Example payload now matches `agent_fqn_from_path` output. |
| **G.3.2** UNC-prefix strip too aggressive | ✓ | §4.2 `canonicalize_for_compare` rewrite | Strips `\\?\UNC\` → `\\` first, then `\\?\` → ``. Drive paths and UNC paths both render correctly in JSON `cwd` output. Existing helper at `entity_creation.rs:903` left unchanged (out of scope per "do not regress DIRTY_REPOS path"). |
| **G.3.3** No cancellation propagation to `spawn_blocking` | ✓ (doc-only) | Acknowledged below in this round-2 section | Behavior is acceptable in v1 (bounded ~1 s wasted CPU on cancel). Documented here so the next reader doesn't re-discover it. |
| **G.3.4** 260-char exe-name buffer too small | ✓ | §4.2 `pid_to_exe_basename` | Bumped `[u16; 260]` → `[u16; 1024]`. 2 KB stack per call, called at most once per blocker PID. |
| **G.3.5** `RM_PROCESS_INFO` size note | ✓ (doc-only) | Acknowledged below in this round-2 section | Grinch flagged for reader awareness only; no spec change requested. |
| **G.4.1** Test for BLOCKERS-from-dirty-confirm flow | ✓ | §7.16 (new) | Manual scenario added; covers G.2.1 + G.2.2 end-to-end. |
| **G.4.2** Test for JSON-parse failure | ✓ | §7.17 (new) | Frontend mock test for both Delete-path and Retry-path parse-failure copy. |
| **G.4.3** Test for bulk-pass error injection | ✓ | §7.18 (new) | Manual: `icacls /deny` a file inside the WG, expect partial diagnostic to still surface other blockers. |
| **G.4.4** Test for deeply-nested-WG file selection | ✓ | §7.19 (new) | Manual: PowerShell hold of `repo-bar/.git/index.lock`; with hot-priority + BFS the lock surfaces. |
| **G.4.5** `set_len` precondition documentation | ✓ | §7.20 (new) + inline comment in §4.2 already added | Code-review verification, not a runtime test. `debug_assert!` is the live precondition guard. |
| **G.4.6** Retry-during-cancel test | ✓ | §7.21 (new) | Frontend mock: 200 ms delay, click Cancel mid-await, assert no banner re-appears. |
| **G.4.7** Sentinel collision regression | ✓ | §7.22 (new) | Pure Rust unit test against `validate_existing_name`. Verified at `entity_creation.rs:116` that the validator's `is_ascii_alphanumeric() \|\| c == '-'` whitelist precludes `:` and `_`, so `BLOCKERS:` and `DIRTY_REPOS:` cannot be prefix substrings of any valid WG name. Lock confirmed. |
| **G.4.8** UNC path test for `canonicalize_for_compare` | ✓ | §7.23 (new) | Pure unit test. Implementation note: factor a private `strip_long_prefix(&Path) -> PathBuf` so the test can hit the prefix-strip logic without disk I/O. |
| **G.4.9** Drop-on-panic test for `RmSession` | ✓ (accept-by-inspection per grinch) | §7.24 (new) + inline comment at `impl Drop for RmSession` | Heavy to test directly; the inline `// G.4.9` comment + §4.2 note 2 cover the invariant. |
| **C.10** TS literal union vs Rust String for platform | ✓ | §4.2 — new `pub enum Platform` with `#[serde(rename_all = "lowercase")]`; `BlockerReport.platform: Platform`; `detect_platform() -> Platform` | TS side stays as the literal union (per grinch's recommendation). Rust producer locked. JSON wire shape unchanged. |

**Decision summary: every finding accepted.** No rejects, no defers. Round-2
goal of "no minority opinion remaining for round 3" satisfied — minority is
empty.

### Round-2 doc-only acknowledgements (no spec edit)

**G.3.3 — `spawn_blocking` cancellation behavior.** When the calling Tokio task
is cancelled (window close mid-delete), the blocking task keeps running to
completion (~1 s). The closure writes to no shared state, so nothing leaks —
just wasted CPU and an RM session held for the remainder of the scan. Bounded.
Acceptable in v1.

If we ever want to plumb cancellation, the canonical Tokio approach is to pass
a `tokio::sync::watch` or `AtomicBool` flag and check it between phases of the
scan. Out of scope for this issue.

**G.3.5 — `RM_PROCESS_INFO` size.** The struct is `~580 bytes` (`[u16; 256]
strAppName` + `[u16; 64] strServiceShortName` + smaller fields). With our retry
budget capped at 3 and `needed` typically 1–10, allocation is trivial. Worst
case (10 k blocker processes, pathological) allocates ~5.8 MB and frees it on
each retry iteration. Documented for reader awareness only — no v1 change.

### Stale architect-prose note (won't edit per Step-5 constraint)

§5 ("Dependencies") still mentions adding `Win32_System_ProcessStatus` as one
of two extra `windows-sys` features. This was overridden by my B.1 deviation
in round 1 (we use `QueryFullProcessImageNameW` from the already-enabled
`Win32_System_Threading`, no new feature beyond `Win32_System_RestartManager`).
Per the round-2 brief I am not modifying architect's existing sections; the
authoritative feature-flag list is in §4.1's updated `Cargo.toml` block.
Implementation will follow §4.1, not §5.

### Cargo.toml / function-signature deltas in round 2

No new windows-sys features. No new function signatures. Round-2 fixes are
all internal refactors of code already in §4.2 / §4.6:

- §4.2 imports unchanged (`RmStartSession`, `RmRegisterResources`, `RmGetList`,
  `RmEndSession`, `RM_PROCESS_INFO`, `CCH_RM_SESSION_KEY`, `OpenProcess`,
  `QueryFullProcessImageNameW`, `PROCESS_QUERY_LIMITED_INFORMATION`,
  `CloseHandle`, `FILETIME`).
- New `Platform` enum — internal type, no new dep.
- `collect_blockers_tolerant` — internal helper inside
  `scan_external_processes_windows`, no signature change for callers.

### Summary of sections edited in round 2

- §2.4 — example payload `agentName` updated for G.3.1.
- §4.2 — `Platform` enum (C.10), `BlockerReport.platform` typed as `Platform`,
  `detect_platform()` returns `Platform`, `scan_ac_sessions` dead branch
  trimmed (G.3.1), `canonicalize_for_compare` UNC strip (G.3.2),
  `collect_files_to_probe` BFS+priority rewrite (G.2.6),
  `pid_to_exe_basename` buffer bumped (G.3.4), `rm_get_list` `set_len` cap
  (G.2.5), `scan_external_processes_windows` bulk-pass replaced with
  `collect_blockers_tolerant` (G.2.3).
- §4.6.b — `wgLastForceUsed` signal (G.2.1) + `retryGen` counter (G.2.7).
- §4.6.c — `closeWgDeleteModal` clears new state + bumps `retryGen` (G.2.7).
- §4.6.d — Delete onClick captures `wgLastForceUsed` (G.2.1); BLOCKERS catch
  branch clears orthogonal state (G.2.2); parse-failure shows sanitised
  fallback (G.2.4).
- §4.6.g — `retryWgDelete` reads `wgLastForceUsed()` (G.2.1), guards every
  post-await write with `myGen !== retryGen` (G.2.7), clears orthogonal
  state in BLOCKERS branch (G.2.2), sanitised parse-failure copy (G.2.4).
  Leading prose updated to reflect the round-2 behaviour matrix.
- §7 — added §7.16 through §7.24 covering G.4.1–G.4.9.

### Open questions for grinch / tech-lead

**None.** All G.x findings accepted with inline spec fixes. Verdict
recommended: **ready for grinch round 3** for sign-off, no further iteration
expected.

---

## Grinch round-3 review (Step 5 — round 3)

Verdict up front: **APPROVED WITH NOTES — ready for Step 6.** Dev-rust
accepted all 22 round-1 findings, the round-2 spec edits hold against the
specific failure modes round 1 called out, and no new round-1-class bugs
surfaced in the round-2 changes. Two test-spec hygiene issues introduced
in round 2 (R.1, R.2 below) need to be fixed during implementation, but
neither blocks Step 6 — they're trivial compile-time fixes dev-rust will
encounter on the first `cargo test` run regardless.

### R.1 — Per-fix verification

| Round-1 finding | Round-2 spec edit | Verdict | Notes |
|---|---|:--:|---|
| **G.2.1** Retry hardcodes force=false | `wgLastForceUsed` signal captured at click (§4.6.d edit 1), replayed via `wgLastForceUsed()` in retry (§4.6.g), cleared on close (§4.6.c) | ✅ | Closes the BLOCKERS-after-dirty-confirm UX gap exactly as called out. |
| **G.2.2** Dirty-repos UI rendered behind BLOCKERS banner | `setWgDirtyRepos(false)` + `setWgConfirmText("")` in §4.6.d BLOCKERS catch and §4.6.g BLOCKERS retry-refresh | ✅ | Single-banner invariant holds in every BLOCKERS hop. |
| **G.2.3** Bulk-pass aborts on any single bad file | `collect_blockers_tolerant` recursive binary-search with single-file isolation (§4.2) | ✅ | Termination: each Err recursion halves `wide.len()`; the `len == 1` arm is the base case. Bound: O(log N + bad_files) RM sessions = ~13 worst case at N=200, 5 bad files; phase-2 adds ~15; total ~28, well under the 64 quota. Single-bad-file isolation: reached when binary-search hits `len == 1`, file logged + skipped. Lossy on `rm_get_list` failure inside a successful sub-batch (returns empty, doesn't recurse) — accepted per dev-rust note. Drop-before-recurse pattern at line 685 prevents per-frame session accumulation. |
| **G.2.4** JSON parse failure shows raw `BLOCKERS:{json}` | Sanitised fallback strings in §4.6.d (Delete-path) and §4.6.g (Retry-path) | ✅ | Distinct copy ("locked" vs "still locked") helps users see attempt status. Retry parse-fail also nulls `wgBlockers` (line 1198) so the banner can't get stuck in a half-parsed state — appropriate. |
| **G.2.5** `set_len` UB hole | `debug_assert!(have ≤ needed)` + `let actual = have.min(needed); buf.set_len(actual);` (§4.2 lines 559–569) | ✅ | `debug_assert!` is the right intensity here — soundness comes from the `min` cap (always runs), the assert is observability for dev builds. Tech-lead's question "is `debug_assert!` enough or does prod need a panic?" — `debug_assert!` is enough. A prod panic on `have > needed` would lose the entire diagnostic via the `spawn_blocking` JoinError path; the `min` cap loses at most a few late-arriving entries from this RmGetList call. Trade is correct. |
| **G.2.6** DFS exhausts file budget on one repo's `.git/objects/` | BFS via `VecDeque` + hot-priority bucket for `*.lock` and lock-prone git metadata, 4× soft-ceiling on the walk (§4.2 lines 320–388) | ✅ | Tech-lead's specific question: "If the WG has 50 `*.lock` files spread across repos, does the 200-file cap still cover all of them?" — Yes. With BFS, all `.git/HEAD` / `index` / `*.lock` files at depth 3–4 are visited before the soft-ceiling (800) is hit on level-4 expansion. Hot bucket truncated to `MAX_FILES_TO_PROBE` only if hot count exceeds 200 (50 hot files all kept; 150 cold backfill). `is_hot_lock_candidate` correctly catches both `*.lock` and the non-lock metadata names via `ancestors()`-based `.git/` detection. |
| **G.2.7** Race: modal closed mid-retry | Plain `let retryGen = 0` counter at component scope, bumped on `closeWgDeleteModal` and on retry start, `myGen !== retryGen` guards on every post-await branch (§4.6.b/c/g) | ✅ | Tech-lead's specific questions: (a) "Atomic enough for SolidJS reactive-write timing?" — Yes; `retryGen` is a plain closure-captured `let`, never read reactively. JS is single-threaded and the continuation between an `await` resolution and the immediately-following synchronous check cannot be interrupted by other event-loop work. (b) "Window between await resolution and `myGen !== retryGen` check that can still stomp?" — None. The await schedules the continuation as a microtask; once it runs, the check executes synchronously without yielding. Concurrent state-mutating UI events (Cancel click, second Retry click) can only run BEFORE the await resolves or AFTER the continuation completes — the retryGen comparison catches both windows. The catch-block also has its own entry-guard at line 1183 covering rejection-side cancellation. Multi-await sequence in the success path (delete + reloadProject) has guards between every await pair. |
| **G.3.1** Dead `agent_name.is_empty()` + cosmetic agentName | Branch trimmed (§4.2 line 412), §2.4 example payload now `"agentscommander:wg-7-dev-team/architect"` | ✅ | Trim is correct. Updated example matches `agent_fqn_from_path` output for WG replicas. |
| **G.3.2** UNC-prefix strip too aggressive | `\\?\UNC\` → `\\` first, then `\\?\` → `` (§4.2 lines 296–303) | ✅ | Order is correct (long prefix first). UNC paths now render as `\\server\share\…` in the JSON `cwd` output. Drive paths still render as `C:\…`. |
| **G.3.3** No cancellation propagation to `spawn_blocking` | Doc-only ack | ✅ | Acknowledged behaviour matches my round-1 framing. |
| **G.3.4** 260-char exe buffer too small | Bumped to `[u16; 1024]` (§4.2 line 602) | ✅ | 2 KB stack per call, called at most once per blocker PID. |
| **G.3.5** `RM_PROCESS_INFO` size note | Doc-only ack | ✅ | Reader-awareness only. |
| **C.10** TS literal union vs Rust String | `pub enum Platform { Windows, Linux, Macos, Other }` + `#[serde(rename_all = "lowercase")]` (§4.2 lines 191–198), `BlockerReport.platform: Platform`, `detect_platform() -> Platform` | ✅ | Producer locked. JSON wire shape unchanged. TS literal union in §4.5 untouched. **See R.1.a below — the §7.4 test was not updated for this change.** |

#### R.1.a — Round-2-introduced bug: §7.4 test won't compile after the C.10 enum change

§7.4 (`blocker_report_serializes_with_camelcase_fields`) at line 1569 still
constructs `platform: "windows".into()`. With the round-2 change, the field
is `pub platform: Platform` (an enum) and `Platform` does not implement
`From<&str>`. The test fails to compile.

**Severity:** Trivial — dev-rust will hit this on the first `cargo test`
run, fix it in seconds, no impact on the design or other tests. NOT a
round-3 blocker.

**Spec fix needed in §7.4:**
1. Replace `platform: "windows".into(),` with `platform: Platform::Windows,`.
2. Add an assertion that the JSON `platform` value is the lowercase string
   `"windows"`, to lock the `#[serde(rename_all = "lowercase")]` behaviour
   on the enum. Today's test only asserts the field NAME exists, not the
   serialised value:
   ```rust
   assert_eq!(json.get("platform").and_then(|v| v.as_str()), Some("windows"),
       "Platform enum must serialize to lowercase string");
   ```
   This catches a future contributor who changes `rename_all = "lowercase"`
   to `"camelCase"` (which would round-trip `Macos` → `"macos"` either way
   coincidentally, but `Other` → `"other"` only via lowercase — the test
   should pin the contract regardless).

#### R.1.b — Round-2-introduced gap: §7.22 calls private `validate_existing_name`

§7.22 (sentinel collision invariant test) calls
`crate::commands::entity_creation::validate_existing_name(...)` at line
1774 (and again at 1777, 1778, 1781, 1782). The function at
`entity_creation.rs:116` is `fn` (module-private), not `pub(crate)`.
Test fails to compile until exposed.

**Severity:** Trivial — same pattern as §7.1 / `is_file_in_use_error`,
which the plan flagged with an explicit "expose as `pub(crate)`" note.
§7.22 is missing that note.

**Spec fix needed in §7.22:** Append the same `pub(crate)` note that §7.1
already carries:
> `validate_existing_name` is currently private in `entity_creation.rs`.
> Expose as `pub(crate)` (preferred — keeps the helper next to its call
> sites) or move the test inside `entity_creation.rs`'s own test module.
> dev-rust to pick during implementation; either works.

### R.2 — §5 stale-prose decision

**Footnote is enough. No architect dispatch needed.**

Reasoning:
- §4.1's Cargo.toml block is the authoritative spec edit. It explicitly
  shows `Win32_System_ProcessStatus` REMOVED from the feature list
  (§4.1's "Dev-rust deviation (Step 3) — drop `Win32_System_ProcessStatus`")
  and the §4.1 code block does not include it.
- Dev-rust's round-2 footnote at line 2304–2312 explicitly cross-references
  §4.1 as the authoritative source.
- Implementer reading top-to-bottom will reach §4.1 long before §5 in any
  practical workflow (§4 contains the actual `cargo` block to apply).
- Risk of mis-application is low; if the implementer applies §5's prose
  verbatim instead of §4.1's code block, `cargo build` will succeed (the
  extra feature is harmless), and a follow-up cleanup PR can drop the
  unused feature with no functional impact.

Suggest a doc-only cleanup PR after Step 6 to align §5 with §4.1, but this
is not on the critical path for issue #113. **Not a round-3 blocker.**

### R.3 — Test sufficiency check (§7.16–§7.24)

| Test | Covers | Sufficient? | Notes |
|---|---|:--:|---|
| §7.16 | G.4.1, G.2.1, G.2.2 end-to-end | ✅ | Manual flow exactly matches the failure scenario I called out in G.2.1/G.2.2. |
| §7.17 | G.4.2, G.2.4 (parse-fail copy) | ✅ | Tests both Delete-path and Retry-path; verifies the distinct "locked" vs "still locked" copy. |
| §7.18 | G.4.3, G.2.3 (bulk-pass tolerance) | ⚠ MOSTLY | `icacls /deny` may not reliably trigger `RmRegisterResources` to return an error for the current user (RM uses kernel-mode access for path scanning that can bypass file ACLs in some configs). If the manual test doesn't surface a non-success WIN32_ERROR, the binary-search-fallback isn't actually exercised. **Suggested supplementary:** also try a vanished-file race (script that creates+deletes a file in the WG in tight loop while delete is running) or a path with a reserved Windows name (e.g. `CON.txt`). Not blocking; current §7.18 is a reasonable first attempt. |
| §7.19 | G.4.4, G.2.6 (BFS+hot-priority) | ✅ | Uses `[System.IO.File]::Open(..., "Create", "Read", "Read")` which holds the handle correctly; `index.lock` is hot-priority and would be picked even with a bloated `repo-foo/.git/objects/`. |
| §7.20 | G.4.5, G.2.5 (set_len) | ✅ | Code-review verification per my own round-1 framing ("minimal version is fine"). The `debug_assert!` is the live precondition guard in dev builds. |
| §7.21 | G.4.6, G.2.7 (cancel-mid-retry) | ✅ | 200 ms delay + Cancel + verify-no-banner-after is the correct shape. Spy/DOM-inspection alternative covers test-runner flexibility. |
| §7.22 | G.4.7 (sentinel collision) | ✅ pending R.1.b | Logic is correct (validator's `is_ascii_alphanumeric() \|\| c == '-'` whitelist precludes `:` and `_`). Compile fix per R.1.b. |
| §7.23 | G.4.8, G.3.2 (UNC strip) | ✅ | Pure-Rust unit, no FS dependency. Implementation requires factoring `strip_long_prefix` out of `canonicalize_for_compare` — plan acknowledges this. |
| §7.24 | G.4.9 (Drop-on-panic) | ✅ | Accept-by-inspection per my own round-1 framing. Inline comment + §4.2 note 2 cover the invariant. |

### R.4 — Specific tech-lead scrutiny questions, answered

> **G.2.3 binary-search-fallback — does it actually terminate? Worst-case bound holds? Single-bad-file isolation works as claimed?**

Termination ✅ — `wide.len() / 2` strictly shrinks each frame; base case at `wide.len() == 1` returns without recursing. Tree depth bounded at `ceil(log2(N))` = 8 for N=200. Worst-case sessions: O(log N + bad_files), ~13 for N=200/5-bad-files. Single-bad-file isolation ✅ — `Err(e) if wide.len() == 1` arm logs and returns empty.

> **G.2.5 set_len — is `debug_assert!` enough, or does prod need a panic on `have > needed`?**

`debug_assert!` is enough. Soundness is provided by the `let actual = (have as usize).min(needed as usize);` cap, which always runs. The assert is observability-only — fires in dev/test builds, compiled out in release. Promoting to `assert!` would convert a soundness-preserving truncation into a panic that the `spawn_blocking` task carrier would surface as `JoinError`, blanking the entire diagnostic. The current trade is right.

> **G.2.7 retryGen counter — atomic enough for SolidJS reactive-write timing? Any window between `await` resolution and the `myGen !== retryGen` check that can still stomp?**

Atomic ✅ — JS is single-threaded; `retryGen` is a plain closure-captured `let`, never reactive. The microtask scheduling between an `await` resolution and the synchronous check that immediately follows it cannot be preempted by another UI event. Concurrent state-mutating events (Cancel, second Retry click) can only fire BEFORE the await resolves (caught by post-await guard) or AFTER the continuation completes (no longer matters). Multi-await sequence in the success path has guards between every pair. The catch-block has its own entry-guard. No race window remaining.

> **G.2.6 hot-priority BFS — does the priority queue actually probe `*.lock` first, or just put them in the queue early? If the WG has 50 `*.lock` files spread across repos, does the 200-file cap still cover all of them?**

Hot files are SEGREGATED into a separate `Vec` during the BFS walk, not just queue-ordered. After the walk completes, the output is `hot.truncate(200)` followed by `cold.extend(remaining)`. So the FFI call sees hot files FIRST regardless of BFS visit order. For 50 hot files: all 50 kept, 150 cold backfill. For 250 hot files: 200 hot kept, 0 cold. The `200` cap is on the OUTPUT, not the walk — the walk is bounded separately by `WALK_SOFT_CEILING = 800`.

### R.5 — New genuine bugs spotted in round-2 changes

R.1.a (§7.4 test enum mismatch) and R.1.b (§7.22 visibility) — both
trivial implementation-time fixes; neither blocks Step 6.

No round-1-class bugs introduced by the round-2 changes. The
`collect_blockers_tolerant` recursion, the BFS walk, the retryGen counter,
the orthogonal-state resets, and the Platform enum are all internally
consistent and exercise the stated failure modes.

### R.6 — Approval / verdict

**APPROVED WITH NOTES — ready for Step 6 (implementation).**

Notes for dev-rust to keep in mind during implementation (NOT round-3
blockers; these are guidance):

1. **R.1.a** — when applying §7.4, change `platform: "windows".into(),` to
   `platform: Platform::Windows,`, and add the JSON-value assertion:
   `assert_eq!(json.get("platform").and_then(|v| v.as_str()), Some("windows"));`
2. **R.1.b** — when applying §7.22, expose `validate_existing_name` as
   `pub(crate)` in `entity_creation.rs:116` (or move the test inside
   that module). Mirror the §7.1 / `is_file_in_use_error` pattern.
3. **R.3 / §7.18** — if `icacls /deny` doesn't reliably surface a
   non-success `WIN32_ERROR` from `RmRegisterResources`, swap in a
   vanished-file race or a Windows-reserved-name (e.g. `CON.txt`) probe
   to actually exercise the binary-search-fallback. Optional polish.
4. **R.2 / §5** — §5's "Dependencies" prose still says
   `Win32_System_ProcessStatus`; §4.1's Cargo.toml block is authoritative.
   No edit required for v1; consider a doc-only cleanup PR after Step 6.
5. The `wgLastForceUsed` signal (§4.6.b) and `retryGen` counter (§4.6.b)
   live inside the per-project `{(proj) => { ... }}` callback in
   ProjectPanel.tsx. They reset cleanly on close, but a multi-modal flow
   (which the codebase doesn't currently support) would need its own
   per-modal scoping. Out of scope for issue #113; flag if a future
   change adds parallel WG-delete modals.

No pushback on any specific finding. Plan is sound, fixes hold against
the failure modes round 1 called out, no minority position remaining.
Dev-rust can proceed to Step 6.
