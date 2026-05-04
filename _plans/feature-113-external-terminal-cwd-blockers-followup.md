# Plan - issue #113 follow-up: external terminal CWD blockers

**Repo:** `repo-AgentsCommander`  
**Branch:** `feature/113-wg-delete-blockers-diagnostic`  
**Existing plan:** `_plans/feature-113-wg-delete-blockers-diagnostic.md`  
**Status:** Final architect-reviewed; ready for implementation. Sections 8 and 9 are incorporated into the change spec below.

## 1. Requirement

User-observed failure on the wg-7 build:

- A terminal/process has its current working directory inside the workgroup tree being deleted.
- `try_atomic_delete_wg` correctly refuses to rename/delete the workgroup with Windows `ERROR_ACCESS_DENIED` / os error 5.
- `diagnose_blockers` returns no AC sessions and no Restart Manager processes.
- The modal therefore says: `No blockers identified. The lock may be transient... Raw error: Access is denied. (os error 5)`.

Required behavior:

- Preserve the existing atomic rename probe in `delete_workgroup`.
- Preserve existing AC-session and Restart Manager process detection.
- Add a Windows-only fallback/source that identifies normal user-owned external processes whose CWD is under the target workgroup.
- Show those processes in the existing blockers modal so the user knows what to close.
- Do not shell out to PowerShell, `wmic`, Sysinternals, or similar tools in production.

## 2. Current Code Facts

- `src-tauri/src/commands/entity_creation.rs:848-878` calls `try_atomic_delete_wg(&wg_dir)` and, on `WgDeleteOutcome::Blocked`, calls `crate::commands::wg_delete_diagnostic::diagnose_blockers(...)` over the still-intact tree.
- `src-tauri/src/commands/entity_creation.rs:1460-1505` implements the atomic rename/delete helper.
- `src-tauri/src/commands/entity_creation.rs:1520-1528` classifies os error 5 as a rename blocker.
- `src-tauri/src/commands/wg_delete_diagnostic.rs:65-129` runs the Windows external-process scan inside `tokio::task::spawn_blocking`.
- `src-tauri/src/commands/wg_delete_diagnostic.rs:315-633` currently implements the Restart Manager scan directly in `scan_external_processes_windows`.
- `src-tauri/src/commands/wg_delete_diagnostic.rs:49-57` defines `BlockerProcess` as `{ pid, name, files }`.
- `src/shared/types.ts:298-302` mirrors that TS shape.
- `src/sidebar/components/ProjectPanel.tsx:1384-1395` renders external processes and nested `p.files`.
- `src/sidebar/components/ProjectPanel.tsx:1406-1410` renders the "No blockers identified" text only when `processes.length === 0`.

## 3. Strategy Verdict

Use **PEB ProcessParameters CWD scanning** as the new source, merged with Restart Manager results.

Chosen approach:

- Enumerate processes with ToolHelp (`CreateToolhelp32Snapshot`, `Process32FirstW`, `Process32NextW`).
- For each accessible process, open it with `PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ`.
- Read `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory.DosPath` from the target process via `NtQueryInformationProcess(ProcessBasicInformation)` plus `ReadProcessMemory`.
- If the CWD path is under `wg_dir`, add/merge a `BlockerProcess` entry with a `cwd` sample.

Why this is the right fallback:

- It targets the exact failing condition: a process current directory inside the WG.
- It is bounded by process count, not file count or handle count.
- It works for normal same-user terminals: `powershell.exe`, `pwsh.exe`, `cmd.exe`, `bash.exe`, and similar.
- It does not require admin privileges for ordinary same-user, same-integrity processes.
- It avoids external tools and keeps all production behavior in Rust.

Rejected for this follow-up:

- **System handle enumeration** via `NtQuerySystemInformation(SystemExtendedHandleInformation)`, `DuplicateHandle`, and `GetFinalPathNameByHandleW`. It can find directory handles, but it is heavier, can require `PROCESS_DUP_HANDLE`, is prone to protected/elevated-process access failures, has object-type-number instability, and can hang or become expensive if applied broadly to all handles. Keep it as a later fallback only if PEB CWD scanning still misses important cases.
- **PowerShell/wmic/WMI shell-out.** Not acceptable for production here. WMI does not reliably expose process CWD anyway, and shelling out adds latency, quoting risk, and external runtime assumptions.
- **New Rust process library.** No existing dependency in this repo exposes Windows process CWD. Adding `sysinfo` or a similar crate does not solve CWD on Windows cleanly and increases dependency surface.

## 4. Affected Files

1. `src-tauri/Cargo.toml:34-40`
   - Add Windows feature gates for ToolHelp, ReadProcessMemory, and WDK `NtQueryInformationProcess`.

