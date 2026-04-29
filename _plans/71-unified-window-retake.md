# Plan: Issue #71 Unified Window Retake

## Scope

Issue #71 is open: "Merge Sidebar and Terminal into a single unified window with per-session detach".

Target branch under review:

- `origin/feature/71-unified-window-with-detach`

Base facts already verified by tech-lead:

- Merge-base with `origin/main`: `30a35aa8439041b9664e08e720d0c5fdcb86f268`.
- `origin/main` is 23 commits ahead of the merge-base.
- The feature branch is 8 commits ahead of the merge-base.
- A `merge-tree` preview reports conflicts in:
  - `src-tauri/Cargo.lock`
  - `src-tauri/Cargo.toml`
  - `src-tauri/src/commands/window.rs`
  - `src-tauri/tauri.conf.json`
  - `src/sidebar/App.tsx`

This plan is for preparing the feature branch for test, not for landing it. Do not merge anything into the default branch. Do not push any updated feature branch unless the user or tech-lead later authorizes that feature-branch update.

## Current Status

The branch contains a substantial implementation of the unified-window model:

- New unified main-window frontend entrypoint in `src/main/App.tsx`.
- New quit-confirm modal and main-window CSS.
- Frontend routing updates in `src/main.tsx`.
- Sidebar changes for detach/attach controls and detached-state display.
- Terminal changes for locked detached windows, titlebar behavior, and terminal view integration.
- Backend window/session changes in `commands/window.rs`, `commands/session.rs`, session persistence, settings, web commands, and Tauri configuration.
- Version/config/dependency updates in `package.json`, `Cargo.toml`, `Cargo.lock`, and `tauri.conf.json`.
- Existing branch plan: `_plans/feature-unified-window-with-detach.md`.

The branch is no longer close to current `origin/main`. Current `origin/main` has moved through several fixes and feature changes that touch the same backend command/config surface, especially:

- `src-tauri/src/commands/window.rs`
- `src-tauri/src/commands/session.rs`
- `src-tauri/src/config/sessions_persistence.rs`
- `src-tauri/src/config/settings.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/session/*`
- `src-tauri/src/web/*`
- `src-tauri/tauri.conf.json`
- `src/sidebar/App.tsx`
- shared TypeScript types and sidebar stores

The safe path is a deliberate feature-branch retake onto current `origin/main`, resolving conflicts by intent rather than mechanically accepting either side.

## Update Strategy

1. Work only on the feature branch, after explicit authorization from tech-lead/user.
2. Start from current `origin/main` as the integration target for the feature branch.
3. Prefer replaying the unified-window commits onto current `origin/main` over merging current `origin/main` into the branch, because the branch is a focused feature and the conflict set is architectural.
4. Keep the issue-linked branch name pattern:
   - `feature/71-unified-window-with-detach`
5. Preserve current `origin/main` behavior unless the unified-window feature explicitly replaces it.
6. Do not land, merge, or push to the default branch as part of this work.
7. Do not push the updated feature branch unless tech-lead/user explicitly authorizes that later.

Suggested implementation path for the dev agent after approval:

1. Create or check out a local branch tracking `origin/feature/71-unified-window-with-detach`.
2. Capture a backup ref before rewriting or replaying anything.
3. Retake the feature branch onto current `origin/main` using a controlled rebase/cherry-pick sequence or a fresh branch from `origin/main` plus the issue #71 commits.
4. Resolve each conflict using the conflict strategy below.
5. Run the backend/frontend validation suite before reporting ready for review.

## Conflict Resolution Strategy

### `src-tauri/Cargo.toml` and `src-tauri/Cargo.lock`

Risk: dependency and package-version changes from `origin/main` may overlap with the branch's Tauri/window work and version bump.

Strategy:

- Keep all dependency additions and version constraints required by current `origin/main`.
- Add only dependencies actually required by the unified-window implementation.
- Regenerate `Cargo.lock` from the resolved `Cargo.toml`; do not hand-edit lockfile conflict blocks.
- Verify whether the branch's version bump to `0.8.0` is still intended. If current `origin/main` has its own version progression, choose a coherent version with tech-lead/user instead of silently downgrading or leapfrogging.

### `src-tauri/src/commands/window.rs`

Risk: this is the highest-risk backend conflict. It owns window labels, detach/attach commands, focus behavior, close behavior, geometry persistence, and event emission.

Strategy:

