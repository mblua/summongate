# Plan: Coordinator Sort-by-Recent-Activity Toggle

**Issue:** https://github.com/mblua/AgentsCommander/issues/86
**Branch:** `feature/86-coord-sort-by-activity-toggle` (already cut, currently at HEAD `1bc94d8`)
**Status:** Ready for implementation.
**Anchored against HEAD:** `1bc94d8 docs(session): clarify case-insensitive token matching in should_inject_continue`. All line numbers in §3-§8 reference current files at that commit.

---

## 1. Overview

A new toggle in the sidebar `ActionBar` that, when enabled, sorts the **Coordinator Quick-Access** list so the coordinator with the most recent autonomous activity (busy → idle transition) appears at the top.

Three-layer change:

1. **Rust backend (persistence only):** add a `coord_sort_by_activity: bool` field on `AppSettings` (default `false`). No new Tauri command — the existing `get_settings` / `update_settings` round-trip carries the field via serde.
2. **Shared types:** mirror the new field on the TypeScript `AppSettings` interface; extend `SessionsState` with `coordSortByActivity`, `lastActivityBySessionId`, and `hydrated`.
3. **Sessions store:** add the new state fields, getters, and mutators. The toggle handler is **serialized via an in-flight signal** so rapid clicks cannot race the persistence round-trip. The `hydrated` flag prevents memory/disk divergence if the user clicks during the brief hydration window. Activity timestamps use `performance.now()` (monotonic) to survive system clock corrections.
4. **Frontend wiring + UI:**
   - `App.tsx` hydrates the toggle from settings on mount (which also flips `hydrated` to `true`) and adds a single `sessionsStore.markActivity(id)` line inside the existing `onSessionIdle` callback.
   - `ActionBar.tsx` gets a new 🔥 button to the LEFT of the eye button. The button is `disabled` until hydration completes AND no toggle is in flight.
   - `ProjectPanel.tsx` `coordinators()` memo sorts by activity timestamp descending when the flag is on.
   - `sidebar.css` adds three rules for the new button (mirroring `.show-categories-btn`).

Reactivity is fully derived: the sort memo reacts to `sessionsStore.coordSortByActivity`, `sessionsStore.lastActivityBySessionId`, and the existing reactive sessions array. No new Tauri events. No new IPC commands. No backend instrumentation — `IdleDetector` already emits `session_idle` (`src-tauri/src/pty/idle_detector.rs:118`, fires exactly once per busy→idle transition; see §6.3).

Implementation phases (so dev-rust can land Rust independently of dev-webpage-ui):

- **Phase A (Rust + shared types + sessions store):** §3 + §4 + §5. Self-contained; builds clean (`cargo check` + `npm run typecheck` both green) at every commit. The new `sessionsStore` fields and mutators are inert — they exist but nothing reads them yet because the UI is unchanged. This phase can ship and merge before any UI work.
- **Phase B (App.tsx wiring + UI):** §6 + §7 + §8 + §9. Activates the store fields by hydrating from settings, wiring the idle callback, mounting the toggle button, sorting the memo, and adding the CSS.

The phase split was widened (round 2) to put `sessions.ts` in Phase A. The earlier split kept `sessions.ts` in Phase B, but that broke the TypeScript build between Phase A merge and Phase B merge because §4.2 makes the new `SessionsState` fields required while the `createStore<SessionsState>` initializer would not yet have them. See §17.

---

## 2. Files to touch

| File | Phase | Purpose |
|---|---|---|
| `src-tauri/src/config/settings.rs` | A | Add `coord_sort_by_activity: bool` field with `#[serde(default)]`; initialize in `Default` impl |
| `src/shared/types.ts` | A | Add `coordSortByActivity: boolean` to `AppSettings`; add `coordSortByActivity`, `lastActivityBySessionId`, and `hydrated` to `SessionsState` |
| `src/sidebar/stores/sessions.ts` | A | Add state fields, getters (`coordSortByActivity`, `lastActivityBySessionId`, `hydrated`, `toggleInFlight`), and mutators (`setCoordSortByActivity`, `toggleCoordSortByActivity`, `markActivity`). Toggle handler is serialized via a module-level `toggleInFlight` signal. |
| `src/sidebar/App.tsx` | B | Hydrate the toggle from settings on mount; call `sessionsStore.markActivity(id)` in the existing `onSessionIdle` callback |
| `src/sidebar/components/ActionBar.tsx` | B | Insert the new 🔥 button as the first child of `.action-bar-icons`, before the eye button. Button is `disabled={!hydrated || toggleInFlight}`. |
| `src/sidebar/components/ProjectPanel.tsx` | B | Sort the `coordinators()` memo when `sessionsStore.coordSortByActivity === true` |
| `src/sidebar/styles/sidebar.css` | B | Add `.coord-sort-activity-btn` (base + hover + active) immediately after `.show-categories-btn.active` |

**Files NOT to touch:**

- `src-tauri/src/commands/config.rs` — no new Tauri command. The existing `get_settings` / `update_settings` carry the new field via serde.
- `src-tauri/src/pty/idle_detector.rs` — backend already emits `session_idle`; do NOT instrument.
- `src/shared/stores/settings.ts` — the toggle logic lives in `sessionsStore`; this store is only used to keep the cached `AppSettings` snapshot in sync (we call `settingsStore.refresh()` after persisting).
- Any other module.

---

## 3. Phase A — Rust backend

### 3.1 `src-tauri/src/config/settings.rs` — add the field

**Anchor:** the `AppSettings` struct definition currently spans lines 32–109. The `Default` impl spans lines 139–176.

#### 3.1.1 Insert the new field in the struct

Add the field at the end of `AppSettings`, **immediately after** `pub onboarding_dismissed: bool,` on line 108. The whole struct uses `#[serde(rename_all = "camelCase")]` (line 33), so `coord_sort_by_activity` becomes `coordSortByActivity` on the wire automatically.

```rust
    /// When true, sort the Coordinator Quick-Access list by most-recent-activity descending.
    /// Activity = busy→idle transition (IdleDetector emits session_idle).
    /// Per-session timestamps live in the frontend store and are NOT persisted.
    #[serde(default)]
    pub coord_sort_by_activity: bool,
```

Final struct tail looks like:

```rust
    /// Whether the user has dismissed the first-run onboarding wizard
    #[serde(default)]
    pub onboarding_dismissed: bool,
    /// When true, sort the Coordinator Quick-Access list by most-recent-activity descending.
    /// Activity = busy→idle transition (IdleDetector emits session_idle).
    /// Per-session timestamps live in the frontend store and are NOT persisted.
    #[serde(default)]
    pub coord_sort_by_activity: bool,
}
```

The `#[serde(default)]` attribute is **mandatory** so that older `settings.json` files (written by versions that did not have this field) deserialize to `false`. Without it, deserialization would error on missing keys.

#### 3.1.2 Initialize the field in the `Default` impl

**Anchor:** lines 147–174 (the `Self { ... }` block inside `impl Default for AppSettings`).

Add the line **immediately after** `onboarding_dismissed: false,` (current line 173, which is the last field assignment before the closing `}`):

```rust
            onboarding_dismissed: false,
            coord_sort_by_activity: false,
```

That is the only change to the `Default` impl. Field order inside the struct literal must match the declaration order to keep the diff minimal.

### 3.2 `src-tauri/src/commands/config.rs` — no edits

Re-read by the architect at HEAD `1bc94d8`:

- `get_settings` (lines 23–29) returns the full `AppSettings` minus `root_token`. Serde adds the new camelCase field automatically.
- `update_settings` (lines 31–44) accepts a full `AppSettings`, runs `validate_agent_commands` (which only inspects `agents`), and writes the file. The new field flows through unchanged.

No setter command, no validation hook, no changes here. Confirmed.

### 3.3 Existing Rust tests — no edits

`settings.rs` tests (lines 365–447) exercise `validate_agent_commands` only. They build `AppSettings` via `AppSettings::default()` which now includes `coord_sort_by_activity: false`. No assertion needs updating.

### 3.4 New Rust test (optional, recommended)

If dev-rust wants belt-and-suspenders coverage, add this round-trip test inside the existing `mod tests` block (after line 446, before the closing `}` of the module):

```rust
    #[test]
    fn coord_sort_by_activity_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert_eq!(s.coord_sort_by_activity, false);
        s.coord_sort_by_activity = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"coordSortByActivity\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.coord_sort_by_activity);
    }

    #[test]
    fn coord_sort_by_activity_defaults_when_missing_from_json() {
        // Old settings.json without the new field must deserialize to false.
        let json = r#"{
            "defaultShell": "bash",
            "defaultShellArgs": [],
            "agents": [],
            "telegramBots": [],
            "startOnlyCoordinators": true,
            "sidebarAlwaysOnTop": false,
            "raiseTerminalOnClick": true,
            "voiceToTextEnabled": false,
            "geminiApiKey": "",
            "geminiModel": "gemini-2.5-flash",
            "voiceAutoExecute": true,
            "voiceAutoExecuteDelay": 15,
            "sidebarZoom": 1.0,
            "terminalZoom": 1.0,
            "guideZoom": 1.0,
            "darkfactoryZoom": 1.0,
            "sidebarGeometry": null,
            "terminalGeometry": null,
            "webServerEnabled": false,
            "webServerPort": 7777,
            "webServerBind": "127.0.0.1",
            "projectPath": null,
            "projectPaths": [],
            "sidebarStyle": "noir-minimal",
            "onboardingDismissed": false
        }"#;
        let s: AppSettings = serde_json::from_str(json).expect("deserialize old json");
        assert_eq!(s.coord_sort_by_activity, false);
    }
```

Both tests are optional. They lock the contract that lights up if a future contributor accidentally drops `#[serde(default)]` or renames the field. If included, no other Rust file changes.

---

## 4. Phase A — TypeScript types

### 4.1 `src/shared/types.ts` — extend `AppSettings`

**Anchor:** lines 123–148 (the `AppSettings` interface).

Add the new property **immediately after** `onboardingDismissed: boolean;` on line 147:

```ts
export interface AppSettings {
  defaultShell: string;
  defaultShellArgs: string[];
  // ... unchanged ...
  onboardingDismissed: boolean;
  coordSortByActivity: boolean;
}
```

### 4.2 `src/shared/types.ts` — extend `SessionsState`

**Anchor:** lines 173–182 (the `SessionsState` interface).

Add three fields **immediately after** `repos: RepoMatch[];` on line 181:

```ts
export interface SessionsState {
  sessions: Session[];
  activeId: string | null;
  teams: Team[];
  teamFilter: string | null;
  showInactive: boolean;
  showCategories: boolean;
  repos: RepoMatch[];
  coordSortByActivity: boolean;
  lastActivityBySessionId: Record<string, number>;
  /** Becomes true after App.tsx finishes hydrating settings from disk. The toggle
   * button stays disabled until this flips, so a click during the hydration
   * window cannot race the persisted-value override and cause memory/disk
   * divergence. See §10 "User clicks during hydration window". */
  hydrated: boolean;
}
```

All three fields are required because `createStore<SessionsState>` in `sessions.ts` enforces the interface at compile time. Phase A includes both the interface change AND the matching `createStore` initializer (§5.1) — they cannot be split across phases.

### 4.3 No other type files

`src/shared/ipc.ts` declares `SettingsAPI.update(settings: AppSettings)` — once the interface above is updated, the existing call sites are type-safe automatically. No edits.

---

## 5. Phase A — Sessions store

(Moved into Phase A in round 2 — see §17 H1. The `createStore<SessionsState>` initializer must stay in lock-step with the `SessionsState` interface in §4.2 to keep the build green at every commit.)

### 5.1 `src/sidebar/stores/sessions.ts` — add state fields

**Anchor:** lines 7–15 (the `createStore<SessionsState>({...})` call).

Current:

```ts
const [state, setState] = createStore<SessionsState>({
  sessions: [],
  activeId: null,
  teams: [],
  teamFilter: null,
  showInactive: false,
  showCategories: true,
  repos: [],
});
```

Change to:

```ts
const [state, setState] = createStore<SessionsState>({
  sessions: [],
  activeId: null,
  teams: [],
  teamFilter: null,
  showInactive: false,
  showCategories: true,
  repos: [],
  coordSortByActivity: false,
  lastActivityBySessionId: {},
  hydrated: false,
});
```

### 5.2 Add a top-of-file import for `SettingsAPI`

**Anchor:** lines 1–4 (existing imports).

Current:

```ts
import { createMemo, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { NO_TEAM } from "../../shared/constants";
import type { RepoMatch, Session, SessionRepo, SessionsState, Team, TeamSessionGroup } from "../../shared/types";
import { projectStore } from "./project";
```

Add **immediately after** the `projectStore` import, on a new line 6:

```ts
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
```