2. `src-tauri/src/commands/wg_delete_diagnostic.rs:49-57`
   - Extend `BlockerProcess` with optional `cwd`.

3. `src-tauri/src/commands/wg_delete_diagnostic.rs:143-166`
   - Extend path-prefix stripping/comparison helpers to handle process CWD strings robustly.

4. `src-tauri/src/commands/wg_delete_diagnostic.rs:315-633`
   - Rename the existing RM implementation and wrap it with a merged external-process scanner.

5. `src-tauri/src/commands/wg_delete_diagnostic.rs:88-101`
   - Rename the diagnostic warning text from Restart Manager-specific wording to external-process scan wording.

6. `src-tauri/src/commands/wg_delete_diagnostic.rs:633`
   - Add the Windows CWD scanner helpers before the test module.

7. `src-tauri/src/commands/wg_delete_diagnostic.rs:727-771`
   - Update serialization test for `cwd`.

8. `src-tauri/src/commands/wg_delete_diagnostic.rs:871-959`
   - Add Windows unit/integration-style tests for CWD scanning.

9. `src/shared/types.ts:298-302`
   - Add optional `cwd` to `BlockerProcess`.

10. `src/sidebar/components/ProjectPanel.tsx:1388-1395`
   - Render `p.cwd` above any file samples.

No Tauri command registration changes. No `src/shared/ipc.ts` changes. The existing `BLOCKERS:` sentinel remains the transport.

## 5. Detailed Change Spec

### 5.1 `src-tauri/Cargo.toml`

Current at `src-tauri/Cargo.toml:34-40`:

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
    "Win32_System_Console",
    "Win32_Foundation",
    "Win32_System_Threading",
    "Win32_System_RestartManager",
] }
```

Replace with:

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
    "Win32_System_Console",
    "Win32_Foundation",
    "Win32_System_Threading",
    "Win32_System_RestartManager",
    "Win32_System_Diagnostics_Debug",
    "Win32_System_Diagnostics_ToolHelp",
    "Wdk_System_Threading",
] }
```

Rationale:

- `Win32_System_Diagnostics_Debug` exposes `ReadProcessMemory`.
- `Win32_System_Diagnostics_ToolHelp` exposes process enumeration.
- `Wdk_System_Threading` exposes `NtQueryInformationProcess`, `ProcessBasicInformation`, and `ProcessWow64Information`.

Do not add a new crate.

### 5.2 Extend `BlockerProcess`

Current at `src-tauri/src/commands/wg_delete_diagnostic.rs:49-57`:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerProcess {
    pub pid: u32,
    /// Executable file name (e.g. "git.exe", "node.exe"). Best-effort.
    pub name: String,
    /// Sample of paths inside the workgroup that this process holds. Capped at MAX_FILES_PER_PROCESS.
    pub files: Vec<String>,
}
```

Replace with:

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerProcess {
    pub pid: u32,
    /// Executable file name (e.g. "git.exe", "node.exe"). Best-effort.
    pub name: String,
    /// Current working directory if this process was identified by the CWD fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Sample of paths inside the workgroup that this process holds. Capped at MAX_FILES_PER_PROCESS.
    pub files: Vec<String>,
}
```

Update every Rust literal constructing `BlockerProcess`:

- At `wg_delete_diagnostic.rs:574-578`, set `cwd: None`.
- In serialization tests around `wg_delete_diagnostic.rs:738-742`, set `cwd: Some(r"C:\foo".into())` so the JSON test verifies the new camelCase field.

### 5.3 TS Type Shape

Current at `src/shared/types.ts:298-302`:

```ts
export interface BlockerProcess {
  pid: number;
  name: string;
  files: string[];
}
```

Replace with:

```ts
export interface BlockerProcess {
  pid: number;
  name: string;
  cwd?: string;
  files: string[];
}
```

`cwd` is optional because RM-only blockers will not serialize it. This keeps the modal tolerant of older payloads during development.

### 5.4 UI Rendering

Current at `src/sidebar/components/ProjectPanel.tsx:1388-1395`:

```tsx
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
```

Replace with:

```tsx
{(p) => (
  <li>
    {p.name} (PID {p.pid})
    <Show when={p.cwd}>
      {(cwd) => (
        <div style={{ "font-size": "11px", opacity: 0.85 }}>
          CWD: {cwd()}
        </div>
      )}
    </Show>
    <Show when={p.files.length > 0}>
      <ul style={{ margin: "2px 0 0 16px", padding: "0", "font-size": "11px", opacity: 0.85 }}>
        <For each={p.files}>{(f) => <li>{f}</li>}</For>
      </ul>
    </Show>
  </li>
)}
```