- Treat current `origin/main` command signatures, error handling style, imports, and state access patterns as the baseline.
- Preserve issue #71 behavior:
  - one persistent `main` window,
  - detached windows labelled with the existing `terminal-<session-id-without-dashes>` convention,
  - `detach_terminal`,
  - `attach_terminal`,
  - main-window focus behavior replacing the old sidebar/terminal split focus behavior,
  - events for detached/attached state changes.
- Re-check all window lifecycle paths:
  - detach active session,
  - attach detached session,
  - close detached window through its titlebar/X,
  - destroy a detached session from the sidebar,
  - quit main window while detached windows exist,
  - app restart with detached sessions persisted.
- Avoid duplicate sources of truth. The backend detached-session state and actual Tauri window list must stay consistent.
- Confirm whether `close()` triggers frontend `onCloseRequested` in Tauri v2. If it does, use the correct destructive close/destroy method when a session is intentionally destroyed so it does not accidentally reattach.

### `src-tauri/tauri.conf.json`

Risk: config changes from current `origin/main` may include app identity, windows, permissions, bundle, or release settings.

Strategy:

- Start with current `origin/main` config.
- Remove the old two-window startup model only where the unified-window architecture requires it.
- Add only the `main` window/default URL behavior required by issue #71.
- Preserve all unrelated current config from `origin/main`, especially release, permissions, security, bundle, and plugin settings.
- Verify that detached windows are created programmatically and do not require static config entries unless Tauri requires capabilities/permissions to be declared.

### `src/sidebar/App.tsx`

Risk: current `origin/main` likely changed sidebar state, initialization, shortcuts, or coordinator/session behavior while the feature branch changed rendering context from standalone sidebar window to embedded main pane.

Strategy:

- Preserve current `origin/main` sidebar data loading, stores, actions, shortcuts, and bug fixes.
- Add a narrow embedded-mode contract for rendering inside `MainApp`.
- Avoid duplicating event listeners when Sidebar and Terminal are mounted in the same document.
- Ensure standalone/browser/sidebar compatibility decisions are explicit:
  - `?window=main` should mount the unified app.
  - legacy `?window=sidebar` and `?window=terminal` should either redirect/fallback intentionally or remain supported for one version if needed.
- Re-test session creation, switching, filtering, context menus, coordinator status, and any current-main sidebar features after conflict resolution.

## Backend Validation

Minimum backend checks before asking for grinch review:

- `cargo fmt --check` from `src-tauri`.
- `cargo clippy --workspace --all-targets -- -D warnings` from `src-tauri`, or the repo's canonical clippy command if different.
- `cargo test --workspace` from `src-tauri`, or the repo's canonical Rust test command if different.
- Verify Tauri command registration includes all new/renamed commands:
  - `detach_terminal`
  - `attach_terminal`
  - main-window focus command
  - any retained compatibility alias, if intentionally kept.
- Verify session persistence serialization/deserialization remains backward compatible with older session files that lack `was_detached`.
- Verify no shutdown path loses session persistence when detached windows exist.
- Verify web-command behavior remains coherent if browser mode still exists.

Backend manual scenarios:

- Start app with zero sessions: main window remains usable.
- Create a session: terminal pane appears in main window.
- Detach active session: detached window opens; main pane switches to a non-detached session or empty state.
- Attach detached session: detached window closes; main pane switches to that session.
- Destroy detached session from sidebar: PTY terminates and detached window closes without reattaching.
- Quit main with detached windows: custom confirmation appears; Cancel is the default safe action; explicit Quit exits cleanly.
- Restart after detached sessions exist: restored sessions recreate expected detached/main layout, subject to PTY recovery success.

## Frontend Validation

Minimum frontend checks before asking for grinch review:

- `npm install` only if lockfiles/package metadata require it.
- `npm run typecheck`, if available.
- `npm run lint`, if available.
- `npm run build`, or the repo's canonical frontend build command.
- Launch the Tauri app from the feature branch and inspect actual windows.

Frontend manual scenarios:

- Main window displays sidebar and terminal in one frame.
- Splitter resizes without layout jumps; terminal fit/PTY resize follows the pane size.
- Sidebar width persistence survives restart.
- No visible overlap between titlebars, sidebars, terminal view, status bars, modals, and context menus.
- Detached terminal titlebar has a clear reattach affordance and does not show obsolete two-window controls.
- Sidebar session rows correctly show detached state and expose Open in new window/Re-attach actions.
- Closing a detached window reattaches rather than terminates the session.
- Main-window close confirmation:
  - Enter on default focus cancels,
  - Escape cancels,
  - Quit requires explicit click or focus movement plus Enter,
  - repeated close attempts do not stack dialogs.