Both imports are needed:
- `SettingsAPI` to call `get`/`update` from inside `toggleCoordSortByActivity`.
- `settingsStore` to call `refresh()` after a successful save so the cached `AppSettings` snapshot stays in sync (downstream readers of `settingsStore.current` see the new value).

### 5.3 Add getters and mutators

**Anchor:** the exported `sessionsStore` object spans lines 222–346. The new members go **immediately after** `toggleShowCategories()` on line 335 (which currently ends `},` before `toggleTeamCollapsed`).

#### 5.3.1 Module-level `toggleInFlight` signal

`toggleInFlight` is **transient runtime UI state**, not domain state — it doesn't belong in `SessionsState`. Use a module-level `createSignal` (already imported at line 1 of `sessions.ts`). Add it **immediately above** the `const [state, setState] = createStore<SessionsState>(...)` block on line 7 (i.e., insert as a new top-level statement just inside the module, after the imports updated in §5.2):

```ts
/** True while a toggleCoordSortByActivity() call is in flight. The UI button
 * uses this (via the reactive getter below) to disable itself, which prevents
 * rapid clicks from racing concurrent update_settings round-trips. See §17 H2. */
const [toggleInFlight, setToggleInFlight] = createSignal(false);
```

#### 5.3.2 Getters and mutators inside `sessionsStore`

Insert the following block between `toggleShowCategories() { ... },` and `toggleTeamCollapsed(teamId: string) { ... },`:

```ts
  get coordSortByActivity() {
    return state.coordSortByActivity;
  },
  get lastActivityBySessionId() {
    return state.lastActivityBySessionId;
  },
  /** True after App.tsx finishes calling setCoordSortByActivity(persistedValue) on mount.
   * The toggle button stays disabled until this is true so a click during
   * hydration cannot race the persisted-value override. See §17 M1. */
  get hydrated() {
    return state.hydrated;
  },
  /** True while a persistence round-trip is in flight. Bound to the button's
   * `disabled` attribute to serialize rapid clicks. See §17 H2. */
  get toggleInFlight() {
    return toggleInFlight();
  },

  /**
   * Hydrate the toggle from persisted settings (called once on app start from
   * App.tsx). Also flips `hydrated` to true so the button becomes clickable.
   * The two state writes are folded into one mutator so the implementing dev
   * cannot accidentally enable the button before the persisted value is
   * applied (which would re-introduce the M1 race).
   */
  setCoordSortByActivity(value: boolean) {
    setState("coordSortByActivity", value);
    setState("hydrated", true);
  },

  /**
   * Flip the toggle and persist it via update_settings. Serialized via
   * `toggleInFlight` so concurrent clicks cannot race the persistence
   * round-trip — early-returns on the second click until the first completes
   * (round-trip latency ~10-50ms, well below human re-click threshold).
   * Optimistically updates the in-memory flag first so the UI is responsive;
   * reverts on error.
   */
  async toggleCoordSortByActivity() {
    if (toggleInFlight()) return;
    setToggleInFlight(true);
    const next = !state.coordSortByActivity;
    setState("coordSortByActivity", next);
    try {
      const current = await SettingsAPI.get();
      await SettingsAPI.update({ ...current, coordSortByActivity: next });
      void settingsStore.refresh();
    } catch (e) {
      console.error("[coord-sort] Failed to persist coordSortByActivity:", e);
      setState("coordSortByActivity", !next);
    } finally {
      setToggleInFlight(false);
    }
  },

  /**
   * Record that this session just transitioned busy→idle. Uses
   * `performance.now()` (monotonic, time-origin = page load) instead of
   * `Date.now()` so NTP corrections, DST changes, manual clock adjustments,
   * and VM clock drift cannot reorder timestamps backwards. The map is
   * in-memory only (resets on app restart), so absolute time is irrelevant —
   * only ordering matters, which is exactly what `performance.now()` provides.
   * See §17 M2.
   *
   * Replaces the whole map reference (rather than
   * `setState("lastActivityBySessionId", id, ts)`) so the SolidJS proxy emits
   * a top-level change — this is what guarantees the coordinators() memo
   * re-runs even when adding a new key for the first time. Trade-off: O(N)
   * per call with N = number of session keys in the map, fine at realistic
   * sizes (<= ~50).
   */
  markActivity(sessionId: string) {
    setState("lastActivityBySessionId", (prev) => ({ ...prev, [sessionId]: performance.now() }));
  },
```