No other modal state changes are needed. Once the CWD fallback emits at least one process, the existing `processes.length === 0` guard at `ProjectPanel.tsx:1406` prevents the misleading "No blockers identified" text from rendering.

### 5.5 Path Normalization Helpers

At `src-tauri/src/commands/wg_delete_diagnostic.rs:150-158`, extend `strip_long_prefix_str` to handle the NT DOS prefix that can appear in process parameters:

```rust
fn strip_long_prefix_str(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else if let Some(rest) = s.strip_prefix(r"\??\") {
        rest.to_string()
    } else {
        s.to_string()
    }
}
```

Add this helper after `canonicalize_for_compare` at `wg_delete_diagnostic.rs:166`:

```rust
#[cfg(windows)]
fn path_is_under_windows(candidate: &Path, root: &Path) -> bool {
    fn normalize(p: &Path) -> String {
        strip_long_prefix_str(&p.to_string_lossy())
            .replace('/', r"\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    }

    let candidate = normalize(candidate);
    let root = normalize(root);
    candidate == root || candidate.starts_with(&format!(r"{}\", root))
}
```

Use this helper for process CWD matching. Do not replace the existing AC session scan unless the dev wants the same case-insensitive behavior there too; this follow-up does not require that broader change.

### 5.6 Preserve RM, Add CWD Fallback, Merge Results

At `src-tauri/src/commands/wg_delete_diagnostic.rs:315`, rename the existing function:

```rust
fn scan_external_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
```

to:

```rust
fn scan_restart_manager_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
```

Leave the existing RM body intact except for adding `cwd: None` in its `BlockerProcess` initializer.

In `diagnose_blockers` around `wg_delete_diagnostic.rs:88-101`, update the warning strings because the command is no longer only a Restart Manager scan:

```rust
Ok(Err(e)) => {
    log::warn!("[wg_delete_diagnostic] external process scan failed: {}", e);
    (Vec::<BlockerProcess>::new(), false)
}
Err(join_err) => {
    log::warn!(
        "[wg_delete_diagnostic] external process scan join failed: {}",
        join_err
    );
    (Vec::<BlockerProcess>::new(), false)
}
```

Then add a new `scan_external_processes_windows` wrapper immediately before `scan_restart_manager_processes_windows`:

```rust
#[cfg(windows)]
fn scan_external_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
    let rm = scan_restart_manager_processes_windows(wg_dir);
    let cwd = scan_cwd_processes_windows(wg_dir);

    match (rm, cwd) {
        (Ok(rm_processes), Ok(cwd_processes)) => Ok(merge_blocker_processes(rm_processes, cwd_processes)),
        (Ok(rm_processes), Err(cwd_err)) => {
            log::warn!(
                "[wg_delete_diagnostic] CWD fallback scan failed; preserving Restart Manager result: {}",
                cwd_err
            );
            Ok(rm_processes)
        }
        (Err(rm_err), Ok(cwd_processes)) => {
            log::warn!(
                "[wg_delete_diagnostic] Restart Manager scan failed; preserving CWD fallback result: {}",
                rm_err
            );
            Ok(cwd_processes)
        }
        (Err(rm_err), Err(cwd_err)) => Err(format!(
            "Restart Manager scan failed: {}; CWD fallback scan failed: {}",
            rm_err, cwd_err
        )),
    }
}
```

Add `merge_blocker_processes` near the wrapper:

```rust
#[cfg(windows)]
fn merge_blocker_processes(
    rm_processes: Vec<BlockerProcess>,
    cwd_processes: Vec<BlockerProcess>,
) -> Vec<BlockerProcess> {
    use std::collections::HashMap;

    let mut by_pid: HashMap<u32, BlockerProcess> = HashMap::new();
    for process in rm_processes.into_iter().chain(cwd_processes) {
        by_pid
            .entry(process.pid)
            .and_modify(|existing| {
                if existing.cwd.is_none() {
                    existing.cwd = process.cwd.clone();
                }
                for file in &process.files {
                    if existing.files.len() < MAX_FILES_PER_PROCESS && !existing.files.contains(file) {
                        existing.files.push(file.clone());
                    }
                }
                // PID-only cross-source merge is best effort. Preserve a non-placeholder
                // RM name instead of replacing it with a later CWD snapshot name.
                if existing.name.starts_with("pid ") && !process.name.starts_with("pid ") {
                    existing.name = process.name.clone();
                }
            })
            .or_insert(process);
    }

    let mut out: Vec<BlockerProcess> = by_pid.into_values().collect();
    out.sort_by_key(|p| p.pid);
    out
}
```

