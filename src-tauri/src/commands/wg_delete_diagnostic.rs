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

/// Locks the producer to the four-variant promise so the wire shape can't drift.
/// Lowercase JSON matches the TS literal union in `src/shared/types.ts`.
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
    /// FQN like `agentscommander:wg-7-dev-team/architect` derived via
    /// `crate::config::teams::agent_fqn_from_path`.
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
#[cfg(windows)]
const MAX_FILES_TO_PROBE: usize = 200;

/// Top-level diagnostic. Always returns a `BlockerReport`; on non-Windows the body is empty
/// and `diagnostic_available = false`.
pub async fn diagnose_blockers(
    wg_dir: &Path,
    workgroup_name: &str,
    raw_os_error: &str,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
) -> BlockerReport {
    log::info!(
        "[wg_delete_diagnostic] starting blocker scan for workgroup '{}'",
        workgroup_name
    );
    let canonical_wg = canonicalize_for_compare(wg_dir);

    let sessions = scan_ac_sessions(&canonical_wg, session_mgr).await;

    // The Windows scan is fully synchronous (FFI + file-tree walk). We expect ~1 s
    // wall-time post-failure; running it on the current Tokio worker would block
    // that worker for the duration. Hand it off via `spawn_blocking` so other
    // async work (PTY reads, IPC events) keeps ticking. JoinError is treated as a
    // scan failure and falls through to the `diagnostic_available = false` path —
    // matches the FFI-error branch.
    #[cfg(windows)]
    let (processes, diagnostic_available) = {
        let wg_for_scan = canonical_wg.clone();
        match tokio::task::spawn_blocking(move || scan_external_processes_windows(&wg_for_scan))
            .await
        {
            Ok(Ok(p)) => (p, true),
            Ok(Err(e)) => {
                log::warn!("[wg_delete_diagnostic] Restart Manager scan failed: {}", e);
                (Vec::new(), false)
            }
            Err(join_err) => {
                log::warn!(
                    "[wg_delete_diagnostic] Restart Manager scan task failed: {}",
                    join_err
                );
                (Vec::new(), false)
            }
        }
    };

    #[cfg(not(windows))]
    let (processes, diagnostic_available) = {
        let _ = canonical_wg;
        (Vec::<BlockerProcess>::new(), false)
    };

    let report = BlockerReport {
        workgroup: workgroup_name.to_string(),
        platform: detect_platform(),
        diagnostic_available,
        raw_os_error: raw_os_error.to_string(),
        sessions,
        processes,
    };

    log::info!(
        "[wg_delete_diagnostic] diagnostic done: {} sessions, {} processes, available={}",
        report.sessions.len(),
        report.processes.len(),
        report.diagnostic_available
    );

    report
}

fn detect_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Other
    }
}

/// Strip Windows extended-length prefixes from a canonical path string.
/// Factored out of `canonicalize_for_compare` so unit tests can hit the
/// prefix-strip logic without disk I/O (G.4.8).
///
/// Order matters: `\\?\UNC\` starts with `\\?\`, so the longer prefix must
/// be tried first. `\\?\UNC\server\share\…` becomes `\\server\share\…` and
/// `\\?\C:\Users\…` becomes `C:\Users\…`.
fn strip_long_prefix_str(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}

/// Return wg_dir canonicalised, with the Windows extended-length prefixes stripped
/// for shape parity with `entity_creation.rs:903`.
fn canonicalize_for_compare(p: &Path) -> PathBuf {
    let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = canon.to_string_lossy();
    PathBuf::from(strip_long_prefix_str(&s))
}