- Legacy routes or old window-state recovery do not open blank windows.

## Acceptance Criteria

The branch is ready to test when all of the following are true:

- It is based on current `origin/main` behavior with issue #71 changes reapplied cleanly.
- No conflict markers remain.
- Rust formatting, linting, and tests pass or any failures are explained as pre-existing and unrelated.
- Frontend typecheck/build pass or any failures are explained as pre-existing and unrelated.
- Tauri app launches into one unified main window by default.
- The app no longer relies on separate default sidebar and terminal windows.
- Any number of sessions can be detached into individual windows.
- Detached sessions can be reattached without terminating their PTYs.
- Closing a detached window reattaches; destroying a session terminates it.
- Session switching avoids showing detached sessions in the main terminal pane unless reattached.
- Detached state persists across restart where session recovery succeeds.
- Main-window quit behavior is safe and explicit when detached windows exist.
- Browser/remote mode behavior is either preserved or intentionally documented if out of scope.
- No changes are merged to the default branch.
- No feature-branch update is pushed unless tech-lead/user separately authorizes it.

## Highest-Risk Findings

1. `src-tauri/src/commands/window.rs` is the critical conflict. It combines lifecycle, state, IPC events, and Tauri window semantics. A superficially clean conflict resolution could still break attach/detach, close-vs-destroy, or restart behavior.
2. Detached window close semantics are subtle. If frontend close interception and backend programmatic close use the same path, destroying a session may accidentally reattach it instead of closing it.
3. `origin/main` has moved across the same session/config/window surface. Losing recent fixes while replaying the feature is a realistic risk unless every conflict starts from current-main behavior.
4. `Cargo.lock` should be regenerated from the resolved manifest, not manually blended.
5. The old two-window assumptions may survive in frontend routing, settings geometry, titlebars, shortcuts, web commands, or Tauri config. These need search-based cleanup and manual app testing, not only compiler checks.
6. Quit confirmation and persistence interact with detached windows. The branch must prove that Cancel is safe, Quit is explicit, and restart preserves detached intent without corrupting session recovery.

## Team Review Additions

These findings came from backend, frontend, and adversarial review of `origin/feature/71-unified-window-with-detach` against current `origin/main`.

### Blockers to Fix Before Test Build

1. `src-tauri/capabilities/default.json` does not include the new `main` window label. Current allowed windows include `sidebar`, `terminal`, `terminal-*`, and `guide`; Tauri v2 can block IPC/window APIs from `main`. Add `main` to the capability allowlist and verify main-window IPC works.
2. Preserve issue #82 behavior in `src-tauri/src/commands/session.rs` and `src-tauri/src/web/commands.rs`. The feature branch predates the fix that suppresses provider auto-resume flags on fresh session creation. Fresh creates must keep `skip_auto_resume=true`; restore/deferred wake paths may pass `false` intentionally.
3. Preserve issue #70/#77 project-scoped coordinator restore behavior in `src-tauri/src/lib.rs`. Current `main` uses `agent_fqn_from_path` in startup restore; do not regress to `agent_name_from_path`.
4. Preserve issue #86 coordinator activity sorting across Rust settings, TS types, sidebar store, action bar, project panel, and `markActivity()` calls. The feature branch drops this current-main UI behavior unless conflicts are resolved by union.

### Backend-Specific Additions

- Merge `AppSettings` as a union: keep `coord_sort_by_activity` plus #71's `main_zoom`, `main_geometry`, `main_sidebar_width`, and `main_always_on_top`.
- Keep current-main command registrations and managed state, then add #71 commands: `attach_terminal`, `list_detached_sessions`, `set_detached_geometry`, and `focus_main_window`.
- Rework web remote commands so browser destroy/switch paths do not bypass native detached-session lifecycle. Browser destroy should use the same cleanup semantics as native session destroy, and browser switch must not make native main render a session that is detached in its own window.
- Treat `win.destroy()` failure during attach as a real error or rollback point. Do not clear detached state and switch main if the detached window could not be destroyed.
- Validate detached persistence through the lifecycle dependency: user X-close must attach, while intentional session destroy must not reattach.

### Frontend-Specific Additions