This preserves all RM detections and enriches duplicate PIDs with `cwd`. The cross-source merge is intentionally PID-only for this follow-up: the existing RM pass still keeps its per-file `ProcessStartTime` checks, but the combined modal entry can theoretically race PID reuse between the RM and CWD passes. Do not add creation-time plumbing in this patch; document the merge as best effort and keep the non-placeholder RM name when present.

### 5.7 Add Windows CWD Scanner Helpers

Add these helpers after the renamed RM function body (`wg_delete_diagnostic.rs:633`) and before `#[cfg(test)] mod tests`.

Required helper signatures:

```rust
#[cfg(windows)]
#[derive(Debug)]
struct ProcessSnapshotEntry {
    pid: u32,
    name: String,
}

#[cfg(windows)]
fn scan_cwd_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String>;

#[cfg(windows)]
fn enumerate_processes_windows() -> Result<Vec<ProcessSnapshotEntry>, String>;

#[cfg(windows)]
fn read_process_cwd_windows(pid: u32) -> Option<String>;
```

`scan_cwd_processes_windows` behavior:

```rust
#[cfg(windows)]
fn scan_cwd_processes_windows(wg_dir: &Path) -> Result<Vec<BlockerProcess>, String> {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    let canonical_wg = canonicalize_for_compare(wg_dir);
    let current_pid = unsafe { GetCurrentProcessId() };
    let mut out = Vec::new();

    for process in enumerate_processes_windows()? {
        if process.pid == 0 || process.pid == current_pid {
            continue;
        }
        let Some(cwd) = read_process_cwd_windows(process.pid) else {
            continue;
        };
        let cwd_compare = canonicalize_for_compare(Path::new(&cwd));
        if path_is_under_windows(&cwd_compare, &canonical_wg) {
            out.push(BlockerProcess {
                pid: process.pid,
                name: process.name,
                cwd: Some(cwd),
                files: Vec::new(),
            });
        }
    }

    Ok(out)
}
```

`enumerate_processes_windows` implementation requirements:

- Use `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)`.
- Treat `INVALID_HANDLE_VALUE` as an error.
- Wrap the snapshot handle in a small RAII guard that calls `CloseHandle`.
- Initialize `PROCESSENTRY32W` with `unsafe { std::mem::zeroed::<PROCESSENTRY32W>() }`, then set `dwSize = size_of::<PROCESSENTRY32W>() as u32`. `windows-sys 0.59` does not implement `Default` for this struct.
- Iterate with `Process32FirstW` and `Process32NextW`.
- Convert `szExeFile` with a local NUL-terminated UTF-16 helper.
- Return `Err(...)` only when snapshot creation fails. Individual process read failures are not errors.

`read_process_cwd_windows` implementation requirements:

- Open the process with `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid)`.
- Return `None` on `OpenProcess` failure. This is expected for protected, elevated, cross-user, or exited processes.
- Wrap successful process handles in the same small RAII handle guard used for the snapshot. `read_process_cwd_windows` has many early returns and must not leak one process handle per skipped PID.
- Use `NtQueryInformationProcess(ProcessBasicInformation, ...)` to get the target PEB address.
- Treat `NtQueryInformationProcess` success using NTSTATUS semantics. For the classes used here, require `status == 0`; any nonzero status returns `None` for that process.
- Use `ReadProcessMemory` to read:
  1. the minimal PEB prefix containing `ProcessParameters`;
  2. the minimal `RTL_USER_PROCESS_PARAMETERS` prefix containing `CurrentDirectory`;
  3. the UTF-16 buffer referenced by `CurrentDirectory.DosPath`.
- Add local remote-read helpers, for example `read_remote_struct<T: Copy>` and a byte-buffer variant. Each must pass a real `usize` bytes-read out parameter to `ReadProcessMemory` and require `ok != 0 && bytes_read == requested`. In `windows-sys 0.59`, the bytes-read parameter is `*mut usize`, not an `Option`.
- Before reading `CurrentDirectory.DosPath.Buffer`, validate the remote `UNICODE_STRING`: `length` must be nonzero and even, `maximum_length` must be even, `length <= maximum_length`, `maximum_length <= MAX_CWD_BYTES + 2`, and `buffer` must be non-null. Treat any violation as `None`.
- Cap the actual UTF-16 read to `MAX_CWD_BYTES = 32 * 1024`.
- Convert UTF-16 lossily, then pass through `strip_long_prefix_str`.

Use local `#[repr(C)]` remote-layout structs rather than the current `windows-sys` `RTL_USER_PROCESS_PARAMETERS`, because the `windows-sys 0.59` struct at `Win32/System/Threading/mod.rs:976-981` only exposes `ImagePathName` and `CommandLine`, not `CurrentDirectory`.

Minimum 64-bit layout:

```rust
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix64 {
    reserved: [u8; 0x20],
    process_parameters: *mut core::ffi::c_void,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteUnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteCurDir {
    dos_path: RemoteUnicodeString,
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemoteProcessParametersPrefix {
    maximum_length: u32,
    length: u32,
    flags: u32,
    debug_flags: u32,
    console_handle: windows_sys::Win32::Foundation::HANDLE,
    console_flags: u32,
    standard_input: windows_sys::Win32::Foundation::HANDLE,
    standard_output: windows_sys::Win32::Foundation::HANDLE,
    standard_error: windows_sys::Win32::Foundation::HANDLE,
    current_directory: RemoteCurDir,
}
```

Also define a local `ProcessBasicInformationRaw` for the `NtQueryInformationProcess` output:

```rust
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcessBasicInformationRaw {
    exit_status: windows_sys::Win32::Foundation::NTSTATUS,
    peb_base_address: *mut core::ffi::c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}
```

For 32-bit target processes on 64-bit Windows:

- Prefer adding support with `NtQueryInformationProcess(ProcessWow64Information, ...)`.
- If it returns a nonzero 32-bit PEB address, read 32-bit versions of the same minimal structs where pointer/handle fields are `u32`. The 32-bit PEB `ProcessParameters` offset is `0x10`:

```rust
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix32 {
    reserved: [u8; 0x10],
    process_parameters: u32,
}
```

- If implementing WOW64 support, define parallel `RemoteUnicodeString32`, `RemoteCurDir32`, and `RemoteProcessParametersPrefix32`; do not reuse the native-width structs for 32-bit remote memory.
- If the dev chooses not to implement WOW64 support in the first patch, document that limitation in a code comment and skip those processes. This is acceptable for the reported repro because Windows Terminal, PowerShell, cmd, and current Git Bash installs are normally 64-bit in this environment.

Do not panic in any CWD helper. Every per-process failure should degrade to `None` and a `debug!` log at most.

### 5.8 Serialization Test Updates

At `wg_delete_diagnostic.rs:727-771`, update `blocker_report_serializes_with_camelcase_fields`:

- Construct `BlockerProcess { pid, name, cwd: Some(r"C:\foo".into()), files }`.
- Add `"cwd"` to the process-field assertion:

```rust
for k in &["pid", "name", "cwd", "files"] {
    assert!(p.get(*k).is_some(), "missing process field: {}", k);
}
```

- Add `"cwd"` to any negative snake-case checks only if a future Rust field name would otherwise be snake_case. Here it is already lower-case, so no extra negative check is required.

### 5.9 New Rust Tests

Add these tests at the end of `wg_delete_diagnostic.rs` test module, after `collect_files_to_probe_orders_dirs_before_files` (`wg_delete_diagnostic.rs:929-959`).

1. Merge test, all platforms or Windows-gated:

```rust
#[cfg(windows)]
#[test]
fn merge_blocker_processes_combines_rm_files_and_cwd_by_pid() {
    let merged = merge_blocker_processes(
        vec![BlockerProcess {
            pid: 123,
            name: "git.exe".into(),
            cwd: None,
            files: vec![r"C:\wg\repo\.git\index".into()],
        }],
        vec![BlockerProcess {
            pid: 123,
            name: "powershell.exe".into(),
            cwd: Some(r"C:\wg\repo".into()),
            files: Vec::new(),
        }],
    );

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].pid, 123);
    assert_eq!(merged[0].cwd.as_deref(), Some(r"C:\wg\repo"));
    assert_eq!(merged[0].files, vec![r"C:\wg\repo\.git\index"]);
}
```

2. Path-prefix test:

```rust
#[cfg(windows)]
#[test]
fn path_is_under_windows_is_case_insensitive_and_boundary_safe() {
    assert!(path_is_under_windows(
        Path::new(r"C:\Users\Maria\WG\repo-foo"),
        Path::new(r"c:\users\maria\wg")
    ));
    assert!(!path_is_under_windows(
        Path::new(r"C:\Users\Maria\WG2"),
        Path::new(r"C:\Users\Maria\WG")
    ));
}
```

3. CWD scanner integration-style test:

```rust
#[cfg(windows)]
#[test]
fn scan_cwd_processes_windows_detects_child_process_current_dir() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let tmp = tempfile::tempdir().expect("tempdir");
    let wg_dir = tmp.path().join("wg-1-test");
    let repo_dir = wg_dir.join("repo-foo");
    std::fs::create_dir_all(&repo_dir).expect("create repo");

    let mut child = Command::new("cmd.exe")
        .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
        .current_dir(&repo_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cmd blocker");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while Instant::now() < deadline {
        let blockers = scan_cwd_processes_windows(&wg_dir).expect("cwd scan");
        found = blockers.iter().any(|p| {
            p.pid == child.id()
                && p.cwd
                    .as_deref()
                    .is_some_and(|cwd| path_is_under_windows(Path::new(cwd), &wg_dir))
        });
        if found {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(found, "CWD fallback must detect child cmd.exe under WG");
}
```

