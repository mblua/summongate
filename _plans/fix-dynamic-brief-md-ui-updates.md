# Plan: Dynamic UI updates when `BRIEF.md` changes

> **Rev 2 (2026-05-07).** Supersedes the v1 dedicated `BriefWatcher` design (rejected by user 2026-05-07: *"qué costo tiene tener algo chequeando el BRIEF.md todo el tiempo? […] mi miedo es que agreguemos porquería"*). This rev has **no new background thread, no new poll cadence, no new module**. Brief detection piggybacks on `DiscoveryBranchWatcher`'s existing 15 s tick. All v1 reviewer findings (§9, §10, §11 below) have been folded in or marked moot — see §A at the bottom for the disposition matrix.

---

## 1. Requirement

When `BRIEF.md` of a workgroup changes (via the `brief-set-title` / `brief-append-body` CLI verbs, an external editor, or any other writer), the UI must re-render *without* a session switch:

- **Terminal window** — `WorkgroupBrief` (`src/terminal/components/WorkgroupBrief.tsx`) reads `terminalStore.activeWorkgroupBrief`. Today this is only refreshed on `loadActiveSession()` (mount, `session_switched`, `session_destroyed`, `session_created`-when-no-active) — see `src/terminal/App.tsx:42-67, 131-149`. A coordinator-driven title change therefore stays stale until the user toggles to another session and back. Detached terminal windows are even worse: they don't subscribe to `session_switched` at all, so they go stale until destroyed.
- **Sidebar** — `ProjectPanel` (`src/sidebar/components/ProjectPanel.tsx:843-845`) reads `wg.brief` from `projectStore.projects[].workgroups[].brief`. That field is populated only when `discover_project` runs (`src/sidebar/stores/project.ts:113-127`), i.e. on the user reloading the project; otherwise stale.

The user explicitly accepted *"check on agent-focus change OR every X minutes"* as the SLA, with the constraint *"don't add a new constant 5s polling thread for the brief"*. This rev satisfies both.

---

## 2. High-level design

**One change point: extend `DiscoveryBranchWatcher::poll`.**

`DiscoveryBranchWatcher` already runs on its own thread, polls every **15 s**, and already iterates per-replica. We add a **third gate** to its poll loop (Gate C: brief detection), keyed on the unique workgroup-root paths the watcher already has access to via `replicas`, augmented by active-session walk-up so workgroups without a discovery-loaded project still get coverage.

Concretely:

- **Cadence:** 15 s (unchanged — the watcher's existing tick). User-visible latency for a CLI-driven brief change is ≤ 15 s.
- **Cost:** one `std::fs::metadata()` call per unique workgroup root per existing tick (typically ≤ 5 wgs in practice). The `read_to_string` only fires when stat changes (rare). No new thread, no new tokio runtime, no new `app.manage`.
- **Stat sentinel:** `len + mtime` short-circuits unchanged files at the metadata level — file content is read only when stat changes.
- **Defense in depth:** re-stat after the read (catches the external-editor torn-read window — Notepad does in-place writes, not atomic rename, see §11.2 in the historical reviews); 256 KiB read cap (defensive against an accidental giant `BRIEF.md`); BOM-strip (Notepad writes UTF-8 BOM; existing `read_workgroup_brief_for_cwd` does NOT strip BOM — see §11.7).
- **Emit-then-cache ordering:** mirror `GitWatcher::poll` — sentinel updates always (so next-tick stat-equality short-circuit works), but `brief` field is committed to the cache only on a successful `app_handle.emit`. A failed emit leaves the previous content in cache so the next stat-change retries (see §11.1).
- **One IPC event:** `workgroup_brief_updated { workgroupPath, brief, sessionIds }`. Both surfaces (terminal + sidebar) consume the same event from their respective `App.tsx`. Payload shape is identical to v1; no frontend-listener-shape change.

**Why piggyback rather than focus-driven refresh:** focus-driven refresh would need (a) frontend Tauri-window focus subscriptions in two webviews, (b) a new `read_workgroup_brief` Tauri command, and (c) a sidebar bulk-refresh path (because the sidebar's brief is per-wg, not per-active-session). That's more new code, more new patterns, more places for things to go stale. Piggybacking is one block in one file.

**Why not a CLI-side signal:** the user's veto is about **constant** polling. A CLI signal would not solve the external-editor case (Notepad / VS Code with `files.atomicSave: false`), which the user's reported scenario also includes. The piggyback covers both with one mechanism.

---

## 3. Affected files

### MODIFIED (Rust)
- `src-tauri/src/commands/ac_discovery.rs` — extend `DiscoveryBranchWatcher` with a brief cache, brief detection inside `poll()`, and a `BriefUpdatedPayload` struct.

### MODIFIED (TypeScript)
- `src/shared/ipc.ts` — declare `onWorkgroupBriefUpdated(...)`.
- `src/shared/markdown.ts` — add `briefFirstLine(content)` helper (TS port of `extract_brief_first_line`).
- `src/terminal/stores/terminal.ts` — add `setActiveWorkgroupBriefIfActive(id, brief)` setter.
- `src/sidebar/stores/project.ts` — add `updateWorkgroupBrief(workgroupPath, briefLine)` setter.
- `src/terminal/App.tsx` — register listener (outside every conditional — see §5.5).
- `src/sidebar/App.tsx` — register listener.

### NOT MODIFIED
- No new Rust module. No `src-tauri/src/session/brief_watcher.rs`. No `src-tauri/src/session/mod.rs` change. No `src-tauri/src/lib.rs` change (the watcher is already wired up).
- `src-tauri/src/cli/brief_set_title.rs`, `brief_ops.rs`, `brief_append_body.rs` — out-of-process; no IPC plumbing back.
- `src/shared/types.ts` — existing `Session.workgroupBrief` and `AcWorkgroup.brief` are unchanged. Event payload is declared inline in `ipc.ts` (matches the convention for `session_git_repos`, `ac_discovery_branch_updated`).
- `WorkgroupBrief.tsx`, `ProjectPanel.tsx` — already reactive to their stores; no template change needed.

---

## 4. Backend changes — `src-tauri/src/commands/ac_discovery.rs`

All edits are localized to this single file. Apply them in the order below.

### 4.1 Add imports (top of file)

The file already imports `Path`, `HashMap`, `HashSet`, `Mutex`, etc. for the existing watcher. Add what's missing:

- `use std::time::SystemTime;` — needed by `StatSentinel.mtime`.
- `use std::io::Read as _;` — needed by `take(...).read_to_string(...)` for the size cap.

Locate the existing `use std::time::Duration;` import block and extend it.

### 4.2 Add `BriefCacheEntry`, `StatSentinel`, `BriefUpdatedPayload`

Insert these next to the existing `ReplicaBranchEntry` / `DiscoveryBranchPayload` / `SessionGitReposPayload` declarations (currently at lines ~242-264):

```rust
#[derive(Clone, PartialEq, Eq)]
struct StatSentinel {
    len: u64,
    mtime: Option<SystemTime>,
}

#[derive(Clone)]
struct BriefCacheEntry {
    /// Last stat we observed for this workgroup's BRIEF.md. Drives the
    /// next-tick equality short-circuit so we skip the read entirely when
    /// nothing changed. Always refreshed (even on emit failure) so the
    /// short-circuit stays correct.
    sentinel: Option<StatSentinel>,
    /// Last content we successfully shipped to the frontend. Updated only
    /// on a successful `app_handle.emit` (mirrors `GitWatcher::poll`'s
    /// emit-then-cache ordering — see §11.1 in the plan history). A
    /// failed emit leaves this stale so the next stat-change retries.
    brief: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BriefUpdatedPayload {
    /// Absolute path of the wg-* directory (NOT the BRIEF.md file). The
    /// `\\?\` (Windows verbatim) prefix is stripped before emit so
    /// `discover_project`'s `read_dir`-derived `AcWorkgroup.path` and the
    /// watcher's path-walk-derived value compare equal under the
    /// frontend's `normalizePath` lower+forward-slash. See §10.2.1.
    workgroup_path: String,
    /// Trimmed file content with UTF-8 BOM stripped (Notepad on Windows
    /// writes the BOM; `str::trim` does not treat U+FEFF as whitespace).
    /// `None` means file missing / read-empty / read-failed-after-budget.
    brief: Option<String>,
    /// UUIDs of active sessions whose `working_directory` walks up to
    /// this workgroup. The terminal listener checks membership against
    /// the active (or locked) session id. May be empty when only the
    /// sidebar's discovery is driving the brief watch (no session in
    /// this wg) — sidebar updates by `workgroup_path`, not by session.
    session_ids: Vec<String>,
}
```

### 4.3 Add `brief_cache` field to `DiscoveryBranchWatcher`

Locate the struct (lines ~266-281) and extend with one new field at the bottom of the field list:

```rust
pub struct DiscoveryBranchWatcher {
    app_handle: AppHandle,
    session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    replicas: Mutex<HashMap<String, Vec<ReplicaBranchEntry>>>,
    discovery_cache: Mutex<HashMap<String, Option<String>>>,
    repos_cache: Mutex<HashMap<String, Vec<SessionRepo>>>,
    /// Per-workgroup-root cache for Gate C (brief detection). Keyed by the
    /// stripped (no `\\?\`) absolute path of the wg-* directory. Bounded
    /// implicitly by the union of (loaded-project replicas, active sessions);
    /// no explicit prune since entries are ~200B each and the upper bound is
    /// the user's project layout. See §11.5 in the plan history for why
    /// session-churn-driven retain-prune is intentionally NOT used.
    brief_cache: Mutex<HashMap<PathBuf, BriefCacheEntry>>,
}
```

Update `DiscoveryBranchWatcher::new` (lines ~283-295) to initialize the new field:

```rust
impl DiscoveryBranchWatcher {
    pub fn new(
        app_handle: AppHandle,
        session_manager: Arc<tokio::sync::RwLock<SessionManager>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            app_handle,
            session_manager,
            replicas: Mutex::new(HashMap::new()),
            discovery_cache: Mutex::new(HashMap::new()),
            repos_cache: Mutex::new(HashMap::new()),
            brief_cache: Mutex::new(HashMap::new()),
        })
    }
```

### 4.4 Extend `poll()` with Gate C (brief detection)

The current `poll()` (line 439) does this shape:

```rust
async fn poll(&self) {
    let entries: Vec<ReplicaBranchEntry> = { ... };
    if entries.is_empty() { return; }

    for entry in &entries {
        // Gate A: ac_discovery_branch_updated
        // Gate B: session_git_repos
    }
}
```

**Two changes:**

#### 4.4.1 Drop the early-return on empty `entries`

Sessions can exist for workgroups whose project is not loaded in `ProjectPanel` (e.g., the user opened a coordinator session via CLI in a wg whose project isn't on `projectPaths`). In that case `entries` is empty but Gate C still has work to do (driven by sessions). Refactor so the early-return only skips the for-loop, not the brief gate. Concretely, replace:

```rust
let entries: Vec<ReplicaBranchEntry> = { ... };
if entries.is_empty() {
    return;
}

for entry in &entries {
    ...
}
```

with:

```rust
let entries: Vec<ReplicaBranchEntry> = { ... };

if !entries.is_empty() {
    for entry in &entries {
        ...  // Gate A + Gate B unchanged
    }
}

// Gate C: BRIEF.md detection.
self.poll_briefs(&entries).await;
```

#### 4.4.2 Add `poll_briefs` and helpers

Add these as private methods on `impl DiscoveryBranchWatcher` (place them after the existing `detect_branch_with_timeout` / `detect_branch` helpers, near the bottom of the impl block):

```rust
/// Gate C: detect BRIEF.md changes per unique workgroup root and emit
/// `workgroup_brief_updated` on change. Runs on every existing 15s tick
/// of `poll()`; no new thread, no new cadence.
async fn poll_briefs(&self, entries: &[ReplicaBranchEntry]) {
    // Build the union of workgroup roots to watch:
    //   1. Replicas in loaded projects (from `entries`) — covers the
    //      sidebar `ProjectPanel` surface.
    //   2. Active sessions (walked up from cwd via the existing helper)
    //      — covers the terminal `WorkgroupBrief` surface for sessions
    //      whose project is NOT loaded.
    // The map's value is the list of session UUIDs that resolve to this
    // wg-root (used for the event payload's `sessionIds`).
    let mut wg_roots: HashMap<PathBuf, Vec<Uuid>> = HashMap::new();

    for entry in entries {
        // replica.path is `<wg-root>/__agent_<name>` — its parent IS the
        // wg-root. We do NOT call `find_workgroup_brief_path_for_cwd`
        // here because the parent is already the answer; calling it
        // would re-walk and add no information.
        if let Some(parent) = Path::new(&entry.replica_path).parent() {
            wg_roots
                .entry(strip_verbatim_prefix(parent))
                .or_default();
        }
    }

    let sessions: Vec<(Uuid, String)> = {
        let mgr = self.session_manager.read().await;
        mgr.get_sessions_working_dirs().await
    };
    for (id, cwd) in sessions {
        if let Some(brief_path) =
            crate::session::session::find_workgroup_brief_path_for_cwd(&cwd)
        {
            if let Some(parent) = brief_path.parent() {
                wg_roots
                    .entry(strip_verbatim_prefix(parent))
                    .or_default()
                    .push(id);
            }
        }
    }

    if wg_roots.is_empty() {
        return;
    }

    for (wg_root, session_ids) in wg_roots {
        self.check_workgroup_brief(wg_root, session_ids).await;
    }
}

/// Per-workgroup brief check. Stat short-circuits unchanged files; on
/// stat-change, reads (with size cap), re-stats (defends against torn
/// in-place editor saves), and emits if content actually changed.
async fn check_workgroup_brief(&self, wg_root: PathBuf, session_ids: Vec<Uuid>) {
    let brief_path = wg_root.join("BRIEF.md");

    let now_sentinel = std::fs::metadata(&brief_path).ok().map(|m| StatSentinel {
        len: m.len(),
        mtime: m.modified().ok(),
    });

    // Mutex held only for the duration of the get; released before any I/O.
    let prev = self.brief_cache.lock().unwrap().get(&wg_root).cloned();

    // Stat-equality short-circuit — the steady-state path. Cost: one
    // metadata() call per wg per tick when nothing has changed.
    if let Some(ref prev_entry) = prev {
        if prev_entry.sentinel == now_sentinel {
            return;
        }
    }

    // Read with a 256 KiB cap. A bigger BRIEF.md is either accidental
    // (someone catted /dev/urandom into it) or adversarial; either way,
    // we don't want it streamed through Tauri IPC every 15s. On overflow
    // we treat the file as effectively missing (None) and log; the
    // frontend already handles `brief: null` (panel falls back to "...").
    let new_brief = read_brief_capped(&brief_path);

    // Re-stat. If the file changed during our read window (external
    // editor mid-save — Notepad does CreateFile(OPEN_EXISTING) +
    // SetEndOfFile + write, NOT atomic rename), the read may be torn.
    // Defer to the next tick when the stat has settled.
    let post_sentinel = std::fs::metadata(&brief_path).ok().map(|m| StatSentinel {
        len: m.len(),
        mtime: m.modified().ok(),
    });
    if post_sentinel != now_sentinel {
        log::debug!(
            "[DiscoveryBranchWatcher] stat changed during read of {} (likely torn — external editor mid-save); deferring to next tick",
            brief_path.display()
        );
        return;
    }

    let content_changed = match prev.as_ref() {
        Some(p) => p.brief != new_brief,
        None => true,
    };

    // ALWAYS refresh the sentinel (next-tick short-circuit depends on
    // it). Insert a placeholder `brief = prev.brief` so a failed emit
    // below leaves the cache holding the previously-shipped content,
    // not the new content — that way the next stat-change retries
    // emission instead of silently accepting the failed state.
    {
        let mut cache = self.brief_cache.lock().unwrap();
        cache
            .entry(wg_root.clone())
            .and_modify(|e| e.sentinel = now_sentinel.clone())
            .or_insert(BriefCacheEntry {
                sentinel: now_sentinel.clone(),
                brief: prev.as_ref().and_then(|p| p.brief.clone()),
            });
    }

    if !content_changed {
        return;
    }

    let payload = BriefUpdatedPayload {
        workgroup_path: wg_root.to_string_lossy().into_owned(),
        brief: new_brief.clone(),
        session_ids: session_ids.iter().map(|u| u.to_string()).collect(),
    };
    match self.app_handle.emit("workgroup_brief_updated", payload) {
        Ok(()) => {
            // Commit shipped content. Mirrors GitWatcher's emit-then-cache
            // ordering (`git_watcher.rs:131-147`) — invariant: the cache's
            // `brief` field is the last value the FRONTEND has, not the
            // last value we read.
            self.brief_cache
                .lock()
                .unwrap()
                .entry(wg_root.clone())
                .and_modify(|e| e.brief = new_brief);
        }
        Err(e) => {
            log::warn!(
                "[DiscoveryBranchWatcher] brief emit failed for {} ({}); leaving cached brief stale so next stat-change retries",
                wg_root.display(),
                e
            );
        }
    }
}
```

And add the two free helpers (place them at module scope, near the existing `detect_branch_sync` at line ~209):

```rust
/// Strip the Windows verbatim/UNC `\\?\` prefix if present so the emitted
/// path matches the form `discover_project` produces (which never has the
/// prefix because it comes from a `read_dir` walk). See §10.2.1 in the plan
/// history; the same strip is applied in §9.4 of `entity_creation.rs` when
/// embedding paths into the agent init prompt.
fn strip_verbatim_prefix(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| p.to_path_buf())
}

/// Read BRIEF.md at most 256 KiB, strip a UTF-8 BOM if present, trim,
/// and return None on empty / read-error / file-missing. Bigger files are
/// truncated; we do NOT attempt to stream the whole file through Tauri IPC.
fn read_brief_capped(brief_path: &Path) -> Option<String> {
    const MAX_BYTES: u64 = 256 * 1024;
    let file = std::fs::File::open(brief_path).ok()?;
    let mut buf = String::new();
    if file.take(MAX_BYTES).read_to_string(&mut buf).is_err() {
        return None;
    }
    let trimmed = buf
        .strip_prefix('\u{FEFF}')
        .unwrap_or(&buf)
        .trim()
        .to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
```

`PathBuf` is already in scope at the top of `ac_discovery.rs` (used elsewhere in the file). If not, add `use std::path::PathBuf;`.

### 4.5 No `lib.rs` change

`DiscoveryBranchWatcher` is already constructed, started, and `app.manage`d in `lib.rs:266-271`. The new functionality piggybacks on the existing wiring. **Do not add a new `app.manage` call.**

---

## 5. Frontend changes

These are unchanged from v1 in shape, with two corrections folded in from the round-1 reviews:

- **§9.2** — `briefFirstLine` uses regex strip (greedy `# ` removal) to match Rust's `trim_start_matches("# ")`.
- **§10.2.2** — terminal listener is registered OUTSIDE every surrounding conditional (`if (!props.embedded)`, `if (!props.lockedSessionId)`).

### 5.1 MODIFIED: `src/shared/ipc.ts`

Append at the end of the file (after `onTelegramIncoming`, line ~509):

```typescript
export function onWorkgroupBriefUpdated(
  callback: (data: { workgroupPath: string; brief: string | null; sessionIds: string[] }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ workgroupPath: string; brief: string | null; sessionIds: string[] }>(
    "workgroup_brief_updated",
    callback
  );
}
```

Inline payload type — no `types.ts` entry, matches the `session_git_repos` declaration at line 288-295.

### 5.2 MODIFIED: `src/shared/markdown.ts`

Append a new helper that mirrors `extract_brief_first_line` (`src-tauri/src/commands/ac_discovery.rs:184-207`). Note the **regex-based greedy `# ` strip** — the Rust source uses `l.trim_start_matches("# ")`, which removes every repeated `"# "` prefix; a single-`slice(2)` port would diverge for inputs like `"# # Title"` (Rust → `"Title"`, naive port → `"# Title"`).

```typescript
/**
 * Mirror of `extract_brief_first_line` in `src-tauri/src/commands/ac_discovery.rs`.
 * Returns the first non-empty content line (frontmatter stripped, leading "# "
 * prefixes greedily removed), or null when the input has none. Keeping this in
 * lockstep with the Rust function avoids the sidebar showing a different value
 * after a watcher-emit vs. after a fresh `discover_project` call.
 */
export function briefFirstLine(content: string | null | undefined): string | null {
  if (!content) return null;
  const stripped = stripFrontmatter(content);
  for (const raw of stripped.split(/\r?\n/)) {
    const line = raw.trim();
    if (line.length === 0) continue;
    return line.replace(/^(?:# )+/, "");
  }
  return null;
}
```

### 5.3 MODIFIED: `src/terminal/stores/terminal.ts`

Insert inside `terminalStore` (after `setActiveSession`, just before the closing `}` at line 52):

```typescript
  /**
   * Update only the workgroup-brief field of the active session. Used by
   * the `workgroup_brief_updated` IPC listener — the rest of the active
   * session state is untouched. No-op when `id` does not match the
   * currently-active session id (race guard: the active session may have
   * switched between event emit and dispatch).
   */
  setActiveWorkgroupBriefIfActive(id: string, brief: string | null) {
    if (activeSessionId() !== id) return;
    setActiveWorkgroupBrief(brief);
  },
```

### 5.4 MODIFIED: `src/sidebar/stores/project.ts`

Add a setter that updates `wg.brief` for a single workgroup, keyed on its `path`. Insert after `updateReplicaBranch` (lines 95-110):

```typescript
  /**
   * Update a workgroup's brief (first line, post-frontmatter) from the
   * `workgroup_brief_updated` IPC listener. The Rust side strips the
   * Windows `\\?\` prefix before emit (see ac_discovery.rs::strip_verbatim_prefix),
   * so `normalizePath` here is defense-in-depth, not load-bearing.
   * Caller is responsible for deriving the first-line representation
   * via `briefFirstLine` so the value matches what `discover_project`
   * would produce.
   */
  updateWorkgroupBrief(workgroupPath: string, brief: string | null) {
    const normalized = normalizePath(workgroupPath);
    setProjects((prev) =>
      prev.map((proj) => ({
        ...proj,
        workgroups: proj.workgroups.map((wg) =>
          normalizePath(wg.path) === normalized
            ? { ...wg, brief: brief ?? undefined }
            : wg
        ),
      }))
    );
  },
```

`AcWorkgroup.brief` is typed `string | undefined` in `src/shared/types.ts:256`, hence the `?? undefined` coercion at the boundary.

`normalizePath` already exists in `project.ts` (lines 17-19) — same signature shape as `sessions.ts`, but does NOT strip trailing slashes. That's fine: the Rust side strips the `\\?\` prefix and never emits trailing slashes, so the two normalizations agree on the wire shape we care about (lowercase + forward-slash).

### 5.5 MODIFIED: `src/terminal/App.tsx`

**Import** — extend the existing IPC import block at lines 4-12 with `onWorkgroupBriefUpdated`:

```typescript
import {
  SessionAPI,
  WindowAPI,
  onSessionSwitched,
  onSessionCreated,
  onSessionDestroyed,
  onSessionRenamed,
  onThemeChanged,
  onWorkgroupBriefUpdated,
} from "../shared/ipc";
```

**Register the listener** — insert AFTER the `onSessionRenamed` block (closes on line 173) and BEFORE the `// Theme sync` comment (line 175). **Critical: the insertion point is OUTSIDE every surrounding `if`** — outside `if (!props.embedded)`, outside `if (!props.lockedSessionId)`. The brief listener applies to all modes (normal, detached, embedded). A dev who reads "before line 177" and inserts at line 178 (inside `if (!props.embedded)`) would silently disable brief updates in the unified Main window, the dominant UX path.

```typescript
    // Brief updates from DiscoveryBranchWatcher's Gate C (15s piggyback).
    // Registered OUTSIDE every conditional — applies to all modes (normal,
    // detached, embedded). DO NOT hoist above loadActiveSession(): an event
    // racing with the initial SessionAPI.list() would overwrite the freshly-
    // loaded brief with a value that's actually older.
    unlisteners.push(
      await onWorkgroupBriefUpdated(({ brief, sessionIds }) => {
        // Normal mode: follow the active session. Detached mode: locked id.
        const targetId = props.lockedSessionId ?? terminalStore.activeSessionId;
        if (!targetId) return;
        if (!sessionIds.includes(targetId)) return;
        terminalStore.setActiveWorkgroupBriefIfActive(targetId, brief);
      })
    );
```

The race guard inside `setActiveWorkgroupBriefIfActive` is intentional double-locking: between event emit and dispatch the user could have switched sessions, in which case the active id no longer matches `targetId` and the setter no-ops. The membership check on `sessionIds` is the primary gate; the active-id check is the secondary one for the normal-mode race.

### 5.6 MODIFIED: `src/sidebar/App.tsx`

**Import** — extend the existing IPC import block (currently includes `onSessionGitRepos` at line 16) with `onWorkgroupBriefUpdated`. Add a new import line for `briefFirstLine`:

```typescript
  onWorkgroupBriefUpdated,   // append to existing "../shared/ipc" import
```

```typescript
import { briefFirstLine } from "../shared/markdown";   // new import
```

`projectStore` is already imported in this file (used elsewhere in mount).

**Register the listener** — after the `onSessionGitRepos` block at lines 222-226 and before the `onSessionCoordinatorChanged` block at line 228, insert:

```typescript
    unlisteners.push(
      await onWorkgroupBriefUpdated(({ workgroupPath, brief }) => {
        projectStore.updateWorkgroupBrief(workgroupPath, briefFirstLine(brief));
      })
    );
```

Side benefit (worth knowing): detached terminal windows currently do NOT subscribe to `session_switched` (`terminal/App.tsx:128`), which is why their briefs are stale forever today. The unconditional registration of `onWorkgroupBriefUpdated` in §5.5 means detached windows start receiving live brief updates for the first time.

---

## 6. Dependencies

No new crates. No new npm packages. Used:
- `tokio` (already a dependency).
- `tauri::Emitter` (already used by the watcher).
- `std::fs::metadata`, `std::fs::File`, `std::io::Read::take` (stdlib).
- `std::time::SystemTime` (stdlib).
- `std::path::PathBuf` (stdlib).

---

## 7. Manual end-to-end test

User-visible latency on these tests is **≤ 15 s** (the existing watcher's tick), not the v1 plan's 5 s. Time the assertions accordingly.

1. Start AC with two coordinator sessions in two different workgroups (wg-A and wg-B), both with their projects loaded in `ProjectPanel`.
2. From a third terminal:
   `<bin> brief-set-title --token <wg-A-coord-token> --root <wg-A-coord-root> --title "Smoke A"`
3. **Within ≤ 15 s**: wg-A's terminal `WorkgroupBrief` panel updates to "Smoke A". wg-B unchanged. Sidebar `ProjectPanel` row for wg-A updates to "Smoke A". wg-B's row unchanged.
4. **External editor**: open `<wg-A-root>/BRIEF.md` in Notepad, change the body, save. **Within ≤ 15 s**: the same surfaces re-render with the new body content (terminal shows trimmed full content; sidebar shows first non-empty content line).
5. **Session switch unaffected**: switch the active terminal session to wg-A's coordinator and back to wg-B's. Behavior is unchanged from today (`session_switched` still does its full reload).
6. **Delete BRIEF.md**: remove `<wg-A-root>/BRIEF.md`. Within ≤ 15 s the wg-A terminal panel falls back to "..." and the sidebar wg-A row drops its brief subtitle.
7. **Two sessions, same wg**: open a second session in wg-A (e.g., a member alongside the coordinator). Trigger `brief-set-title` on wg-A. Both terminal panels update on the same tick — `sessionIds` in the payload includes both ids; each window's listener picks the one matching `targetId`.
8. **Detached window**: detach wg-A's coordinator. Switch the main window to wg-B. Externally edit `<wg-A-root>/BRIEF.md`. **Expected**: the **detached** window's `WorkgroupBrief` updates within ≤ 15 s; the main window's terminal does not (its active session is wg-B); the sidebar updates wg-A's row in both windows. This validates the §10.2.2 unconditional-registration fix and is the case that broke before this rev.
9. **Session-only workgroup (no project loaded)**: create a session in a wg-* whose parent project is NOT in `settings.projectPaths`. Edit its BRIEF.md externally. The terminal panel still updates (Gate C's session-walkup branch covers it). The sidebar shows nothing for it (the project isn't loaded — same as today).
10. **Torn-read defense**: while running a stress test that opens BRIEF.md in Notepad, types ~10KB, and saves once per second for 30 s, the watcher must NOT cache a torn read. After the stress test ends, the displayed brief equals the last-saved content (not a half-written interior state).

---

## 8. Notes — things the dev MUST NOT do

- **Do NOT** introduce a new background thread. The user's veto on v1 was specifically against a dedicated `BriefWatcher`; piggybacking on the existing `DiscoveryBranchWatcher` tick is the load-bearing constraint of this rev.
- **Do NOT** call `read_workgroup_brief_for_cwd` inside `SessionInfo::from(&Session)` to "force" a refresh on every `list_sessions` call. That helper already runs there (line 207); the bug is that `SessionInfo::from` is only triggered by IPC commands, not by file change.
- **Do NOT** wire the CLI verb to call back into the running app via the outbox or a sentinel file. The CLI is out-of-process; the watcher's polling already covers the CLI case AND the external-editor case in one mechanism. Multiple emit paths would only race.
- **Do NOT** widen `terminalStore.setActiveSession` to take just a brief — keep `setActiveWorkgroupBriefIfActive` narrow. The undefined-skip semantics on `setActiveSession` (documented at lines 30-36) are load-bearing for `session_renamed` and other partial updates.
- **Do NOT** insert the terminal listener inside `if (!props.embedded)` or `if (!props.lockedSessionId)` (see §5.5). It applies to all modes.
- **Do NOT** flip the cache-update / emit order back to "cache first, then emit" — the emit-then-cache ordering is what makes a transient emit failure recoverable on the next stat-change (see §11.1 in the plan history). If you "simplify" by inverting it, a single failed emit silently loses the update forever.
- **Do NOT** drop the post-read re-stat (the torn-read defense). External-editor saves are NOT atomic — Notepad and VS-Code-with-`atomicSave:false` both do in-place writes (see §11.2). The CLI verb's atomic-rename guarantee covers ONLY the CLI path.
- **Do NOT** drop the 256 KiB read cap. A pathological brief (or `cat /dev/urandom > BRIEF.md`) would otherwise stream through Tauri IPC every 15 s.
- **Do NOT** strip the `\\?\` prefix on the frontend instead of in Rust. Single canonical form on the wire is the simpler invariant; doing it in Rust means the frontend's `normalizePath` becomes defense-in-depth, not load-bearing for matching.
- **Do NOT** add `notify-rs`. Polling matches the existing watcher pattern; the per-tick cost is ≤ 5 `metadata()` calls.
- **Do NOT** call `app.manage(brief_watcher_arc)` for the new functionality — there is no new watcher to manage. The existing `app.manage(discovery_branch_watcher)` at `lib.rs:271` already covers it.
- **Do NOT** shrink the `BriefCacheEntry` to remove the `sentinel` field "since we already store `brief`". The sentinel is the stat-equality short-circuit; without it every tick reads every BRIEF.md unconditionally, which is exactly the cost the user vetoed.
- **Bump `src-tauri/tauri.conf.json#version`** (patch component) on the build that ships this fix — per the user's standing rule about visually confirming new builds.

---

## A. Disposition of round-1 reviewer findings

(Reviews preserved verbatim in §9, §10, §11 below for the historical record.)

| Finding | Severity | v2 disposition |
|---------|----------|----------------|
| §9.2 — `briefFirstLine` greedy strip | correctness | **Folded in** — §5.2 uses regex `^(?:# )+`. |
| §9.3 G1 — listener-vs-`loadActiveSession` ordering | clarity | **Folded in** — §5.5 comment calls this out. |
| §9.3 G2 — listener registered unconditionally | correctness | **Folded in** — §5.5 places it OUTSIDE every conditional, with §10.2.2 as the authoritative pin. |
| §9.3 G3 — emit reaches all webviews; web/WS clients silent | known gap | **No change** — matches existing GitWatcher / DiscoveryBranchWatcher limitation, not a regression. |
| §9.3 G4 — first-tick redundant emit | nit | **No change** — harmless; the listener writes the same string. |
| §9.3 G5 — destroyed-session race | nit | **No change** — destroyed-window cleanup happens before user notices. |
| §9.3 G6 — pre-existing `<Show>` double-strip in ProjectPanel | out of scope | **No change**. |
| §9.4 — reference-preserving reducer | optional | **Not adopted** — consistency with `updateReplicaBranch` wins; flagged for future profiling sweep. |
| §9.5 — additional test cases | testing | **Folded in** — §7.7 (multi-session same wg) and §7.8 (detached window). |
| §10.2.1 — Windows `\\?\` prefix strip | correctness | **Folded in** — `strip_verbatim_prefix` helper in §4.4.2 applied at watcher emit time. |
| §10.2.2 — terminal listener insertion position | correctness | **Folded in** — §5.5 explicitly OUTSIDE every conditional, with the embedded-mode trap called out. |
| §10.2.3 — Mutex-across-await audit | invariant | **Re-verified** in §4.4.2; brief comment in the new code calls out the constraint. |
| §10.4 — build-and-verify gate | process | **Restated** in §10 below; still applies. |
| §11.1 — cache-before-emit critical | correctness | **Folded in** — §4.4.2 mirrors `GitWatcher`'s emit-then-cache ordering; sentinel always refreshed, `brief` only on emit success. |
| §11.2 — external-editor torn-read defense | correctness | **Folded in** — §4.4.2 re-stats after read and defers on mismatch. §1's "atomic" claim revised — applies only to the CLI path. |
| §11.3 — read budget + per-wg timeout | robustness | **Partially folded in** — 256 KiB read cap applied (§4.4.2 `read_brief_capped`). Per-wg `tokio::time::timeout` NOT applied: this is local FS, the existing `DiscoveryBranchWatcher` doesn't apply timeouts to its own metadata calls either, and adding only here would be inconsistent. If a future issue surfaces a stalling read (e.g., NFS mount), wrap the call in `spawn_blocking + timeout` then. |
| §11.4 — drop dormant `app.manage` | hygiene | **Moot** — v2 introduces no new watcher to manage. |
| §11.5 — retain-prune causes redundant emits | low | **Folded in** — v2 does NOT prune by session-membership; a stale entry's next-tick stat is a free `metadata()` short-circuit. Brief comment in §4.3 calls this out. |
| §11.6 — implicit invariant on `read_workgroup_brief_for_cwd(wg_root)` | low | **Folded in** — §4.4.2 calls `read_brief_capped(&brief_path)` directly; no implicit walk-up reliance. |
| §11.7 — BOM passthrough | nit | **Folded in** — `read_brief_capped` strips the BOM in the watcher's emit path. The existing `read_workgroup_brief_for_cwd` is left untouched (its BOM-passthrough is harmless for the `SessionInfo::from` path because both downstream consumers run `stripFrontmatter`). |

---

## 9. Frontend review (dev-webpage-ui) — round 1, historical

I verified the plan against the current source. Summary first, then per-finding detail.

### 9.1 Verifications that passed

- `terminal/App.tsx:48,62,65,133,145,160` — every `setActiveSession` call already passes `workgroupBrief` as the 6th positional arg. The undefined-skip contract on the partial setter (lines 30-36 of `terminal/stores/terminal.ts`) means the new narrow setter is the right shape: a positional widening would force every existing call site to thread an extra arg, churn unrelated tests, and break the contract.
- `sidebar/App.tsx:30` — `projectStore` is already imported, so the §5.6 conditional import-add is unnecessary. Just append `briefFirstLine` to the `shared/markdown` import (a new line — `markdown.ts` is not currently imported in this file).
- `main/App.tsx:213-231` — confirms my mental model of the unified window: `<SidebarApp embedded />` + `<TerminalApp embedded />` mount in the **same** webview, so both `onWorkgroupBriefUpdated` listeners fire on every emit. They write to disjoint stores (`projectStore` vs `terminalStore`) — no conflict, no double-write. This matches how `onSessionDestroyed` is already double-registered in both Apps (terminal/App.tsx:76, sidebar/App.tsx:165), so the plan stays consistent with the established multi-listener pattern.
- `terminal/components/WorkgroupBrief.tsx:6` reads `terminalStore.activeWorkgroupBrief` through a `createMemo` over `stripFrontmatter(...).trim()`. The full-content shape the watcher emits is what this component already expects. CSS at `terminal/styles/terminal.css:189-230` (`white-space: pre-wrap`, `max-height: 120px`, `overflow-y: auto`) handles multi-line briefs correctly today, so a body-update via the watcher renders the same way a session-switch reload would.
- `sidebar/components/ProjectPanel.tsx:843` reads `wg.brief` (already first-line shape produced by `extract_brief_first_line` in `discover_project`), so feeding the listener through `briefFirstLine(brief)` before storing keeps the two write paths in lockstep.
- The `<For each={proj.workgroups}>` keying behavior (SolidJS keys by reference) plus the `updateWorkgroupBrief` reducer that builds a new wg object **only for the matching path** means the affected wg row remounts; siblings keep their references and are not torn down. Reactivity is fine.
- `terminalStore` and `projectStore` are module-level singletons but every Tauri webview is a separate JS context, so detached terminal windows each have their own `terminalStore` instance. The listener writing into "the" store is correctly scoped to the window that registered it.

### 9.2 `briefFirstLine` does not actually mirror `extract_brief_first_line` (correctness — please fix in §5.2)

The plan's TS port uses `line.startsWith("# ") ? line.slice(2) : line` — strips at most one `"# "` prefix. The Rust source uses `l.trim_start_matches("# ").to_string()`, which is **greedy** and strips every repeated `"# "` prefix.

Divergence on inputs like `"# # Title"`:
- Rust → `"Title"`
- TS port (as written) → `"# Title"`

The sidebar would then render the watcher-derived value as `"# Title"` while a fresh `discover_project` (which goes through Rust) would render the same file as `"Title"`. Two write paths, two different results — exactly what the docstring says we are trying to avoid.

Fix: replace the conditional slice with a regex strip so the function is provably equivalent to the Rust greedy variant:

```typescript
export function briefFirstLine(content: string | null | undefined): string | null {
  if (!content) return null;
  const stripped = stripFrontmatter(content);
  for (const raw of stripped.split(/\r?\n/)) {
    const line = raw.trim();
    if (line.length === 0) continue;
    return line.replace(/^(?:# )+/, "");
  }
  return null;
}
```

(Edge case is rare in practice — almost no one writes `"# # Title"` — but the docstring explicitly promises a mirror, so the cheapest path is to make it true.)

### 9.3 Grinches

**G1 — Listener-vs-`loadActiveSession` ordering (terminal/App.tsx).** The plan inserts the listener *after* `loadActiveSession()`. That is the right call and I want to make it explicit so a future refactor does not "tidy" by hoisting the listener to the top: registering before `loadActiveSession()` would let an event fired during the `await SessionAPI.list()` round-trip race with the load and overwrite the freshly-pulled brief with one that is staler than what `loadActiveSession` is about to write. Add a comment by the listener block calling this out.

**G2 — Detached-window listener intentionally registered outside `if (!props.lockedSessionId)`.** Detached `TerminalApp` does not subscribe to `session_switched` (App.tsx:128), which is why detached briefs are stale today. The plan's listener registration happens unconditionally on line 167-173-equivalent insertion point — keep it that way. This is in fact a side-benefit fix: detached windows will start receiving live brief updates for the first time. Worth a single sentence in §1.

**G3 — `app.handle().emit(...)` reaches every webview, including the guide window and any future webviews.** No listener is registered there, so the payload is dropped silently. Fine for now, but if the WS broadcaster ever starts mirroring this event for browser/web clients, do it through the same code path the other watchers use (none of them do today — see `git_watcher.rs` which is also Tauri-only). Browser clients will not receive `workgroup_brief_updated` until that is wired up; this matches the existing limitation for `session_git_repos` so it is **not a regression**, just a known gap.

**G4 — First-poll redundant emit.** On the first tick after app start, `prev` is `None` for every workgroup, so `content_changed = true` and the watcher emits an event whose value is identical to what `loadActiveSession()` already wrote. No bug — the listener overwrites the same string into the signal — but worth knowing if/when somebody adds a `console.log` to the listener and is confused why it fires once at startup.

**G5 — Detached window for a destroyed-but-not-yet-cleaned session.** If the session is destroyed between the watcher's `get_sessions_working_dirs()` snapshot and the event reaching the detached window, `sessionIds` could include the destroyed id. The membership check passes, the setter no-ops because `activeSessionId() === lockedSessionId` is true and the brief gets written into a window that is about to be closed by `onSessionDestroyed`. Harmless; the window destroys before the user notices. No mitigation needed.

**G6 — Minor: existing `<Show when={stripFrontmatter(wg.brief ?? "").trim()}>` in ProjectPanel.tsx:843 is a no-op double-strip** (since `wg.brief` is already first-line, post-frontmatter, post-heading-strip). Pre-existing, not introduced by this plan, do not touch — out of scope.

### 9.4 Optional reducer optimization (will NOT change in this PR)

`updateWorkgroupBrief` (§5.4) and the existing `updateReplicaBranch` both rebuild every project's reference on every event. For N projects, M workgroups, A replicas, every `<For each={projects()}>` re-keys all N items even when only one wg in one project changed. That cascades into wasted DOM diff work for the unaffected projects. A reference-preserving reducer would be:

```typescript
updateWorkgroupBrief(workgroupPath: string, brief: string | null) {
  const normalized = normalizePath(workgroupPath);
  setProjects((prev) =>
    prev.map((proj) => {
      const idx = proj.workgroups.findIndex((wg) => normalizePath(wg.path) === normalized);
      if (idx === -1) return proj; // preserve reference
      const workgroups = proj.workgroups.slice();
      workgroups[idx] = { ...workgroups[idx], brief: brief ?? undefined };
      return { ...proj, workgroups };
    })
  );
}
```

I am **not** adopting this in the implementation because (a) `updateReplicaBranch` (lines 95-110) and `reloadProject` follow the existing always-rebuild pattern, (b) project counts are typically 1-3, and (c) consistency wins over micro-optimization. Flagging it for a separate sweep if profiling ever points here. Same applies symmetrically to `updateReplicaBranch` if/when someone files an issue.

### 9.5 Test plan additions to §7

Adding two cases:

7. **Two sessions in the same wg-A** (e.g. coordinator + a member). Trigger `brief-set-title` on wg-A. Both terminal panels update on the same tick. `sessionIds` in the payload includes both ids; the listener picks the one matching `targetId` for each window (or both, if the user has both windows open).
8. **Detached terminal window for wg-A's coordinator.** With the main window showing wg-B's session, edit `<wg-A-root>/BRIEF.md` externally. **Expected within ≤ 5 s**: the **detached** window's `WorkgroupBrief` updates. The main window's terminal does not (because its active is wg-B). The sidebar updates wg-A's row in both windows. This case validates the §G2 side-benefit and is worth keeping in the regression suite — it's the case that broke before this fix and is the most invisible.

### 9.6 Implementation order I'll follow

When the plan is ratified, I'll apply the frontend pieces in this order:

1. `src/shared/markdown.ts` — add `briefFirstLine` (with the §9.2 regex form).
2. `src/shared/ipc.ts` — declare `onWorkgroupBriefUpdated`.
3. `src/terminal/stores/terminal.ts` — add `setActiveWorkgroupBriefIfActive`.
4. `src/sidebar/stores/project.ts` — add `updateWorkgroupBrief`.
5. `src/terminal/App.tsx` — register listener (with §G1 comment).
6. `src/sidebar/App.tsx` — register listener.
7. `npx tsc --noEmit` to verify.

Backend pieces (§4.1-4.3) are dev-rust's responsibility; I block on the `workgroup_brief_updated` event being live before final verify.


---

## 10. Backend review (dev-rust, 2026-05-06) — round 1, historical

I verified every Rust file path, line number, and code snippet against the current branch (`fix/161-brief-frontmatter-rendering`). The plan's design is sound and matches the existing watcher conventions (`GitWatcher`, `DiscoveryBranchWatcher`). I found one real correctness issue (§10.2.1) and one factual error in §8 that needs revising. Items below are additive to dev-webpage-ui's §9 and do not duplicate it.

### 10.1 Backend verifications passed

- `find_workgroup_brief_path_for_cwd` (`session.rs:126`) and `read_workgroup_brief_for_cwd` (`session.rs:141`) are `pub(crate)` — the new `crate::session::brief_watcher` module can import them directly. No visibility change needed.
- `SessionManager::get_sessions_working_dirs()` exists at `manager.rs:418` returning `Vec<(Uuid, String)>`, matching the snapshot type the plan uses.
- `GitWatcher::start(self: &Arc<Self>, shutdown: ShutdownSignal)` (`git_watcher.rs:49-69`) and `DiscoveryBranchWatcher::start` (`ac_discovery.rs:413`) both use the same shape — `BriefWatcher::start` slots in identically.
- `ShutdownSignal` is `Clone` (`shutdown.rs:19`); passing `shutdown_for_setup.clone()` into `BriefWatcher::start` is correct.
- `lib.rs` line citations all match the current file: `use session::manager::SessionManager;` at line 23, the four `Arc::clone` calls at lines 199-202, the `setup` closure at line 250, the `GitWatcher` block at lines 257-263, and the `DiscoveryBranchWatcher` block starting at line 265.
- `session/mod.rs` is exactly the 4-line file the plan describes (after the comment); appending `pub mod brief_watcher;` is a one-line edit.
- `BRIEF.md` write is `tmp + rename` with retry on Windows AV transients (`brief_ops.rs:537-561`). The watcher's `metadata()` snapshot is therefore always coherent — no torn-read risk.
- `app_handle.emit()` for the new event matches `GitWatcher`'s pattern. As dev-webpage-ui's §9 G3 notes, this means web/WS clients won't receive it — that mirrors the existing GitWatcher / DiscoveryBranchWatcher limitation, not a regression.

### 10.2 Required additions / corrections

#### 10.2.1 — Strip the Windows `\\?\` (verbatim) prefix in Rust before emit (correctness)

**§8's bullet about `normalizePath` is factually wrong on two counts**, and the underlying issue is a real silent-miss class on Windows.

1. The plan claims `project.ts` "re-defines or imports" `normalizePath` from `sessions.ts`. **It re-defines it differently.** Compare:
   - `src/sidebar/stores/sessions.ts:24-26` — `p.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "")` (strips trailing slashes).
   - `src/sidebar/stores/project.ts:17-19` — `p.replace(/\\/g, "/").toLowerCase()` (does NOT strip trailing slashes).

   `updateWorkgroupBrief` calls `project.ts`'s version. Trailing-slash strip is not in play.

2. **Neither `normalizePath` strips a `\\?\` (verbatim/UNC long-path) prefix.** The existing test `find_workgroup_brief_path_handles_unc_prefix_input` at `session.rs:308-317` documents that the prefix flows through unchanged: `find_workgroup_brief_path_for_cwd(r"\\?\C:\...\wg-3-team")` returns a `PathBuf` whose `.parent()` keeps the prefix. After `to_string_lossy().into_owned()`, the watcher emits `\\?\C:\...\wg-3-team`. Meanwhile, `discover_project` populates `AcWorkgroup.path` from a direct `read_dir` walk (`ac_discovery.rs:835`), which never has the prefix. Normalized:
   - Watcher: `//?/c:/.../wg-3-team`
   - Sidebar `wg.path`: `c:/.../wg-3-team`
   - Result: silent miss in `updateWorkgroupBrief`'s `normalizePath` comparison.

   The same test file (`session.rs:309`) explicitly notes that "§9.4 strips the prefix downstream when embedding into the prompt" — the codebase already has precedent for stripping it at use sites.

**Fix:** strip the prefix once, in `BriefWatcher::poll`, before using `wg_root` as either a cache key or an emit value. Add this helper inside `brief_watcher.rs`:

```rust
/// Strip the Windows verbatim/UNC `\\?\` prefix if present so the emitted
/// path matches the form `discover_project` produces (which never has the
/// prefix because it comes from a `read_dir` walk). The codebase already
/// applies the same strip downstream when embedding paths into the agent
/// init prompt — see the `find_workgroup_brief_path_handles_unc_prefix_input`
/// test note in `session.rs:309`.
fn strip_verbatim_prefix(p: &std::path::Path) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| p.to_path_buf())
}
```

Then in `poll()`, replace:
```rust
let wg_root = match brief_path.parent() {
    Some(p) => p.to_path_buf(),
    None => continue,
};
```
with:
```rust
let wg_root = match brief_path.parent() {
    Some(p) => strip_verbatim_prefix(p),
    None => continue,
};
```

Cost: ~10 LoC. Eliminates the entire normalization-divergence class by enforcing a single canonical form on the wire — frontend `normalizePath` becomes defense-in-depth, not load-bearing.

**Also revise §8's `normalizePath` bullet** to reflect reality: drop the "re-defined or imported" wording (the two definitions differ), and note that after the Rust-side strip, frontend normalization is no longer load-bearing for matching.

#### 10.2.2 — Re-emphasis on terminal listener insertion position (complements §9 G2)

dev-webpage-ui's §9 G2 already notes the listener must be registered unconditionally. I want to call out a SECOND conditional that could trap a dev reading the plan literally: §5.5 says "before the theme-sync block at line 177." Lines 175-187 of the current `terminal/App.tsx`:

```tsx
    // Theme sync: follow sidebar theme toggle (redundant in embedded mode —
    // sidebar's toggle already flips the shared documentElement classList).
    if (!props.embedded) {
      unlisteners.push(
        await onThemeChanged(({ light }) => { ... })
      );
    }
```

The theme-sync `onThemeChanged` block is wrapped in `if (!props.embedded)`. A dev who reads "before line 177" and inserts at line 178 (inside the conditional) silently disables brief updates in embedded mode — the dominant UX path (the unified Main window). 

**Action:** insert the `onWorkgroupBriefUpdated` block between line 173 (closing `);` of `onSessionRenamed`) and line 175 (the `// Theme sync` comment), **outside** `if (!props.embedded)` AND outside `if (!props.lockedSessionId)`. The brief listener applies to all modes (normal, detached, embedded). Update §5.5 with a one-line note: *"Insert OUTSIDE every surrounding conditional — the brief listener applies to all modes."*

#### 10.2.3 — Mutex-across-await audit (verified clean, recording for future readers)

`BriefWatcher::poll()` is `async` and acquires `std::sync::Mutex` three times: at line 196 (cache.retain inside a block), line 207 (`self.cache.lock().unwrap().get(&wg_root).cloned()`), and line 233 (`self.cache.lock().unwrap().insert(...)`). In all three cases the lock is held for one statement and released before any `await`. ✓

This is the #1 footgun mixing `tokio` async with `std::sync::Mutex` — holding the lock across an `await` would cause a runtime deadlock under load. The plan got it right; future readers should not "refactor for clarity" by extending the lock scope across `await` boundaries. Worth a brief comment in the new file.

### 10.3 Implementation order I'll follow

When the plan is ratified, backend pieces in this order:

1. `src-tauri/src/session/mod.rs` — add `pub mod brief_watcher;`.
2. `src-tauri/src/session/brief_watcher.rs` — new file (with the §10.2.1 `strip_verbatim_prefix` helper applied).
3. `src-tauri/src/lib.rs` — import + clone + `start` + `app.manage`.
4. `cargo check --manifest-path src-tauri/Cargo.toml` — must be clean.
5. `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` — must be clean.
6. `cargo test --manifest-path src-tauri/Cargo.toml` — full suite; the existing `session::session::tests::find_workgroup_brief_path_*` tests at `session.rs:276-318` must still pass since this PR doesn't modify them.

I block on dev-webpage-ui's frontend pieces being merged before the §7 manual smoke test, because the smoke test verifies end-to-end behaviour that needs both halves wired up.

### 10.4 Build-and-verify gate (must pass before commit)

1. `cargo check` clean.
2. `cargo clippy -- -D warnings` clean.
3. `cargo test` — full suite passes.
4. `npm run build` — TS compiles.
5. **Bump `src-tauri/tauri.conf.json#version`** (patch component) so the user can visually confirm the new build.
6. Run §7 manual smoke test end-to-end (with dev-webpage-ui's §9.5 additions for the multi-session and detached-window cases).

### 10.5 Estimated diff size

- Rust: ~+170 LoC (new `brief_watcher.rs` ~155 LoC + ~10 LoC for `strip_verbatim_prefix` from §10.2.1 + ~5 LoC across `mod.rs` and `lib.rs`).
- No new crates.
- Combined with dev-webpage-ui's frontend ~+50 LoC, the whole feature is well under 250 LoC of code change.


---

## 11. Grinch Review (dev-rust-grinch, 2026-05-06) — round 1, historical

Verified the plan against `repo-AgentsCommander` HEAD (branch `fix/brief-frontmatter-rendering`). Read every Rust file the plan touches plus the helpers it reuses, the existing `GitWatcher` / `DiscoveryBranchWatcher` patterns it claims to mirror, and every frontend touchpoint. dev-webpage-ui's §9 (especially §9.2 on `briefFirstLine` and §9.3 G1/G2) and dev-rust's §10 (especially §10.2.1 on the `\\?\` strip and §10.2.2 on the listener-conditional trap) cover most of what I would have flagged. The findings below are **purely additive to §9 and §10** — I am not restating their findings, and where I disagree I say so explicitly.

I want to block on §11.1 and §11.2 in particular before this lands; the others are robustness/clarity.

### 11.1 — CRITICAL: cache is updated *before* emit, opposite of GitWatcher; a single failed emit silently loses the update forever

**What.** §4.1 of the plan updates `self.cache` **before** `app_handle.emit`, and the emit return value is silenced with `let _ =`. Concretely, lines 233-250 of the plan:

```rust
self.cache.lock().unwrap().insert(
    wg_root.clone(),
    CacheEntry {
        sentinel: now_sentinel.clone(),
        brief: new_brief.clone(),
    },
);

if !content_changed {
    continue;
}

let payload = BriefUpdatedPayload { ... };
let _ = self.app_handle.emit("workgroup_brief_updated", payload);
```

**Why this matters.** `GitWatcher::poll` (`src-tauri/src/pty/git_watcher.rs:131-147`) does the **opposite** order — it emits first, and only inserts into the cache if the emit (and the CAS write) succeeded. That ordering is load-bearing: if the emit returns `Err` (transient Tauri IPC hiccup, all webviews mid-destroy during shutdown, payload serialization edge case), GitWatcher's cache stays at the OLD value, so the next tick sees `cache.get(id) != refreshed` and retries. The plan's order means: emit fails → cache says we already shipped the new content → next tick's `now_sentinel == prev_sentinel` short-circuit fires before the read → **the frontend never receives this brief change until something else mutates the file or the user reloads the project.** A coordinator running `brief-set-title` while the AC main webview is in a transient bad state then sees the title never propagate, with no log line to indicate why (the `let _ =` swallows it).

Note: `metadata()` and the read happen *before* this code path, so the cache write here is the FIRST place where the watcher commits to "we shipped this state". Inverting the order is the right invariant.

**Fix.** Mirror `GitWatcher::poll`'s ordering. Update `sentinel` always (so the next tick's stat-equality short-circuit still fires for unchanged content), but only update `brief` on a successful emit. Concretely:

```rust
// Always refresh sentinel — next tick's stat-equality gate depends on it.
self.cache.lock().unwrap().entry(wg_root.clone())
    .and_modify(|e| e.sentinel = now_sentinel.clone())
    .or_insert_with(|| CacheEntry {
        sentinel: now_sentinel.clone(),
        brief: None, // placeholder — overwritten on successful emit below
    });

if !content_changed {
    continue;
}

let payload = BriefUpdatedPayload {
    workgroup_path: wg_root.to_string_lossy().into_owned(),
    brief: new_brief.clone(),
    session_ids: session_ids.iter().map(|u| u.to_string()).collect(),
};
match self.app_handle.emit("workgroup_brief_updated", payload) {
    Ok(()) => {
        // Commit the content shipped to the frontend.
        self.cache.lock().unwrap().entry(wg_root.clone())
            .and_modify(|e| e.brief = new_brief.clone());
    }
    Err(e) => {
        log::warn!(
            "[BriefWatcher] emit failed for {} ({}); leaving cached brief stale so next stat-change retries",
            wg_root.display(), e
        );
        // Do NOT update `brief` — next change will retry.
    }
}
```

This subsumes my Finding 6 below (silenced emit error) — the `match` makes the failure observable.

### 11.2 — HIGH: external-editor saves are NOT atomic; §10.1's "no torn-read risk" claim is wrong for the case the watcher exists to cover

**What.** §10.1 states: *"BRIEF.md write is `tmp + rename` with retry on Windows AV transients (`brief_ops.rs:537-561`). The watcher's `metadata()` snapshot is therefore always coherent — no torn-read risk."* §1 of the plan makes the same claim: *"The `brief-set-title` verb does an atomic `tmp + rename` (`src-tauri/src/cli/brief_ops.rs:537-561`), so the watcher always observes a coherent pre/post snapshot; partial reads are not a concern."*

**Why this matters.** The atomic-rename guarantee applies **only** to the CLI verbs (`brief_set_title.rs:119` and `brief_append_body.rs:118` both go through `brief_ops::perform`). It does **not** apply to external editor saves, which §1 simultaneously calls out as a primary motivation for polling: *"Polling the file's stat metadata also covers the *external editor* case (a coordinator opening BRIEF.md in VS Code / Notepad and saving), which is part of the requirement."*

Concrete case: Notepad on Windows writes in place — `CreateFile(OPEN_EXISTING)` → `SetEndOfFile` → write new bytes. If the watcher's `read_workgroup_brief_for_cwd` lands during that write window, the read sees a torn file. With small briefs and SSDs the window is sub-millisecond and unlikely to be hit, but if it IS hit, the watcher caches the torn content as the new "stable" value. The torn content's stat (mtime + len) will MATCH the eventual final-state stat in the next tick or two (mtime granularity on NTFS is 100 ns but the rolling stat is captured once per tick and the post-write final stat could equal the captured intermediate stat by coincidence), so the cache holds the torn brief until the file changes again.

Notepad is a real path here: the user's memory note `feedback_brief_md_not_auto_read.md` already documents that coordinators paste content into BRIEF.md as part of the workflow. VS Code's default `files.atomicSave` is `true` but a user CAN disable it; on a network mount the rename can be ignored. The §10.1 sentence "no torn-read risk" needs to be retracted — it covers the CLI path correctly but is false for the external-editor path the plan exists to cover.

**Fix (defensive double-stat).** After the read, re-stat. If `metadata().len()` or `mtime` differs from `now_sentinel`, treat the read as transient: log debug, do NOT update the cache `brief` field, do NOT emit. Next tick's stable stat will yield a clean read. Concretely:

```rust
let new_brief = read_workgroup_brief_for_cwd(&wg_root.to_string_lossy());

// Re-stat after the read. If the file changed during our read window
// (external editor mid-save), the read may be torn. Treat as transient.
let post_sentinel = std::fs::metadata(&brief_path).ok().map(|m| StatSentinel {
    len: m.len(),
    mtime: m.modified().ok(),
});
if post_sentinel != now_sentinel {
    log::debug!(
        "[BriefWatcher] stat changed during read of {} (torn read risk); deferring to next tick",
        brief_path.display()
    );
    continue;
}
```

Cost: one extra `metadata()` call per *changed* workgroup (the stat-equality short-circuit means we only get here when the file actually changed). Negligible.

Also: revise §1 ("partial reads are not a concern") and §10.1 ("no torn-read risk") to reflect that the guarantee holds only for the CLI verb path; the watcher's defense-in-depth covers the editor path.

### 11.3 — MEDIUM: `poll()` runs sync I/O inline and is not cancellable; no read budget

**What.** The poll loop uses `std::fs::metadata` and `std::fs::read_to_string` (via `read_workgroup_brief_for_cwd`) — synchronous, blocking — inside `async fn poll`. The cancellation arm only fires **between** ticks (lines 152-162 of plan §4.1):

```rust
loop {
    tokio::select! {
        biased;
        _ = shutdown.token().cancelled() => break,
        _ = tokio::time::sleep(POLL_INTERVAL) => {
            watcher.poll().await;   // ← runs to completion; not under select
        }
    }
}
```

**Why this matters.** `GitWatcher` (`git_watcher.rs:155-167`) and `DiscoveryBranchWatcher` (`ac_discovery.rs:555-567`) both wrap their detection in `tokio::time::timeout(Duration::from_secs(2), …)` precisely because `git rev-parse` can stall on a network drive. `BriefWatcher` reads from disk and faces the same hazard — a stalled SMB mount, an antivirus stat-blocking the path, a zombie file lock from a crashed editor — and has no timeout. Worse, the read is unbounded in size: a 100MB BRIEF.md (accident, prank, or `cat /dev/urandom > BRIEF.md`) gets streamed into memory, post-trimmed, cloned, and serialized through Tauri IPC every 5s. Shutdown then waits for in-flight `poll().await` to drop; with N stalled paths × inline reads, the runtime won't drop until every read returns.

`brief_ops.rs` already proves the codebase takes this category seriously — `BriefOpError::ReadFailed` exists exactly for "read couldn't complete". The watcher should adopt the same defensive shape.

**Fix (two cheap mitigations, do both).**
1. Wrap the per-workgroup work in a budget. The cleanest shape is `tokio::task::spawn_blocking(move || (metadata(...), read(...)))` wrapped in `tokio::time::timeout(Duration::from_secs(2), ...)`. On timeout, log warn and skip this workgroup this tick.
2. Cap `read_workgroup_brief_for_cwd`'s read at e.g. 256 KiB — `File::open(..)?.take(256 * 1024).read_to_string(..)`. If the brief overflows, log + emit `Some("<brief truncated — see log>")` (or `None`). Prevents an adversarial / accidental giant brief from wedging the IPC channel every 5 s and holds a hard upper bound on payload size.

Optionally: wrap the `poll().await` itself in the outer `tokio::select!` so shutdown can cancel mid-tick. Cost ~5 LoC, brings parity with what every other watcher already does for graceful shutdown.

### 11.4 — MEDIUM: drop the dormant `app.manage(Arc::clone(&brief_watcher))` until something consumes it

**What.** §4.3: *"`app.manage` is included for symmetry with the other watchers, even though no Tauri command currently takes `State<'_, Arc<BriefWatcher>>`. This costs nothing and keeps the option open for an `invalidate_for_session` call site later."*

**Why this matters.** It is not free in the sense that matters here. Tauri's `StateManager` is `HashMap<TypeId, Box<dyn Any>>`; future code that takes `State<'_, Arc<BriefWatcher>>` will resolve silently with no compiler help, exactly the kind of dormant lookup that masks bugs (a developer adds a command later, takes the State, is surprised when the cache appears stale because no invalidation method exists yet — they wire a new one without the existing-pattern review). Role.md is explicit on this style of design ("Don't design for hypothetical future requirements"). DiscoveryBranchWatcher is `app.manage`d (`lib.rs:271`) precisely because it has live invalidation entrypoints today (`update_replicas_for_project`, `invalidate_replicas`); GitWatcher is `app.manage`d (`lib.rs:263`) for the same reason (`invalidate_session_cache`). BriefWatcher has neither and will not until something else changes.

The watcher's spawned thread holds its own `Arc<BriefWatcher>` (line 146 of §4.1: `let watcher = Arc::clone(self);`), so dropping `app.manage` does not risk the watcher being dropped.

**Fix.** Drop the `app.manage(Arc::clone(&brief_watcher))` line in §4.3. When a real consumer needs invalidation, add it then.

### 11.5 — LOW: `cache.retain(|k, _| by_wg.contains_key(k))` causes redundant emits on session churn

**What.** §4.1 lines 195-198:

```rust
{
    let mut cache = self.cache.lock().unwrap();
    cache.retain(|k, _| by_wg.contains_key(k));
}
```

Comment claims this is "so a session re-appearing later re-emits".

**Why this matters.** The re-emit happens **regardless** of whether the brief content actually changed. Concretely: user has session A in wg-A, watcher emits brief X, caches it. User closes A, retain drops the cache entry. User opens session A2 in wg-A (same workgroup, same BRIEF.md, content X unchanged). Next tick: `prev = None` → `content_changed = true` → emits brief X again. The frontend listener fires and re-renders. Harmless on correctness, but in steady state with multiple coordinators churning sessions in one workgroup it produces avoidable IPC traffic and avoidable SolidJS signal flips. Combined with the unconditional registration in dev-rust's §10.2.2 (listener active in all modes including embedded), this is more avoidable churn than it looks.

**Fix.** Don't retain-prune by session-membership. Cache entries cost ~200 bytes each (PathBuf + StatSentinel + Option<String>) and their next-tick read is a free `metadata()` short-circuit. If unbounded growth is a worry, prune on `cache.len() > 256` instead — but no realistic deployment cycles through 256 workgroups in one app session.

### 11.6 — LOW: `read_workgroup_brief_for_cwd(&wg_root.to_string_lossy())` relies on an implicit invariant

**What.** §4.1 line 218: `let new_brief = read_workgroup_brief_for_cwd(&wg_root.to_string_lossy());`. The helper signature is `read_workgroup_brief_for_cwd(cwd: &str) -> Option<String>` (`session.rs:141`).

**Why this matters.** Passing `wg_root` (the BRIEF.md's *parent* — by construction a `wg-*` directory) when the helper expects "any cwd inside the workgroup" works because the helper walks up from the input to the first `wg-*`, and a `wg-*` dir is its own ancestor. Non-obvious from the call site. If a future refactor renames the helper to require the `BRIEF.md` path directly (or an inner cwd), the watcher silently breaks — no compile error.

**Fix.** Prefer `std::fs::read_to_string(&brief_path)` directly inside `BriefWatcher::poll`, trim, filter empty — three lines. `brief_path` is already in scope from `find_workgroup_brief_path_for_cwd(&cwd)`. Removes the implicit "the helper happens to handle wg-roots" assumption and makes the watcher's I/O surface explicit. Alternative: leave a one-line comment in §4.1 explaining why passing a wg-root works.

### 11.7 — NIT: trimmed payload may carry a UTF-8 BOM; both consumers strip it but a third would not

**What.** `read_workgroup_brief_for_cwd` (`session.rs:141-147`) calls `content.trim()`. Rust's `str::trim` follows the Unicode `White_Space` property, which **does not** include `U+FEFF` (BOM). So a BRIEF.md authored by Notepad (which writes a UTF-8 BOM) is emitted to the frontend with the BOM still attached.

**Why this matters.** The terminal path is fine — `WorkgroupBrief.tsx:6` runs `stripFrontmatter` which strips the BOM (`markdown.ts:6`). The sidebar path is fine — `briefFirstLine(content)` calls `stripFrontmatter` first. End-to-end works **only because both consumers happen to call `stripFrontmatter`**. The plan's prose ("matches the existing `read_workgroup_brief_for_cwd` shape") implies the watcher is shipping a clean post-trim string. It isn't quite — it's shipping a BOM-prefixed string when an editor with BOM is involved. Issue #161's whole motivation was BOM-related rendering bugs; we should not relitigate that.

**Fix.** Documentation, not code. Add a one-line note to §4.1 (in the `BriefUpdatedPayload.brief` doc-comment): *"The trimmed payload may begin with a UTF-8 BOM if the source file was BOM-prefixed (Notepad). Both consumers strip it via `stripFrontmatter` (`shared/markdown.ts:6`); a third consumer must do the same."* Or, alternatively, strip the BOM in `read_workgroup_brief_for_cwd` itself and emit a clean string — that is the more robust fix and is local to one helper, but it crosses a function used elsewhere (`SessionInfo::from(&Session)` at `session.rs:207`) and would have a wider blast radius. Document-only is fine.

---

### What I checked and did not flag (additive to §9 and §10's already-clean lists)

- The `tokio::select! { biased; ... }` ordering in `start()` matches `GitWatcher::start` and `DiscoveryBranchWatcher::start`: shutdown polled first, sleep second. Correct — without `biased`, an immediately-cancelled token under load could race the sleep.
- `Arc::clone(self)` capture into the spawned thread (line 146 of plan §4.1) keeps the watcher alive without leaking — when the thread exits on shutdown, the Arc count drops and the watcher can be dropped (assuming Finding 11.4 is taken and `app.manage` is dropped, otherwise Tauri's StateManager keeps it alive until app exit anyway, which is fine).
- The `Mutex::lock().unwrap()` calls do not cross `await` points — confirmed by reading every line of §4.1 — and match the existing pattern in `git_watcher.rs:72,79,118,139,146` and `ac_discovery.rs:367,492,515,546`. Poison panics are theoretically possible but consistent with the codebase's established stance.
- The session-iteration shape (`mgr.read().await` → snapshot tuples → drop guard → do disk I/O) matches both peer watchers' shapes. Lock held only for the duration of cloning N (Uuid, String) tuples — does not starve writers.
- The serialization shape (`#[serde(rename_all = "camelCase")]` on `BriefUpdatedPayload`) matches every other event payload in `git_watcher.rs:24, 31` and `ac_discovery.rs:253, 260`. Frontend `onWorkgroupBriefUpdated`'s inline payload type matches.
- `find_workgroup_brief_path_for_cwd` is `pub(crate)` (`session.rs:126`); accessible from `crate::session::brief_watcher` without a visibility change. `read_workgroup_brief_for_cwd` likewise (`session.rs:141`).

---

**Summary of items I want a fix on before this lands:**

| § | Severity | Theme |
|---|----------|-------|
| 11.1 | CRITICAL | Cache-before-emit ordering inverts GitWatcher's recovery property |
| 11.2 | HIGH | External-editor torn-read defense; correct §10.1's overstatement |
| 11.3 | MEDIUM | `poll()` needs read budget + per-workgroup timeout |
| 11.4 | MEDIUM | Drop dormant `app.manage` |
| 11.5 | LOW | `cache.retain` causes avoidable churn |
| 11.6 | LOW | Implicit invariant on `read_workgroup_brief_for_cwd(wg_root)` |
| 11.7 | NIT | Document BOM passthrough |

§9.2 (briefFirstLine), §10.2.1 (`\\?\` strip), and §10.2.2 (listener-conditional position) are already correctly addressed by the prior reviews and I do not relitigate them.