The order matters: declare `toggleInFlight` **before** the `createStore` call (§5.3.1 lands above §5.1's edit). The getters reference it; if the signal is declared after the store, JS hoisting works for `const` only inside the same module top-level declarations, but co-locating `const` signals adjacent to the store keeps the file readable.

### 5.4 Why `markActivity` replaces the map vs. path-setting it

SolidJS `createStore` proxy tracking is fine-grained per-key for known keys, but reads of *currently-undefined* keys may not establish a tracking dependency in every version. Replacing the whole `lastActivityBySessionId` reference (`(prev) => ({ ...prev, [id]: now })`) emits a change at the top level, so any reader — including `const map = sessionsStore.lastActivityBySessionId;` then `map[id] ?? 0` — re-runs reliably whether the key existed before or not. This is the same defensive pattern the existing `setSessionWaiting` would need if it tracked an unknown id.

The cost is one shallow object spread per idle event. The IdleDetector only fires `session_idle` once per real busy→idle transition (§6.3), so the rate is at most a few events per second across the whole workspace — negligible.

---

## 6. Phase B — App.tsx wiring

### 6.1 Hydrate the toggle from settings

**Anchor:** lines 69–70 (the `appSettings = await SettingsAPI.get()` call followed by `raiseTerminalEnabled = appSettings.raiseTerminalOnClick;`).

Current:

```ts
    const appSettings = await SettingsAPI.get();
    raiseTerminalEnabled = appSettings.raiseTerminalOnClick;
    // Apply sidebar style from settings (remap removed themes to default)
```

**Insert one line** between `raiseTerminalEnabled = ...;` and the `// Apply sidebar style` comment (i.e., between current lines 70 and 71):

```ts
    const appSettings = await SettingsAPI.get();
    raiseTerminalEnabled = appSettings.raiseTerminalOnClick;
    sessionsStore.setCoordSortByActivity(appSettings.coordSortByActivity ?? false);
    // Apply sidebar style from settings (remap removed themes to default)
```

The `?? false` guards the (impossible-after-Phase-A but cheap) case where the field is `undefined` — e.g., if a stale dev build of the backend serves an older settings struct during the brief window between Phase A merge and the next backend rebuild.

`setCoordSortByActivity` (defined in §5.3.2) **also flips `hydrated` to `true`** as part of its own implementation. That is what makes the toggle button transition from `disabled` to clickable. Do NOT call any separate `setHydrated`-style mutator from `App.tsx` — there isn't one, and the fold-in is intentional (a single mutator that always co-changes both fields prevents reintroducing the M1 race).

### 6.2 Wire `markActivity` into the existing idle callback

**Anchor:** lines 148–152.

Current:

```ts
    unlisteners.push(
      await onSessionIdle(({ id }) => {
        sessionsStore.setSessionWaiting(id, true);
      })
    );
```

**Add one line** inside the callback, **before** the existing `setSessionWaiting` call (call order: mark first, then update waiting state, so any reactive memo that fires from the waiting-state change can already see the fresh activity timestamp):

```ts
    unlisteners.push(
      await onSessionIdle(({ id }) => {
        sessionsStore.markActivity(id);
        sessionsStore.setSessionWaiting(id, true);
      })
    );
```

### 6.3 Why a single unconditional `markActivity` line is safe (idle event semantics)

Confirmed by reading `src-tauri/src/pty/idle_detector.rs:84–123`:

- The watcher thread polls every `CHECK_INTERVAL = 500ms`.
- `on_idle` fires when `elapsed > IDLE_THRESHOLD (2500ms)` AND the session is **not already in `idle_set`** (line 107–109).
- Once fired, the session is inserted into `idle_set`. Subsequent ticks see `idle_set.contains(&session_id) == true` and skip the callback.
- The session is removed from `idle_set` only when new PTY activity arrives (via `record_activity_with_bytes`, line 63), and `on_busy` fires.

Result: `session_idle` is emitted **exactly once per busy→idle transition**. Re-detection within the same idle period does NOT re-fire. So `markActivity(id)` is called once per real activity-end moment — the timestamp it records is meaningful, not noise.

The pre-existing `setSessionWaiting`'s `wasAlreadyWaiting` defensive check (`sessions.ts:288–296`) protects against a hypothetical re-fire, but in practice that branch never executes for the idle event. We do not need to replicate that guard inside `markActivity`.

### 6.4 No change to `onSessionBusy` callback

The toggle's spec is "moment activity ended" = idle transition. Busy events do NOT update the timestamp. Leave `onSessionBusy` (lines 154–158) untouched.

### 6.5 No change to `onSessionDestroyed` callback

When a session is destroyed, its entry in `lastActivityBySessionId` becomes stale (the session id no longer matches any live session, so the lookup chain in §8.1 returns `undefined` for `replicaSession`, which yields a `0` timestamp). This is correct: a destroyed coord falls to the bottom of the sorted list, which matches user expectation. We do NOT need to delete from the map. Leaving it leaks a few bytes of memory per destroyed session over the app lifetime — negligible (sessions are destroyed at human pace, app restarts on a longer cadence).

If a future PR wants to garbage-collect, the natural place is inside `removeSession`. Out of scope for this plan.

---

## 7. Phase B — ActionBar.tsx (the toggle button)

### 7.1 Insert the button before the eye button

**Anchor:** lines 126–133 (the `<div class="action-bar-icons">` opening tag and its first child, the show-categories eye button).

Current:

```tsx
        <div class="action-bar-icons">
          <button
            class={`toolbar-gear-btn show-categories-btn ${sessionsStore.showCategories ? "active" : ""}`}
            onClick={() => sessionsStore.toggleShowCategories()}
            title={sessionsStore.showCategories ? "Hide category sections" : "Show category sections"}
          >
            &#x1F441;
          </button>
```

**Insert the new button as the first child** of `.action-bar-icons`, immediately after `<div class="action-bar-icons">` (line 126) and BEFORE the existing show-categories `<button>` block (current line 127):

```tsx
        <div class="action-bar-icons">
          <button
            class={`toolbar-gear-btn coord-sort-activity-btn ${sessionsStore.coordSortByActivity ? "active" : ""}`}
            disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}
            onClick={() => sessionsStore.toggleCoordSortByActivity()}
            title={sessionsStore.coordSortByActivity ? "Show recent coordinators first" : "Show coordinators in default order"}
          >
            &#x1F525;
          </button>
          <button
            class={`toolbar-gear-btn show-categories-btn ${sessionsStore.showCategories ? "active" : ""}`}
            ...
```

Resulting left-to-right order inside `.action-bar-icons`: `[🔥 NEW] [👁] [💡] [☀/🌙] [⚙]`. Matches the intake spec verbatim.

**Why two-condition `disabled`:**

- `!sessionsStore.hydrated` — true only during the brief sub-100ms window between first JSX render and `App.tsx:71`'s `setCoordSortByActivity(...)` call. Prevents the M1 race (a click during this window racing the persisted-value override). Once hydration completes, this flips to `false` and never flips back.
- `sessionsStore.toggleInFlight` — true while a persistence round-trip is mid-flight (~10-50ms per click). Prevents the H2 race (rapid clicks where `update_settings` calls reorder relative to clicks).

Both conditions are reactive getters off `sessionsStore`. SolidJS automatically re-renders the button when either flips. No additional wiring needed.

`disabled` on a Solid `<button>` is set as a real DOM property (not just a class). Browsers natively suppress click events on disabled buttons, so we don't need a guard inside the click handler — but the `if (toggleInFlight()) return;` guard inside `toggleCoordSortByActivity` is still kept as defense-in-depth (e.g., if a future contributor invokes the method programmatically rather than via the button).

### 7.2 Imports

`sessionsStore` is already imported at `ActionBar.tsx:4`. No new imports needed.

### 7.3 Notes on the tooltip

The intake specifies:
- when OFF: `"Show coordinators in default order"` (describes what the button will do *if pressed*)
- when ON:  `"Show recent coordinators first"` (describes the current state — what the user is currently seeing)

The two strings express different things on purpose (one is a future action, one is the current state). I matched the spec verbatim. If dev-rust-grinch flags this asymmetry, the answer is "spec says so".

---

## 8. Phase B — Sort the `coordinators()` memo

### 8.1 `src/sidebar/components/ProjectPanel.tsx:647–657` — extend the memo

**Anchor:** lines 646–657. The memo currently builds the result by iterating workgroups and replicas.

Current:

```tsx
                {(() => {
                  const coordinators = createMemo(() => {
                    const result: { replica: AcAgentReplica; wg: AcWorkgroup }[] = [];
                    for (const wg of proj.workgroups) {
                      for (const replica of wg.agents) {
                        if (replica.isCoordinator) {
                          result.push({ replica, wg });
                        }
                      }
                    }
                    return result;
                  });
```

**Change to:**

```tsx
                {(() => {
                  const coordinators = createMemo(() => {
                    const result: { replica: AcAgentReplica; wg: AcWorkgroup }[] = [];
                    for (const wg of proj.workgroups) {
                      for (const replica of wg.agents) {
                        if (replica.isCoordinator) {
                          result.push({ replica, wg });
                        }
                      }
                    }
                    if (sessionsStore.coordSortByActivity) {
                      const activityMap = sessionsStore.lastActivityBySessionId;
                      const tsFor = (item: { replica: AcAgentReplica; wg: AcWorkgroup }): number => {
                        const session = replicaSession(item.wg, item.replica);
                        if (!session) return 0;
                        return activityMap[session.id] ?? 0;
                      };
                      result.sort((a, b) => tsFor(b) - tsFor(a));
                    }
                    return result;
                  });
```

### 8.2 Why this lookup chain (replica → session → timestamp) and not a path-based match

The intake mentioned the option of matching `replica.path ↔ session.workingDirectory` via `normalizePath`. I rejected that for two reasons:

1. **`normalizePath` is currently a private helper inside `sessions.ts:17`.** Path-based matching would require either exporting it (export-only-for-this scope creep) or duplicating the normalization rule (regression risk if either copy is updated without the other).
2. **`replicaSession(wg, replica)` already exists at `ProjectPanel.tsx:54`** — it's the exact same lookup the rest of `ProjectPanel.tsx` uses (e.g., for status dot color, badge rendering). Reusing it keeps the sort consistent with how every other replica→session mapping is computed in this file.

`replicaSession` calls `sessionsStore.findSessionByName(replicaSessionName(wg, replica))` (sessions.ts:341–343), which does a reactive `find` over `state.sessions`. That makes the sort memo automatically re-run when sessions are created/destroyed/renamed.

### 8.3 Reactivity guarantees

The `coordinators()` memo reads, in order:
1. `proj.workgroups` and `wg.agents` — reactive via `projectStore` (already true today).
2. `sessionsStore.coordSortByActivity` — reactive (added in §5.1).
3. `sessionsStore.lastActivityBySessionId` (when the flag is on) — top-level reference reactive (replaced wholesale by `markActivity`, see §5.4).
4. `replicaSession(wg, replica)` → `state.sessions.find(...)` — reactive.

Any of:
- toggling the flag,
- a new idle event recording activity,
- a session being created/destroyed/renamed,
- a workgroup or replica being added/removed,

will trigger a memo recompute and the sidebar re-renders. No manual invalidation needed.

### 8.4 "No timestamp yet" behavior — confirmed

Per the intake's behavior-detail-to-think-through: a coordinator with no entry in `lastActivityBySessionId` (e.g., right after app restart, or has never idled) **sorts to the bottom** alongside other zero-timestamp coordinators in their default insertion order.

The lookup chain at sort time:

| State | `replicaSession` returns | `activityMap[session.id]` | `tsFor` returns |
|---|---|---|---|
| Coord has no session yet (never launched) | `undefined` | n/a | `0` |
| Coord has session, has never idled this run | session | `undefined` | `0` |
| Coord has session, idled at time T | session | `T` | `T` |
| Coord's session was destroyed (live session gone) | `undefined` | n/a | `0` |

`Array.prototype.sort` is stable since ES2019. Coordinators that all return `0` keep their default insertion order (project iteration order × workgroup iteration order × replica order). They cluster together at the bottom; among themselves, the visual ordering is identical to the toggle-OFF view.

When a zero-timestamp coordinator generates its first idle event, its timestamp jumps from `0` to `performance.now()` and it bubbles up immediately on the next reactive tick. This matches the user's expectation per the intake.

### 8.5 Imports

`sessionsStore` is already imported at `ProjectPanel.tsx:8`. `createMemo` is at `ProjectPanel.tsx:1`. `replicaSession` is a module-level helper at `ProjectPanel.tsx:54`. **No new imports needed.**

### 8.6 Do NOT change other lists

The sort applies **only** to the Coordinator Quick-Access section. Specifically do NOT touch:
- The Workgroups list at `ProjectPanel.tsx:700–730` — coordinators inside their workgroup list keep workgroup order.
- The Agents list at `ProjectPanel.tsx:802–840`.
- The Teams list at `ProjectPanel.tsx:973–1024`.
- `SessionItem.tsx` — irrelevant here.
- `filteredSessionsMemo` and `groupedSessionsMemo` in `sessions.ts:71–220`.

---

## 9. Phase B — CSS

### 9.1 Add the new button rules

**Anchor:** lines 514–525 (the `.show-categories-btn` block).

Current:

```css
/* Show inactive toggle */
.show-categories-btn {
  font-size: 14px;
  opacity: 0.4;
  transition: opacity 150ms ease-out;
}
.show-categories-btn:hover {
  opacity: 0.7;
}
.show-categories-btn.active {
  opacity: 1;
}
```

**Insert immediately after** `.show-categories-btn.active { opacity: 1; }` (line 525), **before** the blank line that precedes `/* Close button on session item */`:

```css
/* Coord sort-by-activity toggle */
.coord-sort-activity-btn {
  font-size: 14px;
  opacity: 0.4;
  transition: opacity 150ms ease-out;
}
.coord-sort-activity-btn:hover {
  opacity: 0.7;
}
.coord-sort-activity-btn.active {
  opacity: 1;
}
```

### 9.2 Why a separate class instead of reusing `.show-categories-btn`

Two reasons:
1. **Semantic clarity.** Reading the JSX, `class="toolbar-gear-btn show-categories-btn"` on a sort-by-activity button is misleading — anyone debugging will grep for the wrong thing.
2. **Future-proofing isolation.** If someone later changes `.show-categories-btn` (say, dim opacity on a specific theme), the new button shouldn't silently inherit that. A separate class keeps the two visually-identical-today buttons independently tuneable.

The CSS duplication is three rules totaling ~10 lines. Acceptable per the architect's "minimal blast radius" principle.

### 9.3 No theme-specific overrides

Confirmed at `sidebar.css` HEAD `1bc94d8`: `.show-categories-btn` is NOT overridden in any of the per-sidebar-style blocks (`noir-minimal`, `card-sections`, `command-center`, `deep-space`, `arctic-ops`, `obsidian-mesh`, `neon-circuit`). The new `.coord-sort-activity-btn` will render consistently across all styles for the same reason. No overrides needed.

---

## 10. Edge cases & invariants

| Case | Behavior |
|---|---|
| App start with a fresh settings.json (no `coordSortByActivity` key) | Rust serde default → `false`. Frontend reads `appSettings.coordSortByActivity ?? false`. Toggle starts off. Button is disabled for sub-100ms until hydration completes. ✓ |
| App start with `coordSortByActivity: true` in settings.json | Rust deserializes to `true`. `App.tsx:71` calls `setCoordSortByActivity(true)`, which also flips `hydrated` to `true`. Sort applies immediately on first render after hydration. `lastActivityBySessionId` starts empty so all coords have ts=0 → they render in default order until the first idle fires. ✓ |
| User toggles ON, then OFF, then closes app | Each toggle persists to settings.json (serialized via `toggleInFlight` so the second click cannot start until the first round-trip completes). On next start, the saved value (OFF) is read. ✓ |
| Two coords with identical timestamps (e.g., both idled within the same tick) | `tsFor(b) - tsFor(a) === 0`; stable sort preserves their relative insertion order. `performance.now()` quantization is typically 100µs or finer in modern Wry, so true ties are rare and only occur for genuinely simultaneous events — not for clock anomalies. ✓ |
| Coord A idles, then user destroys Coord A's session | `lastActivityBySessionId[A.id]` remains. `replicaSession(wg, A)` now returns `undefined`. `tsFor` returns `0`. Coord A drops to the bottom. Stale map entry leaks until app restart — out of scope to clean up. ✓ |
| Coord A's session is restarted (new session id) | New session id has no entry yet → `tsFor = 0`. Coord A drops to the bottom until the new session idles for the first time. ✓ |
| Toggle ON, update_settings call fails (e.g., disk full) | The async toggle catches the error and reverts the in-memory flag. UI snaps back to OFF. `console.error` logs the failure. The `finally` block resets `toggleInFlight` so the button is re-enabled and the user can retry. ✓ |
| User toggles rapidly (clicks 5 times in 200ms) | First click sets `toggleInFlight = true` → button is disabled at the DOM level; subsequent clicks within the round-trip window (~10-50ms) are suppressed by the browser AND by the early `if (toggleInFlight()) return;` guard. After the first round-trip completes, the button re-enables and the user's next click is processed. The persisted state always matches the user's last accepted click. ✓ |
| User clicks the toggle during the hydration window (between first JSX render and `setCoordSortByActivity(persisted)`) | Button has `disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}`. During hydration, `hydrated === false` → button is disabled at the DOM level → click is suppressed by the browser before the handler runs. No memory/disk divergence possible. Once hydration completes (sub-100ms), the button enables and clicks work normally. ✓ |
| Multiple sidebar windows open (web remote + Tauri sidebar) | Each window has its own `sessionsStore`. Toggling in one window updates settings.json but the other window's in-memory `coordSortByActivity` does NOT auto-update (no event broadcast for this field). The other window may also race against the first via TOCTOU (`get_settings` then `update_settings` against a disk that another window already updated) — same pre-existing limitation as any other settings field. Acceptable for v1. Out of scope. |
| `IdleDetector` re-emits `session_idle` due to a future bug | `markActivity` overwrites the timestamp with a new `performance.now()`. UX impact: the coord re-bubbles to the top. No correctness issue. ✓ |
| User is on `card-sections` sidebar style | Same as `feature-coord-running-peers-badges.md` Finding 1: `card-sections` does not override `.coord-quick-access { display: none }`, so the coord-quick-access section is hidden entirely. The toggle button still appears in the action bar; toggling it has no visible effect. **Intentional non-issue** — the user opted out of the section by picking that style. ✓ |
| `markActivity` called during destruction (race) | `setState` on a destroyed store is a no-op. ✓ |
| Coordinator status changes from `isCoordinator: true` to `false` mid-session | Memo re-runs; the (formerly) coord drops out of the list. Its activity timestamp remains in the map but is unused unless it becomes a coordinator again. ✓ |

---

## 11. Testing checklist (manual, in `npm run tauri dev`)

Run `npm run kill-dev` before starting. Then `npm run tauri dev`.

### Setup

1. Create a project with at least 3 workgroups, each containing a different coordinator. Easiest: clone the existing `wg-6-dev-team` thrice with different names, OR use any project with multiple WG coords already configured.

### Path 1 — Toggle off (golden)

2. Confirm the new 🔥 button is the **leftmost** icon in `.action-bar-icons`. Hover it; tooltip reads `"Show coordinators in default order"`.
3. Confirm coord-quick-access renders coords in workgroup-iteration order (today's behavior). No sort applied.
4. Hover other action-bar icons (eye, hint, theme, gear). Their tooltips and click behavior are unchanged.

### Path 2 — Toggle on, no activity yet

5. Click the 🔥 button. It gets the `.active` class (full opacity). Tooltip changes to `"Show recent coordinators first"`.
6. **All coords have ts=0** at this point. Confirm visual order is unchanged from step 3 (stable sort preserves insertion order when all keys are equal).

### Path 3 — Activity reorders coords (the feature)

7. Click into the LAST coord in the visible list (e.g., the third coord). Wait until its agent finishes responding (busy→idle, ≈ 2.5 seconds after last token). Confirm it bubbles to the **top** of the coord-quick-access list within ≤ 1 reactive tick (no visible delay).
8. Trigger activity on the SECOND coord. It bubbles to the top, pushing the previous step's coord to position 2.
9. Trigger activity on the FIRST coord. The list now reads: most-recent-idled at top, others below in idle-recency order.
10. **Coords without any activity yet** stay at the bottom of the list, in their default sub-order.

### Path 4 — Toggle off mid-state

11. Toggle 🔥 off. The list snaps back to default order; the activity map is preserved in memory.
12. Toggle 🔥 on again. The list re-applies the sort using the same activity timestamps from before — no data loss across toggle cycles.

### Path 5 — Persistence across restart

13. With toggle ON, close the app entirely.
14. Reopen the app. Confirm the toggle is still ON (button has `.active` class on first render). Coord-quick-access renders sorted (or in default order if no idle has fired yet, since the activity map is empty after restart per the intake spec).
15. Toggle OFF. Close and reopen. Confirm the toggle is OFF.
16. Open `<config_dir>/settings.json` between steps 13 and 14 to confirm the file contains `"coordSortByActivity": true`.

### Path 5.5 — Rapid-click serialization (H2 regression, see §17)

Verifies the toggle handler serializes via `toggleInFlight` so that rapid clicks cannot land out-of-order persistence writes.

16a. With the toggle OFF (Path 5 left it off), focus the 🔥 button (Tab into it) and press Enter 5 times in rapid succession (Enter-on-focused-button is more reliable than mouse double-clicks for landing clicks within the ~10-50ms round-trip window). Mouse-clicking 5 times rapidly also works if your input device supports it.
16b. Observe during the burst: the button visibly disables for ~10-50ms after the first click (faint `:disabled` cursor; subsequent clicks during the in-flight window do NOT trigger an `.active` class flicker beyond the first toggle).
16c. After the burst settles (wait ~200ms), open `<config_dir>/settings.json` and confirm `"coordSortByActivity": true`. The persisted state matches the FIRST click that landed; subsequent in-flight-window clicks were correctly serialized away. Toggle once more to land OFF for cleanup.

### Path 6 — Failure mode

17. Make `<config_dir>/settings.json` read-only (or delete its parent directory mid-run for a stronger reproduction). Toggle the 🔥 button.
18. Confirm the toggle visually reverts after `SettingsAPI.update` rejects. Check the dev-tools console for `[coord-sort] Failed to persist coordSortByActivity: ...` log line.
19. Restore write permissions; toggle works again.

### Path 6.5 — Hydration-window protection (M1 regression, see §17)

Verifies the `hydrated` flag prevents clicks during the sub-100ms first-render window from racing the persisted-value override.

19a. With toggle persisted=true on disk (verify via `<config_dir>/settings.json`), force-quit the app (close all windows or use OS-level Force Quit). Relaunch via `npm run tauri dev`. As the sidebar window appears, immediately mash-click the 🔥 button repeatedly. To extend the hydration window for easier observation, attach the dev-tools debugger and set a breakpoint at `App.tsx:69` (`await SettingsAPI.get()`) — clicks landed before the breakpoint resumes are guaranteed to be in the disabled window.
19b. Observe: clicks during the disabled-pre-hydration window are suppressed at the DOM level. The button shows the browser-default `:disabled` opacity, the cursor does not change to `pointer`, and the click handler is NOT invoked (no log line, no `.active` class change). After `setCoordSortByActivity` runs at App.tsx:71 (sub-100ms in production, or after resuming the breakpoint in dev), the button enables and subsequent clicks behave normally.
19c. Without clicking the button after hydration, force-quit again. Inspect `<config_dir>/settings.json`. Expected: the persisted state is still `true` — clicks during the hydration window did NOT reach `update_settings` and did NOT corrupt the persisted state.

### Path 7 — Cross-window safety

20. Toggle ON in the sidebar window. Open the web-remote sidebar (if enabled). Confirm both windows render coords correctly per their own state. (Drift between windows is the v1 acceptable behavior — see §10's row on multi-sidebar drift.)

### Path 8 — Sidebar style cycle

21. Open Settings → cycle sidebar style across `noir-minimal`, `arctic-ops`, `deep-space`, `obsidian-mesh`, `neon-circuit`. Confirm the 🔥 button renders consistently in all five styles (uses `.coord-sort-activity-btn` base rules; no theme overrides).
22. Switch to `card-sections`. Confirm `.coord-quick-access` is hidden entirely (default rule, not overridden by this style). Toggling the button has no visible effect — this is correct, not a bug. Documented in §10.
23. Toggle light theme via the ☀/🌙 button. The 🔥 button's opacity-active behavior still works.

### Path 9 — No regression on other UI

24. Confirm the show-categories eye toggle still works.
25. Confirm `Hints`, theme, settings buttons still work.
26. Confirm SessionItem rows in the Agents section render unchanged.
27. Confirm Workgroups section coords still render in workgroup order (the sort is scoped to coord-quick-access only).

All 27 main steps + 6 sub-steps (16a-c, 19a-c) pass before reporting the feature complete. If step 16's settings.json line is missing, Phase A serde wiring is broken — go back to §3. The sub-steps verify the round-2 H2 (rapid-click serialization) and M1 (hydration-window protection) regressions per §17 and §18.6.

---

## 12. Naming summary

| Layer | Name | Type | Default | Lifetime |
|---|---|---|---|---|
| Rust struct field | `coord_sort_by_activity` | `bool` | `false` | persisted |
| JSON key (camelCase via serde) | `coordSortByActivity` | bool | `false` | persisted |
| TS `AppSettings` key | `coordSortByActivity` | `boolean` | `false` | persisted |
| TS `SessionsState` key | `coordSortByActivity` | `boolean` | `false` | in-memory (hydrated on start) |
| TS `SessionsState` key | `lastActivityBySessionId` | `Record<string, number>` | `{}` | in-memory only |
| TS `SessionsState` key | `hydrated` | `boolean` | `false` | in-memory only (flips once on start, never back) |
| Module-level signal in `sessions.ts` | `toggleInFlight` (via `createSignal`) | `boolean` | `false` | in-memory; toggled per-click |
| Activity timestamp source | `performance.now()` | `number` (ms since page load) | — | monotonic, no system-clock dependency |
| Sessions store getter | `coordSortByActivity` | reactive accessor | — | — |
| Sessions store getter | `lastActivityBySessionId` | reactive accessor | — | — |
| Sessions store getter | `hydrated` | reactive accessor | — | bound to button `disabled` |
| Sessions store getter | `toggleInFlight` | reactive accessor | — | bound to button `disabled` |
| Sessions store mutator | `setCoordSortByActivity(value)` | hydrate-on-start setter (also sets `hydrated = true`) | — | called once on start |
| Sessions store mutator | `toggleCoordSortByActivity()` | async, persists, serialized via `toggleInFlight` | — | bound to button click |
| Sessions store mutator | `markActivity(sessionId)` | sync, uses `performance.now()` | — | called from `onSessionIdle` |
| CSS class | `coord-sort-activity-btn` | toolbar-gear-btn variant | — | — |

---

## 13. Things the dev must NOT do

- Do **not** add a new Tauri command for the toggle. The existing `update_settings` round-trips the field via serde.
- Do **not** instrument the Rust backend (`pty/idle_detector.rs`, `pty/manager.rs`). The frontend listens to the existing `session_idle` event.
- Do **not** persist `lastActivityBySessionId`. It is intentionally in-memory only per the intake spec.
- Do **not** apply the sort to any list other than the Coordinator Quick-Access section (no Agents, no Workgroups, no Teams, no SessionItem).
- Do **not** export `normalizePath` from `sessions.ts` — use the existing `replicaSession` helper for the lookup chain (§8.2).
- Do **not** delete entries from `lastActivityBySessionId` on session destroy. Stale entries are harmless and out of scope (§6.5).
- Do **not** touch `setSessionWaiting` to fold `markActivity` inline. The intake explicitly asks for a single `markActivity` line in `App.tsx`.
- Do **not** change `onSessionBusy` — only `onSessionIdle` records activity.
- Do **not** add per-sidebar-style overrides for `.coord-sort-activity-btn`. The base rules render correctly across all styles.
- Do **not** add empty-state UI for "no coords with activity yet" — the user sees the default-order list, which is the correct empty state.
- Do **not** bump the app version in this branch.
- Do **not** introduce new crates, new TS dependencies, or new Tauri events. Pure derived state.
- Do **not** broadcast the toggle change to other windows via a new event in v1 (multi-window drift is acknowledged in §10).

---

## 14. Implementation order checklist (for the implementing dev)

Phase A (dev-rust + sessions store; self-contained, builds clean at every commit):

- [ ] §3.1.1 — add `coord_sort_by_activity: bool` field to `AppSettings` (Rust)
- [ ] §3.1.2 — initialize in `Default` impl
- [ ] §3.4 — (optional) add round-trip + missing-field tests
- [ ] §4.1 — add `coordSortByActivity: boolean` to `AppSettings` TS interface
- [ ] §4.2 — add `coordSortByActivity`, `lastActivityBySessionId`, AND `hydrated` to `SessionsState` TS interface
- [ ] §5.1 — add `coordSortByActivity: false`, `lastActivityBySessionId: {}`, `hydrated: false` to the `createStore<SessionsState>` initializer in `sessions.ts` (lock-step with §4.2 — these MUST land in the same commit as §4.2 to keep the build green)
- [ ] §5.2 — add `SettingsAPI` and `settingsStore` imports at the top of `sessions.ts`
- [ ] §5.3.1 — add the module-level `const [toggleInFlight, setToggleInFlight] = createSignal(false);` above the `createStore` block
- [ ] §5.3.2 — add the four reactive getters (`coordSortByActivity`, `lastActivityBySessionId`, `hydrated`, `toggleInFlight`) and the three mutators (`setCoordSortByActivity`, `toggleCoordSortByActivity`, `markActivity`) to the exported `sessionsStore` object
- [ ] verify `cargo check` and `npm run typecheck` both pass before opening the Phase A PR

Phase B (dev-webpage-ui; activates the store fields by wiring the UI):

- [ ] §6.1 — hydrate the toggle in `App.tsx` after `appSettings = await SettingsAPI.get()` (this also flips `hydrated` to true via the fold-in inside `setCoordSortByActivity`)
- [ ] §6.2 — add `sessionsStore.markActivity(id);` line inside the existing `onSessionIdle` callback
- [ ] §7.1 — insert the new 🔥 button in `ActionBar.tsx` as the first child of `.action-bar-icons`, with `disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}`
- [ ] §8.1 — extend the `coordinators()` memo in `ProjectPanel.tsx` with the `coordSortByActivity` sort
- [ ] §9.1 — add the three CSS rules for `.coord-sort-activity-btn` in `sidebar.css`
- [ ] run §11 testing checklist (paths 1–9, all 27 steps), plus the §17 round-2 regression items (rapid-click serialization, hydration-window disabled state)

When all boxes are checked and the testing checklist passes, the feature is ready for review.

---

## 15. dev-webpage-ui review notes

Verified all §3-§9 anchors against HEAD `1bc94d8`. The plan is implementable as written. The additions below are scoped to frontend concerns the architect did not explicitly address; each cites the file/line context the implementing dev should keep in mind.

### 15.1 SolidJS reactivity — `markActivity` map-replacement is correct (confirms §5.4)

Traced the proxy semantics for the `setState("lastActivityBySessionId", (prev) => ({ ...prev, [id]: ts }))` pattern against what `coordinators()` reads in §8.1:

1. The memo reads `sessionsStore.lastActivityBySessionId`, which returns the current proxy from `state.lastActivityBySessionId`.
2. SolidJS tracks the read at the path `state.lastActivityBySessionId`.
3. `setState(path, fn)` passes the current proxy to `fn`. The spread `{ ...prev }` enumerates own keys of the proxy — works because store proxies expose own keys via standard reflect traps.
4. The returned plain object replaces the value at `state.lastActivityBySessionId`. SolidJS diffs and notifies path-tracked listeners.
5. The memo re-runs and reads `activityMap[session.id] ?? 0` against the fresh map.

**Why the wholesale replace beats the path-keyed `setState("lastActivityBySessionId", id, ts)` form:** The path-keyed form *does* work in current SolidJS versions — proxy reads of unknown keys establish a track. But that relies on internal proxy behavior, not a public API contract. The wholesale-replace pattern is unambiguous: a top-level reference change always notifies. The trade-off (one shallow spread per idle event, ~few/sec workspace-wide) is negligible. **The plan's choice is correct.**

### 15.2 `replicaSession` reuse — confirmed, with one inherited limitation flagged

`replicaSession(wg, replica)` calls `findSessionByName(`${wg.name}/${replica.name}`)` which does a flat `find` over `state.sessions` (sessions.ts:341-344). Confirmed:

- `findSessionByName` does **not** scope by project. If two projects each have a workgroup named e.g. `wg-1-feature` containing a replica named `agent-x`, both bind to the session named `wg-1-feature/agent-x`. The first `find` match wins.
- This is a **pre-existing limitation** of the entire ProjectPanel.tsx replica→session binding (same lookup is used for status dot color, badges, mic button, telegram bridge). The new sort inherits the same behavior — if two coords render the wrong status dot today, they'll also sort by the wrong session's activity timestamp.
- Out of scope to fix here. The plan correctly chose `replicaSession` over a path-based match (which would have its own collision class via `normalizePath`).

**Flag for future hardening**, not for this PR: a `findSessionByPath` helper that tiebreaks by `workingDirectory` would close this gap project-wide.

### 15.3 App.tsx hydrate ordering — first-render flash on persisted=true

Timeline at app start (App.tsx onMount, lines 62-194):

| Step | Line | What happens |
|---|---|---|
| 1 | 62 | `onMount` fires AFTER initial JSX render. `coordSortByActivity` is `false` (store default). |
| 2 | 69 | `await SettingsAPI.get()` |
| 3 | (NEW) 71 | `setCoordSortByActivity(...)` — flag flips if persisted=true |
| 4 | 113 | `await SessionAPI.list()` |
| 5 | 148-152 | `onSessionIdle` listener attached |

If the user persisted `true`, between step 1 (JSX render with default OFF) and step 3 (hydrate to ON), there is a brief visual moment where the 🔥 button shows the OFF style. Typical hydration latency is sub-100ms, so the flash is imperceptible in practice — but it exists.

**This matches the existing pattern for `appSettings.sidebarStyle`** (App.tsx:72-74), which has the same first-render-flash characteristic. Accept as v1; do not add fix-flicker logic.

**Idle-event safety during hydration:** No idle event can fire between step 1 and step 5 because the listener is attached at step 5. Pre-existing sessions need ≥2.5s without activity before the IdleDetector emits, so any "first idle" arrives well after step 5. Confirmed safe.

### 15.4 Failure UX — `console.error` matches the codebase, no toast needed

Verified the existing pattern across all settings-persistence call sites:

- `src/shared/zoom.ts:55-57` — `console.error("Failed to save zoom:", e)`, no revert, no toast.
- `src/shared/window-geometry.ts:31-33` — `console.error("Failed to save window geometry:", e)`, no revert, no toast.
- `src/sidebar/components/OnboardingModal.tsx:35` — silent `try/catch {}`, no revert, no toast.

The plan's `console.error + revert` is **strictly more informative** than any existing settings-persistence path. Toast is not the established pattern for these failures. Keep `console.error`. (Toast is only used in ActionBar for project-load errors visible to the user, where the error is the user-action's primary outcome — a fundamentally different signal.)

### 15.5 CSS — confirmed no per-style override exists for the slot

Grepped the entire codebase for `show-categories-btn` and `coord-sort-activity-btn`:

- `show-categories-btn` appears in 4 places: ActionBar.tsx:128, sidebar.css:515 (base), 520 (`:hover`), 523 (`.active`). **No per-sidebar-style overrides.**
- `coord-sort-activity-btn` does not exist yet.

The plan's three rules at §9.1 will render consistently across all 7 sidebar styles. **Confirmed.**

**Layout sanity flag:** `.action-bar-icons` uses `gap: 2px`, `margin-left: auto`, `flex-shrink: 0` (sidebar.css:720-726). Adding a 5th icon increases the icons-block width by ≈20-22px. At very narrow sidebar widths, the New/Open dropdown could be squeezed. Surface in §11 — see §15.7 below.

### 15.6 TypeScript build — spread compatibility confirmed

The `SettingsAPI.update({ ...current, coordSortByActivity: next })` pattern in §5.3 matches three existing call sites:

- `src/shared/zoom.ts:53` — `await SettingsAPI.update({ ...settings, [key]: currentZoom })`
- `src/shared/window-geometry.ts:30` — `await SettingsAPI.update({ ...settings, [key]: geo })`
- `src/sidebar/components/OnboardingModal.tsx:33` — `await SettingsAPI.update({ ...settings, onboardingDismissed: true })`

After Phase A adds `coordSortByActivity: boolean` to `AppSettings`, the spread is type-checked. **No consumer of `AppSettings` breaks.**

**Note on Rust-only fields:** The Rust `AppSettings` struct (settings.rs:34-109) contains two fields the TS interface intentionally omits — `darkfactory_zoom: f64` (line 78, legacy compat) and `root_token: Option<String>` (line 105, `skip_serializing_if = "Option::is_none"`). These ride along as runtime-extra properties through `{ ...current, ... }` spreads and round-trip back to Rust. This is the existing behavior; the new toggle does not change it.

### 15.7 Testing checklist additions for §11

§11 is solid for functional correctness. Three frontend-specific additions:

**Path 1.5 — Keyboard accessibility:**
- After step 4, Tab through `.action-bar-icons` (expected order: 🔥 → 👁 → 💡 → ☀/🌙 → ⚙). Each button receives a visible focus ring (browser default — no custom `:focus-visible` rule is set for `.toolbar-gear-btn`).
- With 🔥 focused, press Enter (and separately Space). The toggle activates, persists, and the button's `.active` class updates.

**Path 8 addendum — Narrow sidebar regression:**
- After step 21, drag the sidebar to its minimum width (~280px). Confirm `.action-bar-icons` and the New/Open dropdown coexist without overflow or clipping. The 5th icon must not push the dropdown out of view.
- If the icons row wraps or the dropdown chevron clips, file a follow-up for `.action-bar-icons` overflow handling — out of scope for this PR (v0 already exhibited similar behavior with 4 icons at narrow widths).

**Path 9 addendum — Tooltip asymmetry verification:**
- The two tooltip strings (§7.3) are intentionally different in voice — `"Show coordinators in default order"` (future-action) when OFF, `"Show recent coordinators first"` (current-state) when ON. Hover the button in both states; confirm the user can interpret either string standalone. If grinch flags this, the answer is "matches the intake spec; user-locked."

**Accessibility note (out of scope, do not add):** A semantic `aria-pressed={sessionsStore.coordSortByActivity}` would be the correct attribute for a toggle button. Grepped the codebase: **no `aria-*` attributes exist anywhere in `src/`**. Adding one only on this button would be inconsistent. A11y is a project-wide concern that warrants its own PR.

### 15.8 Implementation order checklist (§14) — verified

Walked through the order:

| Step | Compiles after | Dependency |
|---|---|---|
| §3.1.1 + §3.1.2 | (none) | Phase A self-contained |
| §4.1 | (none) | Adds `coordSortByActivity` to TS `AppSettings` |
| §4.2 | (none) | Adds two fields to TS `SessionsState` |
| §5.1 | §4.2 | `createStore<SessionsState>` initializer requires the new fields |
| §5.2 | (none) | Imports — needed for §5.3 |
| §5.3 | §5.1 + §5.2 | Mutators read/write the new state fields and call `SettingsAPI.update` |
| §6.1 | §5.3 | `setCoordSortByActivity` defined in §5.3 |
| §6.2 | §5.3 | `markActivity` defined in §5.3 |
| §7.1 | §5.3 | `coordSortByActivity` getter and `toggleCoordSortByActivity` defined in §5.3 |
| §8.1 | §5.3 | `coordSortByActivity` and `lastActivityBySessionId` getters defined in §5.3 |
| §9.1 | (none) | CSS is independent |

**The §14 order compiles cleanly** provided §5.3 happens after §5.1 + §5.2 (which it does).

**Cosmetic refinement for the implementing dev:** §5.2 (imports) and §5.3 (mutators) should land in a single edit pass — splitting them creates a transient state where the imports are unused and `npx tsc --noEmit` will warn. Not a blocker.

### 15.9 No open questions for the user

All §10 edge cases are correctly handled. No questions requiring user input. Ready for grinch's adversarial pass.

---

## 16. dev-rust-grinch review notes

Verified all §3-§9 anchors against HEAD `1bc94d8`. Read every referenced file in full: `settings.rs`, `commands/config.rs`, `idle_detector.rs`, `types.ts`, `sessions.ts`, `App.tsx`, `ActionBar.tsx`, `ProjectPanel.tsx` (lines 1-100, 630-730), `sidebar.css` (lines 505-555). Traced every code path the plan touches. Found two **HIGH** severity issues that must be fixed before any merge, two **MEDIUM** that should be fixed in this PR, and seven **LOW** that are mostly inherited from the existing architecture and acceptable for v1.

**Cannot approve as-is.** H1 is a build-breaker by definition of the proposed phase split.

### Findings summary

| ID | Severity | Issue |
|---|---|---|
| H1 | HIGH | Phase A as defined breaks `npm run typecheck` |
| H2 | HIGH | Rapid toggle clicks can persist a state that does not match the user's last click |
| M1 | MEDIUM | User clicking the toggle during the hydration window is silently overridden, causing memory/disk divergence |
| M2 | MEDIUM | `Date.now()` is wall-clock — clock jumps reorder the sort incorrectly |
| L1 | LOW  | `lastActivityBySessionId` grows unboundedly until app restart (acknowledged in §6.5) |
| L2 | LOW  | Sort comparator calls `replicaSession` (linear find) on every comparison |
| L3 | LOW  | `findSessionByName` collisions across projects (inherited from §15.2) |
| L4 | LOW  | Optional Rust test §3.4 hardcodes a JSON snapshot of all current fields |
| L5 | LOW  | `markActivity` has no early-return for unknown sessionIds (rare orphan entry) |
| L6 | LOW  | `?? 0` masks "session not found" vs "session found, no activity yet" (documented in §8.4) |
| L7 | LOW  | `void settingsStore.refresh()` swallows the error silently |

---

### HIGH

#### H1. Phase A breaks the TypeScript build

**What**: §1 claims Phase A is "self-contained" and "Frontend builds without errors against the unchanged UI code". §14's Phase A checklist explicitly verifies `npm run typecheck` after merging §3 + §4. But §4.2 makes `coordSortByActivity: boolean` and `lastActivityBySessionId: Record<string, number>` REQUIRED fields on the `SessionsState` interface, while the `createStore<SessionsState>(...)` call at `src/sidebar/stores/sessions.ts:7-15` is not modified until §5.1 (Phase B).

**Why**: After Phase A merges, TypeScript will reject the existing
```ts
const [state, setState] = createStore<SessionsState>({
  sessions: [], activeId: null, teams: [], teamFilter: null,
  showInactive: false, showCategories: true, repos: [],
});
```
with: `Type '{...}' is missing the following properties from type 'SessionsState': coordSortByActivity, lastActivityBySessionId`. The build is broken until §5.1 also lands.

§15.8's table marks §4.2 as "Compiles after (none)" — that is incorrect. §4.2 has an undeclared dependency on §5.1. The two changes cannot be split across phases.

**Concrete failure scenario**: dev-rust merges Phase A per the plan's order. CI runs `npm run typecheck` on the new HEAD. TypeScript error at sessions.ts:7. CI fails. Phase B can't merge until the build is green. The "phase independence" claim collapses.

**Fix**: Choose one:
- (a) **Recommended.** Move §5.1, §5.2, and §5.3 into Phase A. Types and store stay in sync at every commit. The new fields are inert until Phase B wires up the UI. Phase A becomes "Rust + shared types + sessions store"; Phase B becomes "App.tsx + ActionBar + ProjectPanel + CSS".
- (b) Make `coordSortByActivity` and `lastActivityBySessionId` OPTIONAL in `SessionsState` (`coordSortByActivity?: boolean`). Then every reader must handle undefined. Less clean.
- (c) Collapse Phase A and Phase B into a single PR. Loses the "Rust ships independently" benefit but is the simplest correct option.

Update §1, §14, and §15.8 accordingly.

#### H2. Rapid toggle clicks can persist a state that does not match the user's last click

**What**: §10's row "User toggles rapidly (clicks 5 times in 200ms)" claims "the user's final click is what gets saved". This is not guaranteed. The handler in §5.3 captures `next` at click time:

```ts
async toggleCoordSortByActivity() {
  const next = !state.coordSortByActivity;       // 1. read state at click time
  setState("coordSortByActivity", next);         // 2. optimistic flip
  try {
    const current = await SettingsAPI.get();     // 3. fetch from backend
    await SettingsAPI.update({ ...current, coordSortByActivity: next });  // 4. send `next` from step 1
    void settingsStore.refresh();
  } catch (e) { /* revert */ }
}
```

If two clicks land their `update_settings` Tauri commands at the backend in REVERSE order, the persisted state is whichever update completes LAST, not whichever was clicked LAST. Tauri commands run as independent futures on the tokio runtime — there is no FIFO ordering guarantee between concurrent invocations of the same command. `update_settings` itself does R-M-W across multiple `await` points (`commands/config.rs:38-42`: read root_token → validate → save_settings to disk → acquire write lock → memory write), with no lock held across the whole sequence.

**Concrete failure scenario**:
- t=0: Click 1 fires. Optimistic state→true. `SettingsAPI.get()` starts.
- t=20ms: Click 2 fires. state is true (from click 1 optimistic), so `next2 = false`. Optimistic state→false. `SettingsAPI.get()` starts (returns false — backend disk hasn't been updated yet by click 1's still-in-flight update).
- Both `SettingsAPI.update` calls hit the backend.
- Click 2's update completes first: disk = false.
- Click 1's update completes second: disk = true.
- Final state: memory = false (no revert ran — both succeeded). Disk = true.
- User closes app. Reopens. Hydration reads disk = true. Toggle is ON.
- User's last visible action was click 2 → OFF. Result is ON. **Mismatch.**

**Fix**: Serialize toggle calls in the frontend. Smallest correct change — an in-flight flag:

```ts
let toggleInFlight = false;
async toggleCoordSortByActivity() {
  if (toggleInFlight) return;
  toggleInFlight = true;
  const next = !state.coordSortByActivity;
  setState("coordSortByActivity", next);
  try {
    const current = await SettingsAPI.get();
    await SettingsAPI.update({ ...current, coordSortByActivity: next });
    void settingsStore.refresh();
  } catch (e) {
    console.error("[coord-sort] Failed to persist coordSortByActivity:", e);
    setState("coordSortByActivity", !next);
  } finally {
    toggleInFlight = false;
  }
},
```

Pair with `disabled={toggleInFlight}` on the button (requires exposing the flag as a reactive signal — use `createSignal`, not a plain let). This eliminates the race entirely. Round-trip latency is ~10-50ms — well below the human "another click" threshold.

Update §10's "User toggles rapidly" row to: "While an update is in-flight (~10-50ms), subsequent clicks are ignored. The user's first click after the round-trip is what gets saved. The button is briefly disabled during the in-flight window."

---

### MEDIUM

#### M1. User toggle-click during hydration is silently overridden, causing memory/disk divergence

**What**: §15.3 acknowledges a sub-100ms visual flash between first JSX render (toggle = false) and hydration (`setCoordSortByActivity(persisted_value)` at App.tsx:71 per §6.1). It treats this as imperceptible and accepts it as v1. But the analysis stops at the visual symptom — it does NOT cover the case where the user actually CLICKS the toggle during that window.

If the user clicks during hydration, the click and the hydration race. The hydration silently overrides the click in the in-memory store, but the click's `update_settings` request is already in flight to the backend.

**Why**:
- t=0: First render. state.coord = false (default). Persisted on disk: false.
- t=10ms: User clicks. `next = !false = true`. Optimistic: state.coord = true. Click's `await SettingsAPI.get()` starts.
- t=20ms: App.tsx hydration completes. Line 71 calls `setCoordSortByActivity(false)` — persisted value. state.coord = false (overrides the user's click).
- t=50ms: Click's `update(coord=true)` lands at backend. Backend writes disk = true and updates memory state.
- Frontend in-memory `sessionsStore.coordSortByActivity` is **false**. Disk is **true**. UI shows OFF; persisted state is ON.

The user sees the button briefly flip on, then snap back to off. The persisted state on disk is ON. Their next attempt to "fix it" by clicking again toggles to false in memory and false on disk — undoing their original intent.

This is a data correctness bug, not just a visual flicker.

**Concrete failure scenario**: User opens the app with persisted=false. They click 🔥 to enable sort. They see it flip on then back off. They give up, close the app. Reopen — toggle is ON (because their click's `update` reached the backend). Confusing and unreliable.

A more pernicious variant: user opens the app with persisted=true. They click 🔥 (intending to turn it off). At click time, state was false (default before hydration), so the click flips state to true (optimistic) and sends `update(true)`. Hydration fires, sets state to true (already there). Click completes — disk = true (already there). Now the user sees the button ON. They click again — state→false, `update(false)`. Disk = false. They closed the app. Reopen. Toggle is OFF. **Their FIRST click did nothing visible**, but their second click landed.

**Fix**: Disable the toggle button until hydration completes. Track a reactive `hydrated` signal:

```ts
// In sessions.ts (new state field)
const [state, setState] = createStore<SessionsState>({ ..., hydrated: false });

// setCoordSortByActivity sets both
setCoordSortByActivity(value: boolean) {
  setState("coordSortByActivity", value);
  setState("hydrated", true);
},

// Expose a getter
get hydrated() { return state.hydrated; },

// In ActionBar.tsx
<button disabled={!sessionsStore.hydrated} ... />
```

Plus a tooltip variant for the disabled state (`"Loading settings..."` or similar).

Alternative: have `setCoordSortByActivity` no-op if the user has already clicked (track `hasInteracted` flag, clobber on first toggle). I prefer "disable until hydrated" — it's more discoverable.

Update §15.3 to acknowledge that the flash is not just visual; it has a state-divergence consequence if the user clicks during the window.

#### M2. `Date.now()` is wall-clock — system clock changes can reorder the sort incorrectly

**What**: §5.3's `markActivity` records `Date.now()`. This is wall-clock time, not monotonic. NTP corrections, DST changes, manual clock adjustments, or VM clock drift can cause a later activity to record a SMALLER timestamp than an earlier one.

**Why**:
- Coord A idles at wall-clock t=1000. Recorded ts=1000.
- System clock jumps backwards (NTP correction of 500ms drift). Wall-clock now at t=500.
- Coord B idles at wall-clock t=510 (10ms after the NTP correction, in real time). Recorded ts=510.
- Sort comparator: `tsFor(B) - tsFor(A) = 510 - 1000 = -490` → A above B. **WRONG ORDER**: B is genuinely the more recent activity.

The Rust `IdleDetector` itself uses `Instant::now()` (monotonic — line 99 of `idle_detector.rs`), and even guards against last_seen-in-the-future via `checked_duration_since` (line 103). The frontend should do the same.

**Concrete failure scenarios**:
- Laptop suspended for hours; resumes; OS catches up via NTP — the next coord that idles records a "smaller than expected" timestamp.
- Hyper-V VM with poor host-time sync; periodic NTP corrections of seconds; user sees occasional coord reordering they cannot explain.
- DST forward/backward transitions on systems without proper UTC isolation (rare on modern Windows but possible in dev environments).

The `lastActivityBySessionId` map is in-memory only (resets on app restart per §10), so absolute timestamps are irrelevant — only ordering matters. `performance.now()` is monotonic from the time origin (page load), making it a clean drop-in:

**Fix**:
```ts
markActivity(sessionId: string) {
  setState("lastActivityBySessionId", (prev) => ({ ...prev, [sessionId]: performance.now() }));
},
```

One-word change. Update §6.5 ("Per-session timestamps live in the frontend store and are NOT persisted") to clarify the timestamp source. Update §10's "Two coords with identical timestamps" row to mention `performance.now()` quantization (typically 100µs or finer in modern Wry; ties are still possible but for fundamentally simultaneous events, not for clock anomalies).

---

### LOW

#### L1. `lastActivityBySessionId` map grows unboundedly until app restart

§6.5 acknowledges this and rejects cleanup as out of scope. Memory leak is bounded by realistic session creation rate: ~50 bytes per stale entry, sessions destroyed at human pace, app restarts on a longer cadence. <1 KB/hour for typical use, <50 KB even after weeks of uptime.

Cumulative cost: `markActivity` does `{ ...prev, [id]: ts }` — O(N) shallow spread per call. With 1000 stale entries, each idle event copies 1000 keys. Idle events fire at ≤few/sec workspace-wide. CPU cost: <1ms per event at realistic sizes — negligible.

The cleanup is a 5-line addition inside `removeSession`. I'd include it for hygiene, but it's a judgment call.

**Accept as-is** with the architect's reasoning. Worth a follow-up issue if anyone reports sluggish idle handling after weeks of uptime.

#### L2. Sort comparator calls `replicaSession` (linear find) on every comparison

`result.sort((a, b) => tsFor(b) - tsFor(a))`. For N coords sorted with TimSort, ~N log N comparisons; each `tsFor` call invokes `replicaSession` which is O(M) on `state.sessions`. Total: O(NM log N) per memo run.

At realistic sizes (N=5 coords, M=50 sessions): ~1k ops — trivial. Pathological (N=20, M=500): ~50k ops — still well below human-perceptible threshold.

Could pre-decorate (`map → sort → map`) to make it O(NM + N log N). Optimization is straightforward but not necessary for v1.

**Accept as-is**.

#### L3. `findSessionByName` collisions across projects (inherited from §15.2)

Two projects with same-named workgroup + replica bind to one session. The sort uses one session's timestamp for both coords. Pre-existing limitation of `replicaDotClass`, etc. The new sort inherits the same behavior. dev-webpage-ui flagged for future hardening.

**Accept as-is**. Documented in §15.2.

#### L4. Optional Rust test §3.4 hardcodes a JSON snapshot of all current fields

If a future PR adds a new REQUIRED field (no `#[serde(default)]`), the §3.4 backward-compat test fails for the wrong reason — its purpose is to test that the new toggle field defaults correctly when missing, not to enforce that all current fields are present. Brittle to unrelated future struct changes.

**Accept as-is**. The test is documented as optional, and the failure mode is loud and easy to fix when it arrives.

#### L5. `markActivity` has no early-return for unknown sessionIds

If `onSessionIdle` is delivered AFTER `onSessionDestroyed` for the same session (rare event-ordering edge case in Tauri), `markActivity` creates an orphan entry that never gets cleaned. Harmless — `replicaSession` returns undefined for the destroyed session, so the orphan never gets consulted.

**Accept as-is**. Pre-existing reactive pattern. Cleanup would only be needed if L1's bound becomes a real concern.

#### L6. `?? 0` in `tsFor` masks "session not found" vs "session found, no activity yet"

Documented in §8.4's table. Both conditions yield 0 (sort to bottom). No actual bug, just two distinct semantic states folded into one return value. Acceptable since the sort doesn't care about the distinction.

**Accept as-is**.

#### L7. `void settingsStore.refresh()` swallows the error silently

If `settingsStore.refresh()` rejects (transient IPC failure), the in-memory `settingsStore.current` is stale until the next refresh. Pre-existing pattern (§15.4 confirms console.error is the established convention; refresh has no error path at all). Doesn't affect this feature's sort behavior — `sessionsStore` is the source of truth for the toggle, not `settingsStore`.

**Accept as-is**. Suggest adding `.catch(e => console.error(...))` for symmetry with the toggle's error path, but not required.

---

### Attack vector verdicts

The tech-lead asked for ✓/✗ on each of the 7 attack vectors:

| # | Vector | Verdict | Notes |
|---|---|---|---|
| 1 | Race conditions | ✗ | H2 (rapid clicks) and M1 (hydration window) are real bugs. The `setState`-callback / `markActivity`-on-destroyed / store-serialization checks all clean. |
| 2 | Edge cases | ✗ | M2 (`Date.now()` wall-clock) is real. Same-coord-multi-workgroup, coord session restart, multiple coords idling in same tick, timestamp ties — all correctly handled. The "user toggles during hydration" item from the prompt is M1 (was visual-only in §15.3). The "update_settings rejects due to validate_agent_commands" item is acceptable — pre-existing limitation; revert path works. |
| 3 | Resource leaks | ✓ | L1 documented and bounded (<50 KB after weeks of uptime). No real leak. |
| 4 | Logic errors | ✓ | Comparator math is overflow-safe. `??` semantics correct. Reactive tracking via `coordSortByActivity` getter inside the memo is correct (Solid tracks the read; toggling OFF re-runs and produces unsorted result). Stable sort guaranteed by ES2019 (V8 7.0+ in modern Wry). |
| 5 | Persistence | ✗ | Same root cause as H2 — TOCTOU between `get_settings` and `update_settings`. Manifests as "rapid toggle clobbers another window's edit" too. Pre-existing limitation, but the toggle exercises it more aggressively. The §5.3 revert path correctly handles the validate_agent_commands rejection case. |
| 6 | TS strictness | ✗ | H1 (Phase A breaks build) is the headline. Spread compatibility itself is fine per §15.6. `darkfactoryZoom` ride-along confirmed safe. |
| 7 | Anything else | (info) | Tooltip asymmetry per spec. Accessibility (aria-pressed) is project-wide deferral. CSS layout at narrow widths covered in §15.7. Memo created inside render scope on every `collapsed` flip is wasteful but pre-existing pattern; not a correctness issue. |

---

### Disposition

- **HIGH (must fix before merge)**: H1, H2.
- **MEDIUM (should fix in this PR — small, real, low-risk fixes)**: M1, M2.
- **LOW (acceptable for v1)**: L1-L7.

After H1, H2, M1, M2 are addressed, I'll re-review. The architecture is sound; the bugs are localized to the toggle handler, the hydration sequencing, and the phase split. None of them require redesigning the feature.

---

## 17. Round 2 changes (post-grinch)

This section is the changelog for what changed between the round-1 plan and the current text. The four fixes below address all four non-LOW grinch findings (H1, H2, M1, M2 in §16). The seven LOW items (L1–L7) are accepted as-is per tech-lead's direction. The user-locked decisions from the round-1 intake (🔥 icon, button position to the LEFT of the eye, always-visible across all sidebar styles, single-window v1, in-memory `lastActivityBySessionId`) are unchanged.

### H1 — Phase A breaks TS build → moved sessions store into Phase A

**Problem.** Round-1 §4.2 made `coordSortByActivity` and `lastActivityBySessionId` REQUIRED on `SessionsState`, but the existing `createStore<SessionsState>` initializer in `sessions.ts:7-15` only got those fields in §5.1, which was tagged Phase B. After Phase A merged alone, `npm run typecheck` would error with `Type '{...}' is missing the following properties from type 'SessionsState': coordSortByActivity, lastActivityBySessionId`. The "phase independence" claim collapsed.

**Resolution.** Moved §5.1, §5.2, and §5.3 into Phase A. Phase A is now "Rust + shared types + sessions store" (self-contained, builds clean at every commit). Phase B is now "App.tsx + ActionBar + ProjectPanel + CSS" (activates the inert store fields by wiring the UI). Edits applied to §1 Overview, §2 file table, §5 heading, and §14 implementation order checklist. The Phase B activation continues to depend on Phase A landing first.

A note on §15.8: the round-1 dev-webpage-ui review marked §4.2 as "Compiles after (none)", which was incorrect. With the round-2 phase split, §4.2 (interface) and §5.1 (initializer) land in the same commit inside Phase A, so the dependency is internal to Phase A and the build stays green. Per the tech-lead's "do not touch §15" instruction, §15.8's table is left as a historical artifact; this paragraph supersedes that entry.

### H2 — Rapid toggle clicks could persist a value that does not match the user's last click → added in-flight serialization

**Problem.** Two concurrent `update_settings` Tauri commands have no FIFO ordering on tokio. Round-1 §10's "User toggles rapidly" row promised "last write wins", but the disk in fact reflects whichever round-trip *completes* last, which can disagree with the user's last click. Surface symptom: user clicks toggle off, sees it visibly flicker, restarts the app, finds it ON.

**Resolution.** Frontend in-flight serialization. A module-level `const [toggleInFlight, setToggleInFlight] = createSignal(false);` is added above the `createStore` block in `sessions.ts` (§5.3.1). The `toggleCoordSortByActivity` handler now early-returns if `toggleInFlight()` is true, sets it to true at the start of the round-trip, and clears it in `finally` (§5.3.2). The button binds `disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}` so the browser also suppresses the click at the DOM level (§7.1) — defense-in-depth. §10's "User toggles rapidly" row was rewritten to reflect the new behavior. §12 naming summary now includes `toggleInFlight` and its reactive getter.

### M1 — User clicks during the hydration window cause memory/disk divergence → added `hydrated` flag

**Problem.** Round-1 §15.3 acknowledged a sub-100ms first-render flash but treated it as visual-only. Grinch found that if the user actually CLICKS during that window, the click's optimistic flip + in-flight `update_settings(true)` is silently overridden by hydration's `setCoordSortByActivity(persisted_value)`. The persisted value reaches disk; the in-memory store reverts to the persisted value; UI shows the wrong state. A more pernicious variant: if persisted=true and the default=false, the user's first click can be entirely silent.

**Resolution.** Added `hydrated: boolean` to `SessionsState` (default `false`), initialized in the `createStore` block (§5.1). `setCoordSortByActivity` now flips `hydrated` to `true` as part of the same mutator — folded in deliberately so the implementing dev cannot accidentally enable the button before applying the persisted value (§5.3.2). The button binds `disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}` (§7.1), so during hydration the button is disabled at the DOM level and the click is suppressed by the browser before the handler runs. Once hydration completes (sub-100ms), `hydrated` flips and the button becomes clickable. Edits applied to §4.2 (interface), §5.1 (initializer), §5.3.2 (mutator + getter), §7.1 (button JSX), §10 (new edge case row).

### M2 — `Date.now()` is wall-clock and can go backwards → switched to `performance.now()`

**Problem.** NTP corrections, DST changes, manual clock adjustments, and VM clock drift can cause `Date.now()` to record a SMALLER timestamp for a later event than for an earlier one. The sort comparator then orders later activity below earlier activity. The Rust `IdleDetector` itself uses `Instant::now()` (monotonic) and even guards against the future via `checked_duration_since`; the frontend should match.

**Resolution.** One-word change in `markActivity`: `Date.now()` → `performance.now()` (§5.3.2). `performance.now()` is monotonic from the time origin (page load), so it cannot go backwards under any clock-correction scenario. Since `lastActivityBySessionId` is in-memory only and resets on app restart, absolute time is irrelevant — only ordering matters, which is exactly what `performance.now()` provides. Edits applied to §5.3.2 (the mutator), §8.4 (zero-timestamp first-idle bubble-up description), §10 ("two coords with identical timestamps" row mentions `performance.now()` quantization, "IdleDetector re-emits" row references `performance.now()`), §12 naming summary (new "Activity timestamp source" row). System-clock-changes is no longer an attack vector for this feature.

### Sections touched in round 2

For the implementing dev cross-reference: §1, §2, §4.2, §5 heading, §5.1, §5.3 (replaced wholesale, now split into §5.3.1 and §5.3.2), §6.1, §7.1, §8.4, §10 (six rows updated, one added), §12, §14, and this §17 (new). §3, §4.1, §4.3, §5.2, §5.4, §6.2-§6.5, §7.2-§7.3, §8.1-§8.3, §8.5-§8.6, §9, §11, §13, §15 (historical), §16 (historical) are unchanged.

### Test coverage for the new mechanisms

§11 path 6 already covers `update_settings` failure (button reverts via `catch`); the `finally` block is a strict superset and re-enables the button. §11 paths 1-5 implicitly exercise hydration (the toggle works only because hydration completed). The two regression scenarios introduced in round 2 — rapid clicks (H2) and click-during-hydration (M1) — are not yet explicit steps in §11 but should be added by the implementing dev when they confirm the behavior end-to-end:

- **H2 verification:** click 🔥 5 times in 200ms. Expected: only the first click's round-trip fires, the button visibly disables during the round-trip, subsequent clicks are no-ops at the DOM level, and the persisted state matches the *first* click that landed before the user resumed clicking. Confirm via `<config_dir>/settings.json` after the click burst settles.
- **M1 verification:** with persisted=true, force-quit and relaunch the app. As the sidebar window opens, immediately mash the 🔥 button with rapid clicks. Expected: clicks during the sub-100ms hydration window are suppressed (button is disabled); after hydration, clicks land normally. The persisted state on next restart matches the user's last accepted click — never a click that landed during the hydration window.

Both can be added to §11 as Path 5.5 and Path 6.5 in a future revision.

---

## 18. dev-webpage-ui round-2 confirm

Re-read §17 (changelog) and the touched sections (§1, §2, §4.2, §5, §6.1, §7.1, §8.4, §10, §12, §14). The four fixes (H1, H2, M1, M2) are reactivity-correct from the frontend perspective. **APPROVE.**

### 18.1 `toggleInFlight` as module-level `createSignal` — idiomatic and reactive

The architect's choice to use a module-level `createSignal` (not a `createStore` field) for `toggleInFlight` matches an existing precedent in the same file: `sessions.ts:155` already declares `const [collapsedTeams, setCollapsedTeams] = createSignal<Record<string, boolean>>({});` — a module-level signal exposed via a store getter (`get collapsedTeams() { return collapsedTeams(); }` at sessions.ts:250-252). The new `toggleInFlight` follows the same shape exactly.

**Reactivity confirmed for the button's `disabled` binding:**

- `sessionsStore.toggleInFlight` is a getter that calls the signal accessor `toggleInFlight()` — reactive read.
- `sessionsStore.hydrated` is a getter that reads `state.hydrated` (a store path) — reactive read.
- The `disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}` expression is evaluated inside a SolidJS reactive context (JSX prop). Both reads establish tracking dependencies; either flipping triggers a re-render.

**One subtle property worth noting** for the implementing dev: the `||` operator short-circuits. During pre-hydration, `!sessionsStore.hydrated == true` and `sessionsStore.toggleInFlight` is never read in that frame, so the signal is not yet tracked. This is *correct*, not a bug — `toggleInFlight` cannot be `true` during pre-hydration because the button is disabled, so no click handler ever runs to set it. Once `hydrated` flips, the prop expression re-evaluates and starts tracking `toggleInFlight` from that point forward. No cleanup or workaround needed.

### 18.2 `setCoordSortByActivity` fold-in — fuse holds

Verified that `setCoordSortByActivity` is the **only** mutator that touches `state.hydrated`. There is no separate `setHydrated` exposed on the store. The two state writes (`coordSortByActivity` + `hydrated`) are inseparable from any caller's perspective.

**Today's only call site is App.tsx:71** (hydration). The fuse holds because:

1. No code path can flip `hydrated` to `true` without also applying a (presumed-persisted) value to `coordSortByActivity`. Future contributors cannot "enable the button before applying the persisted value" by accident — they would have to bypass the only available API to do so, which is a bigger code smell that would surface in review.
2. `toggleCoordSortByActivity` does NOT call `setCoordSortByActivity` — it writes `setState("coordSortByActivity", next)` directly. So clicks don't redundantly flip `hydrated` after hydration. (Already true at that point, so the redundancy would be harmless, but the architect's design avoids it cleanly.)
3. If a future feature adds a "reset to defaults" code path that calls `setCoordSortByActivity(false)`, it would re-flip `hydrated` to `true` — already true, harmless idempotent write.

The deliberate fold-in is the right call. **Confirmed.**

### 18.3 `performance.now()` math — correct, no pre-existing consumer affected

Two concerns to verify:

**(a) Comparator math.** `tsFor(b) - tsFor(a)` where both operands are floats from `performance.now()` (or `0` for unset). `performance.now()` returns ms since the page-load time origin — a positive non-decreasing float. The Number safe range (2^53) supports `performance.now()` values for ~285 millennia. No overflow risk. Subtraction returns a finite float; sign of the result drives the sort order correctly.

**(b) Pre-existing consumers of `lastActivityBySessionId`.** This is a brand-new field — it does not exist before this PR. Grepped the codebase for the field name: zero references outside the plan and §17. No code anywhere assumes `Date.now()` epoch-millis for this field.

**One edge case worth pinning down:** when comparing `0` (default for unset) vs `performance.now()` from a real `markActivity` call, the comparator gives a strictly negative result (real activity > 0), so the unset coord sorts below the real-activity coord. Exactly as §8.4's table promises. Even on the hypothetical first-tick call where `performance.now()` could return a value very close to 0, the strict-less-than comparison still works correctly since stable sort tie-breaks on insertion order. **Confirmed.**

### 18.4 Phase split coherence across §1, §2, §5, §14

Walked the four sections and verified they describe the same Phase A / Phase B boundary:

| Section | Phase A says | Phase B says |
|---|---|---|
| §1 Overview (lines 27-30) | "Rust + shared types + sessions store" — self-contained, builds clean at every commit | "App.tsx wiring + UI" |
| §2 file table | settings.rs=A, types.ts=A, sessions.ts=A | App.tsx=B, ActionBar.tsx=B, ProjectPanel.tsx=B, sidebar.css=B |
| §5 heading (line 223 + round-2 note line 225) | "Phase A — Sessions store" | (no Phase B content in §5) |
| §14 checklist | §3 + §4 + §5 boxes | §6 + §7 + §8 + §9 boxes |

**Coherent. No orphan Phase-B-says-Phase-A reference remains.** §17's "Sections touched in round 2" list at line 1280 also matches what I found edited.

### 18.5 §15 supersession — clean for future readers

§17's H1 paragraph (line 1242) explicitly identifies §15.8's incorrect "Compiles after (none)" claim and supersedes it. The chronological flow §15 (round 1) → §16 (grinch round 1) → §17 (architect round 2) → §18 (this) is implicit but readable top-to-bottom for anyone reading sequentially.

**One small breadcrumb concern:** a future reader who jumps directly to §15.8 (e.g., via grep for "Compiles after") may miss the §17 supersession. Per the tech-lead's "do not touch §15" instruction, I will not edit §15 — but the lookup risk is worth noting. If a third-round revision opens, consider adding a one-line `> Superseded by §17.` footer to §15.8. Out of scope for this round; the supersession note in §17 is sufficient for serial readers.

### 18.6 §11 Path 5.5 + Path 6.5 — added

The architect's suggestion to add explicit regression coverage for H2 (rapid-click serialization) and M1 (hydration-window protection) is meaningful enough to add now. Without explicit steps, the implementing dev (me) might verify only the happy paths and ship without confirming the round-2 fixes actually work. Better to codify them in the testing protocol than to discover the gap during implementation.

The new test paths are inserted into §11 above as **Path 5.5** (rapid-click serialization, H2) and **Path 6.5** (hydration-window protection, M1). Steps use a sub-letter convention (`16a-c`, `19a-c`) so the existing main-numbered steps don't churn. The closing summary in §11 was updated from "27 steps" to "27 main steps + 6 sub-steps".

Path 6.5 step `19a` notes that the easiest way to observe the disabled-pre-hydration window in dev is to attach the debugger and breakpoint at `App.tsx:69`. In production builds the window is sub-100ms and clicks have to be timed carefully to land inside it.

### 18.7 No open questions for the user

All four round-2 fixes are sound. No questions requiring user input. Ready for round-2 grinch confirm and Step 6 implementation.

---

## 19. dev-rust-grinch round-2 confirm

Re-read §17 (architect's changelog), §18 (dev-webpage-ui's confirm), and the touched sections (§1, §2, §4.2, §5.1, §5.3.1, §5.3.2, §6.1, §7.1, §8.4, §10, §11 paths 5.5 and 6.5, §12, §14). Verified each fix landed correctly against the same code base I scanned for §16.

### Verdict

**APPROVE.** All four fixes from §16 (H1, H2, M1, M2) are correctly addressed. Phase A is now genuinely self-contained and builds clean. The toggle handler is properly serialized at both DOM and store levels. The hydration-window race is closed at the DOM level via `disabled={!hydrated || toggleInFlight}`. `performance.now()` cleanly replaces `Date.now()` with no comparator math regression.

One non-blocking nice-to-have noted at the end (programmatic `toggleCoordSortByActivity` bypass of M1). Not a blocker — the only current call site is the disabled button.

### Check items (the 6 you asked about)

#### 1. H1 fix landed correctly — Phase A builds clean ✓

- §4.2 declares three required `SessionsState` fields: `coordSortByActivity`, `lastActivityBySessionId`, `hydrated`.
- §5.1 updates the `createStore<SessionsState>` initializer to include all three, in the same Phase A boundary.
- §5.3.1 adds the module-level `const [toggleInFlight, setToggleInFlight] = createSignal(false);` above the `createStore` block. `createSignal` is already imported at `sessions.ts:1`.
- §5.3.2 adds the four reactive getters and three mutators, fully inside Phase A.
- §2 file table tags `sessions.ts` as Phase A. §14 checklist tags §3 + §4 + §5 as Phase A. §1 Overview describes Phase A as "Rust + shared types + sessions store" and explicitly states "builds clean (`cargo check` + `npm run typecheck` both green) at every commit".

After Phase A merges alone, `npm run typecheck` will see SessionsState requiring three fields AND the createStore initializer providing all three. Build green. ✓

The new fields are inert in Phase A: nothing reads them yet (the UI is unchanged). Phase B activates them by wiring `App.tsx`, the button, the memo, and CSS.

#### 2. H2 fix is complete; cross-window residual race acceptable v1 ✓

The `toggleInFlight` serialization closes the **single-window** TOCTOU. Two layers of defense:

- **DOM-level**: `disabled={!sessionsStore.hydrated || sessionsStore.toggleInFlight}` on the button. Browsers natively suppress click events on disabled buttons.
- **Store-level**: `if (toggleInFlight()) return;` early-return at the top of `toggleCoordSortByActivity`. Defense-in-depth for programmatic invocation.

The `setToggleInFlight(true)` happens before any `await`, so JavaScript's single-threaded execution guarantees no second concurrent invocation can pass the early-return check. The `finally` block clears the signal regardless of try/catch outcome — no leak path even if `SettingsAPI.update` throws or `void settingsStore.refresh()` later rejects.

**Cross-window question (the architect's "unaddressed corner"):** With per-window `toggleInFlight` signals, two windows toggling concurrently each have their own in-flight gate. They do NOT serialize across windows. The TOCTOU between Window A's `get_settings`/`update_settings` and Window B's same pair remains.

This residual race is **acceptable for v1**, for three reasons:

1. **Pre-existing limitation, not a regression.** Every settings field has it (`showCategories`, `sidebarStyle`, zoom, geometry, onboarding). The toggle does not introduce a new attack surface — it inherits the existing one.
2. **Locked decisions explicitly forbid the closes.** Closing the cross-window race requires either a backend partial-update Tauri command (`set_setting(key, value)`) or a settings-changed event broadcast — both ruled out by §13 ("Do not introduce new crates, new TS dependencies, or new Tauri events"). Per the round-2 prompt: locked.
3. **The toggle no longer exercises it more aggressively.** In round 1, rapid clicks within ONE window would race within that window AND clobber across windows. With `toggleInFlight`, the within-window race is gone, so the only remaining cross-window scenario is two human users actively toggling in two windows at the same instant — significantly rarer than user mash-clicking in one window.

§10's "Multiple sidebar windows open" row was correctly updated in round 2 to acknowledge the TOCTOU explicitly. No additional plan edit needed.

The architect did not leave an unaddressed corner — the cross-window race is a known v1-acceptable limitation that the locked decisions prevent us from closing.

#### 3. M1 fix is complete; fused flip holds; one minor defense-in-depth gap ✓ (with note)

The fuse on `hydrated` is sound: `setCoordSortByActivity` is the only mutator that writes `state.hydrated`, and it always co-writes `state.coordSortByActivity`. Verified by grep: §5.3.2 shows `setState("hydrated", ...)` only inside `setCoordSortByActivity`. No other mutator references the field.

**The fused flip cannot be bypassed via the public store API.** Future contributors cannot accidentally enable the button before applying the persisted value because there is no separate `setHydrated` exposed.

DOM-level defense: button is `disabled={!sessionsStore.hydrated || ...}`. During pre-hydration, the browser suppresses clicks. The handler never runs, no `update_settings` call is made, no memory/disk divergence is possible. Path 6.5 verifies this end-to-end.

**Minor defense-in-depth gap (non-blocking note):** `toggleCoordSortByActivity` checks `toggleInFlight` but does NOT check `hydrated`. If a future programmatic caller invokes `sessionsStore.toggleCoordSortByActivity()` before hydration completes (e.g., from DevTools or a future programmatic call site), the M1 race re-emerges:

- Optimistic flip writes coord at click time.
- Click's `update_settings` lands at backend.
- App.tsx hydration's `setCoordSortByActivity(persisted_value)` overrides memory state.
- Memory ≠ disk if optimistic and persisted disagreed.

**Today's attack surface is zero** — only the disabled button calls `toggleCoordSortByActivity`. But §7.1 explicitly justifies the `toggleInFlight` early-return as defense-in-depth for "future contributors who invoke the method programmatically rather than via the button". By the same logic, an early-return for `if (!state.hydrated) return;` would be symmetric and free.

**Suggested fix (optional, can be deferred):**

```ts
async toggleCoordSortByActivity() {
  if (!state.hydrated) return;       // NEW — symmetric defense-in-depth for M1
  if (toggleInFlight()) return;       // existing
  setToggleInFlight(true);
  // ... rest unchanged
}
```

One additional line. No downside. Mirrors the architect's existing defense-in-depth stance. **Not a blocker** — happy to APPROVE without it. Flagging for completeness because tech-lead asked specifically about "any call path" bypass.

#### 4. M2 fix is complete; no comparator math regression ✓

`performance.now()` returns a `DOMHighResTimeStamp` (positive double) since `performance.timeOrigin` (page load). Properties verified:

- **Monotonic** (per W3C spec): cannot decrease, immune to NTP / DST / manual clock changes.
- **Always positive** after the first JS turn (zero is theoretically possible only at the exact time origin, unreachable from script).
- **No subnormal floats**: returns ms-scale values; even at the first millisecond, returns ~1.0 — well above subnormal range.
- **No integer overflow**: Number.MAX_SAFE_INTEGER (2^53) supports `performance.now()` for ~285 millennia. Subtraction stays in safe-integer range.
- **Sufficient resolution**: typical 100µs in modern Chromium (better than `Date.now()`'s 1ms). True ties in the comparator are rarer.

Comparator `tsFor(b) - tsFor(a)`:
- Both operands are non-negative finite floats (or `0` for unset).
- Subtraction returns a finite double. Sign drives the sort order correctly.
- `0 - performance.now() < 0` → unset coord sorts below real-activity coord. Matches §8.4's table.
- §18.3 confirms no pre-existing consumer of `lastActivityBySessionId` is affected (brand-new field, zero references outside the plan).

#### 5. Path 5.5 and Path 6.5 actually test the intended behavior ✓ (with a small caveat)

**Path 5.5 (H2 regression):** ✓
- 16a directs the user to "Enter on focused button × 5" or rapid mouse clicks. The Enter-spam variant reliably lands all 5 within the round-trip window (typical hand-eye loop ~50ms; round-trip ~10-50ms).
- 16b expects the button to visually disable during the in-flight window — exactly what the `disabled` attribute provides.
- 16c asserts `coordSortByActivity: true` post-burst.

**Small caveat on the 16c assertion's robustness:** the test starts with toggle OFF and asserts ON after the burst. This works for ANY odd number of accepted clicks (1, 3, or 5). If only click 1 lands (subsequent clicks within in-flight window): 1 toggle = ON. ✓. If all 5 land independently (clicks spaced wider than round-trip): 5 toggles = ON. ✓. If 2 or 4 land: OFF — TEST FAILS.

In practice, "Enter ×5 in rapid succession" lands all within ~50ms (hand-eye), so all-but-one fall inside click 1's round-trip window. The test reliably passes for the intended Enter-spam mode. For mouse clicks at lower frequency, the test could give a false-positive ON via 5 separate toggles — but the user would see 5 visible flips, which is also a meaningful signal. The test is sound; the wording "matches the FIRST click that landed; subsequent in-flight-window clicks were correctly serialized away" should be read as the EXPECTED outcome under the recommended Enter-spam method.

**Path 6.5 (M1 regression):** ✓
- 19a sets persisted=true, force-quits, relaunches, mash-clicks during window appearance. Suggests breakpoint at App.tsx:69 in dev for reliable reproduction.
- 19b expects clicks during the disabled-pre-hydration window to be suppressed at DOM level (no handler invocation, no `.active` class change, no log line).
- 19c asserts persisted=true unchanged after force-quit-without-post-hydration-click.

The test correctly verifies that pre-hydration clicks do not reach `update_settings`. If the `disabled` attribute is removed in a future regression, step 19c would fail (memory's optimistic flip + click's `update_settings(false)` would land, persisted=false).

Both paths would catch a regression of their respective fixes.

#### 6. LOW findings still acceptable; no unexpected interaction with the new `toggleInFlight` ✓

Re-evaluated all seven LOW findings against the round-2 changes:

| ID | Was | Round-2 effect | Disposition |
|---|---|---|---|
| L1 | Map grows unboundedly | Unchanged. `markActivity` still spreads the whole map. | Accept. |
| L2 | O(NM log N) sort | Unchanged. | Accept. |
| L3 | `findSessionByName` collisions | Unchanged. | Accept. |
| L4 | Optional Rust test brittleness | Unchanged. | Accept. |
| L5 | `markActivity` orphans | Unchanged. | Accept. |
| L6 | `?? 0` ambiguity | `performance.now()` is always > 0 in practice (zero only at unreachable time origin), so the "real activity vs unset" distinction is preserved. The `?? 0` semantics still works. | Accept. |
| L7 | `void settingsStore.refresh()` swallows errors | Verified non-interaction with `toggleInFlight`. The `void` is fire-and-forget, evaluates synchronously; `try` block completes; `finally` runs `setToggleInFlight(false)` BEFORE the refresh promise rejects. No leak path. If refresh's promise rejects later, it triggers an unhandledrejection event but does not affect the in-flight signal. | Accept. |

No unexpected interaction. The new `toggleInFlight` does not amplify any LOW finding.

### Attack vector verdicts (revised for round 2)

| # | Vector | Round 1 | Round 2 | Notes |
|---|---|---|---|---|
| 1 | Race conditions | ✗ | ✓ | H2 closed via in-flight serialization. M1 closed via DOM-disabled + fused flip. |
| 2 | Edge cases | ✗ | ✓ | M2 closed via `performance.now()`. Click-during-hydration was M1, now closed. |
| 3 | Resource leaks | ✓ | ✓ | Unchanged. |
| 4 | Logic errors | ✓ | ✓ | Unchanged; comparator math still safe with `performance.now()`. |
| 5 | Persistence | ✗ | ✓ | Single-window TOCTOU closed by H2 fix. Cross-window TOCTOU is pre-existing v1-acceptable per locked decisions; the toggle no longer exercises it more aggressively than other settings fields. |
| 6 | TS strictness | ✗ | ✓ | H1 closed via Phase A widening. Spread compatibility unchanged. |
| 7 | Anything else | (info) | (info) | Programmatic-bypass of M1 noted as non-blocking nice-to-have. CSS narrow-width / a11y / tooltip asymmetry all unchanged. |

All seven attack vectors now ✓ or info-only. The plan is ready for implementation.

### Final recommendation

**APPROVE → Step 6 (implementation).**

- dev-rust takes Phase A (Rust + shared types + sessions store) in one self-contained commit. Verify `cargo check` AND `npm run typecheck` both pass before opening the Phase A PR.
- dev-webpage-ui takes Phase B (App.tsx + ActionBar + ProjectPanel + CSS) once Phase A is on `feature/86-coord-sort-by-activity-toggle`. Run §11's full testing checklist including the round-2 regression paths 5.5 and 6.5.

Optional follow-up (architect's discretion): add `if (!state.hydrated) return;` at the top of `toggleCoordSortByActivity` for symmetric defense-in-depth with the existing `toggleInFlight` guard. One line, no downside.

I will re-review the implementation per Step 7 of the workflow.