If `child.id()` is the short-lived `cmd.exe` and the long-lived process is instead `ping.exe` on a particular Windows image, loosen the PID assertion to accept any blocker with `cwd` under `repo_dir`. Prefer first trying the strict PID assertion because it proves exact attribution.

### 5.10 Verification Commands

Dev should run:

```powershell
cd C:\Users\maria\0_repos\agentscommander\.ac-new\wg-7-dev-team\repo-AgentsCommander\src-tauri
cargo test wg_delete_diagnostic -- --nocapture
cargo test try_atomic_delete_wg -- --nocapture
```

Then from repo root:

```powershell
cd C:\Users\maria\0_repos\agentscommander\.ac-new\wg-7-dev-team\repo-AgentsCommander
npm run build
```

There is no dedicated typecheck script in `package.json`; `npm run build` is the available frontend compile check.

## 6. Manual Repro Test

Use the exact failure shape:

1. Build/run the wg-7 app from `feature/113-wg-delete-blockers-diagnostic`.
2. Create or choose a workgroup, for example:
   `C:\Users\maria\0_repos\agentscommander\.ac-new\wg-7-dev-team`
3. Open Windows Terminal, PowerShell, cmd, or Git Bash.
4. `cd` into a repo under that workgroup, for example:
   `C:\Users\maria\0_repos\agentscommander\.ac-new\wg-7-dev-team\repo-AgentsCommander`
5. In AC, delete the workgroup.
6. Expected:
   - Delete still fails safely with the blockers modal.
   - The modal has an `External processes` entry.
   - The entry name will usually be the shell process (`powershell.exe`, `pwsh.exe`, `cmd.exe`, `bash.exe`), not necessarily `WindowsTerminal.exe`.
   - The entry includes `CWD: <path under the workgroup>`.
   - The misleading "No blockers identified" text does not render.
7. Close that terminal/shell.
8. Click Retry.
9. Expected: delete succeeds unless another blocker remains.

Also manually test:

- RM still works for a file handle or IDE case by opening a file under the WG and confirming `files` still render.
- Dirty repo flow still returns `DIRTY_REPOS:` before any blocker diagnostic when `force` is false.
- Non-Windows build still compiles because all new process scanning is under `#[cfg(windows)]`.

## 7. Risks and Failure Modes

- **Access denied to process memory.** Elevated terminals, protected processes, cross-user processes, and some antivirus-instrumented processes may reject `OpenProcess` or `ReadProcessMemory`. Degrade by skipping that PID and logging at `debug!`; do not fail the whole diagnostic.
- **Bitness mismatch.** 64-bit AC can support 32-bit target processes with `ProcessWow64Information` plus 32-bit remote structs. If not implemented in the first dev patch, document that skip. 32-bit AC reading 64-bit targets is not worth supporting for this release.
- **Undocumented layout.** `RTL_USER_PROCESS_PARAMETERS` layout is not a stable Win32 contract, but `CurrentDirectory` is a long-lived NT layout used by process tooling. Keep structs minimal, read only the prefix, cap all remote reads, and treat every mismatch as `None`.
- **Process exit races.** A process can exit between ToolHelp enumeration, `OpenProcess`, PEB query, and memory reads. Treat all such failures as benign skips.
- **False positives.** A process whose CWD is under the WG is a real rename/delete blocker on Windows, so listing it is appropriate. If the process changes CWD before the user sees the modal, Retry will succeed.
- **RM failure semantics.** The wrapper must not lose RM results if the CWD scan fails, and must not lose CWD results if RM fails. Only return `Err` when both sources fail at source-level initialization.
- **Performance.** Process count is usually hundreds, not thousands. Run remains inside existing `spawn_blocking`; do not move this scan onto the async worker.

## 8. Dev Review Addendum

Review date: 2026-05-04, dev-rust.

Current code references in this plan match the branch state: RM is still the only
external-process source, `BlockerProcess` has only `{ pid, name, files }`, the TS
interface mirrors that shape, and the modal renders only file samples today.

Implementation notes to apply with the spec above:

- The proposed `windows-sys = 0.59` feature gates are sufficient for the new calls:
  `Win32_System_Diagnostics_Debug` exposes `ReadProcessMemory`,
  `Win32_System_Diagnostics_ToolHelp` exposes ToolHelp process enumeration, and
  `Wdk_System_Threading` exposes `NtQueryInformationProcess`,
  `ProcessBasicInformation`, and `ProcessWow64Information`. Do not import
  `windows_sys::Win32::System::Threading::PROCESS_BASIC_INFORMATION` unless also
  adding `Win32_System_Kernel`; the local `ProcessBasicInformationRaw` in this plan
  intentionally avoids that extra feature.