- Make sidebar width presets update the live unified layout immediately. Persisting `mainSidebarWidth` is not enough if `MainApp` only reads settings on mount.
- Bring `ProjectPanel` detach controls to parity with `SessionItem`: show attach/detach based on `sessionsStore.isDetached()` and call `WindowAPI.attach()` where appropriate.
- Preserve `coordSortByActivity` UI and store behavior from current `main`, including toolbar control, setting hydration, project panel sort behavior, and `markActivity()` on activity events.
- Keep the embedded-mode design for `SidebarApp` and `TerminalApp`, but verify it does not duplicate listeners or skip required global setup in the unified main window.
- Harden splitter dragging if practical with a document-level fallback for pointer-capture failure.
- Merge the frontend `SessionsState` by union, not replacement. Keep #71's `detachedIds` helpers and also keep current-main `coordSortByActivity`, `lastActivityBySessionId`, `hydrated`, `toggleInFlight`, `setCoordSortByActivity()`, `toggleCoordSortByActivity()`, and `markActivity()`. Current `origin/main` initializes the setting in `SidebarApp`, marks activity on idle events, persists toggles through `SettingsAPI`, and exposes the `ActionBar` flame toggle plus CSS.
- Merge the frontend `AppSettings` type by union as well. Keep current-main settings such as `sidebarZoom`, `voiceAutoExecute`, `voiceAutoExecuteDelay`, and `coordSortByActivity`, then add #71's `mainZoom`, `mainGeometry`, `mainSidebarWidth`, and `mainAlwaysOnTop`. Do not let the TS type drift from the Rust settings struct after serde camelCase conversion.
- Replace stale frontend "terminal window" semantics deliberately. Existing calls to `WindowAPI.ensureTerminal()` in `SessionItem`, `ProjectPanel`, and `RootAgentBanner` should become `WindowAPI.focusMain()` for attached sessions, while detached sessions must not be switched into the native main pane by a normal row click. Use one shared helper/contract for "activate session" so `SessionItem` and `ProjectPanel` do not each hand-roll detached-window label checks.
- Define the click behavior for detached sessions explicitly: normal row/replica click should either focus the detached window or be a no-op with clear detached state, while reattach must happen only through the attach control or an intentional attach action. Avoid calling `SessionAPI.switch()` on a detached session from the native main UI unless the backend guarantees it will not make main render that detached session.
- Sidebar width presets need a live Solid contract owned by `MainApp`, such as an optional `Titlebar` callback prop or a small shared signal/store. The current feature-branch pattern of only writing `mainSidebarWidth` via `SettingsAPI.update()` will not resize the mounted layout until remount/restart.
- Re-check global listener ownership in unified mode. `SidebarApp embedded` and `TerminalApp embedded` currently both call `registerShortcuts()`; either keep this truly ref-counted/idempotent or let `MainApp` be the single shortcut owner. Also scope document-level context-menu blocking when embedded so sidebar behavior does not unintentionally suppress terminal-pane interactions.
- Confirm `TerminalView` behavior across detach/attach: hidden pre-warmed terminals in main should receive output without stealing focus, dispose on session destroy, survive reattach with scrollback intact, and not exhaust WebGL contexts when many sessions are detached. Splitter resize, window resize, zoom changes, and reattach must all run the xterm fit -> PTY resize sequence.
- Keep route compatibility concrete: in browser/non-Tauri mode, render `BrowserApp` regardless of `?window=`; in Tauri, `?window=main`, missing window param, legacy `?window=sidebar`, and legacy non-detached `?window=terminal` should render `MainApp`; detached should support both `?window=detached&sessionId=...` and one-version legacy `?window=terminal&sessionId=...&detached=true`.
- Audit CSS layering after the merge: `main.css` must give the terminal pane `min-width: 0`, avoid nested fixed-height scroll traps, keep portals/context menus/modals above the unified titlebar and splitter, and preserve existing sidebar theme selectors such as `data-sidebar-style`.

### Must-Test Scenarios Added By Review

- Main window can invoke commands under Tauri capabilities: list sessions, create session, detach, attach, close confirmation, and geometry save.
- Native detached session cannot be activated into native main from browser remote.
- Browser remote destroying a detached session closes the native detached window and clears detached state.
- Detach multiple sessions, close main, cancel quit, then quit explicitly; restart restores only sessions that were still detached.
- Attach through detached titlebar button and through window X; no duplicate terminal remains.
- Destroy active attached session when the next available session is detached; main chooses a non-detached fallback or empty state.
- Restart with a persisted detached active session; main must not also render that session.
- Coordinator quick-access/activity sorting still works after the #71 UI merge.
- Toggle coordinator sort-by-activity in the unified main window, verify the `ActionBar` button state changes, coordinator quick-access reorders immediately, activity bumps a coordinator to the front, and the setting survives restart.
- Use both `SessionItem` and `ProjectPanel` controls to detach and reattach the same session; both surfaces must show the same icon/title/menu state before and after backend events.
- Click a detached session row/replica from the native main UI and verify the main terminal pane does not render that detached session unless the user explicitly reattaches it.
- Use sidebar width presets from the unified titlebar and verify the splitter moves immediately, the terminal refits/resizes its PTY, and the width persists after restart.
- Exercise global shortcuts in unified main, detached terminal, and browser mode. `Ctrl+Shift+N`, `Ctrl+Shift+W`, and `Ctrl+Shift+R` should fire once, not once per mounted embedded app.
- Open sidebar context menus and interact with the terminal pane in unified mode; document-level sidebar listeners must not break terminal focus, selection, paste, or xterm keyboard input.
- Verify old URL/window labels from persisted state do not produce blank webviews: legacy `sidebar`, legacy non-detached `terminal`, new `main`, new `detached`, `guide`, and browser remote paths.

