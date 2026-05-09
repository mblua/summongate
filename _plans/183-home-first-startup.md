# Plan: Show Home first on every app startup (#183)

**Branch:** `fix/183-home-first-startup`
**Issue:** #183 — Show Home first on every app startup
**Supersedes:** commit `c796649` (Grinch-rejected attempt)
**Status:** READY_FOR_IMPLEMENTATION

---

## Requirement

- Every app open must first show **Home** in the main terminal pane, regardless of restored or persisted active sessions.
- Once the user **explicitly** selects/switches/creates/opens a session (or re-attaches a detached one), Home auto-hides and the terminal/session view takes over.
- After Home auto-hides, it is restored only manually via the sidebar Home button (current `homeStore.toggle()` wiring).
- The destroy-of-last-session auto-show rule (#164) is preserved.
- Detached / locked terminal windows are unaffected (they never render Home — `terminal/App.tsx:52` already gates on `props.embedded && !props.detached && !props.lockedSessionId`).
- No timers, no polling, no `await Promise.resolve()` heuristics.

---

## Why the previous attempt (commit `c796649`) failed

`c796649` made Home visible at startup but then hid it again from event listeners that fire during restore and from auto-promotions:

1. `onSessionCreated → homeStore.hide()` fires during restore for **every** restored session (`src-tauri/src/lib.rs:619` deferred path; `src-tauri/src/commands/session.rs:671` for the normal restore path through `create_session_inner`). Home is hidden before the user does anything.
2. `onSessionSwitched(({id}) => if (id) hide())` fires at the **end** of restore (`src-tauri/src/lib.rs:739, 746, 754`) when the previously-active session is re-promoted. This is the dominant case.
3. The same `onSessionSwitched` listener also hides Home on **automatic** sibling promotion after destroy (`commands/session.rs:898, 904, 910`), after detach (`commands/window.rs:120, 126, 134`), and during restart side-effects (`commands/session.rs:1028`). Those are not user "go look at this session" intents — yet they tear Home away.

The fix has to make the frontend distinguish **user intent** from **automatic backend bookkeeping**.

---

## Design Summary

**Mechanism:** the backend tags the three `session_switched` emit sites that are unambiguously user-initiated with `userInitiated: true`. Every other emit site (restore, auto-promote, detach-sibling, error-recovery, web mirror) leaves the field absent, which the frontend treats as `false`. The frontend Home listener hides Home only when `userInitiated === true`. The `onSessionCreated → hide` listener is removed entirely; user-driven creates either follow with an explicit `SessionAPI.switch(...)` (already true for ProjectPanel and RootAgentBanner) or get an imperative `homeStore.hide()` at the click site (4 small sites enumerated below).

**Why this approach (vs. alternatives):**

| Approach | Verdict | Reason |
|---|---|---|
| Backend `userInitiated` tag on `session_switched` (chosen) | ✅ | Backend is the only place that knows the difference between restore/auto-promote and user click. Cross-window cases (detached → attach) work without a separate event because the backend emits the tag. Tag is additive — missing field defaults to false, no shape-break for existing consumers. |
| Wrap `session_created` payload `{ session, userInitiated }` | ❌ | Breaks two existing listeners that destructure the payload as `Session` directly: `terminal/App.tsx:165` (`onSessionCreated((session) => terminalStore.setActiveSession(session.id, session.name, …))`) and `sidebar/App.tsx:157` (`onSessionCreated((session) => sessionsStore.addSession(session))`). Forces a wider migration than #183 needs. |
| Restore-complete signal alone | ❌ | Fixes restore but **not** the Grinch finding #3 (auto-promote after destroy/restart still hides Home if the user has Home open later in the session). |
| Frontend-imperative hide at every user call site, drop both listeners | ❌ | Cannot cover the cross-window case where the **detached** terminal window calls `WindowAPI.attach(sessionId)` (`terminal/App.tsx:118`) — `homeStore` is per-window, so calling `homeStore.hide()` in the detached bundle does nothing for the main window's Home. Would need a custom cross-window event anyway → equivalent to the chosen approach but uglier. |
| `reason: enum` payload | ❌ | More descriptive but the frontend only needs the binary distinction. Boolean keeps the contract minimal. |

---

## Event Contract (additive — non-breaking)

`session_switched` payload extension:

```ts
// before
{ id: string | null }

// after — userInitiated is OPTIONAL
{ id: string | null; userInitiated?: boolean }
```

Rules:

- `userInitiated: true` ⇔ the emit originates from a Tauri command invoked by an explicit user gesture: `switch_session`, `restart_session` (the trailing post-restart switch), `attach_terminal`.
- Field absent (or `false`) ⇔ restore loop, auto-promote after destroy, detach sibling-switch, error-recovery destroy, any other backend-internal bookkeeping.
- Frontend treats absent as `false` — listeners must not hide Home unless the field is the literal boolean `true`.

`session_created` payload is **unchanged** (still serialized `SessionInfo`). The Home-side responsibility is moved off `onSessionCreated` entirely.

No new events.

---

## Backend Changes (Rust)

### B1. `src-tauri/src/commands/session.rs:1027-1030` — `restart_session` post-restart switch (USER intent)

Current:
```rust
let _ = app.emit(
    "session_switched",
    serde_json::json!({ "id": session_info.id }),
);
```

Replace with:
```rust
let _ = app.emit(
    "session_switched",
    serde_json::json!({ "id": session_info.id, "userInitiated": true }),
);
```

### B2. `src-tauri/src/commands/session.rs:1104` — `switch_session` command (USER intent)

Current:
```rust
let _ = app.emit("session_switched", serde_json::json!({ "id": id }));
```

Replace with:
```rust
let _ = app.emit(
    "session_switched",
    serde_json::json!({ "id": id, "userInitiated": true }),
);
```

### B3. `src-tauri/src/commands/window.rs:246` — `attach_terminal` (USER intent)

Current:
```rust
let _ = app.emit("session_switched", serde_json::json!({ "id": session_id }));
```

Replace with:
```rust
let _ = app.emit(
    "session_switched",
    serde_json::json!({ "id": session_id, "userInitiated": true }),
);
```

### B4. (Optional, web parity) `src-tauri/src/web/commands.rs:165-170` — web `switch_session`

Current:
```rust
broadcast_all(
    &state.app_handle,
    &state.broadcaster,
    "session_switched",
    &json!({ "id": id }),
);
```

Replace with:
```rust
broadcast_all(
    &state.app_handle,
    &state.broadcaster,
    "session_switched",
    &json!({ "id": id, "userInitiated": true }),
);
```

This is cosmetic for #183 (web bundle does not render Home). Including it keeps the wire contract uniform; **safe to skip if it complicates review** — the web `destroy_session` auto-promote at `web/commands.rs:133, 137` correctly stays untagged.

### B5. Sites that MUST be left untagged (regression guard — do NOT modify)

These all stay as `{ "id": ... }` (no `userInitiated` field). Tagging any of them reintroduces Grinch finding #3.

| Site | Reason it is automatic, not user intent |
|---|---|
| `src-tauri/src/lib.rs:739, 746, 754` | Restore-loop end (`session_switched` for the previously-active session). |
| `src-tauri/src/commands/session.rs:510` | `create_session_inner` materialize-context error path: backend auto-promotes after destroying a half-built session. |
| `src-tauri/src/commands/session.rs:898, 904, 910` | `destroy_session_inner` sibling-promote (with detached-aware fallback). |
| `src-tauri/src/commands/window.rs:120, 126, 134` | `detach_terminal_inner` sibling-switch when the detached session was the active one. |
| `src-tauri/src/web/commands.rs:133, 137` | Web mirror of `destroy_session_inner` auto-promote. |

---

## Frontend Changes (TypeScript)

### F1. `src/shared/ipc.ts:172-176` — extend `onSessionSwitched` callback type

Current:
```ts
export function onSessionSwitched(
  callback: (data: { id: string | null }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string | null }>("session_switched", callback);
}
```

Replace with:
```ts
export function onSessionSwitched(
  callback: (data: { id: string | null; userInitiated?: boolean }) => void
): Promise<UnlistenFn> {
  return transport.listen<{ id: string | null; userInitiated?: boolean }>(
    "session_switched",
    callback
  );
}
```

This is purely additive. The two other consumers (`terminal/App.tsx:144`, `sidebar/App.tsx:200`) destructure only `{ id }` and continue to compile/run unchanged — no edits needed there.

### F2. `src/main/App.tsx` — replace the Home auto-hide listeners

**F2.a — Remove the `onSessionCreated → hide` listener entirely** (lines 173-178 in current `c796649` HEAD):

Delete:
```ts
// Auto-hide Home when a session is created (user wants the new session).
unlisteners.push(
  await onSessionCreated(() => {
    homeStore.hide();
  })
);
```

Rationale: `session_created` now fires for restored sessions and for backend-driven creations (mailbox / telegram delivery, web). Hiding on this event is exactly what broke #183. User-driven creates either (a) follow with `SessionAPI.switch(newSession.id)` and so are covered by the listener in F2.b, or (b) get an imperative `homeStore.hide()` at the click site (see F3).

After this deletion, also remove `onSessionCreated` from the import list at `src/main/App.tsx:4`:

```ts
import { SettingsAPI, SessionAPI, onSessionCreated, onSessionDestroyed, onSessionSwitched } from "../shared/ipc";
```

becomes

```ts
import { SettingsAPI, SessionAPI, onSessionDestroyed, onSessionSwitched } from "../shared/ipc";
```

**F2.b — Gate the `onSessionSwitched` listener on `userInitiated`** (lines 180-190 in current HEAD):

Replace:
```ts
// Auto-hide Home when the user switches to an existing session (issue
// #183). Backend can emit `session_switched` with id=null when no session
// is active (see commands/window.rs, commands/session.rs); in that case
// keep Home visible.
unlisteners.push(
  await onSessionSwitched(({ id }) => {
    if (id) {
      homeStore.hide();
    }
  })
);
```

With:
```ts
// Auto-hide Home only when the backend marks the switch as user-initiated
// (issue #183). Restore, destroy auto-promote, detach sibling-switch and
// other automatic bookkeeping emit `session_switched` WITHOUT
// `userInitiated`, so they leave Home visible. See _plans/183-home-first-startup.md.
unlisteners.push(
  await onSessionSwitched(({ id, userInitiated }) => {
    if (id && userInitiated === true) {
      homeStore.hide();
    }
  })
);
```

**F2.c — Keep `homeStore.show()` at startup** (line 171, the unconditional Home-on-boot from `c796649`). Unchanged.

**F2.d — Keep the `onSessionDestroyed → show if last` listener** (lines 196-208). Unchanged. Issue #164 contract preserved.

### F3. Imperative `homeStore.hide()` at user-create call sites that do NOT follow with a switch

After F2.a, a user that creates a brand-new session via a flow that does not call `SessionAPI.switch` afterwards would leave Home visible while a fresh session takes over. Fix at the four such call sites by adding an imperative `homeStore.hide()` immediately before `SessionAPI.create(...)` (or `SessionAPI.createRootAgent`).

For each site below, add the import if it is not already present:
```ts
import { homeStore } from "../../main/stores/home";
```
(Adjust the relative path to match each file's location: from `src/sidebar/components/*` and `src/terminal/App.tsx` it is `../../main/stores/home`; from `src/shared/shortcuts.ts` it is `../main/stores/home`.)

**F3.a — `src/sidebar/components/OpenAgentModal.tsx:91-118` (`launchAgent`):**

Insert `homeStore.hide();` directly above `SessionAPI.create({ ... });` at line 109:
```ts
const launchAgent = (repo: RepoMatch, agent: AgentConfig) => {
  // ...existing parsing of executable / cmdArgs / shell / shellArgs...

  homeStore.hide();
  SessionAPI.create({
    shell,
    shellArgs,
    cwd: repo.path,
    sessionName: repo.name,
    agentId: agent.id,
  });

  props.onClose();
};
```

**F3.b — `src/sidebar/components/NewAgentModal.tsx:103` (the `SessionAPI.create({...})` call inside the agent-launch handler):**

Insert `homeStore.hide();` directly above `SessionAPI.create({ ... });` at line 103:
```ts
homeStore.hide();
SessionAPI.create({
  shell,
  shellArgs,
  cwd: createdPath(),
  sessionName,
  agentId: agent.id,
});
```

**F3.c — `src/sidebar/components/AcDiscoveryPanel.tsx` — three call sites:**

Lines 47-57 (`handleAgentClick`):
```ts
const handleAgentClick = (agent: AcAgentMatrix) => {
  if (!agent.preferredAgentId) {
    setPendingLaunch({ path: agent.path, sessionName: agent.name, gitRepos: [] });
    return;
  }
  homeStore.hide();
  SessionAPI.create({
    cwd: agent.path,
    sessionName: agent.name,
    agentId: agent.preferredAgentId,
  });
};
```

Lines 59-77 (`handleReplicaClick`) — insert before line 71:
```ts
homeStore.hide();
SessionAPI.create({
  cwd: replica.path,
  sessionName: `${wg.name}/${replica.name}`,
  agentId: replica.preferredAgentId,
  gitRepos,
});
```

Lines 375-383 (`pendingLaunch` modal `onSelect`):
```ts
onSelect={(agent) => {
  const pending = pendingLaunch()!;
  homeStore.hide();
  SessionAPI.create({
    cwd: pending.path,
    sessionName: pending.sessionName,
    agentId: agent.id,
    gitRepos: pending.gitRepos,
  });
  setPendingLaunch(null);
}}
```

**F3.d — `src/terminal/App.tsx:258-265` (the empty-state "+ New Session" button):**

Replace:
```tsx
<button
  class="terminal-empty-btn"
  onClick={() => SessionAPI.create()}
>
  + New Session
</button>
```

With:
```tsx
<button
  class="terminal-empty-btn"
  onClick={() => { homeStore.hide(); SessionAPI.create(); }}
>
  + New Session
</button>
```

Note: the import for `homeStore` already exists in this file (`src/terminal/App.tsx:19`). No import edit required.

**F3.e — `src/shared/shortcuts.ts:16` (the keyboard shortcut handler that creates a session):**

Replace:
```ts
handler: () => SessionAPI.create(),
```

With:
```ts
handler: () => { homeStore.hide(); SessionAPI.create(); },
```

Add import at the top of `src/shared/shortcuts.ts`:
```ts
import { homeStore } from "../main/stores/home";
```

### F4. Sites that need NO change (already covered by F2.b through `SessionAPI.switch`)

These call `SessionAPI.switch(...)` (or `SessionAPI.restart(...)` which internally emits the post-restart `session_switched` with `userInitiated: true` from B1) and so will hide Home automatically once F2.b ships. Listed for review confidence; do **not** add `homeStore.hide()` here:

- `src/sidebar/components/SessionItem.tsx:88` — `await SessionAPI.switch(props.session.id)` in `handleClick`. ✓
- `src/sidebar/components/SessionItem.tsx:217` — `await SessionAPI.restart(...)`. ✓
- `src/sidebar/components/RootAgentBanner.tsx:12-13` — `createRootAgent()` then `switch(session.id)`. The trailing `switch` covers Home. ✓
- `src/sidebar/components/ProjectPanel.tsx:109` (restart), `:119` (switch), `:143-149` (create + switch), `:163` (switch), `:180-185` (create + switch), `:338` (restart), `:1611-1617` (create + switch). All trailing-switch / restart paths. ✓
- `src/sidebar/components/AcDiscoveryPanel.tsx:350` — `await SessionAPI.restart(session.id)`. ✓
- `src/terminal/components/Titlebar.tsx:36`, `src/terminal/App.tsx:118` — `WindowAPI.attach(...)` from inside the **detached** window. Emits `session_switched` with `userInitiated: true` (B3). Main window's listener in F2.b hides Home. ✓ (cross-window — see "Risks" §R1.)
- `src/sidebar/components/SessionItem.tsx:127, 141` — `WindowAPI.attach(...)` from sidebar. Same as above. ✓
- `src/sidebar/components/ProjectPanel.tsx:349, 518` — `WindowAPI.attach(...)`. Same. ✓
- `WindowAPI.detach(...)` call sites: `detach` does NOT mark its sibling-switch as user-initiated (B5 — `commands/window.rs:120, 126, 134` stay untagged). Home stays visible if the user happens to be on Home and detaches. This is the desired behavior — detach is "move this session out of main", not "go look at the surviving sibling".
- `WindowAPI.ensureTerminal()` / `focusMain()` — these do not emit `session_switched`. No effect on Home.

---

## Dependencies / Imports

- **Rust:** none new. `serde_json::json!` already in scope at every modified emit site.
- **TypeScript:** `homeStore` import added in:
  - `src/sidebar/components/OpenAgentModal.tsx`
  - `src/sidebar/components/NewAgentModal.tsx`
  - `src/sidebar/components/AcDiscoveryPanel.tsx`
  - `src/shared/shortcuts.ts`

  Each adds a single line: `import { homeStore } from "../../main/stores/home";` (or `../main/stores/home` from `src/shared/`). Already imported in `src/main/App.tsx` and `src/terminal/App.tsx`.

- **Config:** none. `tauri.conf.json` does not need a schema change.

- **Version bump:** bump `src-tauri/tauri.conf.json` `version` from `0.8.14` to `0.8.15` so the user can visually confirm the build (per repo convention).

---

## Tests / Verification

### T1. Existing tests that MUST still pass unchanged

- `src/main/stores/home.test.ts` — pure unit tests of `homeStore` API surface. No change to the store; all eight cases stay green.

### T2. New unit tests (recommended, gating)

Add a new file `src/main/App.test.tsx` (or `src/main/listeners.test.ts` if extracting the listener wiring is preferred — see Risks §R3) that mocks the IPC listeners and asserts:

1. `homeStore.show()` is called exactly once on `MainApp.onMount`.
2. Firing a mocked `session_switched` event with `{ id: "abc", userInitiated: true }` calls `homeStore.hide()`.
3. Firing `session_switched` with `{ id: "abc" }` (no `userInitiated`) does NOT call `homeStore.hide()`.
4. Firing `session_switched` with `{ id: "abc", userInitiated: false }` does NOT call `homeStore.hide()`.
5. Firing `session_switched` with `{ id: null, userInitiated: true }` does NOT call `homeStore.hide()` (id-null guard preserved).
6. Firing `session_created` with any payload does NOT call `homeStore.hide()` (the listener was removed).
7. Firing `session_destroyed` after `SessionAPI.list` resolves to `[]` calls `homeStore.show()` (regression guard for #164 auto-show).

The IPC layer is already mockable per the pattern in `home.test.ts` (vitest `vi.mock("../../shared/ipc", …)`). Wire `onSessionCreated`, `onSessionDestroyed`, `onSessionSwitched` as `vi.fn()` returning a mock unsubscriber, then invoke their captured callbacks with crafted payloads.

### T3. Manual test matrix (must run on Windows after build, version 0.8.15)

| Scenario | Expected |
|---|---|
| Cold start with **zero** persisted sessions | Home visible. Sidebar empty. |
| Cold start with **one** persisted attached session that was active at last close | Home visible. Sidebar shows the session in non-active state visually OR sidebar's `setActiveId` reflects backend, but Home overlay is up. Clicking the session in the sidebar → Home hides, terminal shows. |
| Cold start with **multiple** persisted sessions, one previously active | Home visible. The previously-active session is set as backend-active (sidebar shows it active) but Home overlays it. Clicking any session → Home hides. |
| Cold start with **all** persisted sessions detached | Home visible. Backend emits `session_switched` with id=null (lib.rs:744-748) — id-null guard keeps Home visible regardless. |
| Cold start with `start_only_coordinators=true` and deferred non-coordinator restores | Home visible (the `session_created` for deferred sessions emitted at `lib.rs:619` no longer hides Home). |
| After Home is up, click a session in sidebar | Home hides. Terminal shows. |
| After Home is up, double-click a sidebar session → OpenAgentModal → pick agent | Home hides. New session takes over. |
| After Home is up, RootAgentBanner click | Home hides. Root agent session takes over. |
| After Home is up, hit the keyboard shortcut for new session | Home hides. New session takes over. |
| After Home is up, click "+ New Session" in the empty terminal pane | Home hides. New session takes over. |
| After session use, destroy the active session via context menu, with at least one other session remaining | Home stays in whatever state it was in (likely hidden — automatic sibling promotion does not change visibility). |
| After session use, destroy the LAST session | Home shows (#164 auto-show preserved). |
| After session use, restart the active session via context menu | Home stays hidden (restart's post-switch is `userInitiated: true`, but Home was already hidden — net: no visible change). If the user had Home open at the time of restart, Home WILL hide because restart is user intent — acceptable per "Hide Home only for user-driven … restart side-effects unless clearly justified" (restart from a context menu IS user intent; it's the auto-promote-on-destroy that must not hide). |
| Detach a session while Home is open | Home stays open. Detached window appears. (`detach_terminal_inner`'s sibling-switch is untagged.) |
| Re-attach a detached session via the sidebar's detach toggle | Home hides (`attach_terminal` emit is `userInitiated: true`). |
| Re-attach a detached session by closing its window (X button → onCloseRequested handler at `terminal/App.tsx:115-123` calls `WindowAPI.attach`) | Home hides. Cross-window: detached bundle's call → backend `attach_terminal` → main window's listener fires with `userInitiated: true`. |
| Mailbox/telegram delivery spawns a backend session while Home is open | Home stays open. New session appears in sidebar. (`session_created` listener removed.) |

### T4. CLI sanity

`pnpm test` (vitest) and `cargo test --manifest-path src-tauri/Cargo.toml` should be green. `pnpm tsc --noEmit` should pass — F1's optional field is the only type widening.

---

## Risks & Mitigations

### R1. Cross-window emit on detached → attach

`attach_terminal` is a Tauri command invoked from the **detached** terminal window's onCloseRequested handler (`src/terminal/App.tsx:118`). The backend's `app.emit(...)` from B3 broadcasts the event to all WebviewWindows, including main, where F2.b's listener will hide Home. Verified: `tauri::AppHandle::emit` is window-agnostic; only `WebviewWindow::emit` would target a single window. **No code change required for this to work**, but reviewer should confirm by reading `commands/window.rs:242-246` in context (the `let _ = app.emit(...)` chain right before `Ok(())`).

### R2. Web bundle (out of scope, but listed for completeness)

`src-tauri/src/web/commands.rs` mirrors `switch_session` / `destroy_session` for the web remote and uses `broadcast_all`, which broadcasts via `WsBroadcaster` to web clients **and** through Tauri's emitter to the desktop windows. If B4 is applied, a web user clicking switch will hide Home in the desktop main window — this is the consistent and correct behavior. If B4 is skipped, the web switch does not hide Home in the desktop main window, only in the web client; acceptable for #183 since web is a separate UI surface. **Recommendation: apply B4.**

### R3. Listener-wiring testability

`MainApp.onMount` is a long async function with several side effects (zoom, geometry, settings load, multiple listeners, window close handler). Adding a test for the Home listeners means either (a) testing the whole `onMount` with most things mocked, or (b) extracting the three Home-related `unlisteners.push(await onXxx(…))` blocks into a small helper like `wireHomeListeners(homeStore, sessionApiList): UnlistenFn[]` and unit-testing that helper. Option (b) is cleaner and isolates the contract; option (a) is acceptable if the dev wants to keep blast radius minimal. **Recommendation: option (b)** — file `src/main/listeners-home.ts`, single exported function `wireHomeListeners()`, unit test under `src/main/listeners-home.test.ts`. If the dev prefers (a), do not block.

### R4. Frontend listener payload shape (defensive)

`onSessionSwitched` callback now reads `userInitiated` off a payload that historical emits do not include. JS destructure of a missing field yields `undefined`, and `undefined === true` is `false`, so `id && userInitiated === true` is safe. The strict `=== true` (not just truthy) is intentional — protects against future emits that mis-marshal the field as a string `"true"` from some external bridge.

### R5. Restore happens AFTER frontend listener registration (timing)

Confirmed from the code: `MainApp.onMount` registers listeners synchronously inside the async `onMount` call; the restore loop is detached via `tauri::async_runtime::spawn(async move { ... })` from `setup()` and runs concurrently. Frontend may register before, during, or after restore. Either way: restore emits without `userInitiated`, so listeners ignore them. The previous attempt's race (listener registered before restore-end emit → got hit) is fixed not by reordering but by making the events themselves un-actionable for restore.

### R6. `session_created` consumers other than Home

After F2.a, `session_created` is consumed by `terminal/App.tsx:165` (sets active session in terminalStore if none) and `sidebar/App.tsx:157` (adds session to sessionsStore). Neither of these touches Home. Both keep working unchanged. Verified by grep — no other listener exists.

### R7. Coordinator-task auto-spawned sessions

If a future change spawns a session from a coordinator background task and routes through `create_session_inner`, it will fire `session_created` and (if it's the only session) the manager may auto-activate it without emitting `session_switched`. Home stays visible — acceptable. If the future change wants Home to hide (e.g. it represents a "user-equivalent" action), the contract is to either (a) follow with `app.emit("session_switched", json!({"id": new_id, "userInitiated": true}))` or (b) make the call from the frontend so the user-call-site rules apply. Document this in the comment block above the listener in F2.b.

---

## Things the dev MUST NOT do

1. Do **not** add `userInitiated: true` to any `session_switched` emit at the sites listed in **B5** (the regression guard table). Doing so reintroduces Grinch finding #2 or #3 verbatim.
2. Do **not** wrap the `session_created` payload (do not change its shape from `SessionInfo` to `{ session: SessionInfo, userInitiated: bool }`). Two existing listeners depend on the current shape.
3. Do **not** introduce timers, `setTimeout`, polling, or `await Promise.resolve()` heuristics in the Home visibility path. The fix is event-shape based, not time-based.
4. Do **not** change `homeStore`'s API surface (`setInitialVisibility`, `show`, `hide`, `toggle`, `fetch`, `refresh`). The fix lives entirely in App.tsx wiring + a few user-call-site `homeStore.hide()` invocations.
5. Do **not** delete the `onSessionDestroyed → show if last` listener — that is #164's contract, separate from #183.
6. Do **not** modify detached-window behavior. Detached/locked terminal windows already gate Home rendering at `terminal/App.tsx:52`.
7. Do **not** skip the `tauri.conf.json` version bump. Per repo convention the user needs a visible build identifier to confirm they are running the new binary (not a stale instance).
8. Do **not** mark `WindowAPI.detach`-driven `session_switched` as user-initiated. Detach is "move this session out", not "go look at the sibling". The sibling-switch is consequential, not the user's intent.
9. Do **not** rely on `app.emit_to(...)` or window-targeted emits for the new tag. The cross-window detach→attach case (R1) requires broadcast.

---

## Build sequence

1. **B1, B2, B3** — three Rust one-line edits in `commands/session.rs` and `commands/window.rs`. `cargo check --manifest-path src-tauri/Cargo.toml`.
2. **B4** (optional) — one Rust one-line edit in `web/commands.rs`.
3. **F1** — extend `onSessionSwitched` type in `src/shared/ipc.ts`. `pnpm tsc --noEmit`.
4. **F2.a, F2.b, F2.c, F2.d** — edits in `src/main/App.tsx` (delete one listener block, modify another, drop one import).
5. **F3.a–F3.e** — five small edits adding `homeStore.hide()` and (where needed) one import line per file.
6. **R3 (recommended)** — extract Home listener wiring into `src/main/listeners-home.ts`; add `src/main/listeners-home.test.ts`.
7. **Version bump** — `src-tauri/tauri.conf.json` → `0.8.15`.
8. `pnpm test`, `pnpm tsc --noEmit`, `cargo test --manifest-path src-tauri/Cargo.toml`. Build, run, walk the **T3 manual matrix** end-to-end.

---

## Verdict

**READY_FOR_IMPLEMENTATION**
