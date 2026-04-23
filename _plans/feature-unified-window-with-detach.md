# Feature — Unified Sidebar + Terminal Window with Per-Session Detach

**Branch:** `feature/unified-window-with-detach` (already checked out, clean — only untracked sibling plans).
**Repo:** `repo-AgentsCommander`.
**Issue:** [mblua/AgentsCommander#71](https://github.com/mblua/AgentsCommander/issues/71).
**Version bump:** `0.7.5` → `0.8.0` (minor — UX-breaking layout change).
**Anchored against HEAD:** `60dd162 Merge fix/68-update-team-gitwatcher-state`.

---

## 1. Problem statement

Today AgentsCommander runs two separate top-level Tauri windows (`sidebar` and `terminal`) that the user must place side-by-side via the titlebar Layout menu. Issue #71 replaces that with a **single unified main window** that hosts the Sidebar on the left and the active-session Terminal on the right, separated by a resizable splitter; the user can still **detach individual sessions into floating windows** (one window per detached session, any number simultaneously). Default on launch is unified; multi-detach, re-attach, and state persistence across restarts must all work. Out of scope for this iteration: drag-to-detach gesture and sidebar collapse/overlay mode.

---

## 2. Architecture overview

### 2.1 What stays the same (load-bearing invariants)

- **PTY flow is untouched.** `pty_write` (frontend→backend) and `pty_output` (backend→frontend via `app.emit(...)`) keep broadcast semantics. Client-side session-ID filter in `TerminalView` already routes output to the right xterm instance regardless of window — verified at `src/terminal/components/TerminalView.tsx:224-235`. No change to `src-tauri/src/pty/manager.rs`.
- **TerminalView's per-session xterm cache** (`Map<sessionId, SessionTerminal>` at `TerminalView.tsx:30`) is kept; it's the reason multi-detach Just Works — each window mounts its own TerminalView with its own map, each keeps only the xterm(s) it cares about visible.
- **`SessionManager`**, `create_session`, `destroy_session`, `switch_session`, `list_sessions` — unchanged.
- **`DetachedSessionsState`** (Arc<Mutex<HashSet<Uuid>>>) already exists at `src-tauri/src/lib.rs:31,241`. Its semantics are preserved: membership = "this session currently has its own window". We add `attach_terminal` to remove from the set, mirroring `detach_terminal`.
- **Window destroy cleanup** at `lib.rs:697-717` (strips the detached-prefix, removes from set) stays. It already supports detached windows being closed by any mechanism (X, alt-F4, programmatic).
- **Browser mode** (`src/browser/App.tsx`) continues to serve a combined sidebar+terminal layout; its splitter implementation is the direct template for the new unified Tauri layout.

### 2.2 What changes at the Rust layer

1. **Window creation in `setup()`** (`src-tauri/src/lib.rs:474-509`): create ONE window with label `main` and URL `index.html?window=main` instead of the current two windows (`sidebar` + `terminal`). Drop the `terminal_geometry` + fallback-to-"SideBar Right" logic; use a single `main_geometry` (new field) with sensible defaults.
2. **Commands**:
   - `detach_terminal` (existing, `commands/window.rs:11-92`): change the detached window URL from `?window=terminal&sessionId=<id>&detached=true` to `?window=detached&sessionId=<id>`. Keep label format `terminal-<sessionid-no-dashes>` unchanged for back-compat with the destroy-event cleanup path.
   - `attach_terminal(session_id)` **(NEW)**: closes the detached window for `session_id`, removes the UUID from `DetachedSessionsState`, switches the main-terminal pane to that session (via `SessionManager::switch_session`), emits `terminal_attached` + `session_switched`.
   - `close_detached_terminal` (existing, `commands/window.rs:195-213`): retained but becomes a **pure window-close helper** used internally by `destroy_session_inner`. The user-facing "close the detached window" path flows through `attach_terminal` instead (closing a detached window **re-attaches** rather than terminates the session — see §4.Q6).
   - `ensure_terminal_window` (existing, `commands/window.rs:108-159`): **renamed** to `focus_main_window`, behavior simplified — show + focus the `main` window; recreate if missing. No more "only if sessions exist" guard (the main window is always shown while the app is running).
3. **`destroy_session_inner`** (`commands/session.rs:736-743`): remove the "hide terminal window when no sessions remain" branch. The main window stays visible (sidebar is still usable for creating/opening sessions); the embedded terminal pane shows an empty placeholder.
4. **Persistence**: add `was_detached: bool` to `PersistedSession` (`src-tauri/src/config/sessions_persistence.rs:16`). Set from `DetachedSessionsState` in `snapshot_sessions` (line 304). On restore, after creating each session's PTY, if `was_detached` is true, spawn its detached window via the same `WebviewWindowBuilder` call used by `detach_terminal`.
5. **Settings** (`src-tauri/src/config/settings.rs:34`): add `main_sidebar_width: f64` (default 240.0) and `main_geometry: Option<WindowGeometry>`. Keep `sidebar_geometry` and `terminal_geometry` as deprecated read-only fields for one version (so in-flight migrations don't break); they stop being written on the next save (see §6 Migration).

### 2.3 What changes at the frontend layer

1. **Routing** (`src/main.tsx`): new `?window=main` branch mounts a new `MainApp` (sidebar + terminal with splitter). `?window=detached&sessionId=<id>` mounts `TerminalApp` with `lockedSessionId` + `detached=true` (same component, new URL). Legacy `?window=sidebar` and `?window=terminal` redirect to `?window=main` for one version to survive an old window-state restoration (defensive; shouldn't trigger after §6 migration, but cheap insurance).
2. **New component `src/main/App.tsx`**: sidebar on the left (fixed width from `mainSidebarWidth` setting), draggable vertical splitter, terminal pane on the right. Reuses `SidebarApp` and `TerminalApp` as direct children with new props to skip their window-geometry and zoom initializers (those become main-level concerns). Based on `src/browser/App.tsx:12-73` but persists splitter width via `SettingsAPI.update` (debounced 500ms).
3. **Titlebar** — main window gets ONE titlebar (not two). The sidebar's current `Titlebar` (`src/sidebar/components/Titlebar.tsx`) is repurposed as the main-window titlebar: keep the Layout dropdown but change its options to "Sidebar 200 / 280 / 360" presets that just set `mainSidebarWidth` (the side-selection UX becomes moot with one window). The terminal's current `Titlebar` (`src/terminal/components/Titlebar.tsx`) is used only for **detached** windows; the Show-Sidebar button is replaced with a **Re-attach** button (icon: U+21B5 "↵" or `&#x2934;`).
4. **Stores**: `sessionsStore` and `terminalStore` stay as-is. Both are global singletons (module-level signals), so when sidebar + terminal render in the same window they both subscribe to the same store via the same event listeners — SolidJS doesn't duplicate listeners because each listener is registered once in its component's `onMount` (sidebar + terminal sit in separate components; this is fine, but we need to audit for double-subscription — see §7.4).
5. **IPC API** (`src/shared/ipc.ts`): add `WindowAPI.attach(sessionId)` → `invoke("attach_terminal", {sessionId})` and `onTerminalAttached(callback)`. Rename `WindowAPI.ensureTerminal()` → `WindowAPI.focusMain()` (back-compat alias kept for one version).
6. **SessionItem context menu** (`src/sidebar/components/SessionItem.tsx:430-458`): add a new "**Open in new window**" option (when session is not detached) and "**Re-attach to main**" option (when detached). The existing `session-item-detach` button (line 370-375) stays as a one-click detach shortcut; rename its title to "Open in new window". Toggle icon/title between detach and attach based on `detached` state (read via `sessionsStore.isDetached(id)` — new helper backed by a reactive set — see §5.TS).

### 2.4 IPC contract summary

| Direction | Event / Command | When | Payload |
|---|---|---|---|
| FE → BE (existing) | `detach_terminal(sessionId)` | User clicks detach button / context menu | `sessionId: string` → `windowLabel: string` |
| FE → BE (**NEW**) | `attach_terminal(sessionId)` | User clicks re-attach button / context menu / closes detached window via X | `sessionId: string` → `()` |
| FE → BE (renamed) | `focus_main_window()` | Sidebar-click raise-main UX | `()` → `()` |
| BE → FE (existing) | `terminal_detached` | After detach | `{sessionId, windowLabel}` |
| BE → FE (**NEW**) | `terminal_attached` | After attach | `{sessionId}` |
| BE → FE (existing) | `pty_output` | PTY produces data | `{sessionId, data: number[]}` — **broadcast, client filters** |
| BE → FE (existing) | `session_switched` | After switch | `{id: string \| null}` |

The **pty_output remains broadcast**. Every window's `TerminalView` receives every session's chunks and routes via `terminals.get(sessionId)` — chunks for sessions the window doesn't own are no-ops. This is already the behavior today for the current two-window split; adding N detached windows doesn't change the contract. See §4.Q5 for the rationale vs `emit_to`.

---

## 3. Splitter + layout mechanics (design detail)

The unified main window is a single `display: flex; flex-direction: row;` container:

```
┌───────────────────────── main window (decorations: false) ────────────────────────┐
│  ┌──────────────┐   ┌─┐   ┌───────────────────────────────────────────────────┐  │
│  │  sidebar     │   │D│   │                                                    │  │
│  │  - titlebar  │   │i│   │  terminal area                                     │  │
│  │  - actionbar │   │v│   │  - titlebar (session name, detach button)         │  │
│  │  - sessions  │   │i│   │  - LastPrompt                                      │  │
│  │  - ...       │   │d│   │  - TerminalView (xterm host)                       │  │
│  │              │   │e│   │  - StatusBar                                       │  │
│  │              │   │r│   │                                                    │  │
│  └──────────────┘   └─┘   └───────────────────────────────────────────────────┘  │
│   width:             4px    flex: 1 (fills remaining)                             │
│   mainSidebarWidth   grab   min-width: 300px                                     │
│   (clamped 200-600)                                                               │
└───────────────────────────────────────────────────────────────────────────────────┘
```

Implementation model lifted from `src/browser/App.tsx:16-33`:

```tsx
const [sidebarWidth, setSidebarWidth] = createSignal(settings.mainSidebarWidth);
const [dragging, setDragging] = createSignal(false);

const onMouseDown = (e: MouseEvent) => {
  e.preventDefault();
  setDragging(true);
  const onMove = (m: MouseEvent) => {
    const w = Math.max(200, Math.min(600, m.clientX));
    setSidebarWidth(w);
  };
  const onUp = () => {
    setDragging(false);
    document.removeEventListener("mousemove", onMove);
    document.removeEventListener("mouseup", onUp);
    persistWidth(sidebarWidth()); // debounced 500ms via SettingsAPI.update
  };
  document.addEventListener("mousemove", onMove);
  document.addEventListener("mouseup", onUp);
};
```

Key differences vs `BrowserApp`:
- Width is persisted via `SettingsAPI.update` (debounced 500ms — match the `debouncedSave` pattern at `src/shared/window-geometry.ts:22-35`).
- Initial width read from `AppSettings.mainSidebarWidth` via `SettingsAPI.get()` on mount.
- Clamp band is 200-600px logical (matches BrowserApp). Min-main-width check: if the window is narrower than `sidebarWidth + 300`, cap sidebar at `windowWidth - 300`. Prevents the terminal pane from disappearing on narrow windows.

CSS resize handling: inner `TerminalView` already listens via `ResizeObserver` (see `TerminalView.tsx:217-222`) and calls `syncViewport` → `fitAddon.fit()` + `PtyAPI.resize()` on every layout change. Dragging the splitter naturally resizes the terminal pane and triggers the PTY resize. Zero extra plumbing needed for PTY resize.

---

## 4. Answers to the 8 open questions

### Q1 — Re-attach flow

**Decision: two symmetric triggers matching detach, plus close-as-attach.**

- **Button in the detached window's titlebar**: icon `&#x2934;` (↴), title "Re-attach to main". Handler: `WindowAPI.attach(sessionId)`.
- **Sidebar context menu on a detached session**: "**Re-attach to main**" entry (mutually exclusive with "Open in new window"; both share the same row toggle).
- **Closing the detached window (X)**: re-attach (NOT destroy). Rationale: the window is a view on the session, not the session itself; muscle-memory "X to dismiss the popup" should never kill an agent. This is the single most important UX invariant — if the user wanted to terminate the session they would use the sidebar's close button on that session row.

**How "X = re-attach" is implemented**: the detached window's `close_requested` event is intercepted (`getCurrentWindow().onCloseRequested(e => { e.preventDefault(); WindowAPI.attach(sessionId); })`) so the default close is swapped for an attach. `destroy_session_inner` still closes the detached window directly (bypassing the handler, via `win.close()`) when the session is actually destroyed from the sidebar — need to verify that `close()` in Tauri v2 skips `onCloseRequested`; if it doesn't, use `win.destroy()` in destroy paths. **Dev verification item** — flagged in §7.

### Q2 — State persistence

**Decision: remember detached sessions across restarts.** Persisted via a new `was_detached: bool` field on `PersistedSession` (`src-tauri/src/config/sessions_persistence.rs:16`), populated from `DetachedSessionsState` at snapshot time. On app restart, after the existing `create_session_inner` recreates each session's PTY, the restore loop at `lib.rs:528-614` spawns a detached window for any `ps.was_detached = true`. If the session fails to restore, `was_detached` is ignored (session never becomes live, so detaching is moot).

**Rationale**: users who detach typically do so for monitor/ergonomics reasons — losing that state each restart creates friction. The cost is one extra bool per session row in `sessions.json` — trivial.

**Edge case**: if the saved `was_detached` session's PTY fails to spawn (e.g. CWD missing), drop the detached intent silently. The session goes into the `failed_recoverable` bucket and may be retried later; when retried, it comes back non-detached. Acceptable — detach is an ephemeral UI preference.

### Q3 — Main window active session when detached

**Decision: switch to next non-detached session; if none, show empty-state placeholder.**

Already implemented for the current two-window model in `detach_terminal` (`commands/window.rs:58-89`): pick the first session whose UUID is not in `DetachedSessionsState` and `switch_session` to it; emit `session_switched` with `null` when no candidate exists. This logic carries over verbatim. The main window's TerminalView already renders an empty placeholder when `terminalStore.activeSessionId` is null (see `TerminalApp.tsx:153-175`).

**Rationale**: the sidebar always gives the user a way out of the empty state (click another session, create a new one, re-attach a detached one). Picking "next non-detached" preserves workflow continuity.

### Q4 — Custom titlebar for detached windows

**Decision: reuse the existing `src/terminal/components/Titlebar.tsx` with one modification.**

That titlebar already handles `detached: boolean` (renders a DETACHED badge at lines 82-83, hides the Show-Sidebar button on line 98-102). We replace the Show-Sidebar button with a **Re-attach** button when `props.detached` is true. The main window uses the **sidebar's** Titlebar component (`src/sidebar/components/Titlebar.tsx`) as the single titlebar for the whole main window; the Layout dropdown stays but its options become "Sidebar 200 / 280 / 360" width presets (no more left/right split).

**Why not build a third simplified titlebar**: the terminal Titlebar is already the right shape for a session-focused window (session name, version, DETACHED badge, min/max/close). Adding a re-attach button is a single element; duplicating the whole component is strictly worse.

### Q5 — IPC targeting (broadcast vs emit_to)

**Decision: keep broadcast, client-side filter.** No change to `src-tauri/src/pty/manager.rs:448` (`app_handle.emit("pty_output", payload)`).

**Reasoning**:
- **The current design is already multi-window safe.** `TerminalView.tsx:224-235` routes output via `terminals.get(sessionId)` — unknown session ids are no-ops. Detached window = a second `TerminalView` with its own map holding only the locked session; main window = `TerminalView` with a map of all active-in-main sessions. Both receive every event; both filter cheaply.
- **Switching to `emit_to`** would require (a) maintaining a session→window-label map (new shared state), (b) locking that map on every PTY read-loop tick (a hot path — tens of KB/s per active agent), (c) special-casing "session is in main window OR in its own detached window" — with complexity spike for the "active session in main" case where the target label changes on every switch. Net: more complexity, more lock contention, zero user-visible benefit.
- **Browser mode parity**: WsTransport already broadcasts everything to every connected browser client with client-side filtering. Keeping Tauri on the same model means both transports share the same test surface.
- **Bandwidth concern is real but bounded**: an idle detached window with an inactive session still receives pty_output events for other sessions that are active. Each event is `{sessionId: 36 chars, data: [...]}`; the serialize + cross-process hop is wasted work. Measured ballpark: 50KB/s per active agent × 3 active agents × 3 windows = 450KB/s of duplicated frames. That's fine on modern hardware (Tauri's IPC is in-process via WebView postMessage for Tauri 2.x, so it's a memcpy, not a network hop). If it ever becomes a problem, `emit_to` can be added in a follow-up without changing the TerminalView contract.

### Q6 — Detached window lifecycle

**Decision: closing a detached window re-attaches the session to the main window. It does NOT terminate the session.** Implementation via `onCloseRequested` handler that calls `attach_terminal` — see Q1 above. Destroying the session from the sidebar (existing `destroy_session` command) continues to close the detached window as a side effect (existing logic at `commands/session.rs:722-726`).

**Rationale**: X-to-kill would be destructive and ambiguous — the Sidebar's per-session close button is the one source of truth for "terminate this session". Preserving that invariant is worth the one-time cost of implementing the close-requested handler.

### Q7 — Frontend routing

**Decision: the scheme the tech lead proposed, with a one-version back-compat redirect.**

| Query param | Component | Description |
|---|---|---|
| `?window=main` | `MainApp` (NEW) | Unified sidebar + terminal with splitter |
| `?window=detached&sessionId=<id>` | `TerminalApp` (reuse) | Locked to one session, DETACHED |
| `?window=guide` | `GuideApp` (unchanged) | Existing help window |
| `?window=sidebar` | **Redirect** `→ ?window=main` | Legacy from pre-migration settings |
| `?window=terminal` | **Redirect** `→ ?window=main` | Legacy from pre-migration settings |
| (no param, Tauri) | `MainApp` | Matches old "Tauri default: sidebar" fallback |
| (no param, browser) | `BrowserApp` | Unchanged |

Redirect implementation: in `src/main.tsx` before the render dispatch, replace the URL with the canonical form and continue with the new type — no reload loop, no user-visible blink.

### Q8 — Focus/sync behavior

**Decision: clicking a detached session in the sidebar focuses its window — does NOT re-attach.** Already implemented in `switch_session` (`commands/session.rs:905-923`), which focuses the detached window when the session is in `DetachedSessionsState`. We keep that behavior exactly.

**Rationale**: clicking the session item is a lightweight selection action; yanking the window back to main on every click would fight users who deliberately keep a session detached on another monitor. Re-attach is a deliberate gesture (context menu or the re-attach button on the detached titlebar).

**Sidebar `activeId` semantics**: the sidebar's `sessionsStore.activeId` continues to track the user's last-clicked session regardless of detach state. The visual "active" highlight on the row is driven by `activeId`. The main terminal pane's `activeSessionId` is driven by the backend's `SessionManager::active` (which advances only when switch succeeds, i.e. when the session is NOT detached). These two can diverge intentionally: "I'm looking at the detached session in its own window, but my sidebar row still shows I clicked it last". That's the correct UX.

---

## 5. File-level impact map

All paths relative to `repo-AgentsCommander/`. Line numbers anchored to HEAD `60dd162`.

### 5.1 Rust (new / modified)

| File | Lines | Change type | Summary |
|---|---|---|---|
| `src-tauri/src/lib.rs` | 30-31 | keep | `DetachedSessionsState` stays; add doc note that it now backs both window-tracking + persistence |
| `src-tauri/src/lib.rs` | 426-509 | **rewrite** | Replace dual `sidebar` + `terminal` WebviewWindow setup with a single `main` window: label `"main"`, URL `"index.html?window=main"`, geometry from `main_geometry`, default size 1400×900 logical, min 800×500 |
| `src-tauri/src/lib.rs` | 608-614 | modify | After session restore, iterate `persisted` and for each `ps.was_detached == true`, invoke `detach_terminal` inner-fn (extract from `commands/window.rs:11-92` into a reusable `detach_terminal_inner`) |
| `src-tauri/src/lib.rs` | 653-657 | modify | Replace `commands::window::detach_terminal / close_detached_terminal / ensure_terminal_window` in `invoke_handler!` with `detach_terminal / attach_terminal / focus_main_window / close_detached_terminal` (keep `close_detached_terminal` as an internal-only command — remove from `invoke_handler!` since it's no longer called from frontend) |
| `src-tauri/src/commands/window.rs` | 11-92 | modify | `detach_terminal`: change URL to `?window=detached&sessionId=<id>`. Extract spawn-window logic into `pub(crate) async fn detach_terminal_inner(app, mgr, detached, session_id)` callable from both the command handler and the restore path in lib.rs |
| `src-tauri/src/commands/window.rs` | N/A | **NEW** | `pub async fn attach_terminal(app, mgr, detached, session_id)`: removes from DetachedSessionsState, closes the detached window (via `win.close()` — the window's close-handler is for user X only; programmatic close skips it via `win.destroy()` if necessary — see Q6 note), calls `mgr.switch_session(uuid)`, emits `terminal_attached` + `session_switched` |
| `src-tauri/src/commands/window.rs` | 105-159 | rename + modify | `ensure_terminal_window` → `focus_main_window`: drops the "only if sessions exist" guard; always show + focus the `main` window; recreate via `WebviewWindowBuilder` with label `"main"` + URL `?window=main` if missing |
| `src-tauri/src/commands/window.rs` | 193-213 | keep | `close_detached_terminal` stays as internal helper — used by `destroy_session_inner`. Remove from public `invoke_handler!` |
| `src-tauri/src/commands/session.rs` | 530-535 | remove | Delete the "Show the terminal window when a session is created" branch — no more hidden terminal window; `main` is always visible |
| `src-tauri/src/commands/session.rs` | 736-743 | remove | Delete the "hide terminal window when no sessions remain" branch. Main window stays open |
| `src-tauri/src/commands/session.rs` | 722-726 | keep | Still closes detached window when session is destroyed. Unchanged |
| `src-tauri/src/commands/session.rs` | 905-923 | keep | `switch_session` still focuses detached window when session is in DetachedSessionsState. Unchanged |
| `src-tauri/src/config/settings.rs` | 32-109 | modify | Add `main_sidebar_width: f64` (default 240.0), `main_geometry: Option<WindowGeometry>`. Mark `sidebar_geometry` and `terminal_geometry` with `#[serde(default, skip_serializing_if = "Option::is_none")]` and a `// deprecated, read-only until v0.9` comment so next save drops them |
| `src-tauri/src/config/settings.rs` | 139-176 | modify | `AppSettings::default()`: add `main_sidebar_width: 240.0, main_geometry: None`; keep `sidebar_geometry / terminal_geometry` at `None` (never written fresh) |
| `src-tauri/src/config/sessions_persistence.rs` | 14-56 | modify | Add `#[serde(default)] pub was_detached: bool` to `PersistedSession` |
| `src-tauri/src/config/sessions_persistence.rs` | 304-343 | modify | `snapshot_sessions`: read `DetachedSessionsState` (new parameter), set `was_detached` per session. Signature becomes `pub async fn snapshot_sessions(mgr: &SessionManager, detached: &DetachedSessionsState) -> Vec<PersistedSession>` |
| `src-tauri/src/config/sessions_persistence.rs` | 616-633 | modify | `persist_current_state` + `persist_merging_failed` take the extra `detached` arg and forward it to `snapshot_sessions` |
| `src-tauri/src/lib.rs` | 177-222, 517-528, 716-735 | modify | Update all call sites of `snapshot_sessions / persist_current_state / persist_merging_failed` to thread `DetachedSessionsState`. Grep check: also `src-tauri/src/commands/session.rs:638-640, 895-899` and `src-tauri/src/commands/window.rs` — all need the arg |
| `src-tauri/tauri.conf.json` | 4 | modify | Version bump `0.7.5` → `0.8.0` |
| `src-tauri/Cargo.toml` | 3 | modify | Version bump `0.7.5` → `0.8.0` |

### 5.2 Frontend (new / modified)

| File | Change type | Summary |
|---|---|---|
| `src/main.tsx` | modify | Replace the `windowType` switch (lines 20-39) with the new dispatcher (`main` / `detached` / `guide` / legacy redirects). Route `?window=main` → `<MainApp/>`, `?window=detached&sessionId=<id>` → `<TerminalApp lockedSessionId={id} detached/>` |
| `src/main/App.tsx` | **NEW** | Unified sidebar+terminal layout with splitter. Based on `src/browser/App.tsx`. Mounts `<SidebarApp embedded/>` + splitter + `<TerminalApp embedded/>`. Persists splitter width via `SettingsAPI.update` debounced 500ms. Owns the window-level geometry init (moved from sidebar/terminal) |
| `src/main/styles/main.css` | **NEW** | `.main-layout` flex row, `.main-sidebar-pane` fixed width from signal, `.main-divider` 4px grab handle with hover highlight, `.main-terminal-pane` flex:1. Dragging state disables text selection globally (`.main-dragging * { user-select: none; }`) |
| `src/sidebar/App.tsx` | modify | Add optional `embedded?: boolean` prop. When `embedded === true`: skip `initWindowGeometry`, skip `initZoom` (main owns those), skip the `applyWindowLayout` on-startup call (line 85-88 — no longer relevant with single window), skip the `handleRaiseTerminal` mousedown handler (line 79, line 201 — no separate terminal window to raise) |
| `src/terminal/App.tsx` | modify | Add optional `embedded?: boolean` prop with same semantics (skip geometry + zoom when embedded). In detached mode (`lockedSessionId + detached`, not `embedded`) the existing behavior stands |
| `src/terminal/components/Titlebar.tsx` | modify | Replace the Show-Sidebar button (lines 99-101, only rendered when `!props.detached`) with a **Re-attach** button (only rendered when `props.detached`). Re-attach handler calls `WindowAPI.attach(lockedSessionId)`. Pass `lockedSessionId` through the component (new optional prop) |
| `src/sidebar/components/Titlebar.tsx` | modify | The Layout dropdown (lines 68-89) now offers three sidebar-width presets ("Narrow 200 / Default 280 / Wide 360") that write `mainSidebarWidth` to settings. Remove `applyWindowLayout` usage (import + the `handleLayout` function); replace with direct `SettingsAPI.update({...s, mainSidebarWidth: N})` |
| `src/sidebar/components/SessionItem.tsx` | modify | Context menu (lines 430-458): add "**Open in new window**" when session is not detached, "**Re-attach to main**" when detached. Detach/attach check via new `sessionsStore.isDetached(id)` accessor. The existing detach button (lines 369-375) toggles its icon+title based on `isDetached(session.id)`: `&#x29C9;` "Open in new window" vs `&#x2934;` "Re-attach to main" |
| `src/sidebar/stores/sessions.ts` | modify | Add reactive `detachedIds: Set<string>` signal + methods `isDetached(id) / setDetached(id, bool) / clearDetached()`. Populated by listeners on `terminal_detached` and `terminal_attached` events (wired in `SidebarApp.onMount`) |
| `src/shared/ipc.ts` | modify | Add `WindowAPI.attach(sessionId)`, `WindowAPI.focusMain()` (rename of `ensureTerminal`; keep `ensureTerminal` as a deprecated alias that calls `focusMain` for one version). Add `onTerminalDetached(cb)` and `onTerminalAttached(cb)` listener helpers |
| `src/shared/types.ts` | modify | Extend `AppSettings`: add `mainSidebarWidth: number` and `mainGeometry: WindowGeometry \| null`. Keep `sidebarGeometry` and `terminalGeometry` as optional for one version |
| `src/shared/window-geometry.ts` | modify | Extend `WindowType` to `"sidebar" \| "terminal" \| "main"`. Map `"main"` to a new settings key `mainGeometry`. Keep the old mappings as legacy read paths (saved values become write-to-`mainGeometry`-only after migration) |
| `src/shared/window-layout.ts` | **delete call site usage** | The `applyWindowLayout` function is unused after removing its caller in Sidebar.onMount (line 85-88) and in Sidebar Titlebar. The file itself can stay (trivial), but delete its imports. Mark the function as unused / TODO-delete in v0.9 |
| `src/sidebar/components/OnboardingModal.tsx` (or any file calling `applyWindowLayout`) | modify | Audit for any other callers via grep. Only remove the call; do not refactor surrounding code |
| `src/browser/App.tsx` | keep | No change — remote browser UX is unchanged. Possibly refactor the splitter logic into a shared helper in a follow-up, but **out of scope for this iteration** (minimum diff) |
| `package.json` | modify | Version bump `0.7.5` → `0.8.0` (matches Cargo.toml + tauri.conf.json per project convention at `CLAUDE.md:120`) |

### 5.3 Files explicitly NOT touched

- `src-tauri/src/pty/manager.rs` — PTY read loop + emit stay exactly as-is.
- `src-tauri/src/pty/idle_detector.rs` — unchanged.
- `src-tauri/src/session/manager.rs` — unchanged.
- `src-tauri/src/commands/pty.rs` — unchanged.
- `src/terminal/components/TerminalView.tsx` — unchanged. Its per-session xterm cache is exactly what we need.
- `src/terminal/components/LastPrompt.tsx`, `StatusBar.tsx` — unchanged.
- All `src/sidebar/components/*` beyond Titlebar, SessionItem, and store — unchanged.

---

## 6. Data model changes

### 6.1 Rust — `AppSettings` (src-tauri/src/config/settings.rs)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    // ... existing fields ...

    // NEW — unified window
    /// Width of the sidebar pane inside the main window, in logical px.
    /// Clamped to [200, 600] at drag time. Persisted on drag-end.
    #[serde(default = "default_main_sidebar_width")]
    pub main_sidebar_width: f64,

    /// Saved geometry for the main window (replaces terminal_geometry's role).
    #[serde(default)]
    pub main_geometry: Option<WindowGeometry>,

    // DEPRECATED — read-only, not written after migration.
    // Kept for one version so first-boot-after-upgrade can seed main_geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_geometry: Option<WindowGeometry>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_geometry: Option<WindowGeometry>,

    // ... rest unchanged ...
}

fn default_main_sidebar_width() -> f64 { 240.0 }
```

### 6.2 Rust — `PersistedSession` (src-tauri/src/config/sessions_persistence.rs)

```rust
pub struct PersistedSession {
    // ... existing fields ...

    /// True if this session was in DetachedSessionsState at snapshot time.
    /// On restore, a detached window is spawned for this session after its
    /// PTY boots successfully. Defaults to false for back-compat with old
    /// sessions.json files.
    #[serde(default)]
    pub was_detached: bool,

    // ... rest unchanged ...
}
```

### 6.3 TypeScript — `AppSettings` (src/shared/types.ts)

```typescript
export interface AppSettings {
  // ... existing ...
  mainSidebarWidth: number;
  mainGeometry: WindowGeometry | null;
  sidebarGeometry: WindowGeometry | null; // deprecated, retained for one version
  terminalGeometry: WindowGeometry | null; // deprecated, retained for one version
  // ... rest unchanged ...
}
```

### 6.4 TypeScript — `sessionsStore` (src/sidebar/stores/sessions.ts)

New reactive set exposing detach state to components without prop drilling:

```typescript
const [detachedIds, setDetachedIds] = createSignal<Set<string>>(new Set());

export const sessionsStore = {
  // ... existing ...
  isDetached: (id: string) => detachedIds().has(id),
  setDetached: (id: string, detached: boolean) => {
    const next = new Set(detachedIds());
    detached ? next.add(id) : next.delete(id);
    setDetachedIds(next); // new reference → re-run subscribers
  },
  clearDetached: () => setDetachedIds(new Set()),
};
```

Populated via new listeners in `SidebarApp.onMount`:
```typescript
unlisteners.push(await onTerminalDetached(({ sessionId }) =>
  sessionsStore.setDetached(sessionId, true)));
unlisteners.push(await onTerminalAttached(({ sessionId }) =>
  sessionsStore.setDetached(sessionId, false)));
unlisteners.push(await onSessionDestroyed(({ id }) =>
  sessionsStore.setDetached(id, false))); // cleanup on destroy
```

On fresh mount, hydrate via a one-shot list call. Since `DetachedSessionsState` is authoritative in the backend, expose it via a new `list_detached_sessions` command **only if needed** — §7 will verify whether the event-driven fill-in is sufficient given the restore-path already fires `terminal_detached` after spawning the detached window (see §6.5 migration note).

### 6.5 Settings migration (first boot after upgrade)

`load_settings` (`src-tauri/src/config/settings.rs:299-340`) gains a one-time migration branch right after parsing, BEFORE the `root_token` auto-gen step:

```rust
// One-time migration: if main_geometry is None but terminal_geometry was
// set under the legacy two-window model, use terminal_geometry's bounds
// as the seed for main (terminal was the bigger of the two).
if settings.main_geometry.is_none() {
    if let Some(ref g) = settings.terminal_geometry {
        settings.main_geometry = Some(g.clone());
        log::info!("[settings-migration] seeded main_geometry from legacy terminal_geometry");
    }
}
// Intentionally do NOT clear sidebar_geometry / terminal_geometry here —
// skip_serializing_if drops them on the NEXT save automatically.
```

The legacy `sidebar_geometry` is discarded by design — it was a separate window's bounds and has no useful translation to the single-window layout.

For existing users: on first launch after upgrade, the main window appears where their old terminal window was, sized like it was. Their sidebar window simply doesn't get created. Non-obvious UX delta, but predictable — the terminal was the dominant window, so honoring its position is the least surprising choice.

### 6.6 Restored detached windows — event emission

Per §2.2.4 the restore path spawns detached windows after session PTY boot. Those spawns go through `detach_terminal_inner`, which emits `terminal_detached` — so the frontend's `detachedIds` set hydrates naturally from events during startup. No separate "initial detached list" round-trip needed.

---

## 7. Migration & backward compatibility

1. **Settings (`~/.agentscommander/settings.json`)**: the old file deserializes without error because both deprecated fields are still present with `#[serde(default)]`. The `load_settings` migration branch seeds `main_geometry` from `terminal_geometry`. After the first `save_settings` call (happens on any settings mutation, e.g. first splitter drag), the deprecated fields are dropped from disk via `skip_serializing_if`. One version later we delete the fields entirely.
2. **Sessions (`~/.agentscommander/sessions.json`)**: old files deserialize with `was_detached = false` for every entry. No detached windows are auto-spawned, which matches prior behavior (there was no detach persistence before). Zero user impact.
3. **In-flight windows** from a previous version's running process: single-instance mutex (`lib.rs:51-78`) prevents co-existence, so a v0.7.5 process must fully exit before v0.8.0 starts. No cross-version window state to reconcile.
4. **External URLs**: the web remote server URL still points at `?window=sidebar` (`lib.rs:299`). Update to `?window=main` — legacy redirect handles old browser tabs.
5. **Browser-mode `BrowserApp`** is unaffected. The `isTauri` branch in `main.tsx` (line 33-39) is what we rewire; the non-Tauri branch still serves `BrowserApp` unchanged.
6. **Single-instance mutex name** in `config/profile.rs` — unchanged. No reason to invalidate other instances' mutex; this feature doesn't change instance-identity semantics.

---

## 8. Test plan (manual, post-implementation)

Run `npm run kill-dev` then `npm run tauri dev` from `repo-AgentsCommander/`. Execute each case in order; a failure aborts the checklist until fixed.

### 8.1 Unified mode — golden path

1. First launch on a clean profile: main window appears at default 1400×900. Sidebar is 240px wide on the left, terminal pane fills the rest. No separate sidebar window. No separate terminal window.
2. Create a session via the sidebar ActionBar. Terminal pane populates with the new session's xterm; xterm is focused. Type a command — it executes.
3. Create a second session. Sidebar shows both; the new one becomes active; terminal pane shows its xterm.
4. Click the first session in the sidebar. Terminal pane switches back. Second session's xterm is hidden but its buffer is retained (verify by switching back — history intact).

### 8.2 Splitter drag

5. Grab the vertical divider between sidebar and terminal. Drag left — sidebar narrows, terminal pane widens. xterm resizes smoothly. Drag right — sidebar widens, terminal pane shrinks.
6. Drag below the clamp (to the far left): sidebar stops at 200px; terminal can't shrink below (window width - 300).
7. Drag above the clamp (to the far right): sidebar stops at 600px OR at (window width - 300), whichever is smaller.
8. Release drag. After ≤500ms, the width is persisted. Close app, relaunch — sidebar width is restored.

### 8.3 Single detach → re-attach

9. Right-click a session in the sidebar → "Open in new window". A new floating window appears with just the terminal, titlebar says "DETACHED", session name visible. The original session's xterm instance is torn down in the main window (or simply hidden? — verify per TerminalView's `disposeSessionTerminal` logic, should be kept alive).
10. Verify xterm inside detached window is live: type a command, see output. Main window's terminal pane auto-switched to the next available session (or shows empty state if none).
11. Click the re-attach button in the detached titlebar (`↴`). Detached window closes. Main window's terminal pane switches to that session. PTY output continues without interruption.
12. Detach the same session again. This time close via window X. Detached window closes. Main window's terminal pane switches to that session. PTY output continues uninterrupted.
13. Detach the same session again. From sidebar context menu, select "Re-attach to main". Same behavior as step 11.

### 8.4 Multi-detach

14. Create three sessions. Detach session A. Main shows session B or C.
15. Detach session B. Now two detached windows (A + B). Main shows session C.
16. Detach session C. Three detached windows. Main shows empty-state placeholder ("No active session").
17. Each detached window renders its own session's output independently. Simultaneous activity in all three: all three scrollback histories update without cross-contamination.
18. Click session A in the sidebar. Detached window A is focused (not re-attached). Main pane stays in empty state.
19. Re-attach session A via context menu. A returns to main; B and C stay detached.
20. Close window B via X. B re-attaches to main. BUT main is currently showing A — so session-switch semantics kick in: A stays active, B is now available in the sidebar but not displayed. Clicking B switches main to B. (Verify that re-attach of non-primary session does NOT steal focus from main's current session — this is the "re-attach closes the window, doesn't force-switch" contract.)

Wait — re-read the contract. §4.Q1 says re-attach = close window + `SessionManager::switch_session(sessionId)`. That force-switches main to the re-attached session, which conflicts with step 20's expectation. **Decision: re-attach DOES force-switch main to the re-attached session**, because the user's gesture ("bring this session back") most naturally means "show it". If the user wants to keep main on A while un-detaching B, they can click A in the sidebar immediately after. Update step 20:

20. *(revised)* Close window B via X. B re-attaches AND becomes the main-window's active session (main was on A, now on B). A's xterm still lives in main's cache. Click A in sidebar — main switches back to A immediately. Both histories intact.

### 8.5 Close session while detached

21. Detach session A. Click the session X button in the sidebar for session A. PTY is killed, sidebar row vanishes, detached window closes. No zombie window.
22. Repeat with a session whose PTY is currently outputting rapidly. Detached window closes cleanly without hanging the app.

### 8.6 Restart with detached session

23. Create three sessions. Detach A. Quit app (File menu / close main window). Relaunch.
24. On restore: main window appears at saved position+size+splitter. Sessions A, B, C restore (PTY boots). After A's PTY boots, a detached window spawns for A at its previous position+size (saved via the detached window's own geometry tracking — §3 TODO: confirm detached windows get geometry tracking wired; see §9 open dev-question).
25. A's detached window shows A's agent immediately on the line where it left off (or wherever provider auto-resume puts it). B and C are in the main pane.

### 8.7 Edge cases

26. **Session whose CWD no longer exists**: marked as failed_recoverable (`lib.rs:533-536`). `was_detached` is ignored — no detached window spawned. Session appears as exited on next restore attempt once CWD is back. ✓
27. **All sessions exited at once**: main window does NOT hide (regression check on the removed `destroy_session_inner` branch). Sidebar stays usable. Terminal pane shows empty state.
28. **Create session with main window maximized**: xterm fills available terminal pane. Splitter is still draggable.
29. **Detach a session, then resize the main window**: detached window is independent — doesn't resize. Main's terminal pane resizes + fits correctly.
30. **Keyboard shortcuts (Ctrl+N new, Ctrl+W close)**: behave identically to before. No regression.
31. **Telegram bridge attached to a session**: detach the session. Telegram bridge stays attached (bridge is bound to session, not window). Messages flow. Re-attach — no change. ✓
32. **Voice recording active on a session**: detach. Recording continues (voice state is session-keyed, not window-keyed). Stop + transcribe works inside the detached window's sidebar — wait, the sidebar doesn't exist in detached windows. **Implication**: the mic button lives in the sidebar's SessionItem. Detached window has no mic control. OK, that's fine — mic is always initiated from sidebar. Verify the recording's transcription appears in the detached window's xterm (PTY injection is window-agnostic).
33. **Zoom**: Ctrl+= / Ctrl+-: zoom persists per-window. Main's zoom is `mainZoom` (new setting? or `sidebarZoom` reused?) — **decision: reuse `sidebarZoom` for the main window and `terminalZoom` for detached windows, mapped via `initZoom` type param**. The `guideZoom` stays separate. Verify both work independently.
34. **Dark/light theme toggle**: still syncs across main + detached windows via `theme_changed` event. No change.
35. **Narrow window (800×500 min)**: sidebar 200px + terminal ≥300px = 500px minimum. Window min-width honors this.

### 8.8 Legacy-setting compat

36. Copy an old `settings.json` (with `sidebar_geometry` + `terminal_geometry`, no `main_geometry`) into `~/.agentscommander/`. Launch. Main window appears at old terminal_geometry's position+size. After first save (e.g. drag splitter), reopen the JSON — `sidebar_geometry` and `terminal_geometry` are gone; `main_geometry` is populated.
37. Copy an old `sessions.json` (no `was_detached` field on any entry). Launch. All sessions restore; none detached. No errors in the log.

### 8.9 Build checks

38. `cd src-tauri && cargo check` — clean.
39. `cd src-tauri && cargo clippy` — no new warnings.
40. `cd src-tauri && cargo test` — existing tests pass; new `snapshot_sessions` signature is exercised by existing callers (compiler catches missed migrations).
41. `npx tsc --noEmit` — clean.
42. `npm run tauri build` — produces a bundle that launches correctly on a clean Windows machine.

---

## 9. Implementation phases (ordered, demoable early)

Each phase lands as its own commit inside the `feature/unified-window-with-detach` branch. Phase 1 is the minimum testable deliverable; phases 2-4 round out the feature without breaking phase 1.

### Phase 1 — Single main window, detach still works as today (≈60% of the diff)

**Goal**: demo the unified layout. No state persistence of detach across restarts. No re-attach gesture yet (X still behaves as close; explicitly accept that X-to-close = session termination for this phase **ONLY if close_detached_terminal still terminates, which it doesn't currently — so this phase is actually safe to ship with X=window-close=session-stays-alive-but-window-gone; re-attach comes in Phase 2**).

Work:
1. Rust: rewrite `lib.rs` setup to a single `main` window; add `main_sidebar_width` + `main_geometry` to `AppSettings`; add the settings migration; remove the "hide terminal window" branches in `destroy_session_inner` + `create_session_inner`.
2. Frontend: `src/main.tsx` routing; `src/main/App.tsx` + CSS; embed flag on Sidebar + Terminal; replace Sidebar's Layout dropdown with width presets.
3. `detach_terminal` URL change to `?window=detached&sessionId=<id>`.
4. Version bump to 0.8.0-rc1 (or 0.8.0 directly if we don't tag prereleases).
5. Manual test §8.1, §8.2, §8.7, §8.8, §8.9.

**Ship bar**: unified main window works, splitter drags + persists, detach opens a new window, closing detached window doesn't kill the session (session stays live, user can recreate detached window via another detach). Known gap: no re-attach gesture, no restore-with-detached.

### Phase 2 — Re-attach flow + `attach_terminal` command (≈20%)

Work:
1. Rust: add `attach_terminal` command; update `invoke_handler!`; wire `close_requested` handler in detached window (frontend: `TerminalApp.onMount` when `detached`).
2. Frontend: Re-attach button on detached Titlebar; "Re-attach to main" context menu option; `WindowAPI.attach`; `onTerminalAttached` listener helper; `sessionsStore.detachedIds` set + subscribers.
3. Manual test §8.3, §8.4.

**Ship bar**: all re-attach paths (button, menu, X) work.

### Phase 3 — Persistence across restarts (≈15%)

Work:
1. Rust: `was_detached` field on `PersistedSession`; thread `DetachedSessionsState` through `snapshot_sessions` + `persist_current_state` + `persist_merging_failed` (and every call site); extract `detach_terminal_inner` and call from the restore loop.
2. Frontend: no change — the restore path emits `terminal_detached` per session, which hydrates `sessionsStore.detachedIds` through existing listeners.
3. Manual test §8.6.

**Ship bar**: detached windows survive restarts.

### Phase 4 — Polish + edge cases (≈5%)

Work:
1. Verify session destroy closes the detached window without firing `onCloseRequested` (Tauri 2.x `win.close()` vs `win.destroy()` — §7.3 answer).
2. Audit grep for any remaining `applyWindowLayout` call sites; remove; leave the function file as unused (delete in v0.9).
3. Confirm the `open_web_remote` URL uses `?window=main`.
4. Run §8.5 (close while detached), §8.7 (all remaining edge cases).
5. Screenshots + final GIF demo for the PR description.

**Ship bar**: all of §8 passes. Feature is ready for grinch bug hunt + shipper build.

---

## 10. What the dev must NOT do

- Do NOT change `src-tauri/src/pty/manager.rs`. Specifically, do NOT switch `app_handle.emit("pty_output", ...)` to `emit_to(label, ...)` — see §4.Q5.
- Do NOT remove `DetachedSessionsState`. It's the authoritative source for "which sessions are detached"; the frontend's `detachedIds` set is a derived cache.
- Do NOT merge Sidebar and Terminal into a single component. Keep them composed; both already work via their own stores.
- Do NOT delete `src/browser/App.tsx`. The web remote UX depends on it; unifying their splitter implementations into a shared helper is a follow-up.
- Do NOT change the `terminal-<sessionid>` label format for detached windows. The window-destroy cleanup at `lib.rs:697-717` parses this exact format.
- Do NOT introduce a new crate for the splitter. Vanilla CSS + SolidJS signals are sufficient.
- Do NOT persist splitter width to `localStorage`. Use `settings.json` via `SettingsAPI.update` — matches project convention (`CLAUDE.md` §Coding Standards — "no localStorage").
- Do NOT rename `session_created / session_destroyed / session_switched` events or the `pty_output` event. Third-party listeners (the WS broadcaster at `web/broadcast.rs`) depend on the existing names.
- Do NOT bump version in `package.json` to something different from `src-tauri/tauri.conf.json`. Project rule: all three (package.json, Cargo.toml, tauri.conf.json) stay in sync per `CLAUDE.md` §Versioning.
- Do NOT skip the `cargo check` + `tsc --noEmit` steps after each phase. Threading `DetachedSessionsState` through the persistence helpers touches a lot of call sites.
- Do NOT auto-spawn the detached windows in parallel with session restore. Spawn them **after** each session's PTY is live (mirror the order used by the existing restore loop). Spawning before the session exists causes `detach_terminal_inner` to find no session and no-op.

---

## 11. Open dev-questions (to be resolved during review)

These are genuinely-open design sub-questions that should be pinned down in the dev-rust / dev-webpage-ui enrichment round, not by the architect:

1. **Tauri 2.x `win.close()` vs `win.destroy()` semantics** — does `close()` fire `onCloseRequested`? If yes, destroy paths (`destroy_session_inner` at `commands/session.rs:722-726`) need to use `destroy()` to skip the re-attach intercept. If no, `close()` is fine. Short test inside the feature branch will resolve it. See Q6 / §7.1.
2. **Geometry tracking for detached windows** — currently `initWindowGeometry` is wired only for `sidebar / terminal` types. Detached windows have multiple instances so a single-key `mainGeometry` doesn't fit. Proposal: add a new per-session `detached_geometry: HashMap<sessionId, WindowGeometry>` to `AppSettings`, keyed by sessionId. Saved on drag/resize of each detached window, read at spawn time (Phase 3 restore). This is a legitimate addition to §6.1 if tech-lead agrees; flagging as open so dev-rust can decide on Phase 3 scope. (My bias: yes, add it; per-session position memory is the main reason to persist detach state.)
3. **Zoom mapping**: the plan proposes main=`sidebarZoom`, detached=`terminalZoom`. Alternative: introduce `mainZoom` + drop `sidebarZoom`/`terminalZoom` (one version later). For MVP keep the reuse; cleanup is a follow-up. Flagging for grinch.
4. **SolidJS store subscription audit when Sidebar + Terminal render in same document** — each store's `onMount` registers event listeners. With both components mounted in the same window, each emitted Tauri event fires **both** `sidebarApp`'s listener AND `terminalApp`'s listener. This is by design (they react to different concerns) — but worth a grep-level check that no store has a "listener registered per mount, never cleaned up" leak under rapid re-mount. Dev-webpage-ui verification item.
5. **`focus_main_window` command and the sidebar-click raise-main UX**: Sidebar's `handleRaiseTerminal` (`SidebarApp.tsx:47-60`) currently raises the separate terminal window. In unified mode, clicking inside the sidebar can't "raise" the terminal pane because it's already in the same window. The whole handler becomes a no-op and should be deleted when `embedded===true`. But if `embedded===false` (user somehow opens a standalone sidebar — which shouldn't happen post-migration), the handler would try to raise a non-existent terminal. Safest: delete the handler entirely from `SidebarApp`. Keep `focus_main_window` for the programmatic "raise main on create-session" use case at `create_session_inner` — except we already removed that (`commands/session.rs:531-535`). So `focus_main_window` may actually have no callers. **If so, delete the command.** Dev-rust verification item.

Resolving these open questions is the main purpose of the dev-enrichment round; none of them block the macro-architecture.

---

## Dev-webpage-ui enrichment (round 1)

Verified against HEAD `60dd162` on `feature/unified-window-with-detach`. The plan is largely accurate; findings below are **additive** — drift corrections, answers to Q11.3 / Q11.4, and frontend-specific risks the architect could not see from the component-level view.

### DW.1 — Path / line-number verification

Checked every path and line reference in §2, §3, §5, §6 against the actual files. Clean, with these exceptions:

| Plan reference | Actual | Action |
|---|---|---|
| §5.2 row: "`src/sidebar/components/OnboardingModal.tsx` — Audit for any other callers" | `OnboardingModal.tsx` does **not** call `applyWindowLayout` or `ensureTerminal`. Grep is clean. | **Remove the OnboardingModal row from §5.2.** Only two callers of `applyWindowLayout` exist: `src/sidebar/App.tsx:25,86` and `src/sidebar/components/Titlebar.tsx:3,27`. Audit closed. |
| §5.2 row: "`src/shared/ipc.ts` — rename `ensureTerminal` + keep deprecated alias" | The approach is correct, but §5.2 **does not enumerate the 9 call sites**. Back-compat aliasing means no code change to callers, but behavior semantics shift (was "raise terminal window", becomes "focus main window"). | List them explicitly so the shipper knows which surfaces to smoke-test: `src/sidebar/App.tsx:56` (deleted with `handleRaiseTerminal`), `src/sidebar/components/SessionItem.tsx:93`, `src/sidebar/components/ProjectPanel.tsx:112, 126, 156, 170, 192, 1358`, `src/sidebar/components/RootAgentBanner.tsx:15`. In unified mode all but the first become near-no-ops (`focusMain` when main is already focused) — harmless; keep the alias. |
| §2.3.4 claim: "SolidJS doesn't duplicate listeners because each listener is registered once in its component's `onMount`" | True, but **misses the bigger auditable surface** — document-level `addEventListener` on shared globals (keydown/wheel/mousedown/contextmenu). See DW.5 below. | Augment the plan's subscription-audit with the document-level listeners, not just per-component Tauri event listeners. |
| §4.Q5 claim: "Tauri's IPC is in-process via WebView postMessage for Tauri 2.x, so it's a memcpy, not a network hop" | Hand-wavy but **directionally correct**. The actual bandwidth-concern sentence is fine; no correction needed. | None. |
| §11.Q3 says "main=`sidebarZoom`, detached=`terminalZoom`" is MVP-acceptable reuse | I strongly disagree — see DW.6 below. | Upgrade the decision to introduce a new `mainZoom` field in Phase 1, not Phase 4. |
| §11.Q4 asks for the SolidJS subscription audit | Done below in DW.5. | None (this is my deliverable). |

Minor line-number nits (do not block review; flagging for precision):
- §2.2.1 cites `lib.rs:474-509` for window setup; the two `WebviewWindowBuilder` calls span 474-497. Line 498-509 is adjacent setup (event emit, etc.). Range is fine as-is.
- §4.Q4 cites "lines 82-83" for Terminal Titlebar DETACHED badge; actual is `src/terminal/components/Titlebar.tsx:81-83` (the `<Show>` wrapper starts on 81). Trivial.
- §4.Q4 cites "line 98-102" for the Show-Sidebar button hide condition; actual is 98-102 — correct as stated (the `<Show when={!props.detached}>` + button markup).

### DW.2 — `embedded` prop contract must be explicit

Plan §2.3.3 + §5.2 describe `embedded?: boolean` on `SidebarApp` and `TerminalApp` but enumerate only part of what it toggles. For the implementer to have no guesswork, lock down the full list:

**When `embedded === true`, `SidebarApp`:**
- Skip `<Titlebar/>` render (currently line 208). The main window owns a single unified titlebar.
- Skip `initZoom("sidebar")` (line 65). Zoom is main-window level — see DW.6.
- Skip `initWindowGeometry("sidebar")` (line 66). Geometry is main-window level.
- Skip `applyWindowLayout("right")` at line 85-88. (Plan already says this.)
- Skip `document.addEventListener("mousedown", handleRaiseTerminal)` at line 79 **AND** delete the `handleRaiseTerminal` body — dead code once the terminal is in the same window.
- Skip `setAlwaysOnTop(true)` at lines 75-78. Always-on-top becomes a main-window property — see DW.3.
- **Keep** `document.addEventListener("contextmenu", blockContextMenu)` at line 82. Blocking native context menu is still desired in unified mode (native menu is never used in our UX; xterm.js doesn't register a right-click handler on Windows, so no regression).

**When `embedded === true`, `TerminalApp`:**
- Skip `<Titlebar/>` render (line 151).
- Skip `initZoom("terminal")` (line 64). Zoom is owned by main. (Applies even with DW.6's `mainZoom`; see there.)
- Skip `initWindowGeometry("terminal")` (line 65).
- Skip the `onThemeChanged` listener at line 131-139 — redundant in the same document. Sidebar's theme toggle already flips `document.documentElement.classList` locally; the terminal listener would re-apply the same class. (In DETACHED mode, keep it.)
- Keep all session-event listeners (`onSessionSwitched`, `onSessionCreated`, `onSessionRenamed`, `onSessionDestroyed`) — see DW.5.

**Why it matters:** embedded ≠ detached ≠ standalone. There are three modes. Enumerating each avoids the "works on my machine, broken in detached" class of bugs.

### DW.3 — `data-tauri-drag-region` placement in unified titlebar

The sidebar's current `Titlebar` component places `data-tauri-drag-region` on the root `.titlebar` div (line 52) and internal brand elements (lines 53, 55, 58, 62, 65). Controls (`.titlebar-controls` at line 68) do **not** have the drag region — correct, since they are click targets.

**Gap**: Plan §2.3.3 says the sidebar's Titlebar is "repurposed as the main-window titlebar", but the sidebar titlebar's DOM only renders brand elements on its left. In the unified main window the titlebar must span the **full window width**, so the empty middle area must also be draggable, and the structural layout must keep the titlebar **outside** of both `.main-sidebar-pane` and `.main-terminal-pane` — otherwise the titlebar's width collapses to the sidebar pane's width.

**Concrete structure for `src/main/App.tsx`**:

```tsx
<div class="main-root">  {/* flex column, fills window */}
  <MainTitlebar />       {/* spans 100% width, data-tauri-drag-region */}
  <div class="main-body">  {/* flex row */}
    <div class="main-sidebar-pane"><SidebarApp embedded/></div>
    <div class="main-divider" onPointerDown={...} />
    <div class="main-terminal-pane"><TerminalApp embedded/></div>
  </div>
</div>
```

Corresponding CSS obligation in `src/main/styles/main.css`:

```css
.main-root { display: flex; flex-direction: column; height: 100vh; }
.main-body { display: flex; flex-direction: row; flex: 1; min-height: 0; /* CRITICAL */ }
.main-sidebar-pane { flex: 0 0 auto; /* width set via inline style from signal */ overflow: hidden; }
.main-divider { flex: 0 0 4px; cursor: col-resize; background: var(--divider-bg, rgba(255,255,255,0.08)); }
.main-divider:hover { background: var(--divider-bg-hover, rgba(0, 212, 255, 0.25)); }
.main-terminal-pane { flex: 1 1 0; min-width: 300px; overflow: hidden; }
```

**Why `min-height: 0`:** the default `min-height` on flex items is `auto` (= content size), which defeats `flex: 1` when child contains xterm's `canvas`. Without `min-height: 0`, the terminal pane will overflow vertically on first xterm render. This is the classic flex trap; worth spelling out so the implementer doesn't rediscover it.

### DW.4 — Splitter UX hardening beyond the BrowserApp template

`src/browser/App.tsx:16-33` is a correct **starting point** but has three gaps that become painful in a native window:

**(a) Cursor flickers during drag.** The 4px divider loses `:hover` the moment the cursor moves faster than the divider reflows. User sees the cursor stutter from `col-resize` back to default mid-drag. Fix: set `document.body.style.cursor = "col-resize"` at pointer-down and reset on pointer-up. Tiny code, big perceptual difference.

**(b) Drag can get stuck outside the webview.** If the cursor leaves the webview's bounds (e.g. overshoots into the OS chrome on a maximized window), `pointermove`/`pointerup` fire on the outer shell, not the document. Use Pointer Events with `setPointerCapture` on the divider element. Shape:

```tsx
const onPointerDown = (e: PointerEvent) => {
  const divider = e.currentTarget as HTMLElement;
  divider.setPointerCapture(e.pointerId);
  document.body.style.cursor = "col-resize";
  setDragging(true);

  const onMove = (m: PointerEvent) => {
    const raw = m.clientX;
    const windowWidth = window.innerWidth;
    const w = Math.max(200, Math.min(600, Math.min(raw, windowWidth - 300)));
    setSidebarWidth(w);
  };
  const onUp = (u: PointerEvent) => {
    divider.releasePointerCapture(u.pointerId);
    document.body.style.cursor = "";
    setDragging(false);
    divider.removeEventListener("pointermove", onMove);
    divider.removeEventListener("pointerup", onUp);
    divider.removeEventListener("pointercancel", onUp);
    persistWidth(sidebarWidth());  // see DW.7 for debounce ownership
  };
  divider.addEventListener("pointermove", onMove);
  divider.addEventListener("pointerup", onUp);
  divider.addEventListener("pointercancel", onUp);  // critical: capture lost
};
```

`setPointerCapture` + `pointercancel` together guarantee the drag ends cleanly even if the webview loses focus.

**(c) xterm hijacks the drag as text selection.** When the cursor crosses into `.terminal-host` mid-drag, xterm's canvas renderer begins a text-selection. Fix: while `dragging()` is true, set `.main-terminal-pane { pointer-events: none; }` (or `.terminal-host`). Release on pointer-up. Also add `user-select: none` on `body` during drag to suppress any text selection inside the sidebar pane (React-to-Solid muscle memory may try).

**CSS additions:**
```css
.main-root.main-dragging { user-select: none; cursor: col-resize; }
.main-root.main-dragging .terminal-host { pointer-events: none; }
```

Reasoning: the splitter is **the** new interaction introduced by this feature. If it feels sticky or janky, users will blame the whole unification. Spend the ~20 lines to make it feel solid.

### DW.5 — Answer to §11.Q4: SolidJS listener-subscription audit (unified mode)

Enumerated every `document.addEventListener` and Tauri `listen` call currently registered across `SidebarApp`, `TerminalApp`, `TerminalView`, and their direct descendants. Risk categorized:

| Listener | Registered at | Behavior in unified mode | Action |
|---|---|---|---|
| Tauri `session_created` | `SidebarApp.tsx:121` + `TerminalApp.tsx:92` | Both handlers fire in same document. Sidebar adds to store; Terminal sets active-if-first. Distinct responsibilities — both required. | **Keep both.** Document that cross-firing is intentional. |
| Tauri `session_destroyed` | `SidebarApp.tsx:131` + `TerminalApp.tsx:107` + `TerminalView.tsx:237` | Three handlers. Sidebar removes from store; Terminal reloads active-or-closes-detached; TerminalView disposes xterm. Distinct responsibilities — all three required. | **Keep all three.** |
| Tauri `session_switched` | `SidebarApp.tsx:137` + `TerminalApp.tsx:72` | Both required. | Keep both. |
| Tauri `session_renamed` | `SidebarApp.tsx:143` + `TerminalApp.tsx:123` | Both required (sidebar updates row; terminal updates titlebar-visible name in `terminalStore`). | Keep both. |
| Tauri `theme_changed` | `TerminalApp.tsx:132` | **Redundant in embedded mode.** Sidebar's theme-toggle mutates `document.documentElement.classList` — same document, same effect. Terminal's listener then calls `classList.add/remove` on the identical element. Idempotent but wasteful; also doubles up on theme-timed animation reflows. | **Skip when `embedded===true`.** Keep for DETACHED. |
| Document `contextmenu` (blockContextMenu) | `SidebarApp.tsx:82` | Blocks default context menu globally. In unified mode, this now also covers the terminal pane. xterm.js does not register a right-click handler on Windows (verified — copy/paste is ctrl+c/v in xterm). No regression. | Keep. |
| Document `mousedown` (handleRaiseTerminal) | `SidebarApp.tsx:79` | Dead code when embedded (plan removes). | Remove in embedded mode (delete function body too). |
| Document `keydown` (shortcuts) | `SidebarApp.tsx:64` + `TerminalApp.tsx:63` via `registerShortcuts()` | **Already de-duped** inside `src/shared/shortcuts.ts:38-45` — module-level `activeHandler` guard returns no-op on second registration. The code comment at line 38 literally says: "Prevent duplicate registration when SidebarApp + TerminalApp coexist in BrowserApp." Unified window = BrowserApp-style coexistence; the guard already applies. | **No change needed** — existing guard covers us. Add a comment in `shortcuts.ts:38` updating the explanation to mention main window too. |
| Document `wheel`/`keydown` (initZoom) | `SidebarApp.tsx:65` + `TerminalApp.tsx:64` | If both zoom init paths run (plan's proposed main=`sidebarZoom`), BOTH handlers register. Ctrl+= fires both; both call `applyZoom` with independent closure state. **Race: `currentZoom` drifts between them; `setZoom` gets called twice with different values.** Visible as zoom lag / wrong scale. | **Fix via DW.6** (single `mainZoom` + skip `initZoom` in embedded terminal). With DW.2's explicit skip, only one `initZoom` registers in main; no race. |
| Window `onMoved`/`onResized` (initWindowGeometry) | `SidebarApp.tsx:66` + `TerminalApp.tsx:65` | In unified mode both would write different `AppSettings` keys for the **same** Tauri window — racing saves. | Plan already says skip when embedded. ✓ |
| `ResizeObserver` on `hostRef` | `TerminalView.tsx:217` | One observer per mounted TerminalView. Unified mode: one in main, one per detached window. No cross-firing. Splitter drag fires it rapidly (see DW.4 + note on double-rAF below). | Keep. Monitor perf on slow VMs. |
| Inner `document click` (layout dropdown outside-click) | `sidebar/components/Titlebar.tsx:40` | Only relevant while dropdown is open. In unified mode the dropdown options change (plan §2.3.3 — width presets instead of layout sides), but the outside-click teardown pattern is unchanged. | Keep. |
| Window `click/contextmenu/keydown` (SessionItem context menu dismissers) | `SessionItem.tsx:182-184` | Per-item, only attached while menu is open, cleaned in `onCleanup`. Correct under rapid re-mount. | Keep. |

**Leak check** (components that register listeners inside `onMount` and must clean in `onCleanup`):
- `SidebarApp` — ✓ (unlisteners array + global listeners cleaned at line 196-203)
- `TerminalApp` — ✓ (line 142-147)
- `TerminalView` — ✓ (line 258-266, including disposing every xterm in `terminals` map)
- `SessionItem` — ✓ (cleanupContextMenu at line 138-145, registered via `onCleanup(cleanupContextMenu)`)
- sidebar `Titlebar` — ✓ (inner `onCleanup` at line 41)
- terminal `Titlebar` — No document listeners, clean.

**Conclusion:** no leak risks in current code. The unified-mode danger is **double-firing**, not leaking. DW.2 (skip redundant initializers) + DW.6 (single `mainZoom`) close the two concrete races. The `shortcuts.ts` guard at line 38-45 already defuses the keyboard-shortcut doubling for free.

### DW.6 — Answer to §11.Q3: Zoom mapping — **add `mainZoom` now, not later**

Plan proposes reusing `sidebarZoom` for main and `terminalZoom` for detached, with "cleanup is a follow-up" for introducing `mainZoom`. I recommend inverting that: **introduce `mainZoom` in Phase 1**, keep `terminalZoom` for detached windows, deprecate `sidebarZoom`.

**Reasoning:**

1. **Semantic clarity**: Ctrl+= in the unified main window scales the **whole** document — sidebar icons, session rows, terminal font, the works. A setting called `sidebarZoom` that controls the entire main window is a lie waiting to burn a future maintainer. The field name must match what it does.

2. **Implementation cost is minimal**: `src/shared/zoom.ts:9-15` uses a `zoomKeyMap` dict. Adding `main` is two lines:
   ```ts
   type WindowType = "sidebar" | "terminal" | "main" | "guide";
   const zoomKeyMap: Record<WindowType, keyof AppSettings> = {
     sidebar: "sidebarZoom",       // deprecated, used only by legacy redirect paths
     terminal: "terminalZoom",     // used by DETACHED windows only
     main: "mainZoom",             // NEW
     guide: "guideZoom",
   };
   ```
   Plus one line in `AppSettings` (Rust + TS) and one default (1.0). This is strictly cheaper than fighting a confusing field name later.

3. **Race prevention** (see DW.5): if main reuses `sidebarZoom`, the embedded Sidebar+Terminal both running `initZoom` would both register wheel/keydown handlers — even after skipping `initZoom` in embedded terminal as DW.2 says, a future reader might forget that invariant and re-enable it. A dedicated `main` zoom key makes the separation self-documenting.

4. **Migration**: follow the same pattern as §6.5's geometry migration. In `load_settings`, if `main_zoom` is `None` and `sidebar_zoom != 1.0`, seed `main_zoom = sidebar_zoom`. Saved `sidebar_zoom` is dropped via `skip_serializing_if` on next save. One version later, delete `sidebar_zoom` from `AppSettings`.

5. **xterm FitAddon correctness**: verified — xterm measures cells in CSS pixels, and Tauri's `setZoom` scales CSS pixels. zoom.ts already abstracts the Tauri-vs-browser zoom target (line 32-43). No xterm change needed.

**Cost of delay (if tech lead pushes back and insists on MVP reuse):** Phase 1 ships with `main=sidebarZoom`. Users who were happy with sidebar=1.0 and terminal=2.0 now see their main at 1.0 — unexpected ergonomic regression for that cohort. When Phase 4 introduces `mainZoom`, either:
- Migrate from `sidebarZoom` → breaks users who used Phase 1-3 and expected their terminal zoom to follow; OR
- Migrate from `terminalZoom` → breaks users who liked the Phase 1 sidebar-sized main zoom.

Neither path is clean. Do it right in Phase 1. The additional scope is ~10 lines spread across `types.ts`, `settings.rs`, `zoom.ts`, and the migration branch.

**Coordination point with dev-rust:** adding `main_zoom: f64` (default 1.0) to `AppSettings` — confirm with them, they can fit it into the same `settings.rs` change already planned in §5.1.

### DW.7 — Splitter persistence: timer ownership

Plan §3 references `src/shared/window-geometry.ts:22-35`'s `debouncedSave` as the pattern for splitter-width persistence. A subtle issue:

`window-geometry.ts:7` declares `saveTimeout` at **module scope**. This is fine today because sidebar and terminal run in separate webviews (separate module instances). In the unified main window bundle, `main/App.tsx` will import `window-geometry.ts` to persist `mainGeometry` AND will independently run its own debouncer for `mainSidebarWidth`. If both use the shared module-level `saveTimeout`, they **race** — a splitter drag followed by a window resize within 500ms silently drops one of the two saves.

**Fix (recommended, minimal):** keep the splitter debounce **local** to `main/App.tsx`:

```tsx
let splitterSaveTimeout: ReturnType<typeof setTimeout> | null = null;
const persistWidth = (w: number) => {
  if (splitterSaveTimeout) clearTimeout(splitterSaveTimeout);
  splitterSaveTimeout = setTimeout(async () => {
    const settings = await SettingsAPI.get();
    await SettingsAPI.update({ ...settings, mainSidebarWidth: w });
  }, 500);
};
onCleanup(() => { if (splitterSaveTimeout) clearTimeout(splitterSaveTimeout); });
```

**Fix (cleaner, slightly larger scope):** refactor `window-geometry.ts` to make `saveTimeout` closure-local like `zoom.ts:28` does. Leaves the module stateless. Out-of-scope for this feature, but a +3-line follow-up that pays off for anyone who touches that file next.

Recommend option 1 inside this feature; file option 2 as a cleanup chore.

### DW.8 — xterm.js resize cadence during splitter drag

Plan §3 correctly notes `ResizeObserver` (`TerminalView.tsx:217-222`) handles automatic resize. One additional detail worth spelling out so the reviewer doesn't think we missed it:

**Every resize event triggers TWO sync passes** via `scheduleViewportSync` (TerminalView.tsx:44-58) — a double-rAF pattern that calls `fitAddon.fit()` + `PtyAPI.resize()` twice per resize. During continuous splitter drag at 60Hz, this is:
- 60 resize events/sec × 2 rAF passes × (1 `fit()` + 1 `pty_resize`) = **240 layout reads + 120 PTY resizes per second**.

`PtyAPI.resize` is a Tauri IPC round-trip (sessionId, cols, rows). At 120 calls/sec, we're spamming the IPC bus. Two mitigations available:
- **xterm's own guard at `TerminalView.tsx:175-181`** already skips `PtyAPI.resize` when cols/rows haven't changed. Because pixel → cell quantization is coarse, most splitter-drag ticks **do not** change cols/rows. In practice the 120/sec number collapses to maybe 5-10/sec (only crossing cell-boundary pixels).
- **Fallback throttle**: if we ever observe PTY lag during drag, add an rAF-level throttle in `scheduleViewportSync` keyed on `dragging()`. Out of scope for Phase 1.

**No action needed in the feature branch.** Noting it so that if grinch asks "won't the splitter murder the PTY?" the answer is on record: xterm's onResize guard + fit-quantization naturally rate-limit it.

### DW.9 — xterm WebGL context budget

Tech lead's enrichment brief asks about "WebGL context limits with multiple xterm instances across detached windows". Full answer:

**Per-document limit**: browsers cap WebGL contexts per document at around **16** (Chrome/Edge; Firefox is lower). Each `WebglAddon` instance owns one context. Key observations:

- **Each Tauri webview has its own document** → own WebGL budget. Main window's budget is independent of any detached window's budget.
- **`TerminalView`'s per-session `terminals` map (line 30)** caches one xterm per session ever shown by that view. In main, the budget shrinks to ~16 simultaneous *recently-used* sessions before new sessions fall back to canvas.
- **Detached windows** run their own `TerminalView` with its own map, so a detached window sees only one session → trivially within budget.
- **Canvas fallback is already wired** at `TerminalView.tsx:123-131`: the `WebglAddon` constructor is inside a try/catch that silently no-ops on failure; xterm transparently falls back to canvas. User sees slightly worse glyph crispness, not a broken terminal.

**Action:**
- **No code change** needed. Existing fallback is correct.
- Add a one-line comment at `TerminalView.tsx:123` explaining the budget to future readers:
  ```ts
  // WebGL context budget: ~16 per document. Canvas fallback activates silently
  // when the budget is exhausted (e.g. after 16+ concurrent sessions in main).
  ```
- **Phase 4 polish candidate**: dispose WebGL contexts for sessions that haven't been shown in N minutes, freeing budget for active ones. Not a blocker.

### DW.10 — Splitter a11y (Phase 4 polish)

BrowserApp's divider has no keyboard affordance. For the main window, the divider should:
- Be focusable: `<div class="main-divider" role="separator" aria-orientation="vertical" aria-valuenow={sidebarWidth()} aria-valuemin={200} aria-valuemax={600} tabindex="0">`
- Respond to arrow keys while focused: `←`/`→` adjust ±10px, `Shift+←`/`Shift+→` adjust ±40px, `Home`/`End` snap to clamp boundaries.
- Visible focus ring on `:focus-visible`.

Not blocking for Phase 1. File as Phase 4 polish item. Mentioning because screen-reader users will otherwise have no way to resize the pane.

### DW.11 — Initial splitter-width paint flash

`main/App.tsx.onMount` will `await SettingsAPI.get()` to read `mainSidebarWidth`. Before the promise resolves, the splitter renders at the `createSignal` default (e.g. 240). If the user's saved value is 320, the sidebar visibly jumps from 240 → 320 on first paint. On fast machines this is sub-frame and invisible; on slow machines or cold starts it's a perceptible 50-100ms snap.

**Mitigation options:**
- **Synchronous seed** via the backend's initial-settings injection. The Rust side could set a global `window.__AC_SETTINGS__` before the bundle loads (via `initialization_script` in `WebviewWindowBuilder`). Already a pattern used in the codebase? — **dev-rust verification item**; if yes, use it; if no, propose it as a tiny follow-up.
- **CSS fallback**: render the sidebar pane with `visibility: hidden` until settings load, then `visibility: visible`. Invisible but layout-reserving. One-line CSS, zero-JS.
- **Ignore**: single-frame flash is a non-issue for 95% of launches.

Recommend: **ignore for Phase 1**, document for Phase 4 if complaints surface.

### DW.12 — `sidebarAlwaysOnTop` setting disposition

Current code: `AppSettings.sidebarAlwaysOnTop` (types.ts:129) applied at `SidebarApp.tsx:75-78` via `getCurrentWindow().setAlwaysOnTop(true)`. In unified mode, the "sidebar" window no longer exists; the setting would either apply to the main window (changing semantics) or be ignored (silently broken).

**Options:**
- **(a) Rename + repurpose**: `sidebarAlwaysOnTop` → `mainAlwaysOnTop`. Migrate value on first-boot-after-upgrade (copy over unchanged). Adds a field to the deprecation list in §6.1.
- **(b) Retire entirely**: always-on-top for a full main window (which now contains the terminal) is an atypical workflow. Users who want always-on-top for monitor-style use have detached windows now — add a **per-detached-window** always-on-top toggle instead (context-menu option on the detached titlebar).
- **(c) Skip the feature**: drop always-on-top everywhere, don't migrate. Simplest.

My bias: **(a)** — zero surprise for current users. One-line Rust addition in `AppSettings`, one-line JS change in the main-window mount (apply `setAlwaysOnTop` conditionally).

**Flagging for tech-lead decision**; not blocking.

### DW.13 — Frontend coordination points with dev-rust

Threads that cross the backend/frontend boundary — noted here so they're visible but **not changed** in the rust section of the plan:

1. **`win.close()` vs `win.destroy()` (§11.Q1)** — determines whether the detached window's `onCloseRequested` handler fires when `destroy_session_inner` closes it. The **frontend side** of this is the `onCloseRequested` installer in `TerminalApp.tsx` (new code per Phase 2). The handler must unconditionally call `WindowAPI.attach(sessionId)`. If dev-rust's `close()` DOES fire `onCloseRequested`, then dev-rust must switch `destroy_session_inner`'s close path to `destroy()` — OR the frontend handler needs a guard like `if (destroyingViaSessionDestroy) skip`. The second path requires shared state and is ugly. **Strong frontend preference**: dev-rust uses `destroy()` in destroy paths, `close()` elsewhere. Avoid the frontend guard.

2. **Per-detached-window geometry (§11.Q2)** — if dev-rust agrees to `detached_geometry: HashMap<sessionId, WindowGeometry>`, the frontend needs:
   - A new `initDetachedWindowGeometry(sessionId)` variant in `src/shared/window-geometry.ts` that writes to `settings.detachedGeometry[sessionId]` instead of a top-level key.
   - Called from `TerminalApp.onMount` when `props.detached && props.lockedSessionId`.
   - Cleanup in Rust: when session is destroyed, remove its entry from `detached_geometry`.

3. **`focus_main_window` callers (§11.Q5)** — in addition to the Rust-side audit, note that `ensure_terminal_window` is listed in `src-tauri/src/web/commands.rs:279` (web remote command whitelist). If renamed to `focus_main_window`, that entry needs updating. If removed entirely, remove from the whitelist too. Frontend `WindowAPI.ensureTerminal` alias takes care of the TS side.

4. **`WindowAPI.closeDetached` (`src/shared/ipc.ts:179-180`)** — §5.1 removes `close_detached_terminal` from `invoke_handler!`. This **breaks `WindowAPI.closeDetached`** if any frontend code calls it. Grepped: **no callers in TS**. Safe to remove from `ipc.ts` entirely. Add to §5.2: delete `WindowAPI.closeDetached`.

5. **`open_web_remote` URL at `lib.rs:299`** — plan §7.4 already calls this out. Confirming from frontend side: the legacy redirect in `main.tsx` (§2.3.1) catches old browser tabs; no additional FE change needed once dev-rust flips the URL to `?window=main`.

### DW.14 — Test plan additions (append to §8)

Cases §8 should include but doesn't:

- **§8.2.7** — During splitter drag, move the cursor OVER the terminal pane. Verify no xterm text selection begins (DW.4.c).
- **§8.2.8** — Drag the splitter past the right clamp, then release outside the webview (cursor over OS chrome). Drag must end cleanly; width must be clamped, not frozen at the last-seen position (DW.4.b).
- **§8.3.14** — In DETACHED window, Ctrl+= zooms ONLY the detached window. Main window zoom unchanged. Verify `terminalZoom` is the key in settings that changed.
- **§8.3.15** — In MAIN window, Ctrl+= zooms sidebar + terminal **together**. Verify `mainZoom` is the key in settings that changed (not `sidebarZoom` or `terminalZoom`). (Assumes DW.6 accepted.)
- **§8.7.36** — Create 17 sessions in main window (test WebGL context overflow). Verify the 17th session's terminal renders via canvas fallback without error; 1-16 stay on WebGL. Functional correctness, no visible regression.
- **§8.7.37** — Theme toggle in unified mode: verify ONE theme transition happens (not two). If sidebar+terminal both run `onThemeChanged` handlers you'd see double-flipping of `classList`. (Regression check for DW.2's instruction to skip the terminal handler when embedded.)

### DW.15 — Summary of changes requested in the plan

For the architect's re-look prioritization:

**Must-fix (blocks review):**
1. §5.2: remove OnboardingModal row (no callers) — DW.1.
2. §5.2: delete `WindowAPI.closeDetached` from `ipc.ts` (no callers) — DW.13.4.
3. §2.3.3: add explicit `embedded` obligation to skip `<Titlebar/>` render in both apps — DW.2.
4. §3: CSS must include `min-height: 0` on flex row; layout structure must place titlebar outside the flex row — DW.3.
5. §11.Q3: upgrade from "reuse `sidebarZoom`" to "introduce `mainZoom` in Phase 1" — DW.6 (strong recommendation; cost is ~10 lines).

**Should-fix (architect should decide before grinch):**
6. DW.4: splitter UX hardening (pointer capture, cursor, pointer-events on xterm). Lift the ~20-line model into `src/main/App.tsx` explicitly.
7. DW.7: splitter debounce timer must be local to `main/App.tsx`, not shared with `window-geometry.ts`.
8. DW.12: `sidebarAlwaysOnTop` disposition — rename, retire, or skip. Pick one.

**Nice-to-have (Phase 4 or beyond):**
9. DW.10: splitter keyboard a11y.
10. DW.11: initial-width paint flash mitigation.
11. DW.8: PTY resize throttle during splitter drag (only if measured).
12. DW.9: WebGL context dispose for idle sessions.

Round-1 enrichment ends here. Stand by for round-2 after dev-rust enriches and grinch reviews.

---

## Dev-rust enrichment (round 1)

**Author:** dev-rust
**Anchored against HEAD:** `60dd162` (verified via `git log` on `feature/unified-window-with-detach`; status clean except untracked sibling plans).
**Tauri version in use:** `2.10.3` (confirmed in `src-tauri/Cargo.lock`).
**Scope:** Rust backend only. Frontend items are re-stated for visibility but deferred to dev-webpage-ui. Where my findings intersect theirs (e.g. `focus_main_window` disposition), I state my Rust-side position independently; reconcile in round 2 if we diverge.

### R.0 Summary verdict

Macro-architecture is sound. The unified-window direction, the broadcast+client-filter `pty_output` choice, the `was_detached` persistence approach, and the redirect strategy are all correct. I do NOT want any of those re-litigated.

What I am flagging are **seven** concrete things the plan either got wrong, under-counted, or left open in a way that will bite the implementer. None block the phase structure; all are tractable inside the dev-enrichment round.

### R.1 Verification of plan against current HEAD

Spot-verified every file/line reference in §5.1 and §6 against `60dd162`. Results:

| Plan reference | Status |
|---|---|
| `src-tauri/src/lib.rs:30-31` (`DetachedSessionsState`) | ✓ accurate (line 31) |
| `src-tauri/src/lib.rs:426-509` (dual-window setup to rewrite) | ✓ accurate; the `WebviewWindowBuilder` calls live at 475-505 |
| `src-tauri/src/lib.rs:697-717` (destroy-prefix cleanup) | ✓ accurate |
| `src-tauri/src/lib.rs:299` (web remote URL with `?window=sidebar`) | ✓ accurate |
| `src-tauri/src/commands/window.rs:11-92` (`detach_terminal`) | ✓ accurate |
| `src-tauri/src/commands/window.rs:108-159` (`ensure_terminal_window`) | ✓ accurate |
| `src-tauri/src/commands/window.rs:195-213` (`close_detached_terminal`) | ✓ accurate |
| `src-tauri/src/commands/session.rs:530-535` ("Show terminal on create" — to delete) | ✓ accurate |
| `src-tauri/src/commands/session.rs:722-726` (close detached on destroy) | ✓ accurate |
| `src-tauri/src/commands/session.rs:736-743` ("Hide terminal on destroy" — to delete) | ✓ accurate |
| `src-tauri/src/commands/session.rs:905-923` (`switch_session` focus-detached branch) | ✓ accurate |
| `src-tauri/src/config/settings.rs:32-109` (`AppSettings`) | ✓ accurate |
| `src-tauri/src/config/settings.rs:139-176` (`AppSettings::default`) | ✓ accurate |
| `src-tauri/src/config/settings.rs:299-340` (`load_settings`) | ✓ accurate |
| `src-tauri/src/config/sessions_persistence.rs:14-56` (`PersistedSession`) | ✓ accurate |
| `src-tauri/src/config/sessions_persistence.rs:304-343` (`snapshot_sessions`) | ✓ accurate |

Nothing has drifted. The plan is anchored correctly.

### R.2 Answer to §11.Q1 — `close()` vs `destroy()` in Tauri 2.x (definitive)

**Verified against actual Tauri source** at `tauri-2.10.3/src/webview/webview_window.rs:1937-1945`:

```rust
/// Closes this window. It emits [`crate::RunEvent::CloseRequested`] first like a user-initiated close request so you can intercept it.
pub fn close(&self) -> crate::Result<()> { self.window.close() }

/// Destroys this window. Similar to [`Self::close`] but does not emit any events and force close the window instead.
pub fn destroy(&self) -> crate::Result<()> { self.window.destroy() }
```

**Conclusion**: `close()` fires `CloseRequested`; `destroy()` does not. This is authoritative for Tauri 2.10.3.

**Implication**: once Phase 2 lands the re-attach intercept (`getCurrentWindow().onCloseRequested(e => { e.preventDefault(); WindowAPI.attach(sessionId); })`), **every programmatic close path in Rust must switch from `.close()` to `.destroy()`**, or session termination will trigger re-attach instead of actual termination. This is a correctness bug, not polish.

**Mandatory changes to fold into §5.1 before implementation starts**:

| File | Line | Current | Required change |
|---|---|---|---|
| `src-tauri/src/commands/session.rs` | 725 | `let _ = detached_win.close();` | `let _ = detached_win.destroy();` |
| `src-tauri/src/commands/window.rs` (retained `close_detached_terminal`, if any — see R.3) | 210 | `window.close()` | `window.destroy()` |
| `src-tauri/src/commands/window.rs` (NEW `attach_terminal`) | — | — | Use `win.destroy()`. The attach handler is itself invoked from inside the close-requested intercept; calling `close()` there would re-trigger the intercept (infinite recursion / stack overflow risk). |

**Add to §10 (What the dev must NOT do)**:
- Do NOT use `WebviewWindow::close()` on a detached window from any Rust path after Phase 2 lands. The close-requested intercept is user-X-only. Programmatic closes must use `destroy()` to bypass it.

### R.3 Delete `close_detached_terminal` entirely — it is fully redundant

`destroy_session_inner` (`commands/session.rs:680-746`) **already**:

1. Removes the session UUID from `DetachedSessionsState` (lines 683-688), and
2. Closes the detached window (lines 722-726 — which must become `destroy()` per R.2).

That is exactly what `close_detached_terminal` (`commands/window.rs:195-213`) does, minus the `switch_session` sibling-activation. The plan's §2.2.2 text ("retained but becomes a pure window-close helper used internally by `destroy_session_inner`") describes a function with no non-redundant use.

**Proposal**: delete `commands::window::close_detached_terminal` outright. Update §5.1:

- Remove the "keep as internal helper" row.
- Remove `commands::window::close_detached_terminal` from the `invoke_handler!` macro at `lib.rs:654`.
- Delete the function at `commands/window.rs:193-213`.
- Remove `"close_detached_terminal"` from the web-client no-op arm at `src-tauri/src/web/commands.rs:277` (see R.5).

Three cleanup paths for the same thing (the `Destroyed` event at `lib.rs:697-717`, `destroy_session_inner`, and this helper) is already one too many. The event handler is load-bearing (cross-mechanism catch-all); `destroy_session_inner` is the canonical termination path; this helper is the third — with no caller the other two don't cover.

### R.4 Answer to §11.Q5 — `focus_main_window` has 9 live callers, **keep the command**

The architect's §11.5 speculated "`focus_main_window` may actually have no callers. **If so, delete the command.**" It has callers. Verified via `grep "ensureTerminal\|ensure_terminal_window"`:

| Caller | Purpose |
|---|---|
| `src/sidebar/App.tsx:56` | `handleRaiseTerminal` — mousedown-bring-to-front on the sidebar (dev-webpage-ui's DW.2 proposes deleting this specific caller in `embedded` mode; that's fine, it still leaves 8) |
| `src/sidebar/components/RootAgentBanner.tsx:15` | After creating a root-agent session |
| `src/sidebar/components/SessionItem.tsx:93` | Session row action |
| `src/sidebar/components/ProjectPanel.tsx:112, 126, 156, 170, 192, 1358` | Six project-action call sites |

In unified mode most of these become near-no-ops when triggered from within the main window (main already focused). But:

- The gesture remains semantically valid when the main window is minimized / behind another app and a trigger arrives from a non-UI path (keyboard shortcut, messaging-system-triggered activation, future deep-link).
- Migrating 9 call sites only to regret it one refactor later is churn for no benefit. The `ensureTerminal` → `focusMain` deprecated alias already handles the surface rename.
- The "recreate if missing" branch is arguably dead code in practice (closing the only main window should quit the app), but the current codebase has no explicit quit-on-main-close handler, so defensive re-creation is cheap insurance.

**Verdict**: keep `focus_main_window`. Keep the `ensureTerminal` → `focusMain` deprecated alias for one version (as the plan proposes in §5.2 `ipc.ts`). Revisit in v0.9 alongside the `applyWindowLayout` cleanup.

Strike "If so, delete the command" from §11.Q5.

### R.5 §5.1 is missing `src-tauri/src/web/commands.rs`

The web-client command dispatcher at `src-tauri/src/web/commands.rs:276-280` has a no-op match arm for window commands (browser-remote users don't have Tauri windows):

```rust
// --- Window commands (no-ops for web clients) ---
"detach_terminal"
| "close_detached_terminal"
| "open_in_explorer"
| "ensure_terminal_window"
| "open_guide_window" => Ok(json!(null)),
```

Not listed in §5.1. Required edits:

- Rename `"ensure_terminal_window"` → `"focus_main_window"`.
- Remove `"close_detached_terminal"` (per R.3).
- Add `"attach_terminal"` to the no-op list (browser clients have no detached Tauri windows to attach back).

Drift risk: this match arm silently rots every time a new Tauri window command lands. Call it out as a checklist item whenever §5.1 touches `commands/window.rs`.

### R.6 Answer to §11.Q2 — put `detached_geometry` on `PersistedSession`, not on `AppSettings`

Architect §11.2 proposed `AppSettings::detached_geometry: HashMap<sessionId, WindowGeometry>`. My position: take the alternative — put `detached_geometry: Option<WindowGeometry>` on `PersistedSession` next to `was_detached`.

Structure:

```rust
pub struct PersistedSession {
    // ... existing ...
    #[serde(default)]
    pub was_detached: bool,

    /// Last-known geometry of this session's detached window.
    /// Populated only while the session is detached. Cleared implicitly
    /// when the session is destroyed (the whole PersistedSession disappears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_geometry: Option<WindowGeometry>,
}
```

Reasoning:

1. **Auto-GC**. Per-session geometry lives only as long as its session. Destroy the session → the row vanishes; no dangling entry. The `HashMap` variant needs explicit prune-on-destroy bookkeeping — the kind that silently rots (settings.json would grow unbounded over years of use).
2. **Consistency with existing model**. `AppSettings` is **global app** config. `PersistedSession` is **per-session** state. Detached-window geometry is per-session state. It belongs in `sessions.json`, not `settings.json`.
3. **Serialization shape**. `HashMap<String, WindowGeometry>` keyed by stringified UUIDs sprays UUID strings into settings.json — harder to read, noisier diffs for a file users may edit by hand. Co-located on the session row keeps the shape flat.
4. **Save path is simpler**. When a detached window moves/resizes, the save flows through `SessionManager` (same mechanism as `was_active`, `last_prompt`, etc.). No parallel coordination primitive.
5. **Restore path is trivial**. When the restore loop spawns a detached window for `ps.was_detached == true`, it passes `ps.detached_geometry` to the `WebviewWindowBuilder` inside `detach_terminal_inner`. No map lookup.

**Counter to a possible objection** ("geometry should persist even if the session is destroyed while detached, so reopening the same CWD later re-uses the position"): that scenario is rare (destroy-while-detached → recreate-same-CWD), and a single re-position click rescues it. Not worth the GC burden.

**Required changes**:

- §6.2 (PersistedSession): add `detached_geometry: Option<WindowGeometry>` alongside `was_detached`.
- §6.3 (TypeScript `AppSettings`): DO NOT add `detachedGeometry`. It lives on `SessionInfo` / `PersistedSession`, not `AppSettings`.
- §5.2 (`window-geometry.ts`): extend `WindowType` to `"sidebar" | "terminal" | "main" | "detached"`; the `"detached"` case needs a `sessionId` parameter. Two options:
  - (a) Second function `initDetachedWindowGeometry(sessionId: string)` that debounces saves to a new IPC call `SessionAPI.updateDetachedGeometry(sessionId, geo)`.
  - (b) Extend the existing helper to accept an optional `sessionId` for the `"detached"` type only.
  Either is fine; (a) is cleaner. dev-webpage-ui decides.
- New Rust command (add to §5.1):

```rust
#[tauri::command]
pub async fn set_detached_geometry(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
    geometry: WindowGeometry,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let mgr = session_mgr.read().await;
    mgr.set_detached_geometry(uuid, geometry).await;
    // No explicit persist call here — the debounce on the frontend combined
    // with the existing persist-on-state-change cadence covers it. The exit
    // handler at lib.rs:733 ensures a final snapshot on shutdown.
    Ok(())
}
```

`SessionManager::set_detached_geometry(uuid, geo)` becomes a trivial setter on a new `Session::detached_geometry: Option<WindowGeometry>` field; `snapshot_sessions` reads it into the `PersistedSession` like any other session field.

### R.7 Threading `DetachedSessionsState` through persist helpers — 10 call sites, not 3

§5.1 line 222 under-counts drastically. Verified call-site list via `grep -n "persist_current_state\|persist_merging_failed\|snapshot_sessions"`:

| File | Line | Caller context |
|---|---|---|
| `src-tauri/src/lib.rs` | 200 | idle-callback (inside `tauri::async_runtime::spawn`) |
| `src-tauri/src/lib.rs` | 217 | busy-callback (inside `tauri::async_runtime::spawn`) |
| `src-tauri/src/lib.rs` | 618 | restore loop (`persist_merging_failed`) |
| `src-tauri/src/lib.rs` | 733 | exit handler (`RunEvent::Exit`) |
| `src-tauri/src/commands/session.rs` | 640 | `create_session` |
| `src-tauri/src/commands/session.rs` | 718 | `destroy_session_inner` |
| `src-tauri/src/commands/session.rs` | 898 | `restart_session` |
| `src-tauri/src/commands/session.rs` | 929 | `switch_session` |
| `src-tauri/src/commands/session.rs` | 951 | `rename_session` |
| `src-tauri/src/commands/session.rs` | 1223 | `create_root_agent_session` |

**Ten sites.** Plan claims two. Also: `lib.rs:200` / `lib.rs:217` are inside idle-detector callbacks that access `AppHandle` via `OnceLock` and have no direct `DetachedSessionsState` binding today — they'd need `app.state::<DetachedSessionsState>()` fetches added.

**Recommendation: change the shape rather than thread the Arc.** Instead of `snapshot_sessions(mgr, &DetachedSessionsState)`, take a plain `HashSet<Uuid>` snapshot by reference:

```rust
pub async fn snapshot_sessions(
    mgr: &SessionManager,
    detached: &std::collections::HashSet<uuid::Uuid>,
) -> Vec<PersistedSession> { /* ... */ }

pub async fn persist_current_state(
    mgr: &SessionManager,
    detached: &std::collections::HashSet<uuid::Uuid>,
) { /* ... */ }
```

At each call site:

```rust
let detached_snapshot = {
    let state = app.state::<DetachedSessionsState>();
    state.lock().unwrap().clone() // HashSet<Uuid> clone is cheap (<100 UUIDs in practice)
};
persist_current_state(&mgr, &detached_snapshot).await;
```

Two wins:
1. **No lock held across an await.** The project's lock discipline rule ("never hold a lock across an await point" — `.ac-new/_agent_dev-rust/Role.md` and CLAUDE.md conventions) is violated if callers do `let g = detached.lock().unwrap(); persist_current_state(mgr, &*g).await;`. Passing the set by value prevents that foot-gun at the type level.
2. **Test-friendly.** Unit tests and hypothetical future contexts can pass `&HashSet::new()` without wiring up an Arc.

Update §5.1 line 222 with: (a) the full 10-call-site list, and (b) the signature recommendation (plain `HashSet<Uuid>` snapshot, not `&DetachedSessionsState`).

### R.8 `main.tsx` routing vs web-remote browser mode — a hole

§4.Q7 says `?window=sidebar` / `?window=terminal` redirect to `?window=main`. §7.4 says `lib.rs:299` updates the web-remote URL to `?window=main`.

**The hole**: `MainApp` uses Tauri APIs (splitter width persisted via `SettingsAPI.update`, Tauri `getCurrentWindow` for `onCloseRequested`, etc.). In browser mode (`!isTauri`), the existing `?window=sidebar` and `?window=terminal` URLs serve the `BrowserApp` combined layout (per §2.1). If the redirect fires unconditionally, a browser user landing on `?window=main&remoteToken=…` tries to mount `MainApp` → Tauri API calls silently fail.

**Fix**: the new `main.tsx` must check `!isTauri` FIRST and return `BrowserApp` unconditionally, then apply Tauri-only routing. Skeleton:

```tsx
if (!isTauri) {
  // Browser (web remote): BrowserApp handles every URL, regardless of
  // legacy ?window=sidebar / ?window=terminal or the new ?window=main.
  render(() => <BrowserApp />, root);
} else if (windowType === "detached") {
  const lockedSessionId = params.get("sessionId") || undefined;
  render(() => <TerminalApp lockedSessionId={lockedSessionId} detached />, root);
} else if (windowType === "guide") {
  render(() => <GuideApp />, root);
} else {
  // ?window=main, or legacy ?window=sidebar / ?window=terminal, or no param.
  render(() => <MainApp />, root);
}
```

No explicit URL rewrite; the dispatcher just ignores legacy values. This also covers pre-migration in-flight window-state restorations landing on old labels — they route to `MainApp` directly.

Update §5.2 `main.tsx` row + §4.Q7 with this skeleton and the clarification that the legacy `?window=sidebar / ?window=terminal` handling is **Tauri-only**.

### R.9 Deferred-session interaction with `was_detached`

The `start_only_coordinators` path at `lib.rs:544-577` creates deferred sessions without PTYs — they show as `Exited(0)`. These rows **never pass through `create_session_inner`**; they call `mgr.create_session` directly at `lib.rs:547`.

§2.2.4 says "after the existing `create_session_inner` recreates each session's PTY, the restore loop … spawns a detached window for any `ps.was_detached = true`." That branch doesn't run for deferred sessions. What should happen?

**Recommendation**: **skip detached-window spawn for deferred sessions** on restore. A detached window pointing at an Exited-PTY is a worse UX than seeing the session sit dormant in the sidebar. If the user later wakes the deferred session via `restart_session` (the `ProjectPanel.handleReplicaClick` → `skip_auto_resume: Some(false)` path at `session.rs:779-902`), the session becomes live at that point. Simpler: start the awakened session **attached**, let the user re-detach deliberately.

**Required addition to §2.2.4**: guard the `was_detached` spawn on "session reached `create_session_inner` with success". Deferred sessions skip.

**Add to §8.7 test plan**:
> Edge case: session with `startOnlyCoordinators=true` + `was_detached=true` on a deferred team member. On restore: session appears as deferred (Exited(0)); NO detached window is spawned. After `restart_session` (wake), session becomes live, attached to main; user can re-detach manually.

### R.10 Phase 3 — restore-order race with `was_detached`

§9 Phase 3 proposes "extract `detach_terminal_inner` and call from the restore loop". The current restore loop at `lib.rs:527-628` is sequential per session inside one big `tauri::async_runtime::spawn(async move { … })`, then explicitly `switch_session(active_id)` at lines 607-614.

If `detach_terminal_inner` is called inside the per-session loop (right after `create_session_inner` succeeds), the current `detach_terminal` body at `commands/window.rs:58-89` emits `session_switched` to the next non-detached session — racing with / overwriting the intended `active_id` restore at 607-614.

**Fix**: the restore-path detach call must be a variant that skips the "switch main to next non-detached" branch. Two options:

(a) Accept a `skip_switch: bool` parameter on `detach_terminal_inner`; pass `true` from the restore path.

(b) Two-pass restore loop: pass 1 = create sessions + note which are `was_detached`; pass 2 = explicit `switch_session(active_id)`; pass 3 = detach.

(a) is less disruptive; (b) is cleaner. My recommendation: (a) — less churn, and the flag is self-documenting.

Document in §2.2.4 and §9 Phase 3.

### R.11 Minor plan polish (non-blocking)

- **§5.1 row for `lib.rs:426-509`**: the actual range that needs rewriting is `lib.rs:390-505` (from the "SideBar Right" defaults block through both `WebviewWindowBuilder` calls). The `monitors` detection block at 330-388 **stays** — it's still useful for validating `main_geometry`. Tighten the row to "rewrite 390-505; keep 330-388 untouched."
- **§6.5 settings migration** correctly notes the migration must run BEFORE `root_token` auto-gen (which triggers an early `save_settings` at `settings.rs:334`). Confirmed at `settings.rs:330-337`. Good catch; no change.
- **§2.2.2 language** ("close_detached_terminal … becomes a pure window-close helper used internally by destroy_session_inner") — reword per R.3: "delete close_detached_terminal; destroy_session_inner already covers both cleanups (DetachedSessionsState removal at lines 683-688, window close at 722-726 — which must become `destroy()` per R.2)."
- **Phase 1 ship bar** (§9 line 487): "closing detached window doesn't kill the session (session stays live, user can recreate detached window via another detach)". This describes Phase 1 behavior before the re-attach intercept. The existing `Destroyed` event handler at `lib.rs:697-717` fires on X-close regardless of mechanism, so the session is correctly removed from `DetachedSessionsState`. Add a Phase 1 test row: "After X-closing a detached window in Phase 1, the sidebar shows the session as attached again (removed from `detachedIds`). Clicking the session activates it in main."
- **Cargo test for close/destroy discipline (R.2)**: add one. A small test in `commands/session.rs` that asserts `destroy_session_inner` uses `.destroy()` not `.close()`. Simplest form: a doctest is overkill here; a regression comment at line 725 pointing at this plan's R.2 is sufficient.

### R.12 Agreements (explicit, so grinch doesn't re-open)

Where the architect called a right decision, confirming before grinch:

- **Q5 (broadcast + client filter)**: strongly agree. The `TerminalView.tsx:224-235` session-id filter is already the right shape. I would additionally add a cheap `sessionId`-guard on the listener side that drops any event for a session not in the current window's `terminals` map — already present per plan note, so no change.
- **Q1 (X = re-attach)**: agree. "X button on a view should not kill the thing the view is viewing" is the correct UX invariant; the close-requested intercept is the right mechanism (Tauri 2.10.3 explicitly supports this per R.2).
- **Q6 (close != kill)**: same as Q1, load-bearing. Agree.
- **Q7 (routing scheme)**: agree modulo the browser-mode hole in R.8.
- **Q2 (persistence)**: agree with `was_detached` on `PersistedSession`; adjust detach-geometry placement per R.6.
- **Q3 (switch to next non-detached on detach)**: agree. Existing logic at `commands/window.rs:58-89` is the right template; no change.
- **Q4 (reuse Terminal Titlebar + Re-attach button)**: agree. Minimum diff, correct shape.
- **Q8 (sidebar click on detached → focus its window, not re-attach)**: agree. Intentional separation between selection and attach.

### R.13 Top risks to watch during implementation

1. **Close/destroy mis-use (R.2)**. One missed `close()` on the destroy path and session termination starts auto-re-attaching. Regression comment at `session.rs:725` + a smoke-test on Phase 2 is the guard.
2. **`DetachedSessionsState` + deferred sessions (R.9)**. A `was_detached: true` deferred row trying to auto-detach on wake. Explicit guard required.
3. **Phase-boundary UX flip**. Phase 1 ships with X-closes-window-but-keeps-session-alive. Phase 2 ships with X-re-attaches-to-main. Users between phases see behavior flip. Acceptable if we don't cut a public release between 1 and 2; callout in the Phase 2 PR description.
4. **First-boot settings migration**. Seeding `main_geometry` from `terminal_geometry` is fine, but if the user's `terminal_geometry` was off-screen (they dragged it off-screen before upgrade), they get an off-screen main window. The `is_visible_on_monitors` validation at `lib.rs:353-367` already covers this — confirm it runs against the migrated `main_geometry` and falls back to centered default when invalid.
5. **Restore-order race (R.10)**. `detach_terminal_inner` called in the per-session restore loop will overwrite the intended `active_id` restoration unless `skip_switch` is threaded through.

### R.14 Summary for tech-lead

**Top 5 things I changed/added:**

1. **`close()` → `destroy()` at `destroy_session_inner:725`** (and any retained `close_detached_terminal`, and the new `attach_terminal`). Verified from Tauri 2.10.3 source: `close()` DOES fire `CloseRequested`; destroy paths must bypass it.
2. **Delete `close_detached_terminal` entirely** — `destroy_session_inner` already covers both cleanups redundantly. One fewer command; one fewer no-op arm in `web/commands.rs`.
3. **Keep `focus_main_window` — 9 callers verified.** Reject §11.5's "delete if no callers" suggestion.
4. **`detached_geometry: Option<WindowGeometry>` on `PersistedSession`, NOT a `HashMap` on `AppSettings`.** Auto-GC, consistent with existing persistence model, cleaner save/restore paths.
5. **Persist-helper call-site threading**: 10 sites, not 3. Recommend signature change to `&HashSet<Uuid>` snapshot (not `&DetachedSessionsState`) — prevents lock-across-await and simplifies test callers.

**Points the architect should re-look before grinch:**

- **Deferred-session interaction with `was_detached`** (R.9) — not handled in the plan; small but needs an explicit guard.
- **`main.tsx` browser-mode hole** (R.8) — `!isTauri` check must precede the `?window=main` dispatch; otherwise web-remote breaks after `lib.rs:299` URL update.
- **Phase 3 restore-order race** (R.10) — `detach_terminal_inner` needs `skip_switch`, or the restore loop needs a two-pass shape.
- **`src-tauri/src/web/commands.rs:276-280` missing from §5.1** (R.5).

All other sections are implementable as drafted once the R.2 / R.3 / R.6 / R.7 corrections are folded into §5.1 + §6.

Enrichment complete. Standing by for grinch + round 2.

---

## Dev-rust-grinch adversarial review (round 1)

**Author:** dev-rust-grinch (adversarial reviewer)
**Anchored against HEAD:** `60dd162` (verified).
**Scope:** hunt bugs the architect and both devs missed. All findings below are problems the plan as-written will ship with unless they are folded in.

**Counts:** 5 BLOCKER / 4 HIGH / 7 MEDIUM / 4 LOW. Plus 4 arbitrations.

I tried to break the macro-architecture (broadcast + client-filter, X-re-attach intercept, was_detached persistence, routing scheme). All four survived serious attack — see §G.FOOTER for what I simulated and where the plan held up. The problems below are all in the **seams** between components: races between the std Mutex `DetachedSessionsState` and async work, restore-order interactions, and UX state-leaks that cross window boundaries.

### G.0 Executive summary (BLOCKER items and most dangerous findings)

Three findings to internalise before anything else:

1. **G.2 + G.3**: `destroy_session` and the restore loop both emit `session_switched` that can target a **detached** session UUID, because neither path filters against `DetachedSessionsState`. Result: main window and detached window both render the same session → duplicate display, cross-contamination of typing. This is partly pre-existing but the plan institutionalises detach as a first-class persisted concept, so fixing it now is cheap; fixing it later is a breaking-behaviour change.
2. **G.1**: `detach_terminal` inserts UUID into `DetachedSessionsState` **before** `WebviewWindowBuilder::build()`. Any build failure leaves the UUID permanently stranded → session becomes unreachable from main until destroyed.
3. **G.6**: Re-attach of a session that was restored as `was_detached=true` starts with an **empty xterm buffer** in main, because main's `terminals` map is populated lazily and the restored-detached session was never active in main. User loses visible history on re-attach.

---

### G.1 [BLOCKER] `detach_terminal` leaks `DetachedSessionsState` on build failure

- **Location:** `src-tauri/src/commands/window.rs:34-51` (existing code), §2.2 of the plan (no change proposed).
- **The bug:** The current implementation (verified at HEAD `60dd162`) executes:
  ```rust
  // Register as detached                             // <-- line 34-37
  { let mut s = detached.lock().unwrap(); s.insert(uuid); }

  WebviewWindowBuilder::new(...)
      .icon(icon).map_err(|e| e.to_string())?         // <-- can return early
      ...
      .build().map_err(|e| e.to_string())?;           // <-- can return early
  ```
  Both `.map_err(...)?` lines are reachable failure points (icon decode failure, graphics-driver failure, URL parse failure, OS handle exhaustion, WebView2 init race after a Windows update). If either trips, the UUID is left in `DetachedSessionsState` and **nothing subsequently removes it** — the Destroyed event handler at `lib.rs:697-717` only fires on an actual window `Destroyed`, which never happened. The plan keeps this code shape untouched (§2.2 modifies the URL but not the insert-then-build order).
- **Impact:** Session becomes inert. `switch_session` command (`session.rs:915-922`) sees it in `DetachedSessionsState` and tries to focus the non-existent window — silent no-op, main never shows it. User cannot interact with the session until they destroy it. On Phase 3 restart, `was_detached=true` triggers the same failure loop.
- **Fix:** Swap the order. Build the window first, then insert on success. Pattern:
  ```rust
  let win = WebviewWindowBuilder::new(...).build().map_err(|e| e.to_string())?;
  { let mut s = detached.lock().unwrap(); s.insert(uuid); }
  // emits, switch, etc.
  ```
  Or wrap the insert in an RAII guard that removes on `Drop` unless `.commit()` is explicitly called after `build()`. Add a test (smoke): feed a broken `include_bytes!` path in a debug-only path to force an icon failure, assert `DetachedSessionsState` is empty after the command returns Err.
- **Fold into §5.1** as a new required edit to `detach_terminal_inner`.

### G.2 [BLOCKER] `destroy_session_inner` may emit `session_switched` pointing at a detached session

- **Location:** `src-tauri/src/commands/session.rs:713-734` + `src-tauri/src/session/manager.rs:79-104` (`destroy_session`).
- **The bug:** `SessionManager::destroy_session` returns `Option<Uuid>` — whatever is `order.first()` after removal. It has zero awareness of `DetachedSessionsState`. `destroy_session_inner` at `session.rs:728-734` emits `session_switched` with that UUID unconditionally. If `order.first()` is a detached session, main's `TerminalApp.tsx:72-88` handler sets `terminalStore.activeSession = <detached-id>`; `TerminalView.tsx:242-256` calls `showSessionTerminal` which creates a fresh xterm in main for that session. Now **main AND the detached window both own an xterm for the same session**, both receive every `pty_output` event (broadcast), both render identical output, and both can accept keyboard input (keystrokes get sent via `PtyAPI.write` from whichever one is focused — non-deterministic user experience).
- **Trigger:** User has [A detached, B detached, C attached], destroys A via sidebar X. `order.first()` might be B (detached) or C (attached). If B: bug fires.
- **Impact:** Duplicate display; any typing in main is sent to the same PTY the detached window is showing; user sees their keystrokes echo in the "wrong" window. Also breaks the "one session, one view" invariant that the detached-window mental model relies on.
- **Fix:** `destroy_session_inner` must filter `new_active` against `DetachedSessionsState`. Concrete replacement for `session.rs:728-734`:
  ```rust
  if let Some(new_id) = new_active {
      let is_detached = {
          let detached = app.state::<DetachedSessionsState>();
          let set = detached.lock().unwrap();
          set.contains(&new_id)
      };
      if is_detached {
          // Walk the SessionManager order to find the first non-detached.
          let fallback = {
              let sessions = mgr.list_sessions().await;
              let detached = app.state::<DetachedSessionsState>();
              let set = detached.lock().unwrap();
              sessions.iter().find_map(|s| {
                  Uuid::parse_str(&s.id).ok().filter(|u| !set.contains(u))
              })
          };
          let payload = fallback.map(|u| serde_json::json!({ "id": u.to_string() }))
              .unwrap_or_else(|| serde_json::json!({ "id": serde_json::Value::Null }));
          let _ = app.emit("session_switched", payload);
      } else {
          let _ = app.emit("session_switched", serde_json::json!({ "id": new_id.to_string() }));
      }
  }
  ```
  (Also: update `SessionManager::destroy_session` so the backend-`active_session` it assigns is likewise non-detached, or re-call `switch_session` after the filter.)
- **Fold into §5.1**: add a row for `session.rs:728-734` modify + an `AppSettings`-independent guard. This is a pre-existing defect, but the plan institutionalises detach-as-persistent-state, so fixing it now prevents one more cross-version behaviour flip.

### G.3 [BLOCKER] Restore loop emits `session_switched(was_active)` without checking `DetachedSessionsState` — duplicate display on every restart where `was_active` is also `was_detached`

- **Location:** `src-tauri/src/lib.rs:607-614` (in the tokio task spawned from `setup()`), plan §2.2.4 + R.10.
- **The bug:** R.10 identified that `detach_terminal_inner` called inside the per-session loop would overwrite the restored `active_id`; R.10's fix is a `skip_switch: bool` flag that prevents `detach_terminal_inner` from emitting its own `session_switched`. Good. But **the post-loop switch at 607-614** calls `mgr.switch_session(active_id)` + `emit("session_switched", active_id)` directly, bypassing the command-level detached guard at `session.rs:915-922`. If `active_id` is a session with `was_detached=true`, the emitted event tells main to switch to a detached session → main creates its own xterm for it → duplicate display (same failure mode as G.2).
- **Trigger:** User has one session (A) that is `was_active=true AND was_detached=true`. Very common: user has one active detached session, quits, restarts.
- **Impact:** Every restart produces a duplicate display of A. User will see both windows scrolling identically.
- **Fix:** Filter `active_id` against the (now-populated) `DetachedSessionsState` AFTER the per-session detach pass. Replace `lib.rs:607-614` with:
  ```rust
  if let Some(id) = active_id {
      if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
          let is_detached = {
              let detached = app_handle.state::<DetachedSessionsState>();
              let set = detached.lock().unwrap();
              set.contains(&uuid)
          };
          if is_detached {
              // was_active session is detached — main should not display it.
              // Pick the first non-detached as the restored-main session, or null.
              let mgr = session_mgr_clone.read().await;
              let sessions = mgr.list_sessions().await;
              let detached = app_handle.state::<DetachedSessionsState>();
              let set = detached.lock().unwrap();
              let fallback = sessions.iter().find_map(|s| {
                  Uuid::parse_str(&s.id).ok().filter(|u| !set.contains(u))
              });
              if let Some(fb) = fallback {
                  drop(set);
                  let _ = mgr.switch_session(fb).await;
                  let _ = tauri::Emitter::emit(&app_handle, "session_switched",
                      serde_json::json!({ "id": fb.to_string() }));
              } else {
                  let _ = tauri::Emitter::emit(&app_handle, "session_switched",
                      serde_json::json!({ "id": serde_json::Value::Null }));
              }
          } else {
              let mgr = session_mgr_clone.read().await;
              let _ = mgr.switch_session(uuid).await;
              let _ = tauri::Emitter::emit(&app_handle, "session_switched",
                  serde_json::json!({ "id": id }));
          }
      }
  }
  ```
- **Fold into §2.2.4 and §9 Phase 3** alongside R.10's `skip_switch` change.

### G.4 [BLOCKER] Phase 2 `onCloseRequested` handler must catch `attach_terminal` failures or the detached window becomes unclosable

- **Location:** Plan §4.Q1, §7.1 (Phase 2) — the frontend `onCloseRequested` handler.
- **The bug:** Plan prescribes `getCurrentWindow().onCloseRequested(e => { e.preventDefault(); WindowAPI.attach(sessionId); })`. `preventDefault` is already called synchronously. If the `attach_terminal` invoke throws (transient IPC error, session already destroyed server-side mid-flight, backend panic triggering Tauri error serialization, network split in web-remote mode even though it's no-op there), the promise rejects and the detached window stays open. Clicking X again reproduces the same failure.
- **Impact:** Detached window becomes unclosable. The only way out is force-quitting the app process.
- **Fix:** Wrap attach, fall back to `destroy()`:
  ```ts
  await getCurrentWindow().onCloseRequested(async (e) => {
    e.preventDefault();
    try {
      await WindowAPI.attach(sessionId);
    } catch (err) {
      console.error("[detached] attach failed during close, destroying window:", err);
      try { await getCurrentWindow().destroy(); } catch {}
    }
  });
  ```
- **Fold into §4.Q1** as a mandatory handler skeleton (not optional).

### G.5 [BLOCKER] Phase 2 `close()`-in-destroy-paths triggers `terminal_attached` + `session_switched` emissions that race with `session_destroyed`

- **Location:** R.2 + `session.rs:722-726` (destroy_session_inner) + the new attach_terminal handler.
- **The bug:** R.2 correctly says destroy paths must use `destroy()` to skip the close-requested intercept. But **R.2's table prescription is not self-enforcing** — a future contributor can easily add a new destroy path that uses `close()` (e.g. future "bulk close all detached" feature, shutdown handler, error-recovery path). If that happens, the intercept runs, calls `attach_terminal`, which:
  1. Removes UUID from `DetachedSessionsState` — already removed by `destroy_session_inner` at `session.rs:683-688`, so no-op.
  2. Calls `SessionManager::switch_session(uuid)` — session is already removed from the manager → returns `Err(SessionNotFound)`.
  3. Returns `Err` to the frontend. Frontend's onCloseRequested catches and destroys (per G.4). No `terminal_attached` emitted.

  OK — that specific path is covered. But there's a tighter race: if destroy_session_inner emits `session_destroyed` at `session.rs:720` **before** calling `.close()` at 725, and `close()` is what's used (bug: dev forgot destroy()), the flow is:
  - t0: emit `session_destroyed` → frontend sidebar removes session from store.
  - t1: `.close()` → onCloseRequested fires → invokes `attach_terminal`.
  - t2: `attach_terminal` sees session gone → emits `terminal_attached`.
  - t3: frontend sidebar receives `terminal_attached` for a session that doesn't exist → `sessionsStore.setDetached(id, false)` on an id that is already gone (harmless) but also the main window's `TerminalApp` receives `session_switched` from attach_terminal (the plan says attach_terminal emits session_switched) → this might race with the destroyed state.
- **Impact:** Transient UI inconsistency; logs full of spurious "session not found" errors; harder post-hoc debugging.
- **Fix:** Belt-and-braces:
  1. Keep R.2's destroy() rule in destroy paths.
  2. In `attach_terminal` backend command, **do not emit `terminal_attached` or `session_switched` when the session is not found**. Return Ok(()) silently (the window will be destroyed via the frontend fallback per G.4) or return a typed `SessionNotFound` result that the frontend handler special-cases.
  3. Add a §10 rule: "Do NOT emit `terminal_attached` on the destroy race path — only emit when the attach actually committed a state change."
- **Fold into §2.2.2** (attach_terminal's contract) and §10.

---

### G.6 [HIGH] Re-attach of a restored `was_detached=true` session starts with an empty xterm buffer in main — history loss

- **Location:** Plan §2.2.4 restore path + `src/terminal/components/TerminalView.tsx:30` (per-session xterm cache is populated lazily).
- **The bug:** The plan's invariant "main's hidden xterm quietly consumes pty_output while a session is detached" holds ONLY for sessions that were **first visible in main, then detached**. A session that boots **already-detached on restore** never passes through main's TerminalView's active-show codepath:
  - `TerminalView.tsx:242-256` (`createEffect`): only creates the xterm when `terminalStore.activeSessionId` changes to that session.
  - The restore-detached session's id never gets set as main's `activeSessionId` (it's filtered out of `session_switched` emissions by §4.Q3 + G.3's fix).
  - Therefore main's `terminals` map has NO entry for this session.
  - `TerminalView.tsx:224-234` pty_output fallback: `terminals.get(sessionId) ?? (sessionId === activeSessionId ? createSessionTerminal(sessionId) : null)`. Since `sessionId !== activeSessionId` (it's detached, not active in main), the event is **dropped**. Main accumulates no history.
- **Trigger:** User has detached session A, quits, restarts, then re-attaches A. On re-attach, main's TerminalView fires its createEffect for A, calls `createSessionTerminal` → empty xterm instance. Detached window's xterm (with full accumulated history from restore onwards) is disposed on destroy.
- **Impact:** User sees blank main pane after re-attach. All output produced while the detached window existed is gone from the user's view (still exists in the agent's internal state via `claude --continue` etc., but the visible scrollback is wiped).
- **Fix:** On restore, for each `was_detached=true` session, **pre-create a hidden xterm in main's TerminalView cache** so pty_output events accumulate in it from restore time onwards. Re-attach then has a populated buffer ready to show.
  - Concrete: after restore emits `terminal_detached` events, main's `TerminalView` needs a new "pre-warm" codepath. Either (a) a new event type `terminal_prewarm` that tells main's TerminalView to call `createSessionTerminal(id)` without making it active, or (b) main's TerminalView listens to `terminal_detached` events and, when it sees one for a session not in its `terminals` map, pre-creates a hidden entry. Option (b) is self-contained and minimal — add a listener in `TerminalView.tsx` onMount.
- **Alternative (cheap):** Ship Phase 3 with the known gap documented in §8.7, but not silently — add a test case: "§8.6.26: Restore with A detached. Re-attach A. **Expected:** main shows buffer as of re-attach time, NOT the full detach-era history. (Known limitation; Phase 4 pre-warm fixes.)" That at least calls the regression out instead of surprising the user.
- **Fold into §2.2.4** (pre-warm note) and §8.6 test plan.

### G.7 [HIGH] Concurrent detach + destroy race → orphan detached window for a destroyed session

- **Location:** `window.rs` `detach_terminal` + `session.rs` `destroy_session_inner`.
- **The bug:** `WebviewWindowBuilder::build()` is async and takes 50-200ms on Windows (ConPTY + WebView2 init). Timeline:
  - t0: User clicks detach on session X. `detach_terminal` acquires detached_set, inserts X, drops lock. Begins build() (~100ms).
  - t50ms: User clicks destroy on session X. `destroy_session_inner` runs:
    - Removes X from detached_set.
    - Kills PTY.
    - `get_webview_window("terminal-<X>")` returns `None` (window not built yet).
    - Emits session_destroyed. Returns Ok.
  - t100ms: `detach_terminal`'s `build()` completes. Returns the new WebviewWindow handle to a session that no longer exists.
- **Impact:** Ghost detached window for a destroyed session. Its TerminalApp runs `loadActiveSession()` (terminal/App.tsx:34-46), finds no session matching `lockedSessionId`, sets empty state "Session closed". Window is not auto-closed — user must X-close it. Plus: the window is labelled `terminal-<X>` for a session that doesn't exist; any code that does `get_webview_window("terminal-<X>")` later (won't happen now, but may in future) would get a mismatch.
- **Fix (minimal):** After `build()` succeeds in `detach_terminal`, re-check session exists in manager. If not, destroy the just-built window and don't insert into detached_set (per G.1's post-build insert):
  ```rust
  let win = WebviewWindowBuilder::new(...).build().map_err(|e| e.to_string())?;
  {
      let mgr = session_mgr.read().await;
      if mgr.get_session(uuid).await.is_none() {
          let _ = win.destroy();
          return Err("Session destroyed during window build".into());
      }
  }
  { let mut s = detached.lock().unwrap(); s.insert(uuid); }
  ```
- **Fix (belt-and-braces):** Detached window's TerminalApp already auto-closes on `session_destroyed` when `lockedSessionId` matches (terminal/App.tsx:108-115). Ensure this listener is registered BEFORE the `loadActiveSession` await in onMount so a destroy event arriving during mount is caught. Currently the registration is inside onMount after `loadActiveSession` — move earlier.
- **Fold into §2.2 detach_terminal_inner** as a post-build validation step.

### G.8 [HIGH] `detachedIds` frontend store initial hydration is a race with the restore task's event emissions

- **Location:** Plan §6.4 (note "verify whether the event-driven fill-in is sufficient").
- **The bug:** The restore task at `lib.rs:527` is `tauri::async_runtime::spawn`'d during setup(). It iterates sessions sequentially, each `create_session_inner` doing PTY spawn (~50-200ms each on Windows). If a session has `was_detached=true`, `detach_terminal_inner` emits `terminal_detached` **after** its create completes. These emissions start firing ~50-300ms after app launch.
  Meanwhile the webview bundle loads (~200-500ms to first paint on cold launch), `SidebarApp` mounts, and `onMount` calls `await onTerminalDetached(cb)` to register listeners. On slow machines, this takes >300ms. Events fired BEFORE the listener is registered are **lost** — Tauri's `listen` does not buffer past emissions.
- **Impact:** On a slow cold start, `sessionsStore.detachedIds` stays empty. Sidebar's context menu shows "Open in new window" for sessions that are actually detached (wrong). Clicking "Open in new window" calls `detach_terminal`, which sees the UUID IS in `DetachedSessionsState` (backend is authoritative), returns existing window label, focuses it — OK behaviour-wise, but the sidebar's "detached" visual indicator is wrong.
- **Fix:** Add a `list_detached_sessions` Tauri command returning `Vec<String>` (UUIDs from `DetachedSessionsState`). Call it from `SidebarApp.onMount` **after** listener registration:
  ```ts
  unlisteners.push(await onTerminalDetached(...));
  unlisteners.push(await onTerminalAttached(...));
  // Hydrate: catches any events that fired before we listened.
  const detachedIds = await WindowAPI.listDetached();
  detachedIds.forEach(id => sessionsStore.setDetached(id, true));
  ```
  Idempotent with any late-arriving events. Cost: one new Tauri command, ~15 LoC.
- **Fold into §5.1** (new command), §5.2 (`ipc.ts` addition), §2.3.4 (hydration path).

### G.9 [HIGH] Splitter-drag `pointer-events: none` on terminal pane must be a MUST-fix, not DW-15 "should-fix"

- **Location:** DW.4.c + DW.15 (currently listed as "should-fix" #6).
- **The bug:** Without `pointer-events: none` on `.terminal-host` during drag, xterm.js's canvas renderer starts a text-selection the moment the cursor crosses into the terminal pane. This is trivially reproducible on ANY drag past the min-clamp. Users will experience the splitter as "broken" on every single drag that shrinks the sidebar.
- **Impact:** Splitter feels fundamentally broken. First-launch impression of the feature is "janky".
- **Fix:** Promote DW.4.c (the CSS additions `.main-dragging .terminal-host { pointer-events: none }` + body `user-select: none`) from "should-fix" to part of §3's required CSS. Non-negotiable for Phase 1 ship.
- **Fold into §3** (hardening requirement).

---

### G.10 [MEDIUM] TOCTOU in `detach_terminal` sibling selection

- **Location:** `window.rs:58-82`.
- **The bug:** The command (a) reads sessions list under `session_mgr.read()`, (b) re-locks `detached_set` to filter, (c) releases locks, (d) calls `switch_session(next_id)`. Between (c) and (d), another thread can destroy or detach `next_id`. Result: `switch_session` returns `Err(SessionNotFound)` and `detach_terminal` returns an error — but the detached window has ALREADY been built. Dead end.
- **Impact:** Low probability but non-zero; failure mode is confusing because the detach DID succeed visually (window exists) but the command returned Err to the frontend.
- **Fix:** Do the sibling selection and `switch_session` under a single locked block; OR pick `next_id` via a retry-on-failure pattern (if `switch_session` fails because session disappeared, re-pick). Simpler fix: just tolerate the error — log it, don't return Err, because the detach part DID succeed:
  ```rust
  if let Some(next_id) = next_id {
      let next_uuid = Uuid::parse_str(&next_id).map_err(|e| e.to_string())?;
      if let Err(e) = mgr.switch_session(next_uuid).await {
          log::warn!("[detach] switch to sibling {} failed (session disappeared?): {}", next_id, e);
          let _ = app.emit("session_switched", serde_json::json!({ "id": serde_json::Value::Null }));
      } else {
          let _ = app.emit("session_switched", serde_json::json!({ "id": next_id }));
      }
  }
  ```
- **Fold into §5.1** `detach_terminal_inner` signature note.

### G.11 [MEDIUM] Orphaned settings: `raise_terminal_on_click` and `sidebar_always_on_top` become dead storage in unified mode

- **Location:** `settings.rs:50-51, 47-48` + `src/sidebar/App.tsx:70, 75-78`.
- **The bug:** `raise_terminal_on_click` gates the `handleRaiseTerminal` mousedown handler that the plan removes in embedded mode (DW.2). The setting stays in settings.json but does nothing. `sidebar_always_on_top` applies `setAlwaysOnTop(true)` to the sidebar window that no longer exists. DW.12 flags sidebar_always_on_top explicitly; the plan hasn't picked (a)/(b)/(c). `raise_terminal_on_click` isn't flagged at all.
- **Impact:** Settings JSON accumulates dead fields that confuse power-users who edit the file. Debug footprint.
- **Fix:** Architect must pick:
  - `sidebar_always_on_top` → decision from DW.12 options (my bias: rename to `main_always_on_top`, migrate on first boot). Pick one.
  - `raise_terminal_on_click` → silently ignored in unified mode; deprecate via `#[serde(default, skip_serializing_if = "Option::is_none")]` on first save, document as "no effect in 0.8+".
- **Fold into §6.1**.

### G.12 [MEDIUM] Unified-mode CSS import order is unspecified — global rules collide

- **Location:** New `src/main/App.tsx` per plan §5.2.
- **The bug:** `src/main/App.tsx` composes `SidebarApp` + splitter + `TerminalApp`. Both `sidebar.css` and `terminal.css` get imported (directly or transitively). Both files contain global rules scoped only by `html.light-theme` or bare selectors (`body`, `html`, xterm-related styles). Last-import wins for conflicting rules.
  - Verified: `terminal.css:203-208` has `html.light-theme .last-prompt-panel { ... }`. `sidebar.css` has many `html.light-theme ...` rules.
  - Harder to predict: font-family on `body`, custom scrollbar, selection color.
- **Impact:** Unified window may have subtly different styling for one pane vs what it had in the two-window model. No functional break, but visual regressions ("the terminal font looks slightly different now").
- **Fix:** Plan `src/main/styles/main.css` to use CSS cascade layers (`@layer sidebar, terminal, main;`) so conflicting rules have explicit precedence. Or manually audit global rules in both CSS files and scope any that leak (e.g. `body {font-family: ...}` becomes `.sidebar-layout {font-family: ...}` + `.terminal-layout {font-family: ...}`).
- **Fold into §5.2** + §3 (CSS requirements).

### G.13 [MEDIUM] Main window X-close leaves app running with orphaned detached windows and no path back to main

- **Location:** Plan's implicit "main window is always available while app runs" + R.4 "defensive re-creation via focus_main_window".
- **The bug:** Current codebase has no quit-on-close handler. Today's two-window model is forgiving — sidebar is a separate window, user can reach terminal even if sidebar is closed (via taskbar). In unified mode, closing main (via X) leaves:
  - Process alive (tokio runtime, PTY manager, session manager).
  - Detached windows alive (with xterms and PTYs).
  - No UI surface in main → no sidebar → no way to create a new session or re-attach any detached session.
  - `focus_main_window` command has no caller from a detached window's UI (there's no sidebar there to host the trigger).
- **Impact:** User ends up with orphaned detached windows and a running background process; cleanup requires force-quit. Worse on Windows where the task manager shows only the process, not the "Commander" shell.
- **Fix:** Pick ONE behaviour explicitly in the plan:
  - (a) **Quit app on main X-close.** Closes all detached windows as a side effect. Matches most desktop app conventions. Risk: user loses detached work.
  - (b) **Intercept main's close-requested, hide instead of close.** Add a tray icon to restore. Bigger scope but preserves detached work.
  - (c) **Confirmation dialog.** "Closing main will leave N detached sessions running in the background. Quit the app? / Hide main?"
  - My bias: **(a)** — the main window IS the app. Detached windows are satellites. Closing the primary closes everything. If user wants to keep sessions around, they should minimise, not close.
- **Fold into §4.Q1 / §4.Q6** as a new sub-decision.

### G.14 [MEDIUM] `save_settings` in `settings.rs:354` is a NON-atomic write; the splitter drag dramatically increases save frequency

- **Location:** `src-tauri/src/config/settings.rs:354` (`std::fs::write(&path, json)`).
- **The bug:** Pre-existing, but: Today settings.json is saved infrequently (on user edit). After this plan, splitter drag triggers a save every drag-end (debounced 500ms). A crash mid-write truncates settings.json → next load falls back to `AppSettings::default()` → user loses root_token, agents, telegram bots, zoom levels, window geometry — everything.
- **Impact:** Catastrophic settings loss on power failure or OS crash during a splitter drag. Probability low, impact high.
- **Fix:** Mirror `sessions_persistence.rs:290-296`'s atomic-write pattern:
  ```rust
  let tmp_path = dir.join("settings.json.tmp");
  std::fs::write(&tmp_path, json)?;
  std::fs::rename(&tmp_path, &path)?;
  ```
- **Fold into §5.1** as a hardening row (non-blocking but highly recommended).

### G.15 [MEDIUM] Zoom-save race confirmed; supports DW.6 acceptance

- **Location:** `src/shared/zoom.ts:46-59` debouncedSave.
- **The bug:** If main reuses `sidebarZoom` (MVP path per architect's §11.Q3), and both `initZoom("sidebar")` + `initZoom("terminal")` fire in the unified window (even with DW.2's "skip in embedded terminal" rule, the rule is a human invariant that can be broken in a future refactor), both debouncedSaves run: both read SettingsAPI.get around the same 500ms window, both write back their keys. The key written last wins for unrelated keys → data loss on the non-last-written zoom key.
- **Impact:** Under a specific but plausible refactor misstep, zoom values silently disappear.
- **Fix:** Accept DW.6 (introduce `mainZoom` in Phase 1). Each initZoom writes to a distinct key; races on a single key only occur if BOTH handlers target the same key, which can't happen with a dedicated `mainZoom`.
- **Arbitration:** I side with DW.6 — see Arbitrations section.

### G.16 [MEDIUM] Plan §8.8 test step 36 uses snake_case JSON keys; actual keys are camelCase

- **Location:** Plan §8.8 step 36 wording.
- **The bug:** `AppSettings` derives `#[serde(rename_all = "camelCase")]` (verified `settings.rs:33`). Disk JSON keys are `sidebarGeometry`, `terminalGeometry`. Test step 36 says "Copy an old settings.json (with `sidebar_geometry` + `terminal_geometry`, no `main_geometry`)". A developer literally following this step would be confused.
- **Impact:** Test-plan clarity only.
- **Fix:** Rewrite test step using `sidebarGeometry` / `terminalGeometry` / `mainGeometry`.

---

### G.17 [LOW] `was_detached` is stale between deferred-session wake and next persist

- **Location:** R.9 + `restart_session` flow.
- **The bug:** Deferred session has `was_detached=true` (persisted from a previous run where it was detached). On wake via `restart_session`, new session is created attached. Next persist corrects `was_detached=false`. Between wake and next persist, if app crashes, next restart re-detaches the session.
- **Impact:** One extra detached-on-restore on a crash window. Recoverable by one-click re-attach.
- **Fix:** `restart_session` should explicitly call `persist_current_state` as its last step (it already does at 895-899). The gap is pre-wake-to-create — negligible. No action needed beyond R.9's skip-detach-on-deferred-restore rule.

### G.18 [LOW] Plan §5.1 row 207 under-specifies the per-session restore detach ordering

- **Location:** Plan §5.1 row for `lib.rs:608-614` + R.10.
- **The bug:** Plan says "iterate persisted and for each `ps.was_detached == true`, invoke `detach_terminal_inner`". Does not say WHERE in the existing restore loop (before/after create_session_inner, inside/outside the per-session iteration). R.10 clarifies it's inside with `skip_switch:true`.
- **Fix:** Fold R.10's skip_switch + G.3's post-loop filter into the plan's §5.1 row text. Specify: "inside per-session loop, AFTER successful create_session_inner, call detach_terminal_inner(skip_switch=true, geometry=ps.detached_geometry)". Keep the post-loop active_id-switch separate, with G.3's detached-filter.

### G.19 [LOW] Label-based Destroyed cleanup at `lib.rs:697-717` is a brittle namespace

- **Location:** `lib.rs:697-717`.
- **The bug:** Any future window labelled `terminal-<32 hex chars>` where the hex happens to parse as a UUID is treated as a detached terminal and removed from `DetachedSessionsState`. Harmless today (UUIDs are unique), but the prefix is a shared namespace.
- **Fix (not for this plan):** v0.9 migration — use a more explicit prefix like `tdetach-v1-<uuid>`. Out of scope.

### G.20 [LOW] `save_settings` is NOT called inside `load_settings` unless `root_token` is missing — migration runs in-memory only

- **Location:** `settings.rs:330-337`.
- **The bug:** Plan's §6.5 migration (seed `main_geometry` from `terminal_geometry`) runs inside `load_settings` before the root_token check. After v0.7 → v0.8 upgrade, most users already have a root_token → no save triggered → migration runs in RAM only. Deprecated fields persist on disk until the first user-initiated save (splitter drag, theme toggle, agent edit). In the mean time, if the user downgrades, the v0.7 code sees the v0.7 shape intact — which is actually fine (downgrade-safe).
- **Impact:** Documentation / mental-model precision only. Cleanup is eventually consistent.
- **Fix:** Document this in §6.5; no code change.

---

### G.FOOTER — What I simulated that the plan SURVIVED

Per the reviewer's rules, I must say where I tried to break the plan and could not.

- **Broadcast vs emit_to for pty_output (§4.Q5):** attempted attack — can a detached window briefly leak another session's bytes during detach-spawn → mount → listener-ready? **Plan held up.** Before the detached window's `TerminalView` onMount runs, the window has no xterm and no listener. The main window's TerminalView already filters (`terminals.get(sessionId) ?? null`). During the window-build + bundle-load interval, no xterm exists anywhere in the detached document — events are processed by the global Tauri listener registry only when `listen()` has been called. No leak.
  - **Weak defense caveat (open question):** this relies on Tauri's listen() being registered only after `onMount` fires. If a future refactor moves the listen earlier (say, into a module-level side-effect), a late-arriving pty_output for a non-owned session would be received and routed to `terminals.get(sessionId)` — which would be empty → dropped. So even that refactor would hold. I cannot break this.

- **Re-attach window.close race (§4.Q6):** attempted — can `close_requested` fire multiple times if the user rapidly clicks X? Test: user spam-clicks X. preventDefault is synchronous → all close attempts are cancelled. attach_terminal is debounced by its await — but concurrent invocations with the same sessionId would all try to destroy the same window. Second destroy() is a no-op, frontend might see a brief visual duplicate. Not a bug. **Plan held up with minor UX transient.**

- **Browser-mode routing (R.8):** R.8 already identified the hole; I confirmed the fix by reading `main.tsx:33-39` — the `!isTauri` check is there but in a weird position. R.8's skeleton is correct.

- **Migration downgrade 0.8 → 0.7.5:** added `was_detached` and `main*` fields. 0.7.5's serde does not use `deny_unknown_fields` (verified `settings.rs:33` has only rename_all, not deny_unknown). Unknown fields are silently dropped. **Downgrade-safe for both settings and sessions.**

- **Multi-detach PTY bandwidth (§4.Q5 bandwidth claim):** sanity-checked — Tauri 2.10.3 WebView IPC for events is an internal postMessage channel with Rust→JS serialization via `serde_json`. Each pty_output is ~O(bytes) per listener. With 3 active agents × 4 listener windows = 12 round-trips per PTY tick. The rate-limit is the PTY read-loop itself (10-50ms) × average chunk size. Plausibly 500KB/s aggregate on a busy system. Modern machines handle this comfortably. **Hand-wave defense, but I can't force-break it in a synthesised stress test that's within the plan's Phase 1 scope.**

---

### G.ARB — Conflict arbitrations

**Arb-1 — DW.13 vs R.6 on `detached_geometry` placement**

**Winner: R.6.** Put `detached_geometry: Option<WindowGeometry>` on `PersistedSession`, NOT `HashMap<SessionId, WindowGeometry>` on `AppSettings`.

Reasoning (decisive point first):
1. **Auto-GC is the deciding factor.** `AppSettings`-level HashMap has no GC — every session ever detached accumulates an entry, cleared only by an explicit prune-on-destroy hook that future contributors will forget (see how `destroy_session_inner` already threads 6 cleanup concerns). `PersistedSession` is auto-GC'd: when the session row vanishes, its geometry vanishes with it. This matches how `was_active`, `last_prompt`, all other per-session facts are already handled.
2. **Semantic correctness.** AppSettings = "app globals". PersistedSession = "this session's facts". Detach geometry is per-session.
3. **Simpler persist path.** Geometry writes flow through the existing `snapshot_sessions` → `save_sessions` pipeline. No new sync primitive. One new Tauri command (`set_detached_geometry`) rather than extending the AppSettings save cadence.
4. **Downgrade-safe both ways** (both variants rely on `#[serde(default)]` and unknown-field tolerance).

DW.13's counter ("easier for frontend to read `settings.detachedGeometry[sessionId]` once") is weak: frontend can equally well read `session.detachedGeometry` on the SessionInfo it already has.

DW.13's concern ("geometry should survive destroy-while-detached → recreate-same-CWD") is a genuine edge case, but infrequent and one-click recoverable — not worth the GC burden.

**Action:** Update §5.2 `window-geometry.ts` note to R.6's option (a): add `initDetachedWindowGeometry(sessionId)` that writes via a new `set_detached_geometry` Tauri command. Drop DW.13's implication that it lives on AppSettings.

**Arb-2 — DW.6 vs architect §11.Q3 on zoom key**

**Winner: DW.6.** Introduce `mainZoom` in Phase 1; keep `terminalZoom` for detached; deprecate `sidebarZoom`.

Reasoning:
- Race prevention (G.15): A dedicated `mainZoom` key makes the "don't register two initZooms" invariant self-enforcing at the data layer, not just at the runtime layer.
- Semantic honesty: Ctrl+= in main zooms the whole window (sidebar + terminal). A setting named `sidebarZoom` that does that is a trap for the next maintainer.
- Cost is ~10 LOC.

**Action:** §11.Q3 should be resolved as "introduce `mainZoom` in Phase 1" (already DW.6's prescription).

**Arb-3 — R.4 vs architect §11.Q5 on `focus_main_window`**

**Winner: R.4.** Keep `focus_main_window` + `ensureTerminal` alias.

Reasoning: 9 verified call sites. Rename gains clarity; deletion is churn without benefit.

**Action:** Strike "If so, delete the command" from §11.Q5.

**Arb-4 — DW.5's "skip onThemeChanged in embedded terminal" vs architect §2.3.4**

**Winner: DW.5.** Agreed, uncontested.

Reasoning: When sidebar and terminal coexist in one document, both `onThemeChanged` handlers would mutate `document.documentElement.classList` — idempotent, but burns a reflow cycle per theme toggle. Skip in embedded mode.

**Action:** Fold DW.5's skip list into §2.3.3's `embedded` contract.

---

### G.Z — Where the architect and devs should look first (priority stack)

Ordered by risk × effort:

1. **G.1** — single-line swap (build then insert). Lowest effort, highest payoff; prevents a permanent-dead-session class of bug.
2. **G.2 + G.3** — filter `new_active` and `active_id` against `DetachedSessionsState`. ~30 LOC total. Fixes duplicate-display on both destroy and restore paths.
3. **G.4** — try/catch in onCloseRequested. 5 LOC. Prevents unclosable window class of bug.
4. **G.6** — pre-warm main's xterm cache for restored-detached sessions. ~40 LOC of frontend. Fixes the "re-attach shows blank" regression.
5. **G.13** — pick a main-window-close behaviour. Architectural decision, not code.

Everything else is either MEDIUM polish, LOW documentation, or a pre-existing issue this plan could fix as a bonus.

### G.Y — Explicit approvals

Things I attacked and confirm: **I cannot break them.**

- `was_detached` persistence shape (plan §6.2) — with R.6's placement refinement.
- Broadcast + client-filter pty_output (§4.Q5) — across detach / re-attach / destroy + multi-window combinations. Held up.
- X-button as re-attach (§4.Q1, §4.Q6) — UX invariant is correct, mechanism (onCloseRequested + preventDefault + attach_terminal) is correct, modulo G.4 + G.5.
- Unified-mode splitter design (§3 + DW.3/DW.4) — structurally sound, modulo G.9 elevation.
- The 4-phase rollout shape. Phase boundaries are clean.

Adversarial review complete. Standing by for round 2.

---

## Architect round 2 — grinch integration

**Author:** architect
**Scope:** resolve grinch's 5 BLOCKER + 4 HIGH + 7 MEDIUM + 4 LOW findings, accept the 4 arbitrations, close the remaining §11 open items, and update §9 phasing + §5 impact map.
**Anchored against HEAD:** `60dd162` (unchanged since round 1).
**Verdict up front:** 24 findings/arbitrations total, **23 accepted** (9 with modification), **0 rejected**, **1 deferred** (G.19 — v0.9 follow-up per grinch's own recommendation). No architect-escalation needed; G.13 resolved below with reasoning.

### A2.0 Round-2 summary (TL;DR)

- **All 5 BLOCKERs** accepted with concrete plan-text deltas below. G.1 + G.2 + G.3 + G.7 + G.9 move into Phase 1 (Phase 1 scope grows from ~60% to ~70% of the feature diff; still demoable at the same milestone).
- **All 4 HIGH** accepted: G.4 + G.5 + G.8 stay in Phase 2; G.6 pre-warm moves into Phase 3 alongside the restore loop.
- **All 4 arbitrations** accepted as grinch called them (Arb-1 R.6 wins, Arb-2 DW.6 wins, Arb-3 R.4 wins, Arb-4 DW.5 uncontested). Concrete plan deltas below.
- **G.13 (main-window X)** architect-called as **quit-on-X with a confirmation dialog only when detached windows are currently open**. Reasoning + fallback options documented. Not escalating; the implementation is trivially swappable if the user later prefers a different flavor.
- **§11 fully closed.** Q1 → R.2 (destroy() mandatory). Q2 → Arb-1 (lives on PersistedSession). Q3 → Arb-2 (mainZoom in Phase 1). Q4 → DW.5 (audit done). Q5 → Arb-3 (keep focus_main_window).

### A2.1 Triage table (all 24 items)

| # | Finding | Severity | Disposition | Phase landing |
|---|---|---|---|---|
| G.1 | `detach_terminal` leaks DetachedSessionsState on build failure | BLOCKER | **Accept** | Phase 1 |
| G.2 | `destroy_session_inner` emits session_switched for detached UUID | BLOCKER | **Accept** | Phase 1 |
| G.3 | Restore-loop active_id switch skips detached filter | BLOCKER | **Accept** | Phase 3 (restore-path work) |
| G.4 | onCloseRequested must catch attach failure | BLOCKER | **Accept** | Phase 2 |
| G.5 | `close()`-in-destroy emits terminal_attached race | BLOCKER | **Accept with modification** | Phase 2 |
| G.6 | Re-attach of restored-detached shows empty buffer | HIGH | **Accept** | Phase 3 |
| G.7 | Concurrent detach + destroy orphans window | HIGH | **Accept** | Phase 1 |
| G.8 | detachedIds hydration race with restore | HIGH | **Accept** | Phase 2 |
| G.9 | Splitter drag pointer-events must be MUST-fix | HIGH | **Accept (promote to §3 required)** | Phase 1 |
| G.10 | TOCTOU in detach sibling selection | MEDIUM | **Accept** (tolerate, log, emit null) | Phase 1 |
| G.11 | Orphan settings raise_terminal_on_click + sidebar_always_on_top | MEDIUM | **Accept** (rename one, deprecate other — see A2.4.G11) | Phase 1 |
| G.12 | Unified-mode CSS import order | MEDIUM | **Accept** (manual scope audit; cascade layers optional) | Phase 1 |
| G.13 | Main-window X behavior | MEDIUM | **Accept** (decision: quit-with-confirm-if-detached — see A2.6) | Phase 1 |
| G.14 | Non-atomic save_settings write | MEDIUM | **Accept** (atomic write; mirror sessions_persistence pattern) | Phase 1 |
| G.15 | Zoom-save race | MEDIUM | **Accept** (auto-closed by Arb-2 mainZoom) | Phase 1 |
| G.16 | Test §8.8 step 36 snake_case typo | MEDIUM | **Accept** (wording fix) | Phase 1 |
| G.17 | was_detached stale between deferred wake + persist | LOW | **Accept** (no action per grinch; document) | Phase 3 |
| G.18 | §5.1 row 207 under-specifies restore detach ordering | LOW | **Accept** (wording fix; fold R.10 skip_switch text) | Phase 3 |
| G.19 | Label-based Destroyed cleanup is brittle namespace | LOW | **Defer** (grinch: out of scope for v0.8) | — |
| G.20 | save_settings not called inside load_settings — migration in-RAM | LOW | **Accept** (document only; no code change) | Phase 1 |
| Arb-1 | detached_geometry placement | — | **Accept R.6** (PersistedSession, not AppSettings) | Phase 3 |
| Arb-2 | Zoom key | — | **Accept DW.6** (mainZoom in Phase 1) | Phase 1 |
| Arb-3 | focus_main_window disposition | — | **Accept R.4** (keep) | Phase 1 |
| Arb-4 | onThemeChanged skip in embedded | — | **Accept DW.5** (uncontested) | Phase 1 |

**Totals:** accepted 23, modified 9, rejected 0, deferred 1.

### A2.2 BLOCKER resolutions (G.1–G.5)

#### A2.2.G1 — `detach_terminal` leaks DetachedSessionsState on build failure — **Accept**

**Plan-text delta:** update §5.1 row for `src-tauri/src/commands/window.rs:11-92` to require the post-build insert pattern in the extracted `detach_terminal_inner`:

```rust
pub(crate) async fn detach_terminal_inner(
    app: &AppHandle,
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    detached: &DetachedSessionsState,
    session_id: &str,
    geometry: Option<WindowGeometry>,    // from Arb-1 (§6.2 detached_geometry)
    skip_switch: bool,                    // from R.10 (Phase 3 restore path)
) -> Result<String, String> {
    let uuid = Uuid::parse_str(session_id).map_err(|e| e.to_string())?;
    let label = format!("terminal-{}", session_id.replace('-', ""));
    let url = format!("index.html?window=detached&sessionId={}", session_id);

    // Focus-existing short-circuit FIRST (matches current behavior)
    if let Some(existing) = app.get_webview_window(&label) {
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(label);
    }

    // BUILD window first — any failure short-circuits with no state mutation.
    let icon = tauri::image::Image::from_bytes(include_bytes!("../../icons/icon.png"))
        .expect("Failed to load app icon");

    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("Terminal [detached]")
        .icon(icon).map_err(|e| e.to_string())?
        .min_inner_size(400.0, 300.0)
        .decorations(false)
        .zoom_hotkeys_enabled(true);
    if let Some(geo) = geometry {
        builder = builder.inner_size(geo.width, geo.height).position(geo.x, geo.y);
    } else {
        builder = builder.inner_size(900.0, 600.0);
    }
    let win = builder.build().map_err(|e| e.to_string())?;

    // POST-BUILD session-existence check (from G.7) — destroy and bail if gone.
    {
        let mgr = session_mgr.read().await;
        if mgr.get_session(uuid).await.is_none() {
            let _ = win.destroy();
            return Err("Session destroyed during window build".into());
        }
    }

    // ONLY NOW insert into DetachedSessionsState — build succeeded, session lives.
    { let mut s = detached.lock().unwrap(); s.insert(uuid); }

    let _ = app.emit("terminal_detached",
        serde_json::json!({ "sessionId": session_id, "windowLabel": label }));

    // Sibling-switch (with G.10 tolerance) — skipped on restore path.
    if !skip_switch {
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        let next_id = {
            let set = detached.lock().unwrap();
            sessions.iter()
                .find(|s| Uuid::parse_str(&s.id).ok().map_or(false, |u| !set.contains(&u)))
                .map(|s| s.id.clone())
        };
        if let Some(next_id) = next_id {
            let next_uuid = Uuid::parse_str(&next_id).map_err(|e| e.to_string())?;
            match mgr.switch_session(next_uuid).await {
                Ok(()) => {
                    let _ = app.emit("session_switched",
                        serde_json::json!({ "id": next_id }));
                }
                Err(e) => {
                    log::warn!("[detach] switch to sibling {} failed: {}", next_id, e);
                    let _ = app.emit("session_switched",
                        serde_json::json!({ "id": serde_json::Value::Null }));
                }
            }
        } else {
            let _ = app.emit("session_switched",
                serde_json::json!({ "id": serde_json::Value::Null }));
        }
    }

    Ok(label)
}
```

**Section impact:**
- §2.2.2 row for `detach_terminal`: replace the "change URL only" description with "extract + harden into `detach_terminal_inner` per round-2 A2.2.G1 skeleton".
- §5.1: replace the single row for `commands/window.rs:11-92` with one row pointing to A2.2.G1 as the authoritative function body.
- §9 Phase 1: add ship-bar item "`detach_terminal_inner` must match A2.2.G1 skeleton (post-build insert + post-build session check + G.10 sibling-switch tolerance)".

#### A2.2.G2 — `destroy_session_inner` emits session_switched for detached UUID — **Accept**

**Plan-text delta:** add a new §5.1 row modifying `src-tauri/src/commands/session.rs:728-734` (inside `destroy_session_inner`). Implementation verbatim from grinch G.2:

```rust
// REPLACES session.rs:728-734
if let Some(new_id) = new_active {
    let is_detached = {
        let detached = app.state::<DetachedSessionsState>();
        let set = detached.lock().unwrap();
        set.contains(&new_id)
    };
    if is_detached {
        // The manager's chosen "next active" is a detached session.
        // Walk the list for the first non-detached and switch there.
        let fallback = {
            let sessions = mgr.list_sessions().await;
            let detached = app.state::<DetachedSessionsState>();
            let set = detached.lock().unwrap();
            sessions.iter().find_map(|s|
                Uuid::parse_str(&s.id).ok().filter(|u| !set.contains(u)))
        };
        if let Some(fb) = fallback {
            let _ = mgr.switch_session(fb).await;
            let _ = app.emit("session_switched",
                serde_json::json!({ "id": fb.to_string() }));
        } else {
            let _ = app.emit("session_switched",
                serde_json::json!({ "id": serde_json::Value::Null }));
        }
    } else {
        let _ = app.emit("session_switched",
            serde_json::json!({ "id": new_id.to_string() }));
    }
}
```

**Section impact:**
- §5.1: add a new row for `commands/session.rs:728-734` modify (in addition to the existing row for 736-743 removal). Rewrite per above.
- This is a **pre-existing bug** the plan institutionalises; fixing it during Phase 1 is cheap and prevents a cross-version UX flip. Confirmed in grinch's §G.2 framing.

#### A2.2.G3 — Restore loop emits session_switched(was_active) for a detached UUID — **Accept**

**Plan-text delta:** replace the `lib.rs:607-614` block in §2.2.4 and §5.1 with grinch's G.3 code, tweaked to match the plan's variable names (`app_handle`, `session_mgr_clone`):

```rust
// REPLACES lib.rs:607-614
if let Some(id) = active_id {
    if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
        let is_detached = {
            let detached = app_handle.state::<DetachedSessionsState>();
            let set = detached.lock().unwrap();
            set.contains(&uuid)
        };
        let mgr = session_mgr_clone.read().await;
        if is_detached {
            let sessions = mgr.list_sessions().await;
            let fallback = {
                let detached = app_handle.state::<DetachedSessionsState>();
                let set = detached.lock().unwrap();
                sessions.iter().find_map(|s|
                    Uuid::parse_str(&s.id).ok().filter(|u| !set.contains(u)))
            };
            if let Some(fb) = fallback {
                let _ = mgr.switch_session(fb).await;
                let _ = tauri::Emitter::emit(&app_handle, "session_switched",
                    serde_json::json!({ "id": fb.to_string() }));
            } else {
                let _ = tauri::Emitter::emit(&app_handle, "session_switched",
                    serde_json::json!({ "id": serde_json::Value::Null }));
            }
        } else {
            let _ = mgr.switch_session(uuid).await;
            let _ = tauri::Emitter::emit(&app_handle, "session_switched",
                serde_json::json!({ "id": id }));
        }
    }
}
```

**Section impact:**
- §2.2 add sub-bullet under item 4 (restore): "After the per-session detach pass completes (all `was_detached=true` sessions now populate `DetachedSessionsState`), the post-loop `active_id` switch MUST filter against `DetachedSessionsState` per A2.2.G3 skeleton."
- §5.1: tighten the `lib.rs:608-614` row from "modify" to "rewrite per A2.2.G3" and change the phase tag from implicit Phase 3 to **explicit Phase 3**. Do NOT land A2.2.G3 in Phase 1 — the change is only meaningful once Phase 3's `was_detached` restore is live; landing it earlier is harmless no-op, but also churn.
- §9 Phase 3 ship-bar gets one new item: "restore-path post-loop switch uses A2.2.G3's detached-filter".

#### A2.2.G4 — Phase 2 onCloseRequested must catch attach_terminal failure — **Accept**

**Plan-text delta:** §4.Q1 gets the mandatory handler skeleton (was: optional example):

```typescript
// In TerminalApp.tsx onMount, when props.detached && props.lockedSessionId:
const sessionId = props.lockedSessionId!;
const win = (await import("@tauri-apps/api/window")).getCurrentWindow();
const unlistenCloseRequested = await win.onCloseRequested(async (e) => {
  e.preventDefault();
  try {
    await WindowAPI.attach(sessionId);
  } catch (err) {
    console.error("[detached] attach failed during close; destroying window:", err);
    try { await win.destroy(); } catch { /* best-effort */ }
  }
});
// Register in the same unlisteners[] array that TerminalApp already maintains.
```

**Section impact:**
- §4.Q1 paragraph "How X = re-attach is implemented": replace the one-line inline example with the full skeleton above, marked MANDATORY.
- §5.2: add a sub-bullet under `src/terminal/App.tsx` modify: "Register `onCloseRequested` handler per A2.2.G4 skeleton inside onMount when `props.detached`. Must appear BEFORE `loadActiveSession` await to avoid the mount-race documented in G.7."
- §9 Phase 2 ship-bar: "onCloseRequested handler uses A2.2.G4 skeleton; test case proves detached window closes cleanly when attach_terminal returns Err."

#### A2.2.G5 — `attach_terminal` silent-no-op when session not found — **Accept with modification**

Grinch's proposed fix is three parts. I accept parts 1 and 2 verbatim; part 3 (new §10 rule) I reshape into a §2.2.2 contract clause so it lives with the command definition, not just as a prohibition.

**Part 1 (accept):** `destroy()` discipline in all destroy paths per R.2. Already folded in via R.2 acceptance.

**Part 2 (accept):** `attach_terminal` backend command: return `Ok(())` without emitting `terminal_attached` or `session_switched` when the session is absent from `SessionManager`.

**Part 3 (reshape):** move from §10 ("do NOT") to §2.2.2 `attach_terminal` contract (positive statement). Add this bullet to §2.2.2's description of `attach_terminal`:

> **Contract clause (A2.2.G5)**: `attach_terminal` MUST check `SessionManager::get_session(uuid)` before emitting `terminal_attached` or `session_switched`. If the session is gone (destroyed mid-flight), return `Ok(())` silently. The frontend's onCloseRequested fallback (A2.2.G4) handles the window-destroy side; the backend just doesn't emit phantom events for a dead session.

**Canonical attach_terminal body** (for §5.1):

```rust
#[tauri::command]
pub async fn attach_terminal(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    detached: State<'_, DetachedSessionsState>,
    session_id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;

    // Remove from DetachedSessionsState first (idempotent).
    { let mut s = detached.lock().unwrap(); s.remove(&uuid); }

    // Close the detached window if present.
    let label = format!("terminal-{}", session_id.replace('-', ""));
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.destroy(); // R.2: destroy(), not close()
    }

    // A2.2.G5 contract: only emit events if the session still lives in the manager.
    let mgr = session_mgr.read().await;
    if mgr.get_session(uuid).await.is_none() {
        log::info!("[attach] session {} already destroyed; silent no-op", session_id);
        return Ok(());
    }

    // Session lives → promote to active in main.
    mgr.switch_session(uuid).await.map_err(|e| e.to_string())?;
    let _ = app.emit("terminal_attached",
        serde_json::json!({ "sessionId": session_id }));
    let _ = app.emit("session_switched",
        serde_json::json!({ "id": session_id }));

    Ok(())
}
```

**Section impact:**
- §2.2.2: add the `attach_terminal` row with the A2.2.G5 contract clause.
- §5.1: new row for `src-tauri/src/commands/window.rs` NEW `attach_terminal` pointing to A2.2.G5 skeleton.
- §10: add rule "`attach_terminal` MUST NOT emit `terminal_attached` or `session_switched` when SessionManager::get_session returns None. Return Ok(()) silently."

### A2.3 HIGH resolutions (G.6–G.9)

#### A2.3.G6 — Re-attach of restored `was_detached=true` shows empty buffer — **Accept** (grinch option b: listener-based pre-warm)

Grinch offered two options: (a) pre-warm via a new `terminal_prewarm` event, (b) main's `TerminalView` subscribes to `terminal_detached` events and pre-creates hidden entries. **Option (b) is correct** — self-contained in TerminalView, no new event type, no coordination between Rust and frontend beyond the existing event.

**Plan-text delta:** add to §2.2.4 restore path:

> **Pre-warm contract (A2.3.G6)**: main window's `TerminalView` subscribes to `terminal_detached` events. On each event, if the session id is NOT already in `terminals` map, create a hidden xterm entry (same code path as `createSessionTerminal` at `TerminalView.tsx:75-185`, but leave `container.hidden = true`). This ensures pty_output accumulates in main's cache from the moment the detached window spawns (during restore, immediately after PTY boot). On re-attach, `showSessionTerminal` promotes the pre-warmed entry to visible with full history intact.

**Concrete code** for `TerminalView.tsx` onMount (~15 lines):

```typescript
// Pre-warm main's per-session cache for detached sessions.
// This runs ONLY in main; detached windows have no sessionsStore and
// don't receive their own terminal_detached (they ARE the detached window).
// Guard: only pre-warm when we're not locked to a specific session.
const isMainWindow = !props.lockedSessionId;
if (isMainWindow) {
  unlisten_terminal_detached = await onTerminalDetached(({ sessionId }) => {
    if (!terminals.has(sessionId)) {
      const entry = createSessionTerminal(sessionId);
      entry.container.hidden = true; // remain invisible until activated in main
    }
  });
}
```

**Section impact:**
- §2.2.4 add the pre-warm contract paragraph as item 5.
- §5.2: extend the `src/terminal/components/TerminalView.tsx` row from "unchanged" to "add pre-warm listener per A2.3.G6 (~15 LoC)". Update §5.3 "Files NOT touched" to REMOVE `TerminalView.tsx` from the list (it now has a 15-line addition).
- §8.6 test plan: new step §8.6.26: "Restart with detached session A. Let main pane sit empty. Let A's PTY produce ~100 lines of output in the detached window. Re-attach A. Main now shows A with full 100 lines of scrollback visible. (A2.3.G6 pre-warm proof.)"
- **Phase landing:** Phase 3 (pre-warm is only needed when restoring detached sessions; pointless without `was_detached` being live). Grinch's "alternative cheap" (document gap + ship) is rejected in favour of the listener-based fix.

#### A2.3.G7 — Concurrent detach + destroy orphan window — **Accept**

**Plan-text delta:** the post-build session-existence check is already folded into A2.2.G1's `detach_terminal_inner` skeleton. Additionally per grinch's belt-and-braces recommendation:

**`src/terminal/App.tsx`** — move the `onSessionDestroyed` listener registration BEFORE the `loadActiveSession` await in onMount. Current code at `terminal/App.tsx:106-120`:

```typescript
// CURRENT (bad): loadActiveSession runs before session_destroyed listener registers.
onMount(async () => {
  // ... registerShortcuts, initZoom, initWindowGeometry, settingsStore.load ...
  await loadActiveSession();  // line 67 — awaits here
  // ... other listeners ...
  unlisteners.push(await onSessionDestroyed(async ({ id }) => { ... })); // line 107
});
```

Rewrite to register the destroy listener FIRST (before any await):

```typescript
onMount(async () => {
  document.documentElement.classList.add("light-theme");
  shortcutHandler = registerShortcuts();

  // Register destroy listener FIRST to catch any destroy event fired
  // during the other awaits below (G.7 race window).
  unlisteners.push(
    await onSessionDestroyed(async ({ id }) => {
      if (props.lockedSessionId && id === props.lockedSessionId) {
        if (isTauri) {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          getCurrentWindow().destroy(); // R.2: destroy(), not close()
        }
        return;
      }
      if (!props.lockedSessionId) {
        await loadActiveSession();
      }
    })
  );

  cleanupZoom = await initZoom(props.embedded ? "main" : "terminal"); // Arb-2
  cleanupGeometry = await initWindowGeometry(
    props.embedded ? "main" : (props.detached ? "detached" : "terminal")
  );
  settingsStore.load();
  await loadActiveSession();
  // ... other listeners after loadActiveSession ...
});
```

**Section impact:**
- §5.2 row for `src/terminal/App.tsx`: add the listener-order hardening per A2.3.G7.
- §2.2.2 `detach_terminal_inner` contract bullet: "post-build must re-check session exists before inserting into DetachedSessionsState" (already covered by A2.2.G1 but worth noting here for discoverability).

#### A2.3.G8 — `detachedIds` hydration race — **Accept**

**Plan-text delta:** add new Tauri command `list_detached_sessions` + hydration call in SidebarApp.

**Backend** (`src-tauri/src/commands/window.rs`, new):

```rust
#[tauri::command]
pub fn list_detached_sessions(
    detached: State<'_, DetachedSessionsState>,
) -> Vec<String> {
    let set = detached.lock().unwrap();
    set.iter().map(|u| u.to_string()).collect()
}
```

**Register** in `lib.rs:653-657` `invoke_handler!` (alongside `attach_terminal`, etc.).

**Frontend** (`src/shared/ipc.ts`, extend `WindowAPI`):

```typescript
export const WindowAPI = {
  // ... existing ...
  listDetached: () => transport.invoke<string[]>("list_detached_sessions"),
};
```

**Hydration** in `SidebarApp.onMount`, AFTER the `onTerminalDetached` / `onTerminalAttached` listeners register:

```typescript
unlisteners.push(await onTerminalDetached(({ sessionId }) =>
  sessionsStore.setDetached(sessionId, true)));
unlisteners.push(await onTerminalAttached(({ sessionId }) =>
  sessionsStore.setDetached(sessionId, false)));

// Hydrate: catches any detach events that fired before listeners registered.
// Race-safe: idempotent with late-arriving events (Set add is idempotent).
try {
  const ids = await WindowAPI.listDetached();
  ids.forEach(id => sessionsStore.setDetached(id, true));
} catch (e) {
  console.warn("[sidebar] listDetached hydration failed:", e);
}
```

**Section impact:**
- §5.1 new row: `commands/window.rs` NEW `list_detached_sessions` command. Register in `lib.rs:653-657`.
- §5.2 `src/shared/ipc.ts` row: add `WindowAPI.listDetached`.
- §5.2 `src/sidebar/App.tsx` row: add hydration call per A2.3.G8 skeleton.
- §2.3.4 IPC contract table: add `list_detached_sessions` (FE→BE, NEW).
- **Phase landing:** Phase 2 (alongside `sessionsStore.detachedIds` introduction).

#### A2.3.G9 — Splitter drag `pointer-events: none` — **Accept (promote to §3 required)**

**Plan-text delta:** §3 ("Splitter + layout mechanics") gains a new **required-CSS block** appended to the CSS section (not a DW.15 "should-fix"):

```css
/* MANDATORY per G.9 — without these rules the splitter feels broken
   because xterm.js starts a text-selection every time the cursor
   crosses into the terminal pane mid-drag. */
.main-root.main-dragging {
  user-select: none;
  cursor: col-resize;
}
.main-root.main-dragging .terminal-host {
  pointer-events: none;
}
```

And the splitter `onPointerDown` handler (from DW.4) MUST toggle the `.main-dragging` class on `.main-root`:

```tsx
const onPointerDown = (e: PointerEvent) => {
  setDragging(true);  // drives classList:{ "main-dragging": dragging() }
  // ... rest per DW.4 ...
};
```

With the SolidJS pattern `<div class="main-root" classList={{ "main-dragging": dragging() }}>` already implied by DW.4's `.main-root.main-dragging` selector.

**Section impact:**
- §3: promote the "Implementation model lifted from BrowserApp" block to require DW.4 + the G.9 CSS block as non-negotiable.
- §5.2 `src/main/styles/main.css` row: add explicit mention of the G.9 CSS rules.
- §8.2 test plan: add step 7 "During splitter drag, move cursor over terminal pane — NO xterm text selection begins" (already in DW.14 as §8.2.7; accepted as the round-2 phrasing).

### A2.4 Arbitration integrations (Arb-1 — Arb-4)

#### A2.4.Arb1 — `detached_geometry` on PersistedSession (R.6 wins)

**Plan-text delta:**

**§6.2** — extend `PersistedSession`:

```rust
pub struct PersistedSession {
    // ... existing ...
    #[serde(default)]
    pub was_detached: bool,

    /// Last-known geometry of this session's detached window.
    /// Populated only while the session is detached. Cleared implicitly
    /// when the session is destroyed (the whole PersistedSession disappears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_geometry: Option<WindowGeometry>,
    // ... rest ...
}
```

Also add a matching `detached_geometry: Option<WindowGeometry>` field to `session::session::Session` (runtime, `#[serde(skip)]` style — not serialized to the frontend but populated from disk on restore and from `set_detached_geometry` command at runtime). `snapshot_sessions` reads it into the PersistedSession on save.

**§5.1** — add new row for `commands/window.rs` NEW `set_detached_geometry` (grinch/R.6 body already given above; reproduced briefly here for the dev):

```rust
#[tauri::command]
pub async fn set_detached_geometry(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
    geometry: WindowGeometry,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    let mgr = session_mgr.read().await;
    mgr.set_detached_geometry(uuid, geometry).await;
    Ok(())
}
```

Register in `lib.rs:653-657` `invoke_handler!`.

**§5.2** — extend `window-geometry.ts` per R.6 option (a): add `initDetachedWindowGeometry(sessionId: string)` variant that debounces to `WindowAPI.setDetachedGeometry(sessionId, geo)`. Called from `TerminalApp.onMount` when `props.detached && props.lockedSessionId`. TypeScript `WindowAPI` gets:

```typescript
setDetachedGeometry: (sessionId: string, geometry: WindowGeometry) =>
  transport.invoke<void>("set_detached_geometry", { sessionId, geometry }),
```

**§6.3** — **no change**. `detachedGeometry` does NOT go on TS `AppSettings`.

**Downstream impact:**
- §7 migration: `detached_geometry` defaults to `None` for legacy `sessions.json` rows — zero impact on upgrade. Downgrade-safe (serde ignores unknown fields).
- §8.6 test plan: add step §8.6.27: "Detach session A, drag its window to position (500,500), resize to 800×500, restart app. A's detached window re-spawns at (500,500) 800×500. (Arb-1 auto-GC proof: also destroy A while detached, recreate fresh A — new A's detached window uses its own default, not the old geometry.)"
- **Phase landing:** Phase 3 (same phase as `was_detached` persistence — co-located field, co-located behavior).

#### A2.4.Arb2 — Introduce `mainZoom` in Phase 1 (DW.6 wins)

**Plan-text delta:**

**§6.1** — extend `AppSettings` (Rust):

```rust
/// Zoom level for the main window (1.0 = 100%). Unified sidebar + terminal.
#[serde(default = "default_zoom")]
pub main_zoom: f64,
```

Default in `AppSettings::default()`: `main_zoom: default_zoom()`.

**§6.3** — extend `AppSettings` (TypeScript): add `mainZoom: number`.

**§5.2** — `src/shared/zoom.ts`: extend `WindowType` to include `"main"`; `zoomKeyMap` adds `main: "mainZoom"`.

**§6.5** — migration branch in `load_settings`, BEFORE root-token auto-gen:

```rust
// One-time migration: seed main_zoom from sidebar_zoom on first boot after upgrade.
if (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
    && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON {
    settings.main_zoom = settings.sidebar_zoom;
    log::info!("[settings-migration] seeded main_zoom from legacy sidebar_zoom");
}
```

(Note: comparing `f64` for "was default" via epsilon avoids the legit "user set their zoom to exactly 1.0" being overwritten — though in that case seeding is a no-op anyway.)

**Deprecation**: `sidebar_zoom` is NOT marked `skip_serializing_if` (fields with simple-type defaults must stay on disk for downgrade compat). Keep it; drop in v0.9.

**Section impact:**
- §5.1: `settings.rs:32-109` row — add `main_zoom` line alongside existing zoom fields.
- §5.1: `settings.rs:139-176` row — default includes `main_zoom`.
- §5.1: new row for `settings.rs:299-340` modify — seed migration per A2.4.Arb2.
- §5.2: `src/shared/zoom.ts` row — add `main` type + zoomKeyMap entry.
- §5.2: update `SidebarApp.embedded` and `TerminalApp.embedded` contracts (§2.3.3 / DW.2) — when embedded, SKIP individual `initZoom`. The unified main window's zoom init happens at `MainApp.onMount` via `initZoom("main")`.
- §8.7 test plan: add step §8.7.38: "In unified main window, Ctrl+= zooms sidebar + terminal together. Settings `mainZoom` updates (not `sidebarZoom` or `terminalZoom`). In detached window, Ctrl+= zooms only that window. Settings `terminalZoom` updates."
- **Phase landing:** Phase 1 (per DW.6 + grinch G.15).

#### A2.4.Arb3 — Keep `focus_main_window` (R.4 wins)

**Plan-text delta:** §11.Q5 (which is being closed anyway — see A2.5) retains "Keep `focus_main_window` with `ensureTerminal` back-compat alias. 9 callers verified."

**Section impact:** no further plan-text changes. Strike "If so, delete the command" from §11.Q5 (already scheduled in A2.5 closure).

#### A2.4.Arb4 — Skip `onThemeChanged` in embedded mode (DW.5 uncontested)

**Plan-text delta:** §2.3.3 + §5.2 `src/terminal/App.tsx` row — add explicit "when `embedded === true`, skip the `onThemeChanged` listener at `terminal/App.tsx:131-139`" to the DW.2 embedded-contract enumeration.

**Section impact:** folded into §5.2's existing row for `src/terminal/App.tsx`; no new row needed.

### A2.5 §11 — open-question closure

Every §11 item now resolved. Replace §11 entirely with the closure table below (kept as §11 for diff stability; not renumbered):

| §11 item | Original question | Resolution | Source |
|---|---|---|---|
| Q1 | `close()` vs `destroy()` in Tauri 2.x | **destroy() mandatory in all programmatic destroy paths** (Tauri 2.10.3 source confirms `close()` fires CloseRequested, `destroy()` does not). | R.2 |
| Q2 | Detached-window geometry persistence | **`detached_geometry: Option<WindowGeometry>` on PersistedSession** (not HashMap on AppSettings). Auto-GC on session destroy. | Arb-1 |
| Q3 | Zoom mapping | **Introduce `mainZoom` in Phase 1.** Main = `mainZoom`; detached = `terminalZoom` (unchanged); `sidebarZoom` deprecated (retained for downgrade compat). | Arb-2 |
| Q4 | SolidJS listener-subscription audit | **Done.** No leak risks. Three races (theme, zoom, geometry) closed by the DW.2 embedded contract + Arb-2. Keyboard shortcuts auto-deduped by existing `shortcuts.ts:38-45` guard. | DW.5 |
| Q5 | `focus_main_window` disposition | **Keep the command. 9 call sites verified.** `ensureTerminal` remains as back-compat alias for one version. | R.4 / Arb-3 |

All five questions are closed. §11 is no longer a blocker for any phase.

### A2.6 G.13 — Main-window X-close behavior (architect decision)

**Grinch-listed options:**
- (a) Quit the app on main X-close. Closes all detached windows. Standard desktop convention.
- (b) Hide main to tray; require tray-click to restore. Preserves detached.
- (c) Confirmation dialog when detached windows exist.

**Decision: compound (a)+(c) — quit-on-X, with a confirmation prompt only when ≥1 detached windows are currently open.**

Specifically:
- **0 detached windows open** → main X quits the app immediately (matches today's behavior where closing the sole visible window effectively exits the process).
- **≥1 detached windows open** → main X shows a confirmation prompt: "You have {N} detached session{s} open. Quit the app and close all detached sessions?" with buttons [Quit] / [Cancel]. Cancel aborts the close; Quit proceeds as if no detached existed.

**Reasoning:**
1. **"Main IS the app"** is the correct mental model — consistent with grinch's §G.13 bias and the current codebase (no tray, no hide semantics anywhere).
2. **Option (b) — tray icon — is significantly out of scope.** Requires a new Tauri tray plugin, tray-icon resources, OS-integration testing, and a "restore from tray" UX. Drop-in cost is low if we choose it, but the feature creep is real (tray icon changes per-session state representation, notification behavior, etc.). Rejected for v0.8; keep on the table for v0.9+ if demand surfaces.
3. **Option (c) — unconditional confirmation — is noisier than it needs to be.** If the user has no detached windows, the dialog is just friction. Gating the prompt on "detached exists" preserves the no-friction common case and adds safety only when data loss is actually possible.
4. **Implementation cost is low.** ~25 lines of frontend code in `MainApp.onMount`: install `onCloseRequested` handler, inspect `sessionsStore.detachedIds.size`, gate on non-zero, call native confirm via `tauri-plugin-dialog`'s `ask` (already imported in `Cargo.toml:31`). If ≥1, `preventDefault` + show dialog; on Quit click, call `WebviewWindow.getAllWebviews().forEach(w => w.destroy())` + `app.exit(0)` from a new tiny Rust command `quit_app` (or just let the destroys cascade — the tokio runtime exits via the existing `RunEvent::Exit` path at `lib.rs:719-736`). On Cancel, do nothing.

**Implementation skeleton** (for §5.2 `src/main/App.tsx`):

```tsx
const win = getCurrentWindow();
const unlistenMainClose = await win.onCloseRequested(async (e) => {
  const detachedCount = sessionsStore.detachedIds?.size ?? 0;
  if (detachedCount === 0) {
    // No confirmation needed — let the close proceed.
    return;
  }
  e.preventDefault();
  const { ask } = await import("@tauri-apps/plugin-dialog");
  const shouldQuit = await ask(
    `You have ${detachedCount} detached session${detachedCount === 1 ? "" : "s"} open. ` +
    `Quit the app and close all detached sessions?`,
    { title: "Quit AgentsCommander?", kind: "warning", okLabel: "Quit", cancelLabel: "Cancel" }
  );
  if (shouldQuit) {
    // Destroy all detached windows first (R.2 destroy discipline),
    // then quit. Tauri's RunEvent::Exit handles final cleanup.
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const all = await WebviewWindow.getAll();
    for (const w of all) {
      if (w.label.startsWith("terminal-")) await w.destroy();
    }
    // Close main last — its destroy cascades to process exit.
    await win.destroy();
  }
});
```

**No new Rust command needed** — `win.destroy()` on main triggers Tauri's normal exit path which already persists sessions (`lib.rs:719-736`). Final `persist_current_state` already runs on `RunEvent::Exit`.

**Section impact:**
- New §4.Q6 sub-decision (main-window-X-close policy): quit-with-confirmation-if-detached. Add after the existing §4.Q6 text.
- §5.2 `src/main/App.tsx` row: add the `onCloseRequested` handler per A2.6 skeleton.
- §8.7 test plan: new step §8.7.39 "Main X-close with 0 detached → app quits immediately, no prompt. Step §8.7.40 Main X-close with 2 detached → prompt appears; Cancel aborts; Quit closes all 2 detached + main → process exits; on relaunch, the detached 2 come back (was_detached persisted per Arb-1 + §6.2)."
- §10: add rule "When the user has ≥1 detached windows, main X MUST present the confirmation dialog per A2.6 before destroying anything. Silent-quit on main X is acceptable only when `detachedIds.size === 0`."
- **Phase landing:** Phase 1. The policy is load-bearing for the "main window is always the way out" invariant; shipping Phase 1 without it means users can orphan detached windows. **Phase 1 scope grows by this amount.**

**Not escalating to user.** Rationale: the compound (a)+(c) decision is architect-callable because (i) it doesn't require product vision — it's a safety/UX tradeoff with clear desktop-convention precedent, (ii) grinch explicitly flagged (a) as bias and the confirmation refinement is a natural hardening, (iii) the implementation is trivially swappable — if user later prefers pure (a) or a tray-icon (b), it's ~25 lines of frontend change. If tech-lead or user disagrees, propose the alternative in round 3 and I'll swap.

### A2.7 MEDIUM triage (G.10 — G.16)

- **G.10** (TOCTOU in detach sibling selection) — **Accept.** Already folded into A2.2.G1's `detach_terminal_inner` skeleton (the `match mgr.switch_session(next_uuid).await` with `Err` → log + emit null). No separate row.
- **G.11** (Orphan settings `raise_terminal_on_click` + `sidebar_always_on_top`) — **Accept with modification.** Two-part:
  - `sidebar_always_on_top` → **rename to `main_always_on_top`** on first-boot migration (DW.12 option a, grinch's bias). Applied at `MainApp.onMount` via `getCurrentWindow().setAlwaysOnTop(val)`. Add `main_always_on_top: bool` (default false) to §6.1 AppSettings. Seed from legacy `sidebar_always_on_top` in §6.5 migration branch. Retain legacy field one version for downgrade.
  - `raise_terminal_on_click` → **silently deprecated.** Stays in settings.json for downgrade compat; ignored in unified mode (no terminal window to raise). Drop in v0.9. Comment in settings.rs explaining. No migration needed (default true is harmless no-op).

  Plan-text delta: add two rows to §6.1 AppSettings additions; §5.1 migration branch picks up `main_always_on_top` seed. Phase 1.
- **G.12** (CSS import order) — **Accept.** Plan-text delta for §3: add CSS-scoping requirement — `src/main/styles/main.css` imports `sidebar.css` and `terminal.css` in that explicit order, **AND** any global selectors (`body`, `html`, bare element selectors without a layout-container prefix) found in either file during Phase 1 implementation MUST be either (a) wrapped in `@layer` cascade layers (preferred; `@layer sidebar, terminal, main;`) or (b) prefixed with `.sidebar-layout` / `.terminal-layout` in their source files. Audit list (for dev-webpage-ui): grep `body\|html\|^::` in both files during implementation, fix each. Non-blocking for architecture, required for ship. Phase 1.
- **G.13** (main-window-X behavior) — **Decided.** See A2.6.
- **G.14** (atomic save_settings) — **Accept.** Plan-text delta: §5.1 add row for `src-tauri/src/config/settings.rs:343-358` modify — mirror the `sessions_persistence.rs:290-296` atomic-write pattern (`write tmp_path; rename tmp_path path`). Low-risk hardening; justified by the dramatic increase in save frequency from splitter-drag. Phase 1.
- **G.15** (zoom-save race) — **Accept.** Auto-resolved by Arb-2 (mainZoom separates the keys); no separate code change. Grinch's finding confirmed the acceptance.
- **G.16** (test §8.8 snake_case typo) — **Accept.** Plan-text delta: §8.8 step 36 rewrite `sidebar_geometry` / `terminal_geometry` / `main_geometry` → `sidebarGeometry` / `terminalGeometry` / `mainGeometry`. Phase 1 (doc only).

### A2.8 LOW triage (G.17 — G.20)

- **G.17** (was_detached stale between deferred wake + next persist) — **Accept.** No code change (grinch recommends none). Folded into the R.9 deferred-session skip rule already in §2.2.4. Phase 3.
- **G.18** (§5.1 row 207 under-specifies restore detach ordering) — **Accept.** Plan-text delta for §5.1 row `lib.rs:608-614`: explicit wording "inside per-session restore loop, AFTER successful `create_session_inner` (SKIPPED for deferred sessions per R.9), call `detach_terminal_inner(app, mgr, detached, &ps.id.as_deref().unwrap_or(...), ps.detached_geometry.clone(), skip_switch: true)`. Post-loop active_id switch filters against DetachedSessionsState per A2.2.G3." Phase 3.
- **G.19** (label-based Destroyed cleanup brittle namespace) — **Defer.** Per grinch's own recommendation, out of scope for v0.8. File as v0.9 cleanup chore ("rename detached-window label prefix from `terminal-` to something version-scoped like `detached-v1-`").
- **G.20** (save_settings not called inside load_settings) — **Accept (doc only).** Plan-text delta: §6.5 migration paragraph add final sentence: "Note: `load_settings` does NOT save unless `root_token` is missing. For most existing users the migration seed (`main_geometry`, `main_zoom`, `main_always_on_top`) runs in-RAM on load; the deprecated fields persist on disk until the first user-triggered save (splitter drag, theme toggle, settings modal, etc.). This is intentional — it makes a v0.8 → v0.7 downgrade safe because v0.7 still sees the old fields untouched until the user's first v0.8 save." Phase 1.

### A2.9 §9 — updated phasing

Phase 1 grows from ~60% to ~70% of the diff. Phase 2 and Phase 3 stay roughly the same proportionally.

**Phase 1 — Single main window + splitter + hardened detach (~70%)**

Inherits all of round-1 Phase 1, plus:
- **A2.2.G1** hardened `detach_terminal_inner` (post-build insert, session-existence recheck, G.10 sibling-switch tolerance).
- **A2.2.G2** filter `new_active` in `destroy_session_inner`.
- **A2.3.G7** move session_destroyed listener earlier in `TerminalApp.onMount`.
- **A2.3.G9** required CSS for `.main-dragging .terminal-host`.
- **A2.4.Arb2** `mainZoom` added to `AppSettings` + zoom.ts + migration seed.
- **A2.6** main-window-X confirmation policy in `MainApp.onMount`.
- **G.11** `main_always_on_top` rename + migration.
- **G.12** CSS scope audit.
- **G.14** atomic `save_settings` write.
- **G.16** §8.8 test wording fix.
- **G.20** doc-only migration note.

Ship bar (updated): all of round-1 Phase 1 + the above. Validated via §8.1, §8.2 (now including §8.2.7/.8 pointer-events), §8.7.39/.40 (main-X confirmation), §8.7.38 (mainZoom), §8.8, §8.9.

**Phase 2 — Re-attach flow + attach_terminal + detachedIds store (~20%)**

Inherits all of round-1 Phase 2, plus:
- **A2.2.G4** mandatory onCloseRequested skeleton.
- **A2.2.G5** `attach_terminal` silent-no-op contract.
- **A2.3.G8** `list_detached_sessions` command + SidebarApp hydration.

Ship bar: §8.3 + §8.4 + §8.5.

**Phase 3 — Persistence across restarts + pre-warm + detached_geometry (~10%)**

Inherits all of round-1 Phase 3, plus:
- **A2.2.G3** restore-path detached-filter.
- **A2.3.G6** TerminalView pre-warm listener.
- **A2.4.Arb1** `detached_geometry` + `set_detached_geometry` command.
- **G.17** was_detached staleness doc (per R.9 deferred-session skip rule).
- **G.18** `lib.rs:608-614` wording fix.

Ship bar: §8.6 (updated with §8.6.26 pre-warm test + §8.6.27 detached_geometry test).

**Phase 4 — Polish + edge cases (~5% — size unchanged)**

Unchanged from round 1. DW.10 (a11y), DW.11 (paint flash), DW.8 (PTY resize throttle), DW.9 (WebGL context dispose) remain as polish candidates.

### A2.10 §5 — consolidated impact-map additions

Round-2 adds these rows to §5.1 (Rust) and §5.2 (Frontend). Appending, not replacing, round-1 rows.

**§5.1 additions:**

| File | Lines | Change type | Summary | Phase |
|---|---|---|---|---|
| `src-tauri/src/commands/window.rs` | — | NEW | `detach_terminal_inner` per A2.2.G1 skeleton (post-build insert + session-recheck + G.10 tolerance + geometry + skip_switch) | 1 |
| `src-tauri/src/commands/window.rs` | — | NEW | `attach_terminal` per A2.2.G5 skeleton (silent-no-op when session gone) | 2 |
| `src-tauri/src/commands/window.rs` | — | NEW | `list_detached_sessions` per A2.3.G8 (hydration for detachedIds) | 2 |
| `src-tauri/src/commands/window.rs` | — | NEW | `set_detached_geometry` per Arb-1 | 3 |
| `src-tauri/src/commands/session.rs` | 728-734 | modify | Filter new_active against DetachedSessionsState per A2.2.G2 | 1 |
| `src-tauri/src/commands/session.rs` | 725 | modify | `close()` → `destroy()` per R.2 | 2 (when re-attach ships) |
| `src-tauri/src/config/settings.rs` | 32-109 | modify | Add `main_zoom: f64` + `main_always_on_top: bool` | 1 |
| `src-tauri/src/config/settings.rs` | 139-176 | modify | Default includes new fields | 1 |
| `src-tauri/src/config/settings.rs` | 299-340 | modify | Migration seeds `main_geometry`, `main_zoom`, `main_always_on_top` per §6.5 / Arb-2 / G.11 | 1 |
| `src-tauri/src/config/settings.rs` | 343-358 | modify | Atomic-write pattern per G.14 | 1 |
| `src-tauri/src/config/sessions_persistence.rs` | 14-56 | modify | Add `detached_geometry: Option<WindowGeometry>` alongside `was_detached` | 3 |
| `src-tauri/src/lib.rs` | 608-614 | rewrite | A2.2.G3 detached-filter on restored active_id | 3 |
| `src-tauri/src/web/commands.rs` | 276-280 | modify | Per R.5: rename `ensure_terminal_window` → `focus_main_window`, remove `close_detached_terminal`, add `attach_terminal` + `list_detached_sessions` + `set_detached_geometry` | 1 (rename) / 2 (attach) / 3 (geometry) |

**§5.2 additions:**

| File | Change type | Summary | Phase |
|---|---|---|---|
| `src/terminal/components/TerminalView.tsx` | modify | Add pre-warm listener per A2.3.G6 (~15 LoC) — REMOVE from §5.3 "files NOT touched" | 3 |
| `src/terminal/App.tsx` | modify | Move `onSessionDestroyed` listener registration BEFORE `loadActiveSession` await per A2.3.G7 | 1 |
| `src/terminal/App.tsx` | modify | `onCloseRequested` handler per A2.2.G4 skeleton (MANDATORY) | 2 |
| `src/main/App.tsx` | modify | `onCloseRequested` main-X policy per A2.6 | 1 |
| `src/main/App.tsx` | modify | `initZoom("main")` call — replaces per-pane zoom init | 1 |
| `src/shared/zoom.ts` | modify | Extend `WindowType` to include `"main"`; zoomKeyMap entry `main: "mainZoom"` | 1 |
| `src/shared/ipc.ts` | modify | Add `WindowAPI.listDetached`, `WindowAPI.setDetachedGeometry`, `onTerminalAttached` | 2/3 |
| `src/shared/window-geometry.ts` | modify | Add `initDetachedWindowGeometry(sessionId)` variant per Arb-1 R.6(a) | 3 |
| `src/sidebar/App.tsx` | modify | Hydration call per A2.3.G8 after listener registration | 2 |
| `src/shared/types.ts` | modify | Add `mainZoom: number`, `mainAlwaysOnTop: boolean` to `AppSettings` | 1 |

### A2.11 New bugs surfaced during round-2 integration

Per the tech-lead's rule ("if integrating a finding reveals a *new* bug, flag and address in the same round"):

1. **A2.11.N1 — `initZoom("main")` must run BEFORE the embedded SidebarApp and TerminalApp mount**, otherwise their legacy `initZoom("sidebar")` / `initZoom("terminal")` paths (if a refactor reintroduces them) would establish double-registration. Mitigation: `MainApp` calls `initZoom("main")` in its own `onMount` BEFORE rendering `<SidebarApp embedded/>` and `<TerminalApp embedded/>`; the DW.2 contract guarantees embedded apps don't init zoom themselves. Defense-in-depth: the `shortcuts.ts:38-45` module-level guard catches shortcut double-registration, but zoom has no equivalent guard. Leaving this as a comment in zoom.ts explaining the single-init invariant. **Action: add a 2-line comment in `zoom.ts` at the top.** Phase 1. No new plan row — folded into Arb-2 implementation.

2. **A2.11.N2 — `detached_geometry` clone on every `snapshot_sessions` call** (Arb-1). `snapshot_sessions` runs on every session state change (rename, idle/busy flip, git-repo update, create, destroy). Each call now clones `Option<WindowGeometry>` for every session. Cost is trivial (~4 f64 copies), but it's a hot path. Verified: `Session::clone` already derives `Clone`, so this is not a behavior change — just noting the additional per-snapshot work. No action needed. Phase 3.

3. **A2.11.N3 — `main_zoom` migration seed (Arb-2) reads `sidebar_zoom` BEFORE it's been migrated to `None`** (it's still on the `AppSettings` struct). OK by design — the seed happens during the one-time `load_settings` pass, after which `skip_serializing_if` drops the deprecated fields on next save. The `f64::EPSILON` guard prevents re-seeding on subsequent loads (when `sidebar_zoom` is still present but `main_zoom` is now non-default). Phase 1. No action.

### A2.12 Consensus status

Round 2 does not require round 3:

- Every BLOCKER resolved with accept + concrete delta.
- Every HIGH resolved with accept + concrete delta.
- Every arbitration closed per grinch's calls.
- §11 fully closed — no open questions remain.
- G.13 decided (architect-called with fallback flag).
- No rejections; no items that would require minority-rule at round 3.
- Two items need dev-enrichment commentary rather than decision: (a) `@layer` vs manual-scope for CSS G.12; (b) whether to adopt the optional f64::EPSILON guard in A2.4.Arb2 migration. Both are implementer judgment calls, not consensus-blocking.

**Ready for round-2 dev enrichment.** Route to dev-rust + dev-webpage-ui. If grinch round 2 surfaces regressions introduced by these deltas, round 3 stays available.

---

## Dev-rust round 2 review

**Author:** dev-rust
**Scope:** Rust-side verification of architect's round-2 integration; consensus/push-back decision.
**Outcome up front:** **NO consensus.** Four concrete push-backs follow (one HIGH-severity bug introduced by A2.6, one wording bug in G.18, two round-1 items whose integration is incomplete). Each has a small delta that round 3 can close mechanically.

### DR2.0 Consensus signal

- Acceptance pattern: **16 consensus / 2 push-back-acceptance / 4 push-back / 2 new concerns (1 NEW flagged under push-back).**
- Round-1 items audited: R.1 through R.14. Items that are fully integrated get one-line "Consensus" entries below. Items that are partially or not integrated get push-back entries with the concrete plan delta needed.
- Architect's 24-item triage: skim-verified against the triage table (A2.1). Dispositions look correct; the issues I raise are in how the deltas were (or weren't) propagated into the authoritative plan text.

### DR2.1 Consensus items (round-1 deltas fully integrated)

| Round-1 item | Architect integration | Verdict |
|---|---|---|
| R.1 (HEAD anchor) | §A2.1 confirms `60dd162` unchanged; no drift. | **Consensus** |
| R.2 (`close()` → `destroy()` everywhere) | Folded into A2.2.G1 (detach rollback), A2.2.G5 (attach body), A2.6 (quit path), A2.10 row for `session.rs:725`, and the Tauri-2.10.3-source quote embedded in §A2.5. `attach_terminal` body explicitly comments "R.2: destroy(), not close()" — caught the self-recursion risk. | **Consensus** |
| R.4 (keep `focus_main_window`, 9 callers) | Arb-3 accepts. §11.Q5 rewritten per A2.5. | **Consensus** |
| R.5 (rename in `web/commands.rs`) | A2.10 row specifies rename + add + remove operations across 3 phases. | **Consensus** |
| R.6 (`detached_geometry` on `PersistedSession`) | Arb-1 accepts. §6.2 extension + new `set_detached_geometry` command in A2.10. The additional `Session::detached_geometry` field and `SessionManager::set_detached_geometry` setter are explicitly added. | **Consensus** |
| R.10 (restore-path `skip_switch`) | A2.2.G1 skeleton ships with `skip_switch: bool`. G.18 text specifies `skip_switch: true` from the restore loop. Phase landing clarified. | **Consensus** |
| R.12 (Tauri 2.x semantics, lock discipline, minor polish) | Cited appropriately; R.13 agreement block respected verbatim by arbitrations Arb-1/2/3/4. | **Consensus** |
| R.13 (agreements with Q1/3/4/5/6/7/8 decisions) | No re-litigation. All eight architect decisions stand. | **Consensus** |

Agreement also with A2.11.N1 (zoom-init ordering; comment-only fix), A2.11.N2 (clone cost; no-op), A2.11.N3 (main_zoom seed; by design). All three new bugs are correctly judged as non-issues.

### DR2.2 A2.2.G1 canonical skeleton — audit

Tech-lead asked specifically for an audit of lock ordering, post-build recheck race, and `skip_switch` sufficiency. Results:

**Lock ordering** ✓ **safe.** The skeleton interleaves tokio async locks and std `Mutex` correctly:

1. `session_mgr.read().await` (async tokio read) → short scope, no nested `detached.lock()` during await.
2. The session-existence check reads `mgr.get_session(uuid).await` inside the tokio guard — no cross-lock holding.
3. After dropping the tokio guard, `{ let mut s = detached.lock().unwrap(); s.insert(uuid); }` is a single-statement scope — no await held while the std Mutex is held.
4. The sibling-switch block reads `sessions = mgr.list_sessions().await` inside a tokio read guard, then acquires `detached.lock()` briefly (no `.await` inside the inner scope), drops it, then calls `mgr.switch_session(next_uuid).await` — this works because `SessionManager::switch_session(&self, id)` takes `&self` (verified: `session/manager.rs:106`, `list_sessions:152`, `get_session:166` all take `&self`). The outer tokio RwLock read guard is sufficient.

**`skip_switch` sufficiency** ✓ **sufficient.** The only branch `skip_switch` controls is the "emit session_switched to next non-detached" block. Phase 3's restore needs exactly that: create every session, detach each without triggering sibling-switches, then let A2.2.G3's post-loop switch handle `active_id`. A bool carries all the state needed. Three-state variant not required.

**Post-build recheck race** ⚠ **narrow residual race remains, acceptable.** One scenario:

- t0: `builder.build()` succeeds. Window exists; set doesn't contain UUID.
- t1: recheck `mgr.get_session(uuid)` returns `Some(session)` (guard dropped immediately at end of `{}`).
- t2: concurrent `destroy_session_inner` runs. It removes from `SessionManager`, then removes from `DetachedSessionsState` (no-op, set empty), then closes the detached window via `destroy_session_inner:722-726` (which uses `win.destroy()` — R.2). The `WindowEvent::Destroyed` event fires → label-cleanup at `lib.rs:697-717` tries to remove UUID from set (still empty, no-op).
- t3: we return from the recheck scope and execute `{ let mut s = detached.lock().unwrap(); s.insert(uuid); }`.

**Post-condition:** UUID is in `DetachedSessionsState`, window is destroyed, session is gone. The Destroyed event already fired and cannot fire again. **The UUID is stale and will not be cleaned up.**

Impact is narrow (requires concurrent destroy between recheck and insert — a sub-microsecond window), and the practical consequence is limited to `DetachedSessionsState` false positives on `switch_session`'s detach check (`session.rs:915-922`) — the user would click a destroyed session in the sidebar (which shouldn't be rendered anyway because `session_destroyed` event fired). This is minor enough to accept as-is, but worth documenting.

**Recommended tightening (optional, ~3 lines):** after the insert, one more check:

```rust
{ let mut s = detached.lock().unwrap(); s.insert(uuid); }
// Final guard: if a destroy raced us, roll back.
let still_alive = {
    let mgr = session_mgr.read().await;
    mgr.get_session(uuid).await.is_some()
};
if !still_alive {
    let _ = win.destroy();
    let mut s = detached.lock().unwrap();
    s.remove(&uuid);
    return Err("Session destroyed during detach setup".into());
}
```

Flagging as OPTIONAL — fold into A2.2.G1 skeleton if the architect agrees, otherwise accept the narrow race.

### DR2.3 G.13 decision — accept decision, but the implementation has a persistence-ordering BUG

**Decision to quit-with-confirmation-if-detached:** accepted. Compound (a)+(c) is the right UX call; implementation swap to pure-quit or tray-hide is trivial if user overrides.

**BUG found during audit of A2.6 implementation — push back.** See DR2.5.NEW-1 below.

**Other Rust-side concerns on G.13:** cleared.
- `RunEvent::Exit` timing on `win.destroy()` of main: verified. Tauri fires `ExitRequested` then `Exit` after the last window is destroyed; current handler at `lib.rs:719-736` receives `Exit` correctly. ✓
- Child-window cleanup ordering: A2.6 destroys detached windows BEFORE main. Each destroy fires `WindowEvent::Destroyed` → label-cleanup at `lib.rs:697-717` runs. ✓ (but introduces DR2.5.NEW-1 — see below.)
- Persist on quit: persist IS called in `RunEvent::Exit` at `lib.rs:733`. ✓ (but reads stale state — see DR2.5.NEW-1.)
- Guide window disposition: A2.6's destroy loop filters to `w.label.startsWith("terminal-")` so guide is untouched. When main is destroyed, the process exits and guide dies with it. Acceptable.

### DR2.4 Push-back items (require plan-text delta before implementation)

#### DR2.4.PB-1 — R.7 not integrated into plan text (persist-helper signature)

**Scope:** §5.1 rows at lines 220-222 still say `snapshot_sessions(mgr, &DetachedSessionsState)` — the original round-1 signature I pushed back on with R.7.

**Delta needed:** one of the following:

- (a) **Preferred — makes the issue moot.** Move `was_detached` from DetachedSessionsState-derived to a `Session::was_detached: bool` field, maintained by `detach_terminal_inner` (set true after insert into set) and `attach_terminal` (set false before emitting `terminal_attached`). `snapshot_sessions` reads from `Session` — no DetachedSessionsState parameter at all. This co-locates with Arb-1's `Session::detached_geometry` field and **also fixes NEW-1 (persistence ordering)**, see DR2.5.NEW-1.
- (b) Explicitly update §5.1 lines 220-222 per R.7 signature: `(mgr: &SessionManager, detached: &HashSet<Uuid>)`. All 10 call sites from R.7's enumeration get updated. Lock-discipline guaranteed by the type.

**Recommendation:** (a). It eliminates threading, simplifies the 10-site change, collapses R.7 + NEW-1 into one fix, and matches the Arb-1 pattern the architect already adopted for `detached_geometry`.

**Severity:** MEDIUM for (a) / HIGH if we stick with the current plan text (lock-across-await footgun + NEW-1 bug).

#### DR2.4.PB-2 — R.8 not integrated into plan text (browser-mode hole)

**Scope:** A2.10 §5.2 additions table does NOT contain a row for `src/main.tsx`. The round-1 §5.2 row at line 230 describes the new dispatcher but does NOT include the `!isTauri` early-return guard from R.8. Dev-webpage-ui's DW section at line 1580 cites "R.8's skeleton is correct" — but without an explicit plan delta, the implementer will follow line 230 and miss the guard.

**Delta needed:** append one row to A2.10 §5.2:

| File | Change type | Summary | Phase |
|---|---|---|---|
| `src/main.tsx` | modify | Router dispatcher MUST check `!isTauri` FIRST (render `<BrowserApp/>` unconditionally). Then Tauri-only branches: `?window=detached&sessionId=<id>` → TerminalApp detached, `?window=guide` → GuideApp, everything else (incl. legacy `?window=sidebar` / `?window=terminal`) → MainApp. Skeleton per R.8. | 1 |

**Severity:** HIGH. Without this, the URL change at `lib.rs:299` (web-remote URL → `?window=main`) breaks web-remote browsers — they try to mount `MainApp`, which calls Tauri APIs and silently fails.

#### DR2.4.PB-3 — R.3 partially integrated (close_detached_terminal dead-code cleanup)

**Scope:** A2.10 line 2413 removes `close_detached_terminal` from the web no-op arm. But round-1 §5.1 rows 208 and 212 still describe it as "retained as internal helper" and "remove from invoke_handler! but keep function body". Neither row was updated in round 2.

**Delta needed:** update round-1 §5.1:

- Row at line 208: remove the "keep `close_detached_terminal` as internal-only command" clause. `close_detached_terminal` is not added to the `invoke_handler!` at all (per R.3's reasoning: `destroy_session_inner` at `session.rs:683-688` + `722-726` already covers the cleanup path redundantly).
- Row at line 212: delete entirely. Replace with: "`commands/window.rs:193-213` — DELETE `close_detached_terminal` function and the `ipc.ts:179-180` `WindowAPI.closeDetached` wrapper (confirmed zero callers by dev-webpage-ui at plan line 841). The `WindowEvent::Destroyed` handler at `lib.rs:697-717` + `destroy_session_inner` cover all cleanup paths."

**Severity:** LOW. Leaves dead code in the Rust file but doesn't break anything. Flagging to complete the cleanup, not to block.

#### DR2.4.PB-4 — G.18 wording bug (wrong UUID source in restore detach call)

**Scope:** A2.8 G.18 text (plan line 2344) says:

> `detach_terminal_inner(app, mgr, detached, &ps.id.as_deref().unwrap_or(...), ps.detached_geometry.clone(), skip_switch: true)`

`ps.id` is `Option<String>` on `PersistedSession` (line 46: "Session UUID (only present in live snapshots)"). It is a LIVE-SNAPSHOT field. For RESTORE (loading from disk), `ps.id` may be present (if last saved as live snapshot) but it is **the old UUID of the session that existed in the previous run**. The current `create_session_inner` at `lib.rs:579-603` does NOT restore the old UUID — it generates a new one. The restored session's UUID is `info.id` from `create_session_inner`'s `Ok(info)` return.

Passing `ps.id` to `detach_terminal_inner` would call `get_webview_window(&format!("terminal-{}", old_id_no_dashes))` for a label that has no corresponding session, then build a window for a session whose UUID doesn't match — break Phase 3 entirely.

**Delta needed:** G.18 text (plan line 2344) must read:

> `detach_terminal_inner(&app_handle, &session_mgr_clone, &detached, &info.id, ps.detached_geometry.clone(), /* skip_switch */ true)`

where `info` is the `SessionInfo` returned by the successful `create_session_inner` call for this `ps` entry inside the restore loop at `lib.rs:593-597`.

Also: the restore loop must fetch `DetachedSessionsState` once at the top (via `app_handle.state::<DetachedSessionsState>()`) and pass it to every detach call. Phase 3 ship-bar should call this out explicitly.

**Severity:** HIGH. As-written, Phase 3 will compile but run incorrectly. The detach-on-restore path will spawn windows for wrong UUIDs (or fail to parse if `ps.id` is `None` — which is possible for legacy sessions.json rows written before the live-snapshot additions at §6.2 round-1).

### DR2.5 New concerns surfaced during round-2 audit

#### DR2.5.NEW-1 — A2.6 quit path destroys detached windows BEFORE persisting → `was_detached` lost on every quit

**Location:** A2.6 TSX skeleton at plan lines 2287-2313.

**The bug:** When the user clicks Quit in the confirmation dialog:

1. JS loops over all windows; for each `terminal-*` label, calls `await w.destroy()`.
2. Each `w.destroy()` triggers Tauri's `WindowEvent::Destroyed` → our handler at `lib.rs:697-717` removes the UUID from `DetachedSessionsState`. After the loop completes, the set is EMPTY.
3. JS then `await win.destroy()` on main. All windows gone → `RunEvent::ExitRequested` → `RunEvent::Exit` (`lib.rs:719-736`) → `persist_current_state(&mgr)` reads `DetachedSessionsState` (now empty) → `snapshot_sessions` sets `was_detached = false` for every session on disk.

On next launch: no sessions restore as detached. The user's deliberate detach state is lost on every app quit. This is a **silent state loss bug** that contradicts test §8.7.40 as written ("on relaunch, the detached 2 come back").

**Verified dependency chain:**
- Each `await w.destroy()` from JS resolves AFTER the backend's `WindowEvent::Destroyed` is processed (Tauri's IPC round-trip closes the loop).
- The Destroyed handler at `lib.rs:697-717` synchronously modifies `DetachedSessionsState` — no pending queue.
- `RunEvent::Exit` fires AFTER all Destroyed events complete.
- `persist_current_state` is reached with an empty set.

**Fix options:**

- **(A) Preferred: move `was_detached` onto `Session` struct.** Set it in `detach_terminal_inner` (after the insert into `DetachedSessionsState`) and clear it in `attach_terminal` (before emitting `terminal_attached`). `snapshot_sessions` reads from `Session::was_detached` instead of `DetachedSessionsState`. Removes the ordering dependency on `WindowEvent::Destroyed` cleanup. **This also closes PB-1** (R.7 becomes moot).
- **(B) Alternative: persist explicitly in A2.6 BEFORE destroying detached windows.** New Tauri command `force_persist` (or piggyback an existing debounce-flush). A2.6 skeleton calls `await SessionAPI.forcePersist()` as the first line inside `if (shouldQuit)`. Adds one IPC round-trip to quit.

**Recommendation: (A).** Cleaner, co-located with Arb-1's `Session::detached_geometry` pattern, closes PB-1 simultaneously, zero new IPC surface.

**Severity: HIGH.** Detached-persistence is a Phase 3 ship-bar item; this bug silently breaks it.

**Plan-text deltas if (A) is accepted:**

- §6.2: add `Session::was_detached: bool` (runtime, synced with `DetachedSessionsState`).
- §5.1 A2.2.G1: after insert into set, also do `mgr.set_was_detached(uuid, true).await;`.
- §5.1 A2.2.G5 (attach_terminal): before emitting events, do `mgr.set_was_detached(uuid, false).await;`.
- §5.1 `snapshot_sessions` row (line 220): revert to `(mgr: &SessionManager) -> Vec<PersistedSession>` — unchanged signature. `was_detached` is read from `Session`.
- §5.1 the 10 persist call sites: no signature change needed — revert the round-1 threading requirement.
- §2.2.4 persistence bullet: "`was_detached` is stored on `Session` (runtime-synced from `DetachedSessionsState`) and persisted to disk via the existing snapshot path. The Quit path's destroy-then-persist ordering is no longer a concern — by the time persist runs, `Session::was_detached` values already reflect pre-destroy state."

#### DR2.5.NEW-2 — none

(Was placeholder during audit; no additional new bugs beyond NEW-1 and the clarifications above.)

### DR2.6 Summary for tech-lead

**Consensus signal: NO.**

**Blocker for YES:** the four push-backs in DR2.4 and the HIGH bug in DR2.5.NEW-1.

**Counts:**
- 16 items consensus-accepted (R.2, R.4, R.5, R.6, R.10, R.12, R.13, all 8 architect-decision agreements, A2.11.N1/N2/N3)
- 2 push-back-accepted-with-optional-tightening (A2.2.G1 race, G.13 decision-level)
- 4 push-backs (PB-1 R.7 not integrated, PB-2 R.8 not integrated, PB-3 R.3 partial, PB-4 G.18 UUID source)
- 1 new HIGH-severity concern (NEW-1 persistence ordering)

**One-line summary of blocker:** A2.6 Quit path loses `was_detached` state on every quit because detached windows are destroyed (clearing `DetachedSessionsState`) before `RunEvent::Exit` persists — breaking test §8.7.40 silently. Fix by moving `was_detached` onto `Session` struct (also auto-closes R.7).

**What gets consensus to YES in round 3:**
1. Adopt DR2.5.NEW-1 Fix (A) — `Session::was_detached` field.
2. Apply DR2.4.PB-2 delta — one new row in A2.10 §5.2 for `src/main.tsx` `!isTauri` guard.
3. Apply DR2.4.PB-4 delta — G.18 wording correction: `&info.id`, not `&ps.id.as_deref()`.
4. (Optional cleanup) DR2.4.PB-3 — complete `close_detached_terminal` deletion.

Total plan-text delta for round 3: ~15 lines across four sections. No architectural re-litigation.

Standing by idle for round 3.

---

## Dev-webpage-ui round 2 review

**Verdict: CONSENSUS with 2 new concerns + 2 commentary items. No push-backs.** I do NOT trigger round 3.

Audited architect's §A2.1–A2.12 against round-1 DW.1–DW.15, and against tech-lead's 4 verification questions. All round-1 items integrated correctly or explicitly superseded; the architect's arbitration choices stand. Two minor re-entrancy / UX-default concerns in G.13 are flagged below as **new concerns** rather than push-backs — they don't require round 3 but should be folded in before implementation.

### R2.1 — Round-1 item integration check

| My round-1 item | Architect round-2 handling | Verdict |
|---|---|---|
| DW.1 (OnboardingModal row removal, `WindowAPI.closeDetached` delete, 9 ensureTerminal call sites) | Not explicitly called out in §A2. OnboardingModal row still in §5.2. `WindowAPI.closeDetached` still in `ipc.ts`. | **Consensus with nit** — see R2.2 below. |
| DW.2 (embedded contract: skip `<Titlebar/>`, initZoom, initWindowGeometry, handleRaiseTerminal, setAlwaysOnTop, onThemeChanged) | Preserved verbatim — round-1 DW.2 enumeration stands. Arb-2 §5.2 explicitly references "DW.2 contract" when requiring embedded-zoom skip. Arb-4 folds `onThemeChanged` skip into DW.2's enumeration. | **Consensus.** The enumeration including `<Titlebar/>` skip is in round-1 DW.2 and is not overwritten. |
| DW.3 (titlebar outside flex row + `min-height: 0`) | Round-1 DW.3 CSS stands. A2.3.G9 adds the `.main-dragging` layer on top but does not re-assert the foundational layout CSS. Not hand-waved — it's anchored in round-1 DW.3, which the implementer reads. | **Consensus.** See R2.3 for the one nit. |
| DW.4 (Pointer Events + setPointerCapture + pointer-events: none) | All three pieces present: DW.4 round-1 skeleton (pointer capture) + A2.3.G9 (`pointer-events: none` on `.terminal-host` via `.main-dragging` class toggle). A2.3.G9 §Section-impact: "promote the 'Implementation model' block to require DW.4 + the G.9 CSS block as non-negotiable." | **Consensus.** |
| DW.5 (SolidJS listener audit) | Folded into §11.Q4 closure table with "Done. No leak risks. Three races closed..." | **Consensus.** |
| DW.6 (mainZoom in Phase 1) | A2.4.Arb2 accepts. Migration seeds from `sidebar_zoom` only, not `max(sidebar, terminal)`. `f64::EPSILON` guard on "is default?" check. Legacy retained for downgrade compat. | **Consensus.** The `sidebar_zoom`-only seed is a defensible pick vs my original `max()` — the new main window looks sidebar-like, so the sidebar scale is the right anchor. Not a push-back. |
| DW.7 (splitter debounce timer ownership) | Not re-discussed. Implicit: implementer will make `persistWidth` closure-local in `main/App.tsx`. | **Consensus.** Trust the implementer; the round-1 DW.7 warning is on record. |
| DW.8 (xterm resize cadence) | Documented as acceptable. | Consensus. |
| DW.9 (WebGL context budget) | Documented. Phase 4 polish candidate. | Consensus. |
| DW.10 (splitter a11y) | Phase 4. | Consensus. |
| DW.11 (initial-width paint flash) | Phase 4. | Consensus. |
| DW.12 (sidebarAlwaysOnTop) | G.11: rename to `main_always_on_top`, migrate from legacy, seed in §6.5. Retain legacy for downgrade. | **Consensus.** Matches my round-1 option (a) bias exactly. |
| DW.13.1 (close vs destroy) | R.2: `destroy()` mandatory in all programmatic destroy paths. | **Consensus.** Exactly the frontend preference I asked for. |
| DW.13.2 (detached_geometry placement) | Arb-1 R.6: on `PersistedSession`, auto-GC on destroy. | **Consensus.** R.6 is a better design than my original `HashMap` proposal. |
| DW.13.3 (focus_main_window) | Arb-3 R.4: keep. 9 callers verified. | **Consensus.** |
| DW.13.4 (delete `WindowAPI.closeDetached`) | Not explicitly addressed in round-2. | **New concern** — see R2.2. |
| DW.13.5 (web remote URL) | Already addressed in round-1 §7.4. A2.10 §5.1 row for `web/commands.rs` covers the rename. | Consensus. |
| DW.14 (test plan additions) | Folded into §8 test plan additions across A2.2 / A2.3 / A2.4 / A2.6. | Consensus. |
| DW.15 (priority list) | All "must-fix" items shipped; "should-fix" items landed; "nice-to-have" items scheduled Phase 4. | Consensus. |

**Tech-lead's four verification questions answered:**

1. **DW.6 mainZoom migration semantics**: "migrate once then clear" is correct for our model. Keeping a mirror of `sidebar_zoom`→`main_zoom` for N versions would be extra complexity for zero benefit — `sidebar_zoom` has no meaning in unified mode, so there's nothing to keep in sync with. The architect's design (seed on first v0.8 load, `sidebar_zoom` persists on disk untouched for downgrade-safety, drop at v0.9) is the right shape.
2. **DW.3 `min-height: 0`**: **present, not hand-waved** — anchored in round-1 DW.3 CSS skeleton. Round-2 doesn't re-assert it because it didn't need to. If the tech lead wants extra safety, see R2.3.
3. **DW.4 three pieces**: **all present** — Pointer Events + setPointerCapture in DW.4 (promoted to required by A2.3.G9 §Section-impact); `pointer-events: none` on xterm in A2.3.G9 CSS; `.main-dragging` class toggle in A2.3.G9 tsx.
4. **DW.2 skip Titlebar AND initZoom**: **both stated explicitly**. `<Titlebar/>` skip is in round-1 DW.2 at both SidebarApp and TerminalApp bullets. `initZoom` skip is in round-1 DW.2 and re-emphasized by A2.4.Arb2's §5.2 line ("when embedded, SKIP individual `initZoom`").

### R2.2 — NEW CONCERN (MEDIUM): `WindowAPI.closeDetached` cleanup missed

**Severity:** MEDIUM (dead code / runtime failure risk)
**Location:** `src/shared/ipc.ts:179-180`
**Impact:** Round-2 §A2.10 §5.2 additions don't include removing `WindowAPI.closeDetached`. The Rust-side command `close_detached_terminal` is removed from `invoke_handler!` (per round-1 §5.1 and reconfirmed by round-2's `web/commands.rs` row removing it). If any future code or a stale third-party caller invokes `WindowAPI.closeDetached(...)`, Tauri returns "Command not found" at runtime.

Round-1 DW.13.4 verified zero TS callers. Keeping the shim is harmless today but leaves a footgun.

**Proposed fix:** add one row to §A2.10 §5.2 additions:

| File | Change type | Summary | Phase |
|---|---|---|---|
| `src/shared/ipc.ts` | modify | Delete `WindowAPI.closeDetached` (zero callers, backend removal leaves it broken) | 1 |

Trivial, non-controversial. Does not block consensus.

### R2.3 — NIT (LOW): DW.3 foundational CSS deserves a round-2 restate

**Severity:** LOW (stylistic / discoverability)
**Location:** §A2.9 / §A2.10
**Impact:** A2.10 §5.2 has a `src/main/App.tsx` row and CSS mentions in A2.3.G9, but the foundational layout skeleton (the exact `.main-root { flex-direction: column; height: 100vh }` / `.main-body { flex: 1; min-height: 0 }` / `.main-terminal-pane { min-width: 300px }` block) lives only in round-1 DW.3. An implementer reading A2.10 in isolation might miss `min-height: 0` — which is the one non-obvious line that silently breaks xterm vertical sizing.

**Proposed fix (optional):** add an explicit mention in §A2.9 Phase 1 ship-bar: "`src/main/styles/main.css` MUST include the foundational flex skeleton from round-1 DW.3, including `.main-body { min-height: 0 }` (classic flex-item trap — without it, xterm canvas overflows vertically)."

Non-blocking; just discoverability insurance.

### R2.4 — NEW CONCERN (MEDIUM): G.13 `ask()` re-entrancy + destructive-default

Tech lead asked specifically about `ask()` behavior. Two issues worth folding in before implementation:

**(a) Re-entrancy risk.** `win.onCloseRequested` fires on every close attempt — X button, Alt+F4, tray close, programmatic. If the user presses X while `await ask(...)` is still pending (e.g. double-X, Alt+F4 during dialog, hit X just as macOS command-Q arrives), the handler re-enters: `e.preventDefault()` runs, a second `ask()` opens on top of the first. Both dialogs await independently. If user confirms in dialog1 → destroys all detached + main → but handler2 is still alive waiting on dialog2. Handler2's `WebviewWindow.getAll()` now returns an empty list; `win.destroy()` on a destroyed window throws. **No try/catch** wraps the destroy loop in A2.6's skeleton, so an unhandled rejection lands in the console (harmless at app-exit time but ugly).

**Proposed fix:** add a module-level guard at the top of `MainApp`:

```tsx
let quitInProgress = false;

const unlistenMainClose = await win.onCloseRequested(async (e) => {
  if (quitInProgress) { e.preventDefault(); return; }
  const detachedCount = sessionsStore.detachedIds?.size ?? 0;
  if (detachedCount === 0) return;
  e.preventDefault();
  quitInProgress = true;
  try {
    const { ask } = await import("@tauri-apps/plugin-dialog");
    const shouldQuit = await ask(/* ... */);
    if (shouldQuit) {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      for (const w of await WebviewWindow.getAll()) {
        if (w.label.startsWith("terminal-")) { try { await w.destroy(); } catch {} }
      }
      try { await win.destroy(); } catch {}
    }
  } finally {
    quitInProgress = false;
  }
});
```

Three deltas: the boolean guard, the `try/catch` around each destroy, the `finally` reset so Cancel doesn't leave the app stuck in "quitting" state.

**(b) Enter-defaults-to-destructive UX safety.** Tauri-plugin-dialog's `ask()` makes `okLabel` the Enter-default button. In A2.6's skeleton, `okLabel: "Quit"` means pressing Enter in the dialog destroys all detached sessions + quits. **Convention** for destructive confirms (Windows HIG, Apple HIG, web ARIA) is that Enter = the SAFE option, not the destructive one. Users tend to mash Enter after reading a dialog; Enter = Quit is asking for accidental data loss.

**Proposed fix:** flip the semantics so Cancel is the OK button:

```tsx
const shouldCancel = await ask(
  `You have ${detachedCount} detached session${detachedCount === 1 ? "" : "s"} open. ` +
  `Quit the app and close all detached sessions?`,
  { title: "Quit AgentsCommander?", kind: "warning", okLabel: "Cancel", cancelLabel: "Quit" }
);
if (!shouldCancel) {
  // user hit "Quit" (cancel button) → proceed to destroy
  // ...
}
```

This reads a little backwards in code (Cancel is okLabel, Quit is cancelLabel), so add a one-line comment explaining why. Alternative is a custom modal (50-100 LoC + CSS), which would also solve visual inconsistency with the app's other modals (OpenAgentModal, AgentPickerModal, OnboardingModal, SettingsModal — all custom). For Phase 1 MVP I recommend the flipped `ask()` — ship the safety fix now, file custom-modal as Phase 4 polish.

**Severity:** MEDIUM. Neither issue is a showstopper for Phase 1, but both are easy to fix now and ugly to fix later (once user muscle-memory has set in on the Enter-to-Quit habit, swapping it becomes an unpopular change).

**Does not trigger round 3** — these are Phase 1 implementation refinements, not architectural disagreements.

### R2.5 — NEW CONCERN (LOW): Splitter width load-time clamp

**Severity:** LOW
**Location:** §A2.9 Phase 1 / §3 Splitter mechanics
**Impact:** §3 specifies clamp-on-drag (200-600 logical, with `windowWidth - 300` additional cap). It does NOT specify clamp-on-load. Scenario: user sets `mainSidebarWidth=600` on a 1920-wide monitor, restarts with main window on an 800-wide monitor (laptop-only mode). Loaded value 600 + terminal min 300 = 900 > 800 window width → terminal pane renders below 300px OR the sidebar renders at its requested 600px and pushes the terminal to negative flex (which in CSS becomes 0, so terminal is invisible until first splitter drag).

**Proposed fix:** in `main/App.tsx.onMount` where the saved width is read, apply the same clamp that the drag handler applies:

```tsx
const settings = await SettingsAPI.get();
const saved = settings.mainSidebarWidth ?? 240;
const clamped = Math.max(200, Math.min(600, Math.min(saved, window.innerWidth - 300)));
setSidebarWidth(clamped);
// Also re-clamp on window resize (edge case: DPI change, monitor hot-plug):
const onResize = () => {
  setSidebarWidth(w => Math.min(w, window.innerWidth - 300));
};
window.addEventListener("resize", onResize);
onCleanup(() => window.removeEventListener("resize", onResize));
```

The `onResize` re-clamp handles the "user disconnects external monitor and the OS snaps the window narrower" case.

Does not block consensus. Flagging for implementer.

### R2.6 — Embedded-mode reactivity safety (tech-lead Q3)

Traced every store write path across `sessionsStore`, `terminalStore`, `settingsStore`, `projectStore`, `bridgesStore`. In unified mode:

- **sessionsStore** writes (sidebar-originated): session CRUD, `activeId`, `pendingReview`, `waitingForInput`, `detachedIds`. Terminal does NOT read `sessionsStore`. Cross-component cascade: **none**.
- **terminalStore** writes (terminal-originated): only `activeSessionId` / name / shell. Sidebar does NOT read `terminalStore`. Cross-component cascade: **none**.
- **settingsStore** writes: debounced on user-driven setting changes. Both components read `settingsStore.voiceEnabled` (mic button visibility). A write to `voiceEnabled` triggers re-render in both — this is **intentional** (both should react to settings changes). Not a surprise; by design.
- **projectStore / bridgesStore**: sidebar-only.

**Per-component re-render amplification**: `<For each={filteredSessions}>` in sidebar re-runs its diff on every `sessions` mutation. In unified mode, this is the SAME cost as current sidebar-only mode — terminal's co-existence doesn't amplify. ResizeObserver in TerminalView is scoped to `hostRef`, fires on layout-affecting changes only.

**One subtle risk I'll flag but not push-back**: `createStore` in SolidJS uses proxy-based reactivity. When the sidebar mutates a single session's `waitingForInput`, SolidJS updates only that row's reactive subscribers. If a future change accidentally replaces the entire `sessions` array (e.g. `setState("sessions", [...new array])` instead of targeted path-mutations), ALL For-loop children re-create. This is a sidebar-internal discipline concern that exists today and doesn't change with unification. Mention only for completeness.

**Verdict:** embedded-mode reactivity is **safe**. No cascading re-render risk introduced by unifying sidebar + terminal in the same document.

### R2.7 — Commentary on the two dev-judgment items

A2.12 flags two implementer judgment calls awaiting dev-enrichment commentary:

**(a) G.12 CSS scope: `@layer` vs manual-scope.** My call: **manual-scope prefix** (option b). Reasons:
- Cascade layers work in Chromium 99+ and Tauri 2.x bundles recent Chromium, so technically safe. BUT `@layer` still surprises many readers; explicit prefixing (`.sidebar-layout { ... }`, `.terminal-layout { ... }`) is self-documenting.
- Our codebase has no other `@layer` usage. Introducing one layer system for one feature is inconsistent.
- The audit surface is small: grep `^body\|^html\|^::\|^\*` in `sidebar.css` and `terminal.css`. Each finding gets a one-line prefix change. Implementation cost is trivial.
- `@layer` is strictly necessary only when we can't modify the source files. We own both files — we can prefix directly.

If either sidebar.css or terminal.css has dozens of bare selectors, revisit and use `@layer` (one-shot wrap). Otherwise, manual prefix wins.

**(b) `f64::EPSILON` guard in A2.4.Arb2 migration.** My call: **adopt it**. Reasons:
- Zero cost, defense against future floating-point weirdness (e.g. if someone ever changes `default_zoom()` from 1.0 to a computed value).
- Reads as "migrate when the in-memory value is effectively the default" — the intent is semantically clearer than exact `== 1.0`.
- A2.11.N3 already documents that the seed path is one-shot (skip_serializing_if ensures next save drops the legacy field; subsequent loads have `main_zoom != 1.0` and re-seed is skipped). EPSILON guard doesn't conflict with that flow.

### R2.8 — Round-2 review summary

- **Consensus**: YES, no round 3 needed.
- **Items accepted**: 15/15 round-1 items integrated correctly.
- **Push-backs**: 0.
- **New concerns**: 3 — R2.2 (`closeDetached` dead code, MEDIUM), R2.4 (G.13 re-entrancy + Enter-default, MEDIUM), R2.5 (splitter load-time clamp, LOW). All are Phase 1 implementation deltas, not architectural disputes.
- **Nit**: R2.3 (DW.3 foundational CSS discoverability, LOW). Optional.
- **Commentary**: R2.7 picks manual-scope prefixing for G.12 CSS; adopt `f64::EPSILON` guard for Arb-2 migration.

Ready for implementation once dev-rust signals the same or minor round-3 items from either side. No blockers from the frontend.

— dev-webpage-ui (round 2)

---

## Architect round 3 — close blockers

**Author:** architect
**Scope:** close dev-rust's 1 HIGH + 3 integration gaps, accept dev-webpage-ui's 3 Phase-1 concerns, lock in the 2 dev-judgment items, resolve the 1 optional DR2.2 tightening.
**Anchored against HEAD:** `60dd162` (unchanged).
**Verdict up front:** all 9 required items accepted; DR2.2 optional declined with reasoning. **Fix A adopted** for NEW-1 — mooted R.7 entirely, simplifying the plan. Two new bugs surfaced during integration (A3.7) addressed in-round.

### A3.0 Summary (TL;DR)

- **NEW-1: Fix (A) adopted.** `was_detached: bool` lives on `Session` (runtime + persisted via `PersistedSession`). `detach_terminal_inner` sets it true; `attach_terminal` sets it false. `snapshot_sessions` reverts to round-1 signature (no `&HashSet<Uuid>` parameter). R.7's 10-call-site threading is mooted and reverted.
- **PB-4** one-word delta: `&info.id` replaces `&ps.id.as_deref().unwrap_or(...)` in A2.8 G.18 text.
- **PB-2** adds a new §5.2 / A2.10 row for `src/main.tsx` with the R.8 `!isTauri` early-return skeleton.
- **PB-3** completes `close_detached_terminal` deletion across round-1 §5.1 (rows 208 / 212).
- **R2.2** deletes `WindowAPI.closeDetached` from `src/shared/ipc.ts`.
- **R2.4** adds `quitInProgress` guard + Cancel-as-default + per-destroy try/catch to A2.6's skeleton.
- **R2.5** adds splitter clamp-on-load + `window.resize` listener to §3 / A2.9.
- **G.12 CSS scope**: locked to **manual-prefix** (sidebar.css + terminal.css rules get `.sidebar-layout` / `.terminal-layout` prefix; no `@layer`).
- **Arb-2 EPSILON guard**: locked to **adopt** in the migration branch.
- **DR2.2 optional** tightening declined — see A3.6.
- **New bugs surfaced:** NEW-2 (attach_terminal must clear `Session::was_detached`) and NEW-3 (Destroyed handler must emit `terminal_attached` for frontend sync but must NOT mutate `Session::was_detached`, or NEW-1 returns via the quit path). Both addressed in A3.7.

Total round-3 plan-text delta: ~10 sections touched, ~180 lines of net additions (plus reverts of round-2's R.7 threading which subtracts line count). No architectural re-litigation.

### A3.1 Triage table (all 10 items)

| # | Item | Severity | Disposition | Phase |
|---|---|---|---|---|
| 1 | NEW-1 quit-path persistence ordering | HIGH | **Accept Fix (A)** | 1 (skeleton) / 2–3 (runtime path) |
| 2 | PB-4 `&info.id` wording | HIGH | **Accept** | 3 |
| 3 | PB-2 R.8 browser-mode hole | HIGH | **Accept** | 1 |
| 4 | PB-3 complete `close_detached_terminal` deletion | LOW | **Accept** | 1 |
| 5 | R2.2 `WindowAPI.closeDetached` dead wrapper | MEDIUM | **Accept** | 1 |
| 6 | R2.4 G.13 `ask()` re-entrancy + destructive default | MEDIUM | **Accept** | 1 |
| 7 | R2.5 splitter width load-time clamp | LOW | **Accept** | 1 |
| 8 | G.12 CSS scope (manual vs @layer) | — | **Lock manual-prefix** | 1 |
| 9 | Arb-2 `f64::EPSILON` guard | — | **Lock adopt** | 1 |
| (opt) | DR2.2 A2.2.G1 recheck-insert race | LOW | **Decline** | — |

**Totals on the 9 required items:** 9 accepted, 0 modified, 0 rejected. Optional DR2.2 declined.

### A3.2 NEW-1 — Fix (A): `Session::was_detached` adoption

**Decision: Fix (A). Reject (B).**

**Rationale for (A):**
1. **Ordering-free.** `Session::was_detached` is mutated only by `detach_terminal_inner` (→true) and `attach_terminal` (→false). Destroyed-event handler no longer touches it (see A3.7 NEW-3). At `RunEvent::Exit` persist time, every session's `was_detached` already reflects correct pre-destroy state regardless of how many detached windows `A2.6` just ripped down.
2. **Moots R.7.** The round-2 `&HashSet<Uuid>` signature-threading across 10 call sites reverts. Net code churn: **negative** — we remove more than we add.
3. **Pattern parity with Arb-1.** `detached_geometry: Option<WindowGeometry>` already lives on the session. `was_detached: bool` lives next to it. One persistence idiom for the detached-window feature.
4. **No sync primitive added.** No AppState flags, no quit-in-progress booleans on the Rust side, no new Tauri commands.
5. **Rejects (B) reasoning.** Fix (B) required a new `force_persist` IPC command + reliance on `RunEvent::Exit` NOT running a second snapshot (otherwise the second snapshot stomps the force-persisted state with stale `DetachedSessionsState`-derived values). That works only with a new "persist_already_ran" flag in AppState, which is exactly the complexity (A) avoids.

**Rejects (C).** No creative third option surfaced during integration — (A) is strictly simpler and (B) is strictly worse.

#### A3.2.1 — Data model changes (supersedes round-2 §6 additions where noted)

**`src-tauri/src/session/session.rs`** — extend `Session` struct (anchored at round-1 §6.2 / plan line 43):

```rust
pub struct Session {
    // ... existing fields ...
    /// True while this session has a live detached window (or is marked to
    /// re-spawn one on next launch). Source of truth for persistence —
    /// snapshot_sessions reads this directly, NOT from DetachedSessionsState.
    ///
    /// Mutated ONLY by:
    ///   - `detach_terminal_inner` → true (after window build + session recheck)
    ///   - `attach_terminal` → false (before emitting terminal_attached)
    /// The Destroyed event handler at lib.rs:697-717 does NOT touch this
    /// field (NEW-3) — it only clears DetachedSessionsState and emits
    /// terminal_attached for frontend sync.
    #[serde(default)]
    pub was_detached: bool,
}
```

Matching addition to `SessionInfo` + the `From<&Session>` impl copying the field. PersistedSession already has `was_detached` per round-1 §6.2.

`SessionManager` gets a new method alongside `mark_idle` / `mark_busy` / `set_git_repos_if_gen`:

```rust
pub async fn set_was_detached(&self, id: Uuid, detached: bool) {
    let mut sessions = self.sessions.write().await;
    if let Some(s) = sessions.get_mut(&id) {
        s.was_detached = detached;
    }
}
```

#### A3.2.2 — `snapshot_sessions` reverts to round-1 shape

Revert all of round-2 A2.10's changes to `snapshot_sessions` / `persist_current_state` / `persist_merging_failed`:

- Signatures revert to `(mgr: &SessionManager)` — no `&HashSet<Uuid>` parameter.
- `snapshot_sessions` populates `PersistedSession.was_detached` from `session.was_detached` directly:

```rust
// Inside snapshot_sessions, per-session map closure:
PersistedSession {
    // ... existing ...
    was_detached: s.was_detached,    // NEW — reads Session field directly
    detached_geometry: s.detached_geometry.clone(), // from Arb-1
    // ... existing ...
}
```

- All 10 persist call sites (R.7) revert to their round-1 shape. No app-state lookups, no `HashSet` snapshot-clones, no lock-across-await audits. The "don't hold lock across await" concern R.7 raised becomes moot because there's no lock being held to begin with.

#### A3.2.3 — `detach_terminal_inner` skeleton update (supersedes round-2 A2.2.G1)

After the "insert into DetachedSessionsState" step, also set the session field. Concretely (showing only the delta from round-2 A2.2.G1):

```rust
// Insert into DetachedSessionsState (round-2 A2.2.G1 step)
{ let mut s = detached.lock().unwrap(); s.insert(uuid); }

// NEW (Fix A): sync Session::was_detached. Ordering: insert → write field.
// Both happen on the success path only (after post-build session-existence check).
{
    let mgr = session_mgr.read().await;
    mgr.set_was_detached(uuid, true).await;
}
```

The focus-existing short-circuit path at the top of `detach_terminal_inner` is intentionally NOT updated — if the window already exists, `Session::was_detached` is already true (no path clears it without first destroying the window or calling `attach_terminal`). Adding a defensive re-assert would cost a read-lock grab for no benefit. Leave alone.

#### A3.2.4 — `attach_terminal` skeleton update (supersedes round-2 A2.2.G5)

Before the "emit `terminal_attached`" step, clear the session field. Delta from round-2 A2.2.G5:

```rust
// A2.2.G5 contract: session-existence check already done; session lives.

// NEW (Fix A): clear was_detached BEFORE switch + emit, so any intervening
// snapshot captures the correct state.
mgr.set_was_detached(uuid, false).await;

// Session lives → promote to active in main.
mgr.switch_session(uuid).await.map_err(|e| e.to_string())?;
let _ = app.emit("terminal_attached", serde_json::json!({ "sessionId": session_id }));
let _ = app.emit("session_switched", serde_json::json!({ "id": session_id }));

Ok(())
```

#### A3.2.5 — Phase-impact summary for Fix (A)

- **Phase 1:** add `Session::was_detached` field + `SessionManager::set_was_detached`. `detach_terminal_inner` calls the setter. No `attach_terminal` yet, so the clear-path isn't exercised. Quit-path correctness is satisfied because Phase 1 has no re-attach gesture — X-close leaves was_detached true and restart restores the detached window (matches Phase 1 ship-bar which already documents "X-closes-window-but-session-stays-alive").
- **Phase 2:** `attach_terminal` lands with the Fix-A clear-path. onCloseRequested + re-attach semantics compose correctly: X → attach_terminal → was_detached=false → next persist captures no-restore intent.
- **Phase 3:** restore-loop reads `ps.was_detached` from disk; `create_session_inner` writes it onto the newly-created `Session` (need to thread the `was_detached` into the restore path — see §5.1 addition below); `detach_terminal_inner` called on the session then sets it again redundantly (idempotent).

**Critical restore-path fix** (folds into Phase 3 work): `create_session_inner` must accept the restored `was_detached` value. Simplest: after `create_session_inner` succeeds, the restore loop calls `mgr.set_was_detached(uuid, ps.was_detached).await` BEFORE calling `detach_terminal_inner`. This way, if the session is restored but the `detach_terminal_inner` fails (e.g. WebView2 init failure), `Session::was_detached` is correctly true so next snapshot persists it and the user gets another chance on the following launch.

#### A3.2.6 — Section-level plan-text deltas (consolidated for Fix A)

- **§6.1** — no change (AppSettings unchanged; everything lives on Session).
- **§6.2** — amend: add `was_detached: bool` to `Session` (runtime), keep `was_detached` on `PersistedSession` (round-1 §6.2 already has it). Rename the round-2 §6.2 framing from "populated from `DetachedSessionsState` at snapshot time" to "populated from `Session::was_detached` directly."
- **§2.2.4** — replace the round-2 persistence bullet with: "`was_detached` lives on `Session` (runtime-synced by `detach_terminal_inner` → true and `attach_terminal` → false). `snapshot_sessions` reads it directly. The Quit-path destroy-then-persist ordering is no longer a concern because the Destroyed event handler does not mutate `Session::was_detached` (NEW-3)."
- **§5.1** — delete the round-2 "threading `DetachedSessionsState`" rows (part of A2.10's R.7 additions). Add a row for `session/session.rs`: "add `was_detached: bool` field + `SessionInfo` mirror + `From<&Session>` copy". Add a row for `session/manager.rs`: "add `set_was_detached(uuid, bool)` method". Update the A2.2.G1 and A2.2.G5 skeleton references to the A3.2 deltas.
- **§5.1 restore row for `lib.rs:608-614`** — add step: "before calling `detach_terminal_inner` from the restore loop, invoke `mgr.set_was_detached(uuid, true).await` (idempotent but safe if `detach_terminal_inner` fails mid-run)."
- **§9 Phase 1 ship-bar** — remove "10-site call-site threading" bullet added in round 2. Replace with "`Session::was_detached` field + `set_was_detached` method ship in Phase 1 so Phase 2 + Phase 3 inherit the hooks."
- **§10** — add rule: "The `WindowEvent::Destroyed` handler at `lib.rs:697-717` MUST NOT mutate `Session::was_detached`. If you add logic there that needs to flip the bit, you've introduced NEW-1 again. Use `attach_terminal` instead (or call `mgr.set_was_detached(uuid, false).await` explicitly at the call site that knows it's an attach gesture)."

### A3.3 PB-4 — `&info.id` wording — Accept

Round-2 A2.8 G.18 text says:

> "call `detach_terminal_inner(app, mgr, detached, &ps.id.as_deref().unwrap_or(...), ps.detached_geometry.clone(), skip_switch: true)`"

**Delta:** rewrite to use the SessionInfo returned by `create_session_inner` (the newly-live session UUID, not the stale persisted one):

> "call `detach_terminal_inner(app, mgr, detached, &info.id, ps.detached_geometry.clone(), skip_switch: true)`"

Update identical references if present in:
- §2.2.4 item 4 (restore path detach spawn).
- §5.1 row for `lib.rs:608-614` modify (wherever the restore-loop snippet is shown).
- §A2.8 G.18 LOW triage paragraph.

**Phase landing:** Phase 3 (restore work). Doc-only delta — one identifier change.

### A3.4 PB-2 — R.8 browser-mode guard — Accept

**Delta:** add a new row to §5.2 / A2.10 (frontend additions) explicitly for `src/main.tsx`:

| File | Change type | Summary | Phase |
|---|---|---|---|
| `src/main.tsx` | modify | R.8 browser-mode guard: `!isTauri` early-return to `BrowserApp`; only apply Tauri-only `?window=main` / `?window=detached` routing after the guard. See A3.4 skeleton. | 1 |

**Canonical skeleton** (for the implementer):

```tsx
import "./shared/console-capture";
import { render } from "solid-js/web";
import { isTauri } from "./shared/platform";
import MainApp from "./main/App";
import TerminalApp from "./terminal/App";
import GuideApp from "./guide/App";
import BrowserApp from "./browser/App";

const params = new URLSearchParams(window.location.search);
const windowType = params.get("window");

const remoteToken = params.get("remoteToken");
if (remoteToken) sessionStorage.setItem("remoteToken", remoteToken);

const root = document.getElementById("root");
if (!root) throw new Error("Root element not found");

if (!isTauri) {
  // Browser (web remote): BrowserApp handles every URL — legacy ?window=sidebar,
  // legacy ?window=terminal, and the new ?window=main all route here. MainApp's
  // Tauri-only APIs (onCloseRequested, WebviewWindow.getAll, persistWidth via
  // SettingsAPI) would silently fail in browser mode.
  render(() => <BrowserApp />, root);
} else if (windowType === "detached") {
  const lockedSessionId = params.get("sessionId") || undefined;
  render(() => <TerminalApp lockedSessionId={lockedSessionId} detached />, root);
} else if (windowType === "guide") {
  render(() => <GuideApp />, root);
} else {
  // ?window=main, legacy ?window=sidebar / ?window=terminal, or no param.
  render(() => <MainApp />, root);
}
```

Replaces round-1 §5.2's `src/main.tsx` row (which said only "new routing dispatcher") with this authoritative skeleton. §A2.10 gets the new row.

**Phase landing:** Phase 1. Required for shipping Phase 1 without breaking the web-remote path after `lib.rs:299`'s URL rewrite lands.

### A3.5 PB-3 — Complete `close_detached_terminal` deletion — Accept

Round-1 §5.1 still carries two rows that describe `close_detached_terminal` as "retained as internal helper":

- Row for `commands/window.rs:193-213` ("`close_detached_terminal` stays as internal helper — used by `destroy_session_inner`. Remove from public `invoke_handler!`").
- Row for `commands/window.rs:195-213` ("retained but becomes a pure window-close helper").

**Delta:** both rows change to **DELETE the function entirely.** Replacement row text:

> `commands/window.rs:193-213` — **delete** `close_detached_terminal` function. Zero callers post-round-3 (dev-webpage-ui R.2 + R.3 + round-2 A2.10's `web/commands.rs` edit + round-3 R2.2 all agree). `destroy_session_inner` already handles both cleanups (DetachedSessionsState removal at `session.rs:683-688`, window destroy at `session.rs:722-726` — per R.2 uses `destroy()` not `close()`).

**Phase landing:** Phase 1 (simultaneous with R.5 / A2.10's `web/commands.rs` edit that drops `"close_detached_terminal"` from the no-op arm).

### A3.6 R2.2 — Delete `WindowAPI.closeDetached` — Accept

**Delta:** add row to §5.2 / A2.10 additions:

| File | Change type | Summary | Phase |
|---|---|---|---|
| `src/shared/ipc.ts` | modify | Delete `WindowAPI.closeDetached` (lines 179-180). Zero callers confirmed in round 1 / round 2 by DW.13.4 and R2.2. Backend command is gone (PB-3); the shim would throw "Command not found" if invoked. | 1 |

### A3.7 R2.4 — G.13 `ask()` hardening — Accept

Three sub-fixes:

**(a) Re-entrancy guard** via module-level `quitInProgress` flag.
**(b) Cancel-as-default** via flipped `okLabel` / `cancelLabel` semantics.
**(c) Try/catch around each destroy** so one failure doesn't abort the rest.

**Delta:** replace round-2 A2.6's skeleton with dev-webpage-ui's R2.4 skeleton verbatim (shown in R2.4, reproduced here for the record):

```tsx
let quitInProgress = false;

const unlistenMainClose = await win.onCloseRequested(async (e) => {
  if (quitInProgress) { e.preventDefault(); return; }
  const detachedCount = sessionsStore.detachedIds?.size ?? 0;
  if (detachedCount === 0) return;  // no confirmation needed; let close proceed
  e.preventDefault();
  quitInProgress = true;
  try {
    const { ask } = await import("@tauri-apps/plugin-dialog");
    // NOTE: okLabel is the Enter-default. We put the SAFE option (Cancel) on
    // okLabel so Enter does NOT trigger destructive quit. (R2.4 UX safety.)
    const cancelConfirmed = await ask(
      `You have ${detachedCount} detached session${detachedCount === 1 ? "" : "s"} open. ` +
      `Quit the app and close all detached sessions?`,
      { title: "Quit AgentsCommander?", kind: "warning",
        okLabel: "Cancel", cancelLabel: "Quit" }
    );
    if (!cancelConfirmed) {
      // User chose Quit (the cancelLabel button) — proceed.
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      for (const w of await WebviewWindow.getAll()) {
        if (w.label.startsWith("terminal-")) {
          try { await w.destroy(); } catch (err) {
            console.warn("[quit] destroy of", w.label, "failed:", err);
          }
        }
      }
      try { await win.destroy(); } catch (err) {
        console.warn("[quit] destroy of main failed:", err);
      }
    }
  } finally {
    quitInProgress = false;
  }
});
```

**Section impact:**
- §A2.6 supersede skeleton with A3.7 version.
- §10 add rule: "Main X-close destructive confirmation MUST use Cancel as the Enter-default button (okLabel: \"Cancel\", cancelLabel: \"Quit\"). Do not flip these without simultaneously adding a typed-confirmation field — Enter-default = destructive is a data-loss trap."
- §A2.6 test plan §8.7.40 — add sub-case: "Open confirmation dialog, press Enter immediately → Cancel is triggered, app stays open. Users who mash Enter do not lose detach state."
- §A2.6 test plan — add sub-case: "Close main via X while confirmation dialog is already open (double-X, Alt+F4-during-ask): second close attempt is silently ignored (quitInProgress=true)."

### A3.8 R2.5 — Splitter width load-time clamp — Accept

**Delta:** add to §3 (splitter mechanics) the clamp-on-load and `window.resize` listener per R2.5's skeleton:

```tsx
// In main/App.tsx onMount, AFTER reading saved settings:
const settings = await SettingsAPI.get();
const saved = settings.mainSidebarWidth ?? 240;
// Clamp at load: handles "saved on big monitor, restored on small monitor" edge.
const clamped = Math.max(200, Math.min(600,
  Math.min(saved, window.innerWidth - 300)));
setSidebarWidth(clamped);

// Re-clamp on window resize (covers DPI change, monitor hot-plug, OS-snap).
const onResize = () => {
  setSidebarWidth(w => Math.max(200, Math.min(w, window.innerWidth - 300)));
};
window.addEventListener("resize", onResize);
onCleanup(() => window.removeEventListener("resize", onResize));
```

**Section impact:**
- §3 required-behavior list gains: "Splitter width is clamped on load and on window resize to `[200, min(600, windowWidth - 300)]`, matching the drag-time clamp."
- §8.2 test plan gains §8.2.9: "Save `mainSidebarWidth = 600` on a 1920-wide monitor, move window to an 800-wide monitor, restart — sidebar loads at the clamped max (500 = 800-300), terminal pane remains visible with ≥300px."

### A3.9 G.12 CSS scope — Lock manual-prefix (option b)

Both devs aligned on **manual-scope prefix**. Adopted.

**Plan-text delta:**
- §A2.7 G.12 paragraph rewrite: "Resolution: **manual prefix**. During Phase 1 CSS audit, any global selector in `sidebar.css` or `terminal.css` — bare `body`, `html`, `::before/::after`, `*`, or any unprefixed element/pseudo-element selector — gets an explicit `.sidebar-layout` or `.terminal-layout` parent prefix in its source file. Do NOT introduce `@layer` cascade blocks (we have no other `@layer` in the codebase; adding one system for one feature is inconsistent)."
- §5.2 CSS audit row: grep pattern for the audit: `^body|^html|^::[a-z]|^\*|^[a-z]+[^.\w]` in `src/sidebar/styles/sidebar.css` and `src/terminal/styles/terminal.css`. Each hit gets prefixed at source.

### A3.10 Arb-2 `f64::EPSILON` guard — Lock adopt

Both devs aligned on adopting. The epsilon guard already appears in round-2 A2.4.Arb2's migration branch skeleton; this round-3 decision locks it in as non-optional.

**Plan-text delta:**
- §A2.4.Arb2 migration block gets a line-note: "`f64::EPSILON` guard is REQUIRED, not optional. Reason: defense against future changes to `default_zoom()` that would make exact `== 1.0` comparisons miss. Zero cost. Both dev-rust and dev-webpage-ui align."

### A3.11 DR2.2 optional — Decline

Dev-rust flagged a 3-instruction race window in A2.2.G1's post-build session-existence recheck vs UUID-insert:

1. Post-build `mgr.get_session(uuid).await.is_none()` check returns false (session lives).
2. Lock `detached` set, insert UUID.
3. Concurrent thread destroys session between steps 1 and 2.

The result would be a stale UUID in `DetachedSessionsState` for a session that no longer exists. Dev-rust labeled this as "acceptable as-is".

**Decision: decline the tightening.**

**Reasoning:**
1. **Self-healing.** `destroy_session_inner` already removes UUIDs from `DetachedSessionsState` idempotently at `session.rs:683-688`, and closes any `terminal-<uuid>` window via `get_webview_window` + `destroy()` at `session.rs:722-726`. If the session was destroyed concurrently with detach, the destroy path picks up the just-inserted UUID and the just-built window on its next action. The stale-UUID window is at most transient.
2. **Race window is sub-millisecond** on the single Tauri-command-handler thread. The only way to hit it is two separate user actions (click-detach + click-destroy) scheduled by the user within the same event loop tick. Each action goes through its own `#[tauri::command]` boundary, which serializes against the `session_mgr.read().await` lock naturally — so the actual race is against the interleaving of `await` points, which the current code structure avoids by keeping mgr-reads and set-mutations in non-overlapping scopes.
3. **Fix A makes this strictly safer than it was pre-round-3.** With Session::was_detached now authoritative for persistence, a stale `DetachedSessionsState` UUID has no persistence impact — it only affects runtime behavior of `switch_session` (which would try to focus a non-existent window) and the `list_detached_sessions` hydration (which would seed a detachedIds set with a now-invalid entry, self-corrected on the next `terminal_attached` emission from the Destroyed handler). Both are self-healing.
4. **Belt-and-braces cost is real.** Dev-rust's proposed tightening requires holding both locks (manager read + detached write) simultaneously, which violates the "no lock across await" discipline that R.7 itself warned against. Tightening this race creates a bigger problem.

Accepting the current shape. If grinch round-3 finds a concrete exploit path (e.g. a specific user action sequence that leaves a stale UUID producing user-visible breakage that isn't self-healed), revisit in round 4. My current belief: that exploit doesn't exist.

### A3.12 New bugs surfaced during round-3 integration

Per the round-1 rule ("if integrating a finding reveals a new bug, flag and address in the same round"):

#### NEW-2 — `attach_terminal` must clear `Session::was_detached`

Discovered during A3.2.4 skeleton authoring. If Fix (A) adds `Session::was_detached` but `attach_terminal` doesn't clear it, the following breaks: user detaches A (was_detached=true), re-attaches A (attach_terminal runs, window destroyed), but `Session::was_detached` remains true → next persist writes was_detached=true → next restart re-spawns A's detached window even though user explicitly attached it. User sees "re-attach doesn't stick across restarts."

**Fix:** `attach_terminal` calls `mgr.set_was_detached(uuid, false).await` BEFORE emitting events. Already folded into A3.2.4 skeleton. **No additional section impact** — the attach_terminal row in §5.1 references A3.2.4 directly.

#### NEW-3 — Destroyed handler MUST emit `terminal_attached` but MUST NOT mutate `Session::was_detached`

Discovered during A3.2 design review. The `WindowEvent::Destroyed` handler at `lib.rs:697-717` currently only clears `DetachedSessionsState`. For unified mode we need:

**(a) Frontend sync.** If a detached window is destroyed (by X, by Alt+F4, by `destroy()` from any code path), the sidebar's `detachedIds` set must clear for that UUID — otherwise the sidebar shows a detached indicator for a session that no longer has a window. **Fix:** emit `terminal_attached` from the Destroyed handler, which the `sessionsStore.setDetached(id, false)` listener picks up in Phase 2+. In Phase 1 (no `detachedIds` listener yet) the event is received by nothing — harmless.

**(b) Do NOT mutate `Session::was_detached`.** Tempting to mirror the frontend-sync by also clearing the backend field on window-gone. **If we do that, NEW-1 returns** via the quit path: A2.6 destroys every detached window → Destroyed fires for each → was_detached cleared → persist reads all false → restart restores nothing detached. Fix A explicitly reserves `Session::was_detached` mutation for `detach_terminal_inner` (→true) and `attach_terminal` (→false) only.

**Plan-text delta (Destroyed handler):**

```rust
// src-tauri/src/lib.rs:697-717 — UPDATED handler body
tauri::RunEvent::WindowEvent {
    label,
    event: tauri::WindowEvent::Destroyed,
    ..
} => {
    if let Some(id_no_dashes) = label.strip_prefix("terminal-") {
        if id_no_dashes.len() == 32 {
            let formatted = format!(
                "{}-{}-{}-{}-{}",
                &id_no_dashes[0..8],
                &id_no_dashes[8..12],
                &id_no_dashes[12..16],
                &id_no_dashes[16..20],
                &id_no_dashes[20..32],
            );
            if let Ok(uuid) = uuid::Uuid::parse_str(&formatted) {
                // 1) Clear from DetachedSessionsState — existing behavior.
                //    switch_session needs an accurate view of which sessions
                //    have live windows to decide focus-vs-switch.
                {
                    let mut set = detached_set.lock().unwrap();
                    set.remove(&uuid);
                }
                // 2) NEW (A3.12.NEW-3(a)): emit terminal_attached for frontend
                //    detachedIds sync. Phase 2+ subscribers clear the id from
                //    sessionsStore; Phase 1 receives nothing (no subscriber)
                //    — harmless.
                let _ = tauri::Emitter::emit(
                    app_handle,
                    "terminal_attached",
                    serde_json::json!({ "sessionId": formatted }),
                );
                // 3) DELIBERATELY ABSENT (A3.12.NEW-3(b)): we do NOT mutate
                //    Session::was_detached here. The persist-on-quit path
                //    relies on that field staying true until attach_terminal
                //    (user-initiated) clears it. See §10 rule + NEW-1.
            }
        }
    }
}
```

**Section impact:**
- §5.1 row for `src-tauri/src/lib.rs` Destroyed handler: update from "keep" to "modify per A3.12.NEW-3" — emit `terminal_attached`; do NOT touch `Session::was_detached`.
- §10 add rule (mentioned in A3.2.6 already, reproduced for completeness): "The `WindowEvent::Destroyed` handler MUST NOT mutate `Session::was_detached`. Emit `terminal_attached` for frontend sync; that's the only behavior change from today."
- §8.6 test plan gains §8.6.28: "Detach A, quit main window via X (confirmation dialog → Quit), restart — A is restored as detached. (Proves NEW-1 fix: was_detached preserved across destroy-then-persist ordering.)"
- §8.3 test plan gains §8.3.16: "Detach A, re-attach via titlebar button, quit normally, restart — A is restored as attached, NOT detached. (Proves NEW-2 fix: attach_terminal clears was_detached.)"

### A3.13 §11 — no change

Round 2 closed all §11 open questions. Round 3 does not reopen any. The §11 closure table (from A2.5) remains authoritative.

### A3.14 §9 — phase scope adjustments

**Phase 1 (net effect: roughly flat).**

*Added* (round 3):
- PB-2 `src/main.tsx` R.8 guard (~20 LoC).
- PB-3 delete `close_detached_terminal` entirely (subtracts lines).
- R2.2 delete `WindowAPI.closeDetached` (subtracts lines).
- R2.4 G.13 hardening in A2.6 (quitInProgress + try/catch + Cancel-default; ~10 LoC diff from A2.6).
- R2.5 splitter clamp on load + window.resize (~10 LoC).
- G.12 CSS manual-prefix audit (~5-15 prefix edits across two CSS files, depending on what the audit finds).
- Fix A field addition on `Session` + `set_was_detached` method + `detach_terminal_inner` calls the setter (~15 LoC).
- NEW-3 Destroyed handler emits `terminal_attached` (~5 LoC).

*Removed* (round 3):
- R.7 10-call-site threading (reverts round-2 A2.10 additions — negative line count).
- `close_detached_terminal` function body and its `invoke_handler!` reference (negative line count).
- `WindowAPI.closeDetached` wrapper (negative line count).

Net: Phase 1 ship-bar stays at roughly the same complexity as round-2 Phase 1 (~70% of total diff).

**Phase 2 (no change).**

A2.2.G4 onCloseRequested skeleton unchanged; A2.2.G5 attach_terminal now includes the `set_was_detached(false)` call (A3.2.4). `list_detached_sessions` hydration unchanged.

**Phase 3 (minor changes).**

- PB-4 wording fix (`&info.id` not `&ps.id`).
- Restore loop calls `mgr.set_was_detached(uuid, ps.was_detached).await` BEFORE `detach_terminal_inner` (A3.2.5).
- A2.3.G6 TerminalView pre-warm listener unchanged.

Ship-bar unchanged modulo these two edits.

### A3.15 §5 impact map — round-3 consolidated additions

Additive to round-1 + round-2 rows. Appended at the end of §A2.10 scope.

**§5.1 Rust additions / reverts (round 3):**

| File | Lines | Change type | Summary | Phase |
|---|---|---|---|---|
| `src-tauri/src/session/session.rs` | 41-90 | modify | Add `was_detached: bool` to `Session` + `SessionInfo`; update `From<&Session>` for SessionInfo to copy. | 1 |
| `src-tauri/src/session/manager.rs` | — | modify | Add `pub async fn set_was_detached(&self, id: Uuid, detached: bool)` method. | 1 |
| `src-tauri/src/commands/window.rs` | — | modify | `detach_terminal_inner` calls `mgr.set_was_detached(uuid, true).await` after inserting into `DetachedSessionsState` (A3.2.3). | 1 |
| `src-tauri/src/commands/window.rs` | — | modify | `attach_terminal` calls `mgr.set_was_detached(uuid, false).await` before emit (A3.2.4). | 2 |
| `src-tauri/src/commands/window.rs` | 193-213 | **DELETE** | `close_detached_terminal` function entirely (PB-3). Remove from `invoke_handler!`. | 1 |
| `src-tauri/src/lib.rs` | 697-717 | modify | Destroyed handler per A3.12.NEW-3 — emit `terminal_attached`, do NOT touch `Session::was_detached`. | 1 |
| `src-tauri/src/lib.rs` | 608-614 | modify | Restore loop calls `mgr.set_was_detached(uuid, ps.was_detached).await` before `detach_terminal_inner` (A3.2.5); detach call uses `&info.id` (PB-4). | 3 |
| `src-tauri/src/config/sessions_persistence.rs` | 304-343 | modify | `snapshot_sessions` reverts to round-1 signature `(mgr: &SessionManager)`; reads `s.was_detached` from `Session` directly. | 1 |
| `src-tauri/src/config/sessions_persistence.rs` | 616-633 | **REVERT** | `persist_current_state` + `persist_merging_failed` revert to round-1 signatures (no `&HashSet<Uuid>` parameter). | 1 |
| `src-tauri/src/lib.rs` + `commands/session.rs` (10 sites) | — | **REVERT** | Revert round-2 A2.10's call-site threading for `DetachedSessionsState`. Each site returns to its round-1 shape. | 1 |

**§5.2 Frontend additions (round 3):**

| File | Change type | Summary | Phase |
|---|---|---|---|
| `src/main.tsx` | modify | R.8 skeleton per A3.4 (isTauri guard first; Tauri dispatch after). | 1 |
| `src/shared/ipc.ts` | modify | Delete `WindowAPI.closeDetached`. | 1 |
| `src/main/App.tsx` | modify | Replace A2.6 skeleton with A3.7 skeleton (quitInProgress + Cancel-default + try/catch). | 1 |
| `src/main/App.tsx` | modify | Splitter clamp-on-load + window.resize listener per A3.8. | 1 |
| `src/sidebar/styles/sidebar.css`, `src/terminal/styles/terminal.css` | modify | G.12 manual-prefix audit — prefix any global selectors with `.sidebar-layout` / `.terminal-layout` respectively. | 1 |

### A3.16 Consensus status

Round 3 closes consensus:

- **dev-rust push-backs**: all 4 resolved (NEW-1 Fix A + PB-2 + PB-3 + PB-4).
- **dev-rust optional**: DR2.2 declined with self-healing argument.
- **dev-webpage-ui new concerns**: all 3 resolved (R2.2 + R2.4 + R2.5).
- **Dev-judgment items**: both locked (G.12 manual-prefix, Arb-2 EPSILON guard).
- **New bugs surfaced**: NEW-2 + NEW-3 addressed in-round.

No open questions, no deferred items (beyond G.19 which was deferred in round 2 per grinch's own recommendation). R.7 threading reverted (net simplification). `close_detached_terminal` dead-code cleanup completed.

**Ready for grinch round 3 adversarial pass.** Items I expect grinch may probe:
1. **NEW-3 Destroyed handler** — the "emit but don't mutate" invariant is subtle. Grinch will probably test: can any sequence of events land `Session::was_detached=true` but `DetachedSessionsState` empty in a way that breaks `switch_session`? Answer: yes, that's the Phase 1 X-close case. It's covered by `switch_session` already falling through to normal switch when the UUID is missing from the set.
2. **Fix A + restore race** — if `create_session_inner` succeeds but the subsequent `set_was_detached(uuid, ps.was_detached)` fails (lock poisoned, OOM), the session lives but `Session::was_detached=false`, while `ps.was_detached=true`. On next snapshot, was_detached=false written. Mitigation: `set_was_detached` uses `tokio::sync::RwLock::write` which doesn't fail under normal conditions; acquiring the write lock is infallible for this code path.
3. **A3.7 `quitInProgress` re-entry from async path** — if the user somehow triggers close via a non-X path (e.g. tray close, programmatic `win.close()` from another window that isn't present in unified mode but might be in Phase 4 extras) while `await ask(...)` is pending, can the `finally { quitInProgress = false }` race with a second `onCloseRequested` that checks `quitInProgress` before `finally` runs? JavaScript is single-threaded; the `finally` runs to completion before any awaiting handler resumes. Safe.
4. **Migration EPSILON guard edge case** — if a user set `sidebar_zoom = 1.0 + ε` (they can't type that in the UI but a hand-edited settings.json could), the guard skips the migration and main gets default_zoom. Acceptable — the user's effectively-unity value wasn't meaningfully different from default.

If grinch finds something genuinely new at the severity of NEW-1, we've hit the 3-round rule and tech-lead escalates to the user. My current belief: grinch will find nits but not another correctness bug.

— architect (round 3)

---

## Dev-rust-grinch round 3 — final adversarial review

**Author:** dev-rust-grinch (final pass).
**Anchored against HEAD:** `60dd162`.
**Counts:** **1 BLOCKER** / 0 HIGH / 2 MEDIUM / 3 LOW.
**Consensus signal:** **NO** — one BLOCKER forces disposition. *But the fix is a one-line plan-text delta; tech-lead's call whether that is a true round 4 or a typo-level fold-in.*

I attacked every one of the four architect-anticipated probe areas plus the rest of the round-3 hunt list. Three of the four architect-anticipated hotspots **survived** (see "What I tried to break and couldn't"). The fourth (NEW-3 asymmetry under every destroy path) also survives the common scenarios, but one specific Phase-1 sequence exposes a subtler issue → MEDIUM M3-1. The single BLOCKER was NOT on the architect's probe list — it's a Phase-sequencing bug introduced by locking in A3.7 at Phase 1 while `sessionsStore.detachedIds` remains a Phase-2-only addition.

### G3.0 Executive summary

**The one BLOCKER (G3-B1):** A3.7's main-X handler reads `sessionsStore.detachedIds?.size ?? 0`. In Phase 1, `detachedIds` does NOT exist on `sessionsStore` (it's a Phase-2 addition per round-1 §5.2 line 238, round-1 §9 Phase 2 line 493, round-2 A2.3.G8 line 2106, round-2 A2.9 line 2369). Optional chaining resolves to `undefined`; nullish-coalescing yields `0`; the handler returns early with no confirmation. Phase 1 main-X with detached windows open → orphans the detached windows, exact bug G.13 was introduced to prevent. **Fix is two lines** (detect via `WebviewWindow.getAll()` filter instead of the Phase-2 store); does not touch the architectural shape.

Everything else in round 3 is solid. Fix A is correct under all destroy paths I could walk. The `quitInProgress` finally block is airtight under JS single-thread semantics. The EPSILON migration has no realistic miss. The `set_was_detached` lock discipline is clean across all three call sites. The NEW-3 asymmetry survives every destroy path except one noisy-but-correct scenario (M3-1).

---

### G3-B1 [BLOCKER] A3.7 Phase 1 main-X handler reads a Phase-2-only store → silent no-op → orphaned detached windows

- **Location:** Plan line 3120 (A3.15 §5.2 row: "Replace A2.6 skeleton with A3.7 skeleton ... Phase 1") + plan line 2740 (A3.7 skeleton) + plan lines 493, 2106, 2369 (`detachedIds` is Phase 2).
- **The bug:** A3.7 skeleton at line 2740 reads:
  ```tsx
  const detachedCount = sessionsStore.detachedIds?.size ?? 0;
  if (detachedCount === 0) return;  // no confirmation needed; let close proceed
  ```
  In Phase 1:
  - `sessionsStore` exists (round-1 `src/sidebar/stores/sessions.ts`) but does **not** have the `detachedIds` signal — that is explicitly Phase 2 per round-1 §9 line 493 ("sessionsStore.detachedIds set + subscribers") and round-2 A2.3.G8 line 2106 ("Phase landing: Phase 2").
  - `sessionsStore.detachedIds` → `undefined`. `undefined?.size` → `undefined`. `undefined ?? 0` → `0`.
  - Handler reports 0 detached, returns early, main closes.
  - But the user CAN detach in Phase 1 — detach functionality ships in Phase 1 (round-1 §9 Phase 1 ship-bar: "detach opens a new window"). So the real detached count is non-zero while the handler reports zero.
- **Trigger:** Phase 1 user detaches session A; clicks X on main window.
- **Impact:** Main window closes. Process keeps running (detached windows are separate top-level Tauri windows). Detached A stays open with its xterm alive. There is no sidebar in the detached window; the user has no UI path to create a new session, destroy the detached, or quit the app cleanly except via OS task manager. This is the "orphaned detached" state G.13 was explicitly introduced to prevent — shipping Phase 1 with this bug reintroduces the exact failure mode.
- **Proposed fix (plan-text delta):** in A3.7 skeleton at line 2740, replace the `sessionsStore.detachedIds` detection with a stateless enumeration that works in every phase:
  ```tsx
  // REPLACES line 2740:
  const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const allWebviews = await WebviewWindow.getAll();
  const detachedCount = allWebviews.filter(w => w.label.startsWith("terminal-")).length;
  ```
  The dynamic import is already inside the handler's destroy loop a few lines later (plan line 3136) — hoist it to the top of the `try` block so both the detection and the destroy loop use the same `allWebviews` list. Two-line delta. Zero dependency on Phase 2.
- **Why this isn't "just use a different store"**: `sessionsStore.detachedIds` is fine as the sidebar UI state; it becomes authoritative once Phase 2 lands. But A3.7's detection needs to work in Phase 1 too, and the authoritative source for "is there a detached window" is always `WebviewWindow.getAll()` — the same enumeration A3.7 already uses to destroy them. Using it for detection too is strictly cheaper (no store to maintain) and version-independent.

### G3-M1 [MEDIUM] ESC key in A3.7's `ask()` dialog triggers destructive Quit

- **Location:** A3.7 skeleton line 2740 (→ reproduced at line 3120 in A3.7 body).
- **The bug:** A3.7's `ask()` uses `okLabel: "Cancel"` / `cancelLabel: "Quit"` to make **Enter** hit the safe button (Cancel) — per R2.4's intent, correctly preventing the "mash Enter = data loss" trap. But the plan does not address **ESC**. Convention: ESC dismisses a dialog = equivalent to cancelLabel (per the Tauri plugin-dialog plus every native OS dialog convention I've checked). In this flipped setup, ESC → cancelLabel → "Quit" → `cancelConfirmed = false` → `if (!cancelConfirmed)` → **destroy everything**.
- **Trigger:** User clicks X on main while detached windows exist → confirmation dialog appears → user panic-hits ESC ("nvm, don't quit"). App proceeds to destroy all detached windows + quit.
- **Impact:** ESC-to-quit is a real data-loss vector. ESC is the keyboard action most strongly associated with "get me out of this dialog safely" in every GUI convention. The flipped labels hide this trap — reader of the code sees `okLabel: "Cancel"` and thinks "Enter is safe, ESC is also safe" — neither Tauri's dialog nor native OS dialogs honor that assumption. The R2.4 hardening only got us half the job.
- **Proposed fix:** two options, either valid. My preference is (b):
  - (a) **Keep flipped labels; test Tauri's actual ESC behavior.** If Tauri's plugin-dialog ESC returns `null` (not `false`), tighten the check: `if (cancelConfirmed === false)` rather than `if (!cancelConfirmed)`. This treats ESC (if it returns null) as "user dismissed — don't quit". Requires empirical verification of the plugin behavior. Lower confidence.
  - (b) **Replace `ask()` with a tiny custom modal** (~60 LoC + small CSS). Mount inline in MainApp when needed. Custom keyboard handling: Enter and ESC both map to Cancel. Clicking Quit is the ONLY path to destroy. One-off modal matches the rest of the app's custom-modal style anyway (OpenAgentModal / AgentPickerModal / OnboardingModal / SettingsModal are all custom per dev-webpage-ui R2.4, which actually already flagged this option as "Phase 4 polish"). Promoting to Phase 1 is the right call because the ESC trap is mild data loss, not cosmetic.
- **Section impact:** A3.7 skeleton updated or replaced. §10 rule updated: "Destructive confirmation dialogs MUST guarantee that BOTH Enter AND Escape route to the safe action. If using Tauri's `ask()`, verify ESC behavior with the specific plugin version before shipping; if unverifiable, use a custom modal."

### G3-M2 [MEDIUM] Plan §6.2 body still shows round-1 shape; `Session::was_detached` (Fix-A) only appears in A3.2 — implementer reading §6.2 alone misses the runtime field

- **Location:** Plan §6.2 (lines 293-308) vs round-3 A3.2 (line 2911).
- **The bug:** Round-1 §6.2 shows only `PersistedSession` with `was_detached: bool`. Round-3 A3.2.1 adds `Session::was_detached: bool` (runtime field on the in-memory struct) as the **authoritative source of truth for persistence** — per Fix A, `snapshot_sessions` reads from `Session`, not `DetachedSessionsState`. But §6.2's code snippet still shows only `PersistedSession` — a reader scanning §6 for data-model changes could reasonably believe the field is persistence-only.
- **Trigger:** An implementer, under time pressure, reads §6.2 + the §5.1 additions table; misses A3.2.1 buried ~2900 lines deep; implements only `PersistedSession::was_detached` and omits the `Session::was_detached` runtime field. Result: snapshot_sessions has nothing to read from; NEW-1 bug returns by omission.
- **Impact:** Fix A is effectively not shipped. Runtime: detach → persist runs → reads `Session::was_detached` which doesn't exist → compile error → caught at build time. Actually — the compile error catches this. So it's not a runtime bug, just a discoverability friction that slows down implementation.
- **Proposed fix:** amend §6.2's code block to include `Session::was_detached` alongside `PersistedSession::was_detached`:
  ```rust
  // In src-tauri/src/session/session.rs (NEW per Fix A):
  pub struct Session {
      // ... existing fields ...
      /// Runtime flag. Mutated by detach_terminal_inner (→true) and
      /// attach_terminal (→false) only. NEVER mutated by the Destroyed
      /// handler (see §10 rule + NEW-3). snapshot_sessions reads this
      /// directly; DetachedSessionsState is NOT consulted at persist time.
      #[serde(default)]
      pub was_detached: bool,
  }
  ```
  And update the first sentence of §6.2 to name both structs. Belt-and-braces: §5.1 first row under "§A3.15 round-3 additions" should be referenced by §6.2 with a clear "see A3.2.1 for runtime-field placement" pointer.
- **Severity:** MEDIUM (catches at compile; not production bug) but HIGH friction in the implementation round.

### G3-L1 [LOW] Test §8.6.28 does not prove the NEW-3 asymmetry invariant on its own

- **Location:** Plan line 3287 (§8.6.28 proposed: "Detach A, quit main window via X (confirmation dialog → Quit), restart — A is restored as detached.").
- **The bug:** The test as written checks only the ROUND-TRIP outcome (detached before quit → detached after restart). It does NOT prove that the asymmetry — "Destroyed handler emits `terminal_attached` but does NOT mutate `Session::was_detached`" — is what's actually keeping the state correct. If a future refactor accidentally moves `mgr.set_was_detached(uuid, false).await` into the Destroyed handler, the round-trip test still passes in single-detach cases (because the PersistedSession file's pre-destroy snapshot may capture `was_detached=true` before the handler clears it, depending on persist timing), but breaks in the `A2.6 quit → N detached cleared in sequence` path silently.
- **Trigger:** A refactor that moves was_detached-clear into the Destroyed handler (plausible during a "simplification" pass).
- **Impact:** Silent regression of NEW-1. Hard to detect.
- **Proposed fix:** add §8.6.28b: "Detach A, B, C (three sessions). Quit via A3.7. IMMEDIATELY inspect `~/.agentscommander/sessions.json` — all three have `wasDetached: true` (proves Destroyed handler didn't clear the Session field; persist captured all three as detached). Restart — all three respawn. Destroy A via sidebar close, quit, restart — B + C still detached, A gone." The explicit sessions.json inspection step is the regression guard; without it, a future refactor can quietly defeat Fix A without any test failing.
- **Severity:** LOW (a defensive test, not a live bug).

### G3-L2 [LOW] `onCloseRequested` handler (A2.2.G4) and `onSessionDestroyed` handler (A2.3.G7) are specified separately; plan doesn't explicitly show both sharing the "register before loadActiveSession" contract

- **Location:** A2.2.G4 skeleton (line 1891) + A2.3.G7 skeleton (line 2020).
- **The bug:** A2.2.G4 says the onCloseRequested listener must appear "BEFORE `loadActiveSession` await to avoid the mount-race documented in G.7". A2.3.G7 shows the onSessionDestroyed listener registering first. Both are correct in isolation, but the plan doesn't integrate them into a single canonical TerminalApp.onMount skeleton showing BOTH listeners registered before loadActiveSession. An implementer might put one of them (say A2.2.G4) after the other or after loadActiveSession, reintroducing a mount-window race.
- **Trigger:** Implementer reads one section, misses the interaction, ships with onCloseRequested after loadActiveSession. During the short loadActiveSession window, if the detached window gets a close-requested (rapid user X-click on cold start), the default close proceeds — session stays alive, but the re-attach isn't triggered. User confusion.
- **Impact:** Race window is narrow (~50-100ms on cold start). Low probability; low blast radius. But the mitigation is explicit in the plan already — just not integrated.
- **Proposed fix:** in `src/terminal/App.tsx` §5.2 row, add: "In the combined order, register BOTH onSessionDestroyed (A2.3.G7) AND onCloseRequested (A2.2.G4) BEFORE any `await loadActiveSession`. Canonical order: registerShortcuts, onSessionDestroyed, onCloseRequested (only when props.detached), then initZoom/initWindowGeometry/settingsStore.load, then loadActiveSession, then remaining session-event listeners."
- **Severity:** LOW (implementer discipline item).

### G3-L3 [LOW] Phase-1→Phase-3 direct upgrade path can produce a stale "respawn surprise"

- **Location:** Cross-phase rollout, not specific to any line.
- **The bug:** If releases ship as separate versions (v0.8.0-phase1, v0.8.0-phase2, v0.8.0-phase3) and a user skips v0.8.0-phase2 (upgrades direct from Phase 1 to Phase 3 with the app still in the state where their last action was X-closing a detached window without quitting the app afterward):
  - Phase 1 X-close: Destroyed handler clears `DetachedSessionsState` but does NOT mutate `Session::was_detached` (NEW-3). In-memory `Session::was_detached` stays `true`.
  - User quits. `RunEvent::Exit` persist reads `Session::was_detached=true`. Disk: `wasDetached: true`.
  - User installs Phase 3 (skipping Phase 2). Phase 3 restore reads `wasDetached: true`, calls `detach_terminal_inner`. Session re-detaches on first launch of Phase 3.
  - User is confused: "I closed that detached window; why is it back?"
- **Trigger:** Cross-version upgrade skipping Phase 2; prior last-action was X-close of a detached window (not a full quit).
- **Impact:** One-time respawn surprise per affected user on their first Phase 3 launch. Self-corrects: user can re-attach (via Phase 3's attach gesture) and all subsequent launches are clean.
- **Proposed fix:** three options, pick one:
  - (a) Ship Phases 1+2+3 as a single release (bundled v0.8.0). Sidesteps the skip path entirely. **Easiest.**
  - (b) Phase 3 adds a "stale `was_detached` cleanup" migration: on Phase 3's first launch, log any `wasDetached: true` entries, spawn their detached windows (normal Phase 3 behavior), and silently accept the respawn. Document it as intended behavior.
  - (c) Move Phase 1's X-close behavior to set `was_detached=false` in Phase 1 (violates NEW-3 invariant for quit-path). Reject — breaks Fix A.
- **Severity:** LOW (affects only direct Phase-1→Phase-3 upgraders with specific mid-session state). Architecturally consistent with "persistence remembers detach state" — can defensibly be documented as intended.

---

### G3-APPROVED — What I tried to break and couldn't

Per the reviewer's honesty rules, listing what I attacked and confirmed the plan holds up.

**A. NEW-3 asymmetry under every destroy path (architect-anticipated probe #1).** Walked all five destroy paths:

| Destroy path | How Destroyed handler is invoked | was_detached state after | Correct? |
|---|---|---|---|
| User X-click (Phase 2+, onCloseRequested installed) | preventDefault + attach_terminal → attach sets was_detached=false → destroy() → Destroyed fires → handler does NOT mutate (already false) | false | ✓ |
| User X-click (Phase 1, no handler) | close() default proceeds → Destroyed fires → handler does NOT mutate | stays true | Pre-Phase-3: harmless (no restore logic). Post-Phase-3 upgrade: surfaces as G3-L3. |
| destroy_session_inner (Phase 2+, uses destroy()) | destroy() → Destroyed fires → handler does NOT mutate, but session is already removed from manager so set_was_detached would no-op anyway | session gone | ✓ |
| A3.7 quit loop | w.destroy() for each terminal-* → N Destroyed events → handler does NOT mutate was_detached; persist on `RunEvent::Exit` captures all true | true for each | ✓ NEW-1 properly fixed |
| OS Task Manager kill-9 / force-terminate | No Destroyed event, no RunEvent::Exit | last persisted state | Pre-existing best-effort |

**All five paths survive the asymmetry.** The only one that produces an unexpected state (Phase 1 X-close leaving `was_detached=true`) is benign intra-Phase-1 (no restore logic) and surfaces only on Phase-1→Phase-3 skip upgrade (G3-L3, LOW).

**B. Fix A + restore race (architect-anticipated probe #2).** Verified:
- `tokio::sync::RwLock::write()` does not poison on panic (Tokio docs confirm). No poison-recovery path needed.
- `set_was_detached` is a 4-line method with no panic paths: lock acquisition, HashMap get_mut, field assignment, guard drop.
- The "`create_session_inner` succeeds but `set_was_detached` fails" scenario is unreachable under normal conditions; the only failure mode is a runtime panic in the caller or runtime shutdown, both of which tear down the whole task and prevent the next persist from running anyway. Architect's mitigation argument holds.

**C. `quitInProgress` reset via `finally` (architect-anticipated probe #3).** Walked:
- JS is single-threaded; the `finally` block runs to completion before any awaiting `onCloseRequested` callback resumes. No race between `finally = false` and a second handler's `if (quitInProgress)` read.
- Sync throws inside the `try` block: `finally` runs. ✓
- Async rejections from `ask()`, `WebviewWindow.getAll()`, inner destroys: `finally` runs. ✓
- Memory/resource exhaustion: the runtime terminates the script entirely; `quitInProgress` irrelevant.
- Only real edge case: `ask()` returns a promise that never resolves (dialog bug, infinite spinner). `finally` never runs, `quitInProgress` stuck true, user can't close main. This is a Tauri plugin-dialog bug if it happens, not a plan bug. Accept.

**D. Migration EPSILON edge (architect-anticipated probe #4).** Walked:
- Production code path can only write `sidebar_zoom` via `applyZoom` → discrete 0.1-step values (ZOOM_STEP per `src/shared/zoom.ts:5`). All reachable values are 1.0 exactly at Ctrl+0, or 1.0 ± N × 0.1. Each of those is well outside `f64::EPSILON` (~2.22e-16) from default_zoom.
- Hand-edited settings.json with `sidebarZoom: 1.0` (exact): guard correctly skips migration (no-op case — user's effective zoom is default).
- Hand-edited with `sidebarZoom: 1.0000000000000001` (`1.0 + ε`): guard correctly skips (abs diff ≈ ε, which is NOT `> EPSILON` strictly). User loses an effectively-unity custom value → acceptable per architect's reasoning.
- Hand-edited with `sidebarZoom: 1.000001`: (1e-6 > ε) migration runs correctly.

**All four architect-anticipated probes pass.**

**E. Ordering bugs from Fix A reverting R.7 threading.** Fix A moves `was_detached` into Session. `snapshot_sessions` reads `s.was_detached` directly from the session list it already iterates. No new lock acquisition, no cross-structure coordination. The round-2 concern about "lock-across-await" becomes vacuous because no additional lock is threaded. Reverting R.7 is a net simplification — confirmed.

**F. `set_was_detached` lock discipline across all 3 call sites.** Two locks: `session_mgr` outer (tokio `Arc<RwLock<SessionManager>>`) and `self.sessions` inner (tokio `Arc<RwLock<HashMap<Uuid, Session>>>`). These are distinct RwLocks; no re-entrance. All three call sites (`detach_terminal_inner`, `attach_terminal`, restore loop) acquire outer read, then call `set_was_detached` which acquires inner write. No deadlock — outer/inner are different locks. No lock-across-emit (emit is sync and outside the guarded region). ✓

**G. A3.7 re-entrancy beyond double-X.** Walked:
- Alt+F4 routes through same `onCloseRequested` path as user X — covered by `quitInProgress`.
- Task Manager "End Task" / SIGKILL → bypasses onCloseRequested entirely. No persist. Pre-existing best-effort.
- OS shutdown (Windows WM_QUERYENDSESSION/WM_ENDSESSION): Tauri may or may not deliver these as `onCloseRequested`. If yes, `preventDefault` + `ask()` during OS shutdown is hostile to the shutdown UX (dialog hidden behind shutdown overlay), OS force-terminates after ~20s. But this is a pre-existing concern with any Tauri app that uses preventDefault; not specific to this plan.
- Ctrl+C in the launching terminal: goes to the parent process; Rust has no default SIGINT handler that bubbles through `onCloseRequested`. No bug.
- Programmatic `win.close()` from other code: none exist in the plan's new code. Phase 4 might add some (system tray plugin) — flagged for Phase 4 reviewer.

**H. Restore path NEW-2 ordering.** Restore does NOT call `attach_terminal`. Only `detach_terminal_inner`. NEW-2's invariant (`attach_terminal` clears was_detached BEFORE emitting terminal_attached) is therefore vacuous on the restore path. The restore path's own ordering (`create_session_inner` → `set_was_detached(ps.was_detached)` → `detach_terminal_inner`) is correct because `detach_terminal_inner` also sets was_detached=true idempotently. No ordering bug.

**I. DR2.2 residual race (dev-rust's optional tightening, architect declined).** Walked the three-step race window and confirmed the self-healing argument:
- The `WindowEvent::Destroyed` handler fires ASYNCHRONOUSLY on the main event loop. After `detach_terminal_inner` completes (returning Ok to the frontend), the event loop eventually processes any pending Destroyed events. The handler clears `DetachedSessionsState`. So a stale UUID inserted between recheck and insert is cleared on the subsequent event-loop turn.
- Fix A makes this even safer: even if a stale UUID DID remain in `DetachedSessionsState`, it has no persistence impact (was_detached is now authoritative from Session, not the set). Runtime impact is limited to `switch_session`'s focus-detached branch trying to focus a non-existent window — silent no-op.
- Architect's decline is correct. I cannot produce a user-visible exploit from this race.

**J. Plan drift across 8 sections.** Scanned all round-1/2/3 sections against each other. Found:
- G3-M2 (Session::was_detached not echoed in §6.2) — discoverability friction, not correctness.
- No other drift. The three phases' ship-bar definitions are internally consistent.

**K. Implementation non-determinism.** Checked:
- Ordering of event listeners at mount time: A2.3.G7 fixes the TerminalApp mount-race; G3-L2 notes the A2.2.G4 integration needs to be explicit but the handler patterns are correct.
- Ordering of `onMount` across Sidebar + Terminal in unified document: SolidJS guarantees depth-first children-before-parent; MainApp's `initZoom("main")` runs AFTER children's embedded-skip initZoom checks → no race.
- sessions.json file I/O: atomic write via tmp + rename (G.14 accepted round 2). Concurrent writes from persist_current_state callers serialize through the single file path; race-free.

---

### G3-CONSENSUS

- **Round-3 consensus: NO.** G3-B1 is a real Phase-1 shipping bug.
- **Escalation disposition (tech-lead's call):**
  - The fix is a **two-line plan-text delta** in A3.7's skeleton (swap one store lookup for a two-line API call). It does not touch architectural shape or any decision the architect made. If tech-lead treats this as a typo-level fold-in (same class as round-2's PB-4 `&info.id` fix), round 4 is not required.
  - If tech-lead wants strict adherence to the 3-round rule, G3-B1 escalates to user.
- My recommendation: **fold inline as a typo-level fix**. This plan has gone through 3 genuine rounds of adversarial scrutiny with 33 findings integrated; a Phase-sequencing oversight on one skeleton's detection mechanism doesn't justify blowing the rule.
- If G3-B1 is folded inline plus G3-M1 (ESC-key) gets a decision (my preference: custom modal in Phase 1), the plan is ready to implement.

### G3-SHIP-TO-IMPL signal

**(Assuming G3-B1 is folded inline as above.)** All architectural decisions are locked, Fix A is correct under every destroy path, dev-judgment items resolved, test plan covers the critical invariants. Implementer has a clean, complete runbook with no unresolved questions.

— dev-rust-grinch (round 3 final)

---

## Architect round 3-bis — mechanical fold

**Author:** architect
**Scope:** mechanical fold of the two grinch-proposed fixes (G3-B1 BLOCKER + G3-M1 MEDIUM) per tech-lead's round 3-bis directive. No architectural decisions; no re-litigation.
**Anchored against:** HEAD `60dd162`, plan state post-grinch-round-3.
**Verdict up front:** **2 accepted / 0 modified / 0 rejected.** Both folds are grinch's own proposed fixes. Zero cross-cutting issues surfaced during integration.

### A3B.0 Summary

- **G3-B1 (BLOCKER) — A3.7 Phase-sequencing mismatch:** Accepted. Replace `sessionsStore.detachedIds?.size ?? 0` with stateless `WebviewWindow.getAll().filter(w => w.label.startsWith("terminal-")).length`. Works identically across Phase 1, 2, 3 — no dependency on Phase-2 store state.
- **G3-M1 (MEDIUM) — ESC destructive-Quit via flipped `ask()`:** Accepted. Replace Tauri `ask()` dialog with a custom SolidJS modal `QuitConfirmModal`. Enter AND ESC both route to Cancel. Explicit Quit requires mouse click OR Tab-to-Quit + Enter. Promoted from Phase-4 polish to Phase-1 ship-bar per grinch + tech-lead agreement.
- **No new issues surfaced during the fold.**

### A3B.1 G3-B1 — Stateless detached-window count

**Delta:** in A3.7's `onCloseRequested` handler (plan line 3120), replace the store-based count with a stateless Tauri-window query.

**Before (broken in Phase 1):**
```tsx
const detachedCount = sessionsStore.detachedIds?.size ?? 0;
```

**After (stateless, Phase-1/2/3 correct):**
```tsx
const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
const allWebviews = await WebviewWindow.getAll();
const detachedCount = allWebviews.filter(w => w.label.startsWith("terminal-")).length;
```

**Why this is right** (grinch's own argument; confirmed during fold):
- `sessionsStore.detachedIds` is introduced in Phase 2 (§9 Phase 2 ship-bar). In Phase 1, it's `undefined`. `undefined?.size ?? 0` → `0` → handler returns early → main closes silently → detached windows orphan. G.13 bug reintroduced on every Phase-1 quit.
- `WebviewWindow.getAll()` returns the actual live windows — ground truth. `terminal-` prefix uniquely identifies detached windows (main has label `"main"`, guide has `"guide"`, filter excludes both).
- The destroy loop immediately below already imports and iterates `WebviewWindow.getAll()`. Reusing the import dedupes naturally (see unified A3.7 skeleton in §A3B.3).

**Section impact:**
- A3.7 skeleton updated (see §A3B.3 for the consolidated replacement).
- §A3.2.1 + any other section that references the `detachedCount = sessionsStore.detachedIds...` expression — updated to point at the A3B.3 unified skeleton.
- §10 — add rule: "Main-window close-confirmation count MUST be derived from `WebviewWindow.getAll()`, NEVER from `sessionsStore.detachedIds`. The store is Phase-2+ only; the API query is phase-independent."

**Phase landing:** Phase 1 (was already scheduled; the fix just replaces the broken expression).

### A3B.2 G3-M1 — Custom `QuitConfirmModal` component

**Delta:** replace the Tauri `ask()` call with a custom SolidJS modal rendered via `<Portal>` to `document.body`. Modal owns its own keyboard routing so Enter AND ESC both route to the safe (Cancel) action.

#### A3B.2.1 — Component location and file layout

- **File:** `src/main/components/QuitConfirmModal.tsx` (new file; creates the `src/main/components/` directory).
- **Styles:** inline styles are fine for this one-off, but for consistency with the rest of the codebase (which uses vanilla CSS + CSS variables per `CLAUDE.md`), add a rules block to `src/main/styles/main.css` (introduced in round 1). No new CSS file.

#### A3B.2.2 — Component contract

```tsx
// src/main/components/QuitConfirmModal.tsx
import { Component, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";

export interface QuitConfirmModalProps {
  /** Number of detached sessions that will be closed if the user confirms. */
  detachedCount: number;
  /** User cancelled — either clicked Cancel, pressed Enter, pressed ESC, or clicked the backdrop. */
  onCancel: () => void;
  /** User explicitly confirmed — clicked Quit button or Tab-focused Quit then pressed Enter. */
  onQuit: () => void;
}

const QuitConfirmModal: Component<QuitConfirmModalProps> = (props) => {
  let cancelBtnRef: HTMLButtonElement | undefined;
  let quitBtnRef: HTMLButtonElement | undefined;
  let previouslyFocused: HTMLElement | null = null;

  onMount(() => {
    // Remember focus owner so we can restore it on close.
    previouslyFocused = document.activeElement as HTMLElement | null;

    // Initial focus: Cancel (the safe button).
    cancelBtnRef?.focus();

    // Keyboard routing: Enter (on Cancel-focused) = Cancel; Enter (on Quit-focused) = Quit;
    // ESC = Cancel; Tab cycles between the two buttons (focus trap).
    const onKeyDown = (e: KeyboardEvent) => {
      // Only handle if event is inside the modal OR the document (modal captures globally).
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        props.onCancel();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        // Enter triggers whichever button is focused. Default focus is Cancel,
        // so Enter-mash = Cancel. User must Tab to Quit before Enter destroys.
        if (document.activeElement === quitBtnRef) {
          props.onQuit();
        } else {
          props.onCancel();
        }
        return;
      }
      if (e.key === "Tab") {
        // Focus trap: cycle between the two buttons.
        const focusables = [cancelBtnRef, quitBtnRef].filter(Boolean) as HTMLElement[];
        if (focusables.length < 2) return;
        const idx = focusables.indexOf(document.activeElement as HTMLElement);
        if (e.shiftKey) {
          if (idx <= 0) {
            e.preventDefault();
            focusables[focusables.length - 1].focus();
          }
        } else {
          if (idx === focusables.length - 1) {
            e.preventDefault();
            focusables[0].focus();
          }
        }
      }
    };

    document.addEventListener("keydown", onKeyDown, true); // capture phase → we see the event first
    onCleanup(() => {
      document.removeEventListener("keydown", onKeyDown, true);
      // Restore focus to the element that had it before modal opened.
      try { previouslyFocused?.focus(); } catch { /* best-effort */ }
    });
  });

  const onBackdropClick = (e: MouseEvent) => {
    // Click on backdrop (NOT on the modal body) = Cancel.
    if (e.target === e.currentTarget) {
      props.onCancel();
    }
  };

  return (
    <Portal>
      <div
        class="quit-confirm-backdrop"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="quit-confirm-title"
        aria-describedby="quit-confirm-body"
        onClick={onBackdropClick}
      >
        <div class="quit-confirm-modal">
          <h2 id="quit-confirm-title" class="quit-confirm-title">Quit AgentsCommander?</h2>
          <p id="quit-confirm-body" class="quit-confirm-body">
            You have {props.detachedCount} detached session{props.detachedCount === 1 ? "" : "s"} open.
            Quit the app and close all detached sessions?
          </p>
          <div class="quit-confirm-actions">
            <button
              ref={cancelBtnRef}
              class="quit-confirm-btn quit-confirm-btn-cancel"
              onClick={() => props.onCancel()}
              type="button"
            >
              Cancel
            </button>
            <button
              ref={quitBtnRef}
              class="quit-confirm-btn quit-confirm-btn-quit"
              onClick={() => props.onQuit()}
              type="button"
            >
              Quit
            </button>
          </div>
        </div>
      </div>
    </Portal>
  );
};

export default QuitConfirmModal;
```

#### A3B.2.3 — Keyboard routing spec (grinch's requirement)

| Key | Behavior | Why |
|---|---|---|
| **Enter** (default focus on Cancel) | Cancel | Mash-Enter = safe default |
| **Enter** (focus moved to Quit via Tab) | Quit | Explicit deliberate action |
| **ESC** | Cancel (always) | Standard modal-dismiss convention; safer than `ask()`'s ESC=cancelLabel |
| **Tab / Shift+Tab** | Cycle focus [Cancel ↔ Quit] | Focus trap — focus cannot escape the modal |
| **Click on Cancel** | Cancel | Standard |
| **Click on Quit** | Quit | Standard |
| **Click on backdrop** | Cancel | Standard modal-dismiss UX |
| **Click anywhere else** (outside modal, inside backdrop) | Cancel | Same as above — only the modal body is click-safe |

**Why `document.addEventListener(..., true)` (capture phase):** ensures the modal's handler runs BEFORE any other document-level keydown handlers (e.g. shortcuts.ts `registerShortcuts`, SessionItem context-menu dismissers). Without capture-phase, pressing ESC could trigger both Cancel AND some shortcuts-handler side-effect. `stopPropagation()` then halts further handlers from running.

#### A3B.2.4 — Accessibility contract

- `role="alertdialog"` on the backdrop — indicates destructive confirmation; assistive tech announces modally.
- `aria-modal="true"` — content behind modal is effectively hidden from AT.
- `aria-labelledby="quit-confirm-title"` — screen reader announces the heading as the dialog's name.
- `aria-describedby="quit-confirm-body"` — screen reader reads the body as the description after the name.
- Focus-trap — Tab/Shift+Tab cycle between Cancel and Quit; focus cannot exit the modal.
- Initial focus on Cancel — safe default.
- Return focus on close — restore to the element that had focus before the modal opened (usually `document.body`, but preserved correctly if a button was focused).

#### A3B.2.5 — CSS (append to `src/main/styles/main.css`)

```css
/* Quit-confirm modal (G3-M1) — overlays the main window when user attempts to
   close with ≥1 detached windows open. See _plans/feature-unified-window-with-detach.md §A3B. */
.quit-confirm-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 10000;
}
.quit-confirm-modal {
  background: var(--bg-modal, #14141a);
  color: var(--fg-primary, #e8e8e8);
  border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
  border-radius: 6px;
  min-width: 360px;
  max-width: 480px;
  padding: 24px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.6);
  font-family: "Geist", "Outfit", "General Sans", sans-serif;
}
.quit-confirm-title {
  margin: 0 0 12px 0;
  font-size: 16px;
  font-weight: 500;
}
.quit-confirm-body {
  margin: 0 0 20px 0;
  font-size: 13px;
  line-height: 1.5;
  color: var(--fg-secondary, rgba(255, 255, 255, 0.75));
}
.quit-confirm-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}
.quit-confirm-btn {
  padding: 6px 16px;
  border-radius: 4px;
  font-size: 13px;
  font-family: inherit;
  cursor: pointer;
  border: 1px solid transparent;
  transition: background 150ms ease-out, border-color 150ms ease-out;
}
.quit-confirm-btn-cancel {
  background: var(--btn-primary-bg, rgba(0, 212, 255, 0.2));
  color: var(--btn-primary-fg, #00d4ff);
  border-color: var(--btn-primary-border, rgba(0, 212, 255, 0.4));
}
.quit-confirm-btn-cancel:hover {
  background: rgba(0, 212, 255, 0.3);
}
.quit-confirm-btn-cancel:focus-visible {
  outline: 2px solid var(--focus-ring, #00d4ff);
  outline-offset: 2px;
}
.quit-confirm-btn-quit {
  background: transparent;
  color: var(--fg-secondary, rgba(255, 255, 255, 0.7));
  border-color: var(--border-subtle, rgba(255, 255, 255, 0.15));
}
.quit-confirm-btn-quit:hover {
  background: rgba(255, 59, 92, 0.15);
  color: #ff6b80;
  border-color: rgba(255, 59, 92, 0.4);
}
.quit-confirm-btn-quit:focus-visible {
  outline: 2px solid #ff6b80;
  outline-offset: 2px;
}

html.light-theme .quit-confirm-modal {
  background: #f7f7f9;
  color: #1a1a1e;
  border-color: rgba(0, 0, 0, 0.1);
}
html.light-theme .quit-confirm-body {
  color: rgba(0, 0, 0, 0.7);
}
```

Styling rationale (matches project conventions from `CLAUDE.md`):
- Industrial-dark aesthetic — dark backdrop with subtle cyan accent on the primary (Cancel) button.
- Cancel is visually primary (filled cyan), Quit is visually secondary (outlined, turns red on hover) — the primary button is the **safe** option.
- Animations 150ms ease-out (per project standard).
- Font-family uses project defaults (Geist / Outfit / General Sans).
- Light-theme override mirrors the existing `html.light-theme` pattern in other modal CSS.

### A3B.3 — Updated A3.7 skeleton (integrated)

This replaces the A3.7 skeleton at plan lines 3113-3151. **This is the authoritative Phase-1 `MainApp.onMount` skeleton for the main-window close-confirmation flow, superseding all prior versions.**

```tsx
// At the top of main/App.tsx module:
import QuitConfirmModal from "./components/QuitConfirmModal";

// Inside MainApp component:
const [quitModalCount, setQuitModalCount] = createSignal<number | null>(null);
let quitInProgress = false;

// Helper — stateless, Phase-1/2/3 correct (fixes G3-B1).
async function countDetachedWindows(): Promise<number> {
  const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const all = await WebviewWindow.getAll();
  return all.filter(w => w.label.startsWith("terminal-")).length;
}

// Inside onMount:
const win = getCurrentWindow();
const unlistenMainClose = await win.onCloseRequested(async (e) => {
  // Re-entry guard: ignore if a quit is already in flight OR the modal is already open.
  if (quitInProgress || quitModalCount() !== null) {
    e.preventDefault();
    return;
  }
  const detachedCount = await countDetachedWindows();
  if (detachedCount === 0) {
    // No detached windows → let the close proceed normally (Tauri will fire
    // RunEvent::Exit → persist → process exit).
    return;
  }
  e.preventDefault();
  setQuitModalCount(detachedCount); // opens the custom modal via <Show>
});
unlisteners.push(unlistenMainClose); // cleanup on MainApp unmount

const onModalCancel = () => {
  setQuitModalCount(null);
};

const onModalQuit = async () => {
  quitInProgress = true;
  try {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    for (const w of await WebviewWindow.getAll()) {
      if (w.label.startsWith("terminal-")) {
        try { await w.destroy(); }
        catch (err) { console.warn("[quit] destroy of", w.label, "failed:", err); }
      }
    }
    try { await win.destroy(); }
    catch (err) { console.warn("[quit] destroy of main failed:", err); }
  } finally {
    quitInProgress = false;
    setQuitModalCount(null);
  }
};

// Inside the JSX return of MainApp:
<>
  {/* ...existing main layout (titlebar + body + splitter + panes)... */}
  <Show when={quitModalCount() !== null}>
    <QuitConfirmModal
      detachedCount={quitModalCount()!}
      onCancel={onModalCancel}
      onQuit={onModalQuit}
    />
  </Show>
</>
```

**What changed vs round-3 A3.7:**
- G3-B1: `sessionsStore.detachedIds?.size` → stateless `countDetachedWindows()` helper (fix inline).
- G3-M1: `await ask(...)` → custom `QuitConfirmModal` opened via signal; `onQuit` / `onCancel` callbacks own the destroy loop or abort.
- Re-entry guard now checks BOTH `quitInProgress` (destroy in flight) AND `quitModalCount() !== null` (modal already open). Prevents dialog-stacking on double-X / Alt-F4-during-dialog.
- `quitInProgress` still wraps only the destroy phase (modal-open phase is guarded by `quitModalCount`).

### A3B.4 — §5.2 impact-map additions

Append to §5.2 / §A2.10 / §A3.15 frontend additions:

| File | Change type | Summary | Phase |
|---|---|---|---|
| `src/main/components/QuitConfirmModal.tsx` | **NEW** | Custom confirmation modal per §A3B.2.2. Replaces Tauri `ask()` in the main-X close flow. Enter + ESC both route to Cancel; Quit requires explicit click or Tab-then-Enter. | 1 |
| `src/main/styles/main.css` | modify | Append `.quit-confirm-*` CSS rules per §A3B.2.5 (dark + light-theme). | 1 |
| `src/main/App.tsx` | modify | Supersede the A3.7 skeleton with the A3B.3 integrated version (stateless count + custom modal). | 1 |

### A3B.5 — Test plan updates

Supersede the round-3 test cases §8.7.40 + §8.7 ask() tests with these custom-modal tests:

- **§8.7.40 (revised) — Close main with 2 detached, Enter-mash path:** Open 2 detached sessions. Click main X. Confirmation modal appears with "You have 2 detached sessions open..." headline + Cancel focused. Press Enter → modal closes, main stays open, detached sessions untouched. Proves: (a) Enter-default = Cancel (G3-M1); (b) main-X count uses stateless helper (G3-B1).

- **§8.7.41 — ESC path:** Same setup. Click main X. Modal opens. Press ESC → modal closes, main stays open. Proves ESC = Cancel (G3-M1 requirement that `ask()`'s ESC=Quit bug is fixed).

- **§8.7.42 — Explicit Quit via Tab path:** Same setup. Click main X. Modal opens with Cancel focused. Press Tab → Quit gets focus. Press Enter → all detached windows destroyed + main destroyed + app exits. On relaunch (Phase 3), both detached sessions come back (NEW-1 / Fix A preserved).

- **§8.7.43 — Explicit Quit via mouse click:** Same setup. Modal opens. Click Quit button → same outcome as §8.7.42.

- **§8.7.44 — Backdrop click:** Modal opens. Click outside the modal (on the semi-transparent backdrop) → Cancel. Main stays open.

- **§8.7.45 — Re-entrancy / double-X:** Modal opens. Click main X again (or Alt+F4) → onCloseRequested fires, re-entry guard (`quitModalCount() !== null`) kicks in, preventDefault called, no second modal opens. Click Cancel on the original modal → single modal closes cleanly.

- **§8.7.46 — Phase 1 with detached windows:** Phase 1 build (before `sessionsStore.detachedIds` exists). Detach a session, click main X. Modal correctly counts 1 detached window (proves G3-B1 fix — stateless helper works in Phase 1).

- **§8.7.47 — Focus-trap proof:** Modal opens. Press Tab repeatedly. Focus cycles Cancel → Quit → Cancel → Quit. Press Shift+Tab. Focus cycles Quit → Cancel → Quit → Cancel. Focus never leaves the modal.

- **§8.7.48 — Focus return on close:** Before clicking main X, focus an arbitrary element (e.g. the splitter via Tab). Click X → modal opens, focus moves to Cancel. Press Enter → modal closes, focus returns to the splitter (or the nearest valid focusable; best-effort per A3B.2.2).

Remove the old `ask()`-specific test case from round-3 A3.7 (the "press Enter → Cancel" via Tauri dialog) — superseded by §8.7.40 above.

### A3B.6 — §9 Phase 1 ship-bar updates

Append to the Phase 1 ship-bar in §9 / §A3.14:

- `QuitConfirmModal` component + CSS + A3B.3 `MainApp.onMount` integration.
- Stateless `countDetachedWindows()` helper.
- All §8.7.40-48 test cases pass.

**Phase 1 LoC delta from this fold:** +60 LoC for `QuitConfirmModal.tsx`, +50 LoC for `.quit-confirm-*` CSS, +5 LoC net in `MainApp.onMount` (replaces `ask()` call). Net: ~115 LoC added to Phase 1. Still inside the ~70% Phase-1 target from round 2.

### A3B.7 — Cross-cutting checks during fold

Per the round-1 rule ("if integrating a finding reveals a new bug, handle in-round"). I audited the A3B.3 integrated skeleton against every prior invariant:

1. **Re-entry guard interacts correctly with Phase 2's `onCloseRequested` in detached windows.** Detached windows have their own `onCloseRequested` (A2.2.G4 calls `attach_terminal`) with their own lexical-scope `attachInProgress` flag (hypothetically). No shared state between detached windows and main — each window's handler is scoped to its own module instance. Safe.

2. **`countDetachedWindows()` during app shutdown.** If the user mashes X during a slow shutdown (e.g. Windows shutting down the computer and Tauri's cleanup is in progress), `WebviewWindow.getAll()` might return a partially-torn-down list. The filter still correctly counts whatever's labelled `terminal-` at that moment. If the count is 0 (all detached already torn down), the handler lets main close. If the count is N>0, the modal opens — which during shutdown would race with Tauri's exit and probably be cut off. Acceptable: shutdown-during-confirm is an OS-level event and data loss is expected there regardless.

3. **`Show` unmount timing vs the keydown listener cleanup.** When `setQuitModalCount(null)` fires, SolidJS unmounts `<QuitConfirmModal/>`, which triggers its `onCleanup` (removes the document keydown listener + restores focus). This runs synchronously after the signal update. No leak, no stale listener. Verified the pattern matches SessionItem's Portal-based context menu (plan line 182-184) which does the same.

4. **`Portal` destination.** The modal renders into `document.body` (default Portal target). In unified main window, body is the window's root document. No z-index conflict with xterm or sidebar modals (modal's `z-index: 10000` is higher than any app content).

5. **Accessibility conflict with existing modals** (OpenAgentModal, OnboardingModal, SettingsModal, NewTeamModal, etc.). Those modals do NOT use `role="alertdialog"` — they're informational / interactive modals. `alertdialog` is reserved for destructive confirmations. No aria-role conflict; main X while another modal is open would stack modals (backdrop atop backdrop), which is acceptable UX for an alertdialog (OS-level quit confirmation takes precedence visually).

6. **CSS variable fallback values.** `var(--bg-modal, #14141a)` and similar fallbacks mean the modal renders correctly even if the project's global CSS variables aren't defined (e.g. during first-paint before theme CSS loads). Verified the fallback hex values match the project's dark aesthetic palette.

**No new bugs surfaced.** The fold is mechanical. G3-B1's fix is 2 lines; G3-M1's fix is a self-contained new component + CSS + integration point.

### A3B.8 — §10 addition

Add to the "What the dev must NOT do" list:

- Do NOT use Tauri's `ask()` / `message()` / `confirm()` for destructive confirmations in this app. Use a custom modal with explicit Enter/ESC routing so the Enter-default AND the ESC-default both point at the safe action. `ask()`'s API prevents both keys routing to the same button without label-flipping which makes ESC unsafe (see G3-M1).
- Do NOT count detached windows via `sessionsStore.detachedIds` in Phase 1. Use `WebviewWindow.getAll().filter(w => w.label.startsWith("terminal-"))` (phase-independent). The store is introduced in Phase 2 and is not authoritative at any point anyway — the Tauri window list IS the source of truth (see G3-B1).
- Do NOT change `quit-confirm-*` CSS variables to use only non-fallback `var(--foo)` references. The fallback hex values are load-bearing during first-paint before theme CSS is hydrated.

### A3B.9 — Final consensus signal

**YES.** Both items were grinch's own proposed fixes; no architectural re-litigation; zero new issues surfaced during the fold. Plan is ready for implementation.

- dev-rust verification pass expected to be quick (no Rust-side changes).
- No more rounds. No more grinch.
- Implementer has:
  - Complete A3B.3 `MainApp.onMount` skeleton.
  - Complete A3B.2.2 `QuitConfirmModal` component.
  - Complete A3B.2.5 CSS rules.
  - 9 new test cases (§8.7.40-48).
  - Clear §10 rules to avoid regressing.

— architect (round 3-bis)