/// Walk the workgroup tree breadth-first and collect up to MAX_FILES_TO_PROBE absolute
/// file paths to feed RmRegisterResources. Hot files (lock-prone metadata) are always
/// taken first so the budget can't be exhausted on a single `.git/objects/` subtree.
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
/// 1. **Bulk pass** — open RM sessions for batches of probed files (binary-search
///    fallback when RmRegisterResources rejects a batch), call `RmGetList` to learn
///    which PIDs hold any handles. Tolerant of single bad files; ~O(log N + bad_files)
///    sessions.
/// 2. **Per-file attribution pass** — RM doesn't tell us *which* file each PID
///    held. Iterate files; for each, open a fresh session, register only that
///    file, and accumulate (file → matching PID) entries. Cap each PID at
///    `MAX_FILES_PER_PROCESS` (5) and short-circuit once every blocker PID is
///    saturated.
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
    /// This Drop is the only path to RmEndSession; covers panic, ?, return.
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

    fn rm_register(handle: u32, wide_files: &[Vec<u16>]) -> Result<(), String> {
        if wide_files.is_empty() {
            return Ok(());
        }
        // The collected Vec must outlive the FFI call so the pointers stay valid.
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
                // Defensive cap. If RM ever wrote `have > needed` (RM bug, hostile
                // race, or kernel quirk), `set_len` on a Vec with `capacity == needed`
                // and `len > capacity` is immediate UB. The Microsoft docs say this
                // can't happen; one `min` removes the soundness hole entirely.
                debug_assert!(
                    (have as usize) <= (needed as usize),
                    "RmGetList wrote {} entries into a buffer sized for {}",
                    have,
                    needed
                );
                let actual = (have as usize).min(needed as usize);
                // SAFETY: actual ≤ needed = capacity. RM wrote `actual` valid
                // `RM_PROCESS_INFO`s into the buffer.
                unsafe {
                    buf.set_len(actual);
                }
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
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return String::new();
            }
            // 1024 wide chars (2 KB stack) covers all practical exe paths on
            // Windows 10/11 with long-path support.
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

    /// Binary-search-fallback bulk register: try the whole batch in one session;
    /// on RmRegisterResources error, drop the session, split the batch in half,
    /// and recurse. Single-bad-file isolation at `wide.len() == 1`.
    /// Bound: O(log N + bad_files) RM sessions.
    fn collect_blockers_tolerant(
        wide: &[Vec<u16>],
        original_paths: &[&PathBuf],
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
                let mut left = collect_blockers_tolerant(&wide[..mid], &original_paths[..mid]);
                let right = collect_blockers_tolerant(&wide[mid..], &original_paths[mid..]);
                left.extend(right);
                left
            }
        }
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
    let bulk_list: Vec<RM_PROCESS_INFO> = collect_blockers_tolerant(&wide_files, &alive);

    if bulk_list.is_empty() {
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
                if exe.is_empty() {
                    format!("pid {}", pid)
                } else {
                    exe
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// §7.23 (covers G.4.8): `strip_long_prefix_str` handles drive paths,
    /// UNC paths, and unprefixed paths uniformly.
    #[test]
    fn strip_long_prefix_str_handles_drive_unc_and_no_prefix() {
        assert_eq!(
            strip_long_prefix_str(r"\\?\UNC\server\share\proj"),
            r"\\server\share\proj"
        );
        assert_eq!(
            strip_long_prefix_str(r"\\?\C:\Users\me\proj"),
            r"C:\Users\me\proj"
        );
        assert_eq!(
            strip_long_prefix_str(r"C:\Users\me\proj"),
            r"C:\Users\me\proj"
        );
        // Edge: empty string and prefix-only.
        assert_eq!(strip_long_prefix_str(""), "");
        assert_eq!(strip_long_prefix_str(r"\\?\"), "");
    }

    /// §7.2: `canonicalize_for_compare` round-trips against
    /// `std::fs::canonicalize` + manual prefix strip.
    #[test]
    fn canonicalize_for_compare_strips_prefix_for_existing_path() {
        let tmp = std::env::temp_dir();
        let canon_via_helper = canonicalize_for_compare(&tmp);
        let canon_raw = std::fs::canonicalize(&tmp).expect("canonicalize tempdir");
        let raw_str = canon_raw.to_string_lossy();
        let expected = strip_long_prefix_str(&raw_str);
        assert_eq!(canon_via_helper.to_string_lossy(), expected);
    }

    /// §7.3: `scan_ac_sessions` filters sessions by canonical-path prefix.
    /// Exercises the path canonicalisation + `PathBuf::starts_with` filter
    /// on real disk paths.
    #[tokio::test]
    async fn scan_ac_sessions_filters_by_canonical_prefix() {
        use crate::session::manager::SessionManager;

        let tmp = tempfile::tempdir().expect("tempdir");
        let wg_dir = tmp.path().join("wg-test");
        let inside_session_dir = wg_dir.join("__agent_inside");
        let outside_session_dir = tmp.path().join("outside");
        std::fs::create_dir_all(&inside_session_dir).expect("create inside dir");
        std::fs::create_dir_all(&outside_session_dir).expect("create outside dir");

        let mgr = SessionManager::new();
        let _ = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                inside_session_dir.to_string_lossy().to_string(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .expect("create inside session");
        let _ = mgr
            .create_session(
                "powershell.exe".into(),
                vec![],
                outside_session_dir.to_string_lossy().to_string(),
                None,
                None,
                vec![],
                false,
            )
            .await
            .expect("create outside session");
        let mgr = Arc::new(tokio::sync::RwLock::new(mgr));

        let canonical_wg = canonicalize_for_compare(&wg_dir);
        let blockers = scan_ac_sessions(&canonical_wg, &mgr).await;

        assert_eq!(blockers.len(), 1, "exactly one session inside wg_dir");
        assert!(
            blockers[0].cwd.contains("__agent_inside"),
            "filtered session must be the inside one, got cwd={}",
            blockers[0].cwd
        );
    }

    /// §7.4 (with R.1.a fix): `BlockerReport` JSON shape preserves camelCase
    /// at the wire boundary AND the `Platform` enum serializes lowercase.
    #[test]
    fn blocker_report_serializes_with_camelcase_fields() {
        let report = BlockerReport {
            workgroup: "wg-7-dev-team".into(),
            platform: Platform::Windows,
            diagnostic_available: true,
            raw_os_error: "...".into(),
            sessions: vec![BlockerSession {
                session_id: "abc".into(),
                agent_name: "agentscommander:wg-7-dev-team/architect".into(),
                cwd: r"C:\foo".into(),
            }],
            processes: vec![BlockerProcess {
                pid: 42,
                name: "git.exe".into(),
                files: vec![r"C:\foo\bar".into()],
            }],
        };
        let json: serde_json::Value = serde_json::to_value(&report).expect("serialize");
        // Top-level fields
        for k in &[
            "workgroup",
            "platform",
            "diagnosticAvailable",
            "rawOsError",
            "sessions",
            "processes",
        ] {
            assert!(json.get(*k).is_some(), "missing field: {}", k);
        }
        // R.1.a: Platform enum must serialize to lowercase string.
        assert_eq!(
            json.get("platform").and_then(|v| v.as_str()),
            Some("windows"),
            "Platform enum must serialize to lowercase string"
        );
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
        // Snake-case must NOT leak at the wire boundary.
        for k in &[
            "diagnostic_available",
            "raw_os_error",
            "session_id",
            "agent_name",
        ] {
            assert!(
                json.get(*k).is_none(),
                "leaked snake_case field at top level: {}",
                k
            );
        }
        for k in &["session_id", "agent_name"] {
            assert!(
                s.get(*k).is_none(),
                "leaked snake_case session field: {}",
                k
            );
        }
    }

    /// §7.1 (Windows variant): `is_file_in_use_error` matches os error 32 (ERROR_SHARING_VIOLATION).
    #[cfg(windows)]
    #[test]
    fn is_file_in_use_error_matches_sharing_violation_on_windows() {
        use crate::commands::entity_creation::is_file_in_use_error;
        let e = std::io::Error::from_raw_os_error(32);
        assert!(is_file_in_use_error(&e), "os error 32 must match");

        let other = std::io::Error::from_raw_os_error(2); // ERROR_FILE_NOT_FOUND
        assert!(!is_file_in_use_error(&other), "non-32 must not match");
    }

    /// §7.1 (non-Windows variant): `is_file_in_use_error` always returns false off Windows.
    #[cfg(not(windows))]
    #[test]
    fn is_file_in_use_error_no_op_on_non_windows() {
        use crate::commands::entity_creation::is_file_in_use_error;
        let e = std::io::Error::from_raw_os_error(32);
        assert!(
            !is_file_in_use_error(&e),
            "non-Windows must always return false"
        );
    }

    /// §7.22 (covers G.4.7): no valid workgroup name can collide with the
    /// `BLOCKERS:` or `DIRTY_REPOS:` sentinel prefixes — `validate_existing_name`
    /// rejects both `:` and `_`. Locks the wire-protocol invariant.
    #[test]
    fn workgroup_names_cannot_collide_with_sentinels() {
        use crate::commands::entity_creation::validate_existing_name;
        // Both prefixes contain ':' (BLOCKERS:) or '_' and ':' (DIRTY_REPOS:),
        // neither of which is in the validator's alphanumeric+'-' whitelist.
        assert!(validate_existing_name("BLOCKERS:foo", "Workgroup").is_err());
        assert!(validate_existing_name("DIRTY_REPOS:foo", "Workgroup").is_err());
        // Bare-prefix-without-colon would be alphanumeric and pass the validator,
        // but bare prefixes aren't sentinel hits (frontend uses startsWith with the colon).
        assert!(validate_existing_name("BLOCKERS", "Workgroup").is_ok());
        assert!(validate_existing_name("DIRTY-REPOS", "Workgroup").is_ok());
    }
}