- `windows-sys 0.59` does not implement `Default` for `PROCESSENTRY32W`. Initialize it
  with `unsafe { std::mem::zeroed::<PROCESSENTRY32W>() }`, then set
  `dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32` before calling
  `Process32FirstW`.
- Add a small RAII handle guard for `OpenProcess` results too, not just the ToolHelp
  snapshot. `read_process_cwd_windows` has many early-return paths, and leaking one
  process handle per skipped PID would be easy otherwise.
- Add a local remote-read helper, for example `read_remote_struct<T: Copy>` and a
  byte-buffer variant, that calls `ReadProcessMemory` with a real `usize` bytes-read
  out parameter and requires `ok != 0 && bytes_read == requested`. In `windows-sys
  0.59`, `ReadProcessMemory` takes `*mut usize`, not an `Option`.
- Treat `NtQueryInformationProcess` success with NTSTATUS semantics (`status >= 0`,
  or equivalently `status == 0` for the classes used here). Per-process failures
  should return `None` and log at `debug!` at most.
- Spell out the PEB prefixes in code. For a 64-bit target read by a 64-bit AC build,
  `ProcessParameters` is at PEB offset `0x20`:

```rust
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix64 {
    reserved: [u8; 0x20],
    process_parameters: *mut core::ffi::c_void,
}
```

  For a WOW64 target read by a 64-bit AC build, `ProcessParameters` is at 32-bit PEB
  offset `0x10`, and all pointers/handles in the 32-bit remote structs must be `u32`:

```rust
#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct RemotePebPrefix32 {
    reserved: [u8; 0x10],
    process_parameters: u32,
}
```

  If WOW64 support is deferred, add a code comment and skip non-native-width targets.
  A 32-bit AC build reading 64-bit target processes is explicitly out of scope for
  this release.
- Keep `RemoteProcessParametersPrefix` only for native-width reads. Define parallel
  `RemoteUnicodeString32`, `RemoteCurDir32`, and `RemoteProcessParametersPrefix32`
  if implementing WOW64 support.
- Rename the `diagnose_blockers` warning text from "Restart Manager scan failed" to
  "external process scan failed" once `scan_external_processes_windows` becomes the
  RM+CWD wrapper, otherwise logs will be misleading when only the fallback fails.
- The Rust wire-shape change requires the TS interface and modal rendering changes
  listed in this plan. Because dev-rust's role normally forbids frontend edits,
  coordinate those two frontend files with `dev-webpage-ui` unless tech-lead grants
  an explicit exception for this issue.
- The proposed Windows integration-style test is feasible, but avoid leaving the
  spawned `cmd.exe` alive on an unexpected scan error. Prefer a small cleanup guard
  or avoid `expect` inside the polling loop after the child has spawned.

## 9. Explicit Verdict

READY_FOR_IMPLEMENTATION_WITH_NOTES

## Grinch Review

The Dev Review Addendum above closes the main implementation blockers I would
have raised: process-handle RAII, exact `ReadProcessMemory` byte counts, explicit
PEB prefixes, and non-native-width skip/WOW64 handling. Remaining findings are
non-blocking but should be carried into the dev patch.

1. **NON-BLOCKER - merge-by-PID still has a PID-reuse attribution race.**
   - **What** - Section 5.6 merges RM and CWD results by PID only. The current RM
     per-file pass already compares `ProcessStartTime` to avoid PID recycling
     inside RM, but the cross-source merge loses that protection.
   - **Why** - A process can block the RM pass, exit, and have its PID reused before
     the CWD pass. The modal could then combine old file samples with the new
     process name/CWD and tell the user to close the wrong process. Rare, but it is
     the same race class the existing RM code already defends against.
   - **Fix** - Best fix: carry process creation time in `ProcessSnapshotEntry`
     using `GetProcessTimes` and merge by `(pid, creation_time)` when both sources
     have it. Acceptable for this follow-up: document PID-only merge as best effort
     and do not overwrite an RM-provided non-placeholder name from a CWD-only entry.

2. **NON-BLOCKER - validate remote `UNICODE_STRING` consistency before reading.**
   - **What** - The plan caps `CurrentDirectory.DosPath.Length`, rejects odd/zero
     lengths, and checks null buffers, but it does not require `Length <=
     MaximumLength`.
   - **Why** - The target process can update its CWD while the scanner is reading
     `RTL_USER_PROCESS_PARAMETERS`. Reading a transient or inconsistent
     `UNICODE_STRING` can pull adjacent remote memory into the decoded path and
     produce false positives or noisy diagnostics.
   - **Fix** - Require `dos_path.length <= dos_path.maximum_length`, both even,
     and `dos_path.maximum_length <= MAX_CWD_BYTES + 2` before reading the UTF-16
     buffer. Treat violations as `None`.