## Dev-Rust Review Additions

Verified on 2026-04-28 after `git fetch origin --prune`:

- `origin/main`: `bd10a1bf4518e3463b3651e711f06bad86ead341`
- `origin/feature/71-unified-window-with-detach`: `b17589aecf5856e5b5624ced73e2e0a8b0076f15`
- merge-base remains `30a35aa8439041b9664e08e720d0c5fdcb86f268`
- `merge-tree` still reports only the five known conflicts: `Cargo.lock`, `Cargo.toml`, `commands/window.rs`, `tauri.conf.json`, and `src/sidebar/App.tsx`

Additional backend constraints before implementation:

- Retake must be commit replay/rebase/cherry-pick onto `origin/main`, not a tree/file overlay from the old feature branch. The feature branch predates large non-conflicting fixes, especially #70/#77 in `src-tauri/src/config/teams.rs`, `src-tauri/src/commands/ac_discovery.rs`, `src-tauri/src/phone/mailbox.rs`, and CLI send/list-peers code. A file overlay would silently delete those current-main fixes even when Git reports no conflict.
- Preserve #91 clippy hygiene in touched Rust files. In particular keep the current-main `SessionManager: Default` impl, `#[allow(clippy::too_many_arguments)]` annotations, `#[allow(clippy::module_inception)]` in `session/mod.rs`, collapsed `strip_auto_injected_args` condition in `sessions_persistence.rs`, and the `commands/session.rs` clippy/test cleanups around `should_inject_continue`. Re-run clippy after resolving, because the feature branch currently regresses several of these hunks.
- Preserve #89 `.gitattributes` and avoid Windows CRLF churn in `.toml`, `.json`, and `.rs` files. Add `git diff --check` to validation; if conflict resolution tools rewrite line endings, normalize before review.
- Preserve #84 by verifying the final retake has no unintended diff from `origin/main` in `src-tauri/src/commands/entity_creation.rs`, `src-tauri/src/config/claude_settings.rs`, and `.gitattributes`.
- Do not carry the feature branch's `save_settings` tmp-plus-`std::fs::rename` change as-is without Windows validation. On Windows, `std::fs::rename(tmp, existing_settings_json)` can fail when the destination exists. Either keep current-main direct write or implement a Windows-safe replacement strategy and add/manual-test saving settings when `settings.json` already exists.
- `src-tauri/capabilities/default.json` is not a merge conflict, but it is still a required manual edit: add `main` to the `windows` allowlist while keeping `sidebar`/`terminal` only if legacy routes remain intentionally supported.
- `attach_terminal` must not clear detached state or mark `was_detached=false` unless destroying the detached window succeeds, or unless a deliberate rollback path restores the previous state. The current feature-branch implementation clears state before checking window-destroy errors.
- WebSocket `create_session` must keep #82's `skip_auto_resume=true` fresh-create behavior. Browser `destroy_session` and `switch_session` should share or mirror native detached-session semantics so remote clients cannot make native main render a session that is still detached.
- Merged `AppSettings`/TypeScript types must include both current-main `coord_sort_by_activity` and #71 `main_*` fields. Add/keep serde default tests for old settings JSON missing both groups of fields.

## Remaining Work Before Test Build

1. Get explicit authorization to update the feature branch.
2. Retake/rebase the feature branch onto current `origin/main`.
3. Resolve the five known conflicts using the strategy above.
4. Search for stale two-window assumptions and compatibility aliases.
5. Run backend validation.
6. Run frontend validation.
7. Perform manual attach/detach/restart/quit scenarios.
8. Send to grinch for implementation review.
9. Fix any grinch findings.
10. Send to shipper for a feature-branch test build only after review is clean.