3. **NON-BLOCKER - the new NT prefix normalization needs a direct unit test.**
   - **What** - Section 5.5 adds `\??\` stripping because process parameters can
     expose that form, but the proposed test only covers case-insensitive boundary
     behavior and the existing strip test covered `\\?\` forms.
   - **Why** - If the `\??\` branch is mistyped or regresses, the CWD read can
     succeed and still fail to match the workgroup path, returning the same empty
     blockers list as today.
   - **Fix** - Extend the prefix-strip test with
     `strip_long_prefix_str(r"\??\C:\Users\me\proj") == r"C:\Users\me\proj"` and
     add a `path_is_under_windows` assertion for a `\??\C:\...` candidate.

4. **NON-BLOCKER - make the Windows CWD test single-process or kill the tree.**
   - **What** - Section 5.9 still shows `cmd.exe /C ping ...`; the addendum asks
     for cleanup, but the concrete example can still leave the `ping.exe` child
     alive after killing the returned `cmd.exe` handle.
   - **Why** - A surviving child can keep the temp directory as CWD, make `TempDir`
     cleanup fail, and make strict PID matching flaky if the scanner sees the child
     instead of the shell.
   - **Fix** - Prefer a single long-lived process such as
     `powershell.exe -NoProfile -Command Start-Sleep -Seconds 30`, or create a
     Windows job object / process-tree cleanup for the test. If using the shell
     form, assert on any blocker whose `cwd` is under `repo_dir`, not only
     `child.id()`.

**Verdict:** PASS WITH NON-BLOCKERS. It does not need to go back to architect if
the addendum remains part of the plan; dev should address the four notes during
implementation.

## 10. Final Architect Verdict

READY_FOR_IMPLEMENTATION

## Grinch Implementation Review

APPROVED_FOR_BUILD

I reviewed the implementation diff for the Windows workgroup-delete blocker
diagnostic follow-up: unsafe PEB/RTL layout reads, handle RAII, exact
`ReadProcessMemory` byte-count checks, WOW64 structs, RM+CWD merge behavior,
Rust/TS wire shape, modal rendering, and the blocker-empty UI guard. I also
reran `cargo test wg_delete_diagnostic -- --nocapture`: 14 passed.

1. **MEDIUM - non-blocking: self-process CWD can still produce an empty blocker report.**
   - **File/line** - `src-tauri/src/commands/wg_delete_diagnostic.rs:868`
   - **Impact** - `scan_cwd_processes_windows` skips `current_pid`. If the
     AgentsCommander process itself has its CWD under the workgroup being
     deleted, Windows refuses to rename the workgroup ancestor, but the CWD
     fallback omits the actual blocking process. I verified Windows refuses
     renaming an ancestor of the current process CWD.
   - **Suggested fix** - Add a special self-process blocker when
     `std::env::current_dir()` is under `wg_dir`, or remove the self-PID skip if
     the resulting UI entry is acceptable.

2. **LOW - non-blocking: NT UNC prefix form can false-negative.**
   - **File/line** - `src-tauri/src/commands/wg_delete_diagnostic.rs:155`
   - **Impact** - `strip_long_prefix_str` handles `\??\C:\...` but not
     `\??\UNC\server\share\...`; the current normalization would produce
     `UNC\server\share\...` instead of `\\server\share\...`, so a process CWD
     under a UNC workgroup could fail the path-under-workgroup match.
   - **Suggested fix** - Add a `r"\??\UNC\"` branch before the generic
     `r"\??\"` branch, mirroring the existing `r"\\?\UNC\"` handling, and add a
     direct unit test for that form.

## Grinch Final Re-review

APPROVED_FOR_BUILD

I reviewed only the final relevant diff in
`src-tauri/src/commands/wg_delete_diagnostic.rs`.

The two prior findings are resolved:

- `strip_long_prefix_str` now handles `\??\UNC\server\share\...` before the
  generic `\??\` branch, and the direct strip/path-under tests cover that form.
- `scan_cwd_processes_windows` now adds a current-process CWD blocker via
  `current_process_cwd_blocker_from_parts(...)` before skipping the current PID
  during ToolHelp enumeration, with unit coverage for both under-WG and outside-WG
  self-CWD cases.

No new blocking issues found. I rechecked the RM+CWD merge behavior, handle RAII,
remote-read byte-count checks, native/WOW64 struct separation, `UNICODE_STRING`
validation, and child-process CWD test cleanup.
