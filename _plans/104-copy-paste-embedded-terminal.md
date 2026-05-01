# Plan: Issue #104 — Copy/Paste in embedded terminal

- **Issue**: https://github.com/mblua/AgentsCommander/issues/104
- **Branch**: `feature/104-copy-paste-embedded-terminal`
- **Architect**: wg-1-dev-team/architect
- **Status**: Round 2 — fixes from grinch adversarial review applied. F1 (CRITICAL paste injection), F2 (HIGH race), F3 (MEDIUM key code), F4+F5 (MEDIUM clipboard error handling), F8 (LOW IME). Awaits round-2 dev-webpage-ui re-review and round-2 grinch re-review.

---

## 1. Final design (refinement of tentative plan)

**Verdict (round 2): ACCEPT the tentative plan with seven precisions.** dev-webpage-ui's root-cause analysis is correct. The original plan had three precisions (A, B, C); grinch's adversarial review surfaced one inverted assumption (C — fixed below) and four additional must-fix concerns (D, E, F, G). All seven are now baked in.

The tentative plan stands on two pillars, both of which I verified by reading the code and DOM hierarchy:

1. **Scope** the document-level `contextmenu` blocker in `src/sidebar/App.tsx` so it ignores events whose target is inside `.terminal-host`. This is the minimum-blast-radius fix: it preserves all existing custom menus (`SessionItem`, `ProjectPanel`, `AcDiscoveryPanel`) and restores the WebView2 native menu over xterm only.
2. **Extend** the existing `attachCustomKeyEventHandler` in `src/terminal/components/TerminalView.tsx` to handle `Ctrl+Shift+C` (copy via clipboard API + xterm selection) and `Ctrl+Shift+V` (paste via `terminal.paste()` so bracketed-paste wrapping is applied).

### 1.1 Refinements over dev-webpage-ui's tentative plan

#### Refinement A — `preventDefault` + `stopPropagation` when we DO copy/paste

`attachCustomKeyEventHandler` returning `false` only tells xterm.js to suppress its own processing of the event; it does NOT automatically `preventDefault()` the underlying KeyboardEvent. That means the keydown still bubbles to `document` and is visible to browser/WebView2 accelerators (in particular, `Ctrl+Shift+C` opens DevTools in dev). To make the copy path deterministic:

- Inside the handler, when we actually copy or paste, call BOTH `event.preventDefault()` AND `event.stopPropagation()` on the KeyboardEvent before returning `false`.
- When we do NOT handle (copy with no selection), do nothing extra and return `true` so xterm sees the event normally and the document-level / browser accelerators run.

This converts the dev-mode DevTools collision from "always opens" into "only opens when there is no selection to copy" — which matches the UX intent: the shortcut prefers to copy, but is still a fallback path to DevTools when no selection.

#### Refinement B — only run the side effects on `keydown`, not `keyup`

`attachCustomKeyEventHandler` is invoked for both `keydown` and `keyup`. The existing `Shift+Enter` handler already guards on `event.type === "keydown"` (line 156). Apply the same guard to the new branches: read selection / write clipboard / call `terminal.paste()` only on `keydown`. On `keyup` of the same shortcut, just return `false` to suppress xterm's keyup processing and avoid double-fire.

#### Refinement C — use `event.key.toLowerCase() === 'c'` / `'v'` (CORRECTED in round 2)

**Round-1 mistake:** I argued for `event.code === 'KeyC'` claiming it was "layout-independent and matches the physical key labeled C". Per grinch's F3, that reasoning is inverted. `event.code` returns the key name from the **physical position** on a US-QWERTY keyboard (the Code Values spec). On Dvorak, the physical QWERTY-C position is labeled "J" — pressing the key the Dvorak user sees as "C" emits `event.code === 'KeyI'`, NOT `'KeyC'`. So `event.code === 'KeyC'` would silently break copy/paste for Dvorak / Colemak / non-US users (~3–8% of the population). Windows Terminal does NOT use physical position; it matches against virtual-key codes which reflect the active layout.

**Correct approach:** match `event.key.toLowerCase() === 'c'` (and `'v'`). `event.key` returns the character the user's layout produces — exactly the "key labeled C" in their layout. This matches the codebase convention at `src/shared/shortcuts.ts:53` and is the right answer for international users. With Shift held, `event.key` is "C" uppercase; `.toLowerCase()` normalizes both Shift / Caps-Lock permutations.

No inline comment about diverging from the codebase needed — we ARE consistent with the codebase now.

#### Refinement D — sanitize clipboard payload before `terminal.paste()` (CRITICAL — F1)

`xterm.js@6.0.0` (the version this repo pins via `package.json` → `^6.0.0` resolves to literal 6.0.0; no patched 6.0.x exists on npm) does NOT sanitize the bracketed-paste markers `\x1b[200~` / `\x1b[201~` inside the payload before wrapping it. This is CVE-2019-11848 redux: an attacker who controls the clipboard (pastejacking) can break out of bracketed-paste mode mid-payload and inject literal command bytes into the shell. With a bracketed-paste-aware shell (bash, zsh, PowerShell 7, Claude Code), the injection executes. AgentsCommander runs with the user's full FS permissions; `rm -rf $HOME` actually deletes user data.

The plan therefore MUST sanitize before passing text to `terminal.paste`:

```ts
// Cover both 7-bit ESC[200~/201~ and 8-bit C1 \x9b 200~/201~ forms (round-2 polish O1):
// defense-in-depth against future shells that activate 8-bit recognition.
const sanitized = text.replace(/\x9b20[01]~|\x1b\[20[01]~/g, '');
terminal.paste(sanitized);
```

This strips any embedded markers (both 7-bit and 8-bit C1 forms) from the payload so the shell sees one continuous bracketed-paste block. Strip rather than escape (e.g. U+241B substitution as xterm master does) because we have no rendering need for the visible character; the marker bytes have no legitimate place in a paste payload. If 6.0.0 is replaced with a patched 6.0.x release (where `bracketTextForPaste` does the sanitization itself), the regex becomes a no-op safely — no removal needed at that point, but also no harm. Track upgrade in §6.8.

#### Refinement E — guard `clipboard.readText()` resolution against session/lifecycle changes (HIGH — F2 + F6)

`navigator.clipboard.readText()` is async (5–80 ms typical, longer under contention). Between keydown and resolution, the user may switch sessions, detach, or destroy the active session. Without a guard:

- **Session switch:** paste lands on the previously-active xterm. The `onData` handler at `TerminalView.tsx:165-186` (`if (activeSessionId !== sessionId) return;`) silently drops it. PTY receives nothing. User sees nothing happen.
- **Destroy:** `disposeSessionTerminal` runs `terminal.dispose()`. `terminal.paste()` on a disposed instance may throw (xterm v6 internally accesses `coreService` which can be null after dispose).
- **Detach:** main keeps a pre-warmed xterm; the focused terminal is now in another window. Same silent drop.

Plan therefore MUST re-check before calling `paste`:

```ts
navigator.clipboard.readText().then(text => {
  if (!text) return;
  if (activeSessionId !== sessionId) return;   // session switched during await
  if (terminal.element?.isConnected !== true) return;  // terminal not in DOM (pre-open or post-dispose detached)
  const sanitized = text.replace(/\x9b20[01]~|\x1b\[20[01]~/g, '');
  terminal.paste(sanitized);
}).catch(err => console.warn("[paste] read failed:", err?.name ?? "Error"));
```

*Round-3 fix (Nit 1 from grinch QA):* the original `terminal.element === null` check was DEAD CODE — xterm v6.0.0's `Terminal.dispose()` only removes the element from the DOM via `removeChild`; it does NOT null the field. The corrected guard `terminal.element?.isConnected !== true` covers both pre-open (`element === undefined`) and post-dispose detached (`isConnected === false`) states. `isConnected` is a standard `Node` property. See §6.8 for the upgrade caveat.

`activeSessionId` is a TerminalView-scope variable (line 37); both the closure-captured `sessionId` and the live `activeSessionId` are visible inside the `.then`. No new state needed.

#### Refinement F — `event.isComposing` early-return for IME composition (LOW — F8)

CJK input methods (Microsoft Pinyin, Japanese IME, Korean IME) compose multi-keystroke sequences. During composition, `event.isComposing === true` and the keystrokes belong to the IME, not the application. xterm.js v6 has internal composition handling (`_compositionHelper`), but `attachCustomKeyEventHandler` runs FIRST. If our handler intercepts mid-composition (some Microsoft IMEs map Ctrl+Shift to language-toggle and chord with C/V), we corrupt the composition state.

First line of every new branch (and ideally first line of the WHOLE handler body before any branch) MUST:

```ts
if (event.isComposing) return true;
```

Returning `true` lets xterm process normally, which routes to its composition handler. We pay one bool check per keystroke; cost negligible.

#### Refinement G — `.catch()` is REQUIRED on every clipboard call (MEDIUM — F4 + F5)

Round-1 plan said `void navigator.clipboard.writeText(...)` "fire-and-forget; failures swallowed silently". That is wrong on two counts:

1. **`unhandledrejection` is captured into the error log.** `src/shared/console-capture.ts:52-55` registers a `window.addEventListener("unhandledrejection", ...)` that records rejections as ERROR-LEVEL entries in the captured ring buffer. Missing `.catch` therefore pollutes `getErrorsOnly()` and `copyErrors()` exports with clipboard activity — leaks user behavior into bug reports.
2. **Silent write failures leak old clipboard content.** If the user has password "hunter2" in clipboard from a prior copy and our `writeText` rejects (focus race, AltTab, password manager hook holding the clipboard), the user assumes the new copy succeeded and pastes elsewhere — getting "hunter2". No signal of failure.

Plan therefore REQUIRES (not nice-to-have) a `.catch(err => console.warn(...))` on every clipboard call. `console.warn` (not `console.error`) because clipboard rejection is degraded UX, not a crash — semantically a warning. Note: `getErrorsOnly()` at `console-capture.ts:67-72` actually filters `level === "error" || level === "warn"`, so a `console.warn` IS still surfaced when the user runs `copyErrors()` for a bug report — that is the intent (diagnosability). The reason to log via our own `.catch` rather than letting `unhandledrejection` capture the raw rejection is **content control**: we choose the log format and avoid surfacing whatever the rejection's `.message` contains (which could include partial clipboard content depending on browser implementation). User-visible toast on failure is OUT of scope (would require a new UI component); accept the captured-log feedback as the diagnostic channel and treat a toast as a follow-up if the user reports the issue.

### 1.2 Why not alternative approaches

- **Why not remove the `blockContextMenu` listener entirely?** It is load-bearing for the custom menus in `SessionItem`, `ProjectPanel`, `AcDiscoveryPanel`. Removing it would leak the default browser menu over sidebar items.
- **Why not move the block from `document` to a more specific sidebar element?** Tempting, but the sidebar root is not stable across embedded vs detached layouts and the custom-menu opener code (e.g. `ProjectPanel.tsx:314`) registers its own `contextmenu` listeners on `window` for dismissal. Scoping by target check at the document listener is the cleanest one-line fix that respects the existing architecture.
- **Why not let xterm or the terminal pane mount its own capture-phase `contextmenu` listener that calls `stopPropagation`?** Possible, but requires two coordinated changes (capture-phase listener in terminal + ensuring document listener is in bubble phase). Single conditional check at the source is simpler and easier to reason about.
- **Why not use a custom Solid context menu?** Tech-lead explicitly asked for the WebView2 native menu — "la estética no importa por ahora". A custom menu would also need to bridge xterm's internal selection (canvas, no DOM selection) to the menu's "Copy" item, duplicating the keyboard-shortcut logic.
- **Why not a Tauri clipboard plugin?** `navigator.clipboard.writeText/readText` already works in this codebase (`src/shared/console-capture.ts:76,83`). No new crate or capability needed. Stays inside design principle 3 (minimal blast radius).

---

## 2. Files to modify

### 2.1 `src/sidebar/App.tsx`

**What changes:** the `blockContextMenu` arrow function declared at line 55 becomes target-aware. Today it unconditionally calls `e.preventDefault()`. The new version walks up from `e.target` and bails out (no `preventDefault`) if the target is inside an element matching `.terminal-host`. Everything else still gets blocked.

**Location of change:** line 55 (the `const blockContextMenu = ...` declaration). The two `addEventListener` / `removeEventListener` calls at lines 97 and 246 do NOT change — same listener function, same registration, same cleanup. Only the function body changes.

**Behavioral specification:**

- Input: a `contextmenu` event.
- Resolve `e.target` to an `Element` (guard with `instanceof Element` — `target` could be a `Document` or `Text` node in edge cases).
- If the resolved element's `.closest('.terminal-host')` is non-null → return early (let the event proceed; WebView2 will show its native menu).
- Else → `e.preventDefault()` (current behavior).

Type signature stays `(e: Event) => void`.

### 2.2 `src/terminal/components/TerminalView.tsx`

**What changes:** the `attachCustomKeyEventHandler` registration at lines 154–163 is extended with two new branches before the existing `Shift+Enter` branch. The `Shift+Enter` branch and final `return true` stay exactly as they are.

**Location of change:** inside the `createSessionTerminal` function, between line 153 (the comment `// Shift+Enter → send LF...`) and line 164 (the closing of the handler). The new code lives at the start of the handler body.

**Behavioral specification:**

For each invocation of the handler, in order:

0. **IME guard (NEW, F8)** — first line of the handler body, before any branch matching:
   ```ts
   if (event.isComposing) return true;  // let IME handle
   ```
   This applies to all keystrokes, not just C/V. Cost: one boolean read per keystroke. Returning `true` lets xterm's internal composition handler process the event normally.

1. **`Ctrl+Shift+C` branch** — match when `event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'c'`.
   - On `keydown` only: if `terminal.hasSelection()`:
     - `event.preventDefault()`, `event.stopPropagation()`.
     - `navigator.clipboard.writeText(terminal.getSelection()).catch(err => console.warn("[copy] write failed:", err?.name ?? "Error"))` — `.catch` is REQUIRED (see §1.1.G). Log only `err.name` (round-2 polish O3) — defense-in-depth against theoretical clipboard-content leakage in `err.message`. Fire-and-forget on the promise; we do not await.
     - Return `false`.
   - On `keydown` with no selection: return `true` (let xterm and the browser handle — preserves DevTools fallback in dev).
   - On `keyup`: mirror the keydown decision — re-check `terminal.hasSelection()` and return `false` if we copied, `true` otherwise. Re-check is stateless and cheap; per dev-webpage-ui §11.2 this is preferred over a closure flag.

2. **`Ctrl+Shift+V` branch** — match when `event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'v'`.
   - On `keydown` only:
     - `event.preventDefault()`, `event.stopPropagation()`.
     - Initiate the async paste:
       ```ts
       navigator.clipboard.readText().then(text => {
         if (!text) return;
         if (activeSessionId !== sessionId) return;   // F2: session switched
         if (terminal.element?.isConnected !== true) return;  // F2/F6 + Nit 1: terminal not in DOM (pre-open or post-dispose detached)
         const sanitized = text.replace(/\x9b20[01]~|\x1b\[20[01]~/g, '');  // F1+O1: strip 7-bit and 8-bit C1 markers
         terminal.paste(sanitized);
       }).catch(err => console.warn("[paste] read failed:", err?.name ?? "Error"));  // O3: log only err.name
       ```
     - Return `false`.
   - On `keyup`: return `false`.
   - **Critical (unchanged):** use `terminal.paste(sanitized)`, NOT `PtyAPI.write`. `terminal.paste` wraps the payload in bracketed-paste markers (`\x1b[200~ ... \x1b[201~`) so multi-line paste cannot be interpreted as command submission by the shell. xterm's existing `terminal.onData` listener (line 165) will forward the bracketed sequence to PTY.
   - **Critical (NEW, F1 + O1):** sanitize BEFORE calling `terminal.paste`. xterm.js@6.0.0's `bracketTextForPaste` does NOT strip embedded `\x1b[20[01]~` markers from the payload, allowing pastejacking-style command injection. The regex `/\x9b20[01]~|\x1b\[20[01]~/g` removes both 7-bit (`\x1b[200~` / `\x1b[201~`) and 8-bit C1 (`\x9b 200~` / `\x9b 201~`) forms — defense-in-depth against future shells that activate 8-bit recognition. See §6.8 for the dependency-pin tracker.
   - **Critical (NEW, F2 + Nit 1):** the two guards inside `.then` (`activeSessionId !== sessionId`, `terminal.element?.isConnected !== true`) MUST be present. Without them, the paste either silently routes to the wrong session or throws on a disposed terminal. See §6.9. *Round-3 fix:* the round-2 spec used `terminal.element === null`; that was dead code in xterm v6.0.0 (Terminal.dispose() does not null `element`). Switched to `isConnected` per Nit 1 from grinch QA.

3. **Existing `Shift+Enter` branch** — unchanged.

4. **Final `return true`** — unchanged.

**Activeness guard:** unlike the `Shift+Enter` branch which guards on `activeSessionId === sessionId` before writing to PTY, copy/paste does NOT need that guard at the START of the handler because `terminal.hasSelection()`, `terminal.getSelection()`, and `terminal.paste()` are scoped to the per-session `terminal` instance — they only operate on the focused/active terminal. xterm only dispatches keyboard events to the focused terminal's helper textarea, so the handler will only fire on the active one in practice. **However**, the paste path's `.then` callback runs ASYNCHRONOUSLY and DOES need the guards described above (Refinement E / F2) — by the time the clipboard read resolves, focus may have moved. Copy is synchronous; no async race exists.

---

## 3. Diff plan (high level, no code yet)

### 3.1 `src/sidebar/App.tsx`

- **Modify** one arrow function (`blockContextMenu`, line 55). Net change: ~3 lines added (target check + early return), 1 line modified.
- **No new imports.**
- **No deletions.**
- **No changes** to `onMount`, `onCleanup`, the `addEventListener` / `removeEventListener` calls, or any other listener.

### 3.2 `src/terminal/components/TerminalView.tsx`

- **Modify** the body of the existing `attachCustomKeyEventHandler` callback (lines 154–163). Net change: **~35 lines added** (round-1 was ~25; round-2 adds the IME guard, the sanitization regex, the two race guards, and the `.catch` on both clipboard calls), 0 lines modified, 0 lines deleted.
- **No new imports** — `navigator.clipboard`, `terminal.hasSelection()`, `terminal.getSelection()`, `terminal.paste()`, and `terminal.element` are all already-available global / instance APIs. The `String.prototype.replace` and regex literal need no import.
- **No deletions.**

### 3.3 Out of scope for this PR

- `src/browser/App.tsx` — not touched.
- Any custom Solid context menu — not built.
- Any Tauri capability / `tauri.conf.json` change — not needed (clipboard API is browser-native, already in use).
- Any change to `src/shared/shortcuts.ts` — these shortcuts are terminal-scoped (xterm `attachCustomKeyEventHandler`), not document-global, by design.
- Any CSS / styling changes — `.terminal-host` already exists.
- Any change to detached terminal flow — already works; no regression risk because detached window has no SidebarApp mounted.

---

## 4. Selectors and CSS

### 4.1 The selector: `.terminal-host`

**Already exists.** Confirmed by grep:

- Defined at `src/terminal/components/TerminalView.tsx:296` — `<div class="terminal-host" ref={hostRef!} />`.
- Styled at `src/terminal/styles/terminal.css:226-246` (`.terminal-host`, `.terminal-host .xterm`, `.terminal-host .xterm *`).
- Also referenced in `src/browser/styles/browser.css:86,105` and `src/main/styles/main.css:105` for layout concerns.

**Robustness check:**

- The class is unique to the terminal pane root in TerminalView. Sidebar components do NOT use `.terminal-host`.
- In embedded layout (`src/main/App.tsx:228`), the terminal pane is `<div class="main-terminal-pane"><TerminalApp embedded /></div>`. Inside `TerminalApp`, the `TerminalView` renders `<div class="terminal-host">`. So in DOM order: `main-root > main-body > main-terminal-pane > terminal-layout > terminal-host`.
- `closest('.terminal-host')` from any descendant of the xterm canvas / helper textarea / scrollbar will resolve correctly.
- Right-clicks landing on the `.terminal-empty` fallback (line 207, when no active session), `WorkgroupBrief`, `LastPrompt`, or `StatusBar` will NOT be inside `.terminal-host` — those will continue to be blocked. This is intended: those are non-input UI elements where a paste menu would be misleading.

**No new class needed.** No CSS changes.

### 4.2 Risk on the `.main-dragging` overlay

`src/main/styles/main.css:105` sets `.main-root.main-dragging .terminal-host { pointer-events: none }` during splitter drag. This blocks all pointer events including `contextmenu` over the terminal while dragging. This is harmless for our feature (you can't right-click while dragging the splitter anyway) and was deliberately added to prevent xterm from hijacking the drag as text selection.

---

## 5. Manual testing strategy

Test on Windows in a Tauri build (`npm run tauri dev`). Cover the full matrix below before declaring done. **Both** the embedded layout (default unified window) and the detached layout must be exercised — detached must remain regression-free.

### 5.1 Bug repro (precondition)

Before applying the fix, on the same branch with the changes reverted, verify the bug reproduces:

1. Launch app, create a session.
2. Run a command that produces output (`dir`, `ls`, etc.).
3. Select some output text in the terminal pane (drag with mouse).
4. Right-click on the selection.
5. **Expected (broken):** no menu appears.

This is the sanity check that the fix has something to fix. Skip if dev already has clear video evidence.

### 5.2 Context menu (post-fix)

| # | Scenario | Expected |
|---|---|---|
| 5.2.1 | Right-click on terminal output (no selection) | WebView2 native menu appears. Items may include Reload / View Source / Inspect — that's WebView2's choice, not ours. |
| 5.2.2 | Right-click on terminal output with active selection | WebView2 native menu appears. **Verify if Copy item is present and copies the visible terminal text.** This is empirically confirmed to work in detached mode; we expect parity. See §6.1 for the risk here. |
| 5.2.3 | Right-click on a `SessionItem` in sidebar | Custom session menu still opens (Rename / Destroy / Detach / etc.). No native menu. |
| 5.2.4 | Right-click on a `ProjectPanel` row | Custom project menu still opens. No native menu. |
| 5.2.5 | Right-click on `AcDiscoveryPanel` items | Custom AC discovery menu still opens. No native menu. |
| 5.2.6 | Right-click on `WorkgroupBrief` / `LastPrompt` / `StatusBar` (inside terminal pane but OUTSIDE `.terminal-host`) | No menu. (These are not xterm; native menu would be misleading.) |
| 5.2.7 | Right-click on the terminal-empty fallback (`<span>No active session</span>`) | No menu. |
| 5.2.8 | Right-click on splitter divider | No menu. |
| 5.2.9 | Right-click on titlebar / drag region | No menu (titlebar drag region is sidebar-owned). |

### 5.3 Keyboard shortcuts (post-fix)

| # | Scenario | Expected |
|---|---|---|
| 5.3.1 | Select terminal text, press `Ctrl+Shift+C` | Selection is copied to clipboard. Verify by pasting into Notepad / external app. |
| 5.3.2 | Press `Ctrl+Shift+C` with no selection | In production build: nothing happens. In `npm run dev`: DevTools opens (intentional fallback). |
| 5.3.3 | Press `Ctrl+Shift+V` with text on clipboard | Text is pasted into the terminal at the cursor. Multi-line text is bracketed (shell sees it as a paste, not a series of commands). |
| 5.3.4 | Press `Ctrl+Shift+V` with empty clipboard | Nothing happens. No error in console. |
| 5.3.5 | Press `Ctrl+Shift+V` with clipboard content containing `\n` | Verify shell receives bracketed paste sequence (e.g. bash: paste appears as one block, doesn't auto-execute on first newline if the shell supports bracketed paste). |
| **5.3.10** (round-2, F1) | **Paste-injection probe**: set clipboard to `ls\x1b[201~rm -rf $HOME\r` (use a small test script that calls `navigator.clipboard.writeText`), then press `Ctrl+Shift+V` in a bash session | The shell receives a single bracketed-paste block containing `ls` plus the literal text `rm -rf $HOME` (sanitization stripped the `\x1b[201~`). The `rm` command does NOT execute. To assert: pipe stdout to a file, grep for any actual deletion of `$HOME` artifacts (use a sandbox dir; do NOT actually run against `$HOME`). |
| **5.3.11** (round-2, F2) | **Session-switch race**: open two sessions A and B. In A, copy something to clipboard. Switch focus to A's xterm, press `Ctrl+Shift+V`, IMMEDIATELY (within ~10 ms) click session B in sidebar to switch | Paste does NOT land in B. Paste also does NOT land in A (session-switch guard fired). Console shows no error. (If the user reflex is too slow, wrap the test by adding a `setTimeout(..., 200)` artificial delay around `clipboard.readText` resolution via DevTools async-throttling, OR simulate by destroying session A mid-await.) **Verify by inspecting PTY output buffers via `SessionAPI.getOutput(idA)` and `SessionAPI.getOutput(idB)` — neither should contain the pasted text. OR visually confirm both terminals show no pasted content.** (round-2 polish O4) |
| **5.3.12** (round-2, F3) | **Prerequisite:** Install Dvorak via Settings → Time & Language → Language → English (US) → Options → Add Keyboard. Switch with Win+Space. (round-2 polish O5)<br>**Dvorak / Colemak layout**: switch keyboard layout via Win+Space. Press the key labeled "C" on the new layout (which is in a different physical position) while holding Ctrl+Shift. Verify selection is copied | Copy fires correctly. The key is matched by the user's layout's "C" character, not the QWERTY-C physical position. Repeat for V → paste. |
| **5.3.13** (round-2, F4 + F5) | **Permission-denied clipboard**: open DevTools → Application → manually revoke clipboard permission (or simulate via a test that mocks `navigator.clipboard.readText` to reject). Press `Ctrl+Shift+V` | A `console.warn` entry appears in the captured log buffer (verifiable via `getConsoleText()` exposed in DevTools). NO `unhandledrejection` lands in the buffer (which would surface as level=error). NO crash. UI not broken. Repeat for `writeText` (Ctrl+Shift+C with selection) under the same conditions. |
| **5.3.14** (round-2, F8) | **Prerequisite:** Install Microsoft Pinyin via Settings → Time & Language → Language → Add Chinese (Simplified). Switch with Win+Space. (round-2 polish O5)<br>**IME composition mid-stream**: switch to Microsoft Pinyin (or any CJK IME) via Win+Space. Start composing a character (e.g. type `n` → `i`). While the IME composition popup is showing, press `Ctrl+Shift+C` | Composition continues; the shortcut is NOT intercepted; no copy occurs. Pressing Esc cancels composition; pressing Ctrl+Shift+C again with composition cleared and a real selection copies normally. |
| **5.3.15** (round-2 polish O6, F1 8-bit form) | **Paste payload `\x9b201~rm -rf $HOME\r`** (8-bit C1 form): set clipboard to that byte sequence using a small test script (`navigator.clipboard.writeText` with a string containing `\x9b`). Press `Ctrl+Shift+V` in a bash session | Sanitization strips `\x9b201~`. Shell sees `rm -rf $HOME` outside bracketed paste — but the shell is NOT auto-executing it (no Enter sent). User sees the text as if pasted, NO destructive action. Asserts the extended regex `/\x9b20[01]~\|\x1b\[20[01]~/g` covers the 8-bit form per O1. |

### 5.4 Regression checks (no behavior changes)

| # | Scenario | Expected |
|---|---|---|
| 5.4.1 | Press `Ctrl+C` (no Shift) with running process | SIGINT delivered to PTY. Process interrupts. Behavior unchanged. |
| 5.4.2 | Press `Ctrl+C` (no Shift) at idle prompt | `^C` / new prompt line, depending on shell. Behavior unchanged. |
| 5.4.3 | Press `Shift+Enter` while typing | LF written to PTY (existing handler path, line 154–163). Behavior unchanged. |
| 5.4.4 | Press `Ctrl+Shift+N` | New session created (`src/shared/shortcuts.ts:15`). Behavior unchanged. |
| 5.4.5 | Press `Ctrl+Shift+W` | Active session destroyed (`src/shared/shortcuts.ts:21`). Behavior unchanged. |
| 5.4.6 | Press `Ctrl+Shift+R` | Voice recorder toggle (`src/shared/shortcuts.ts:30`). Behavior unchanged. |
| 5.4.7 | Sidebar custom menu open → right-click somewhere else to dismiss | Menu dismisses (the `window.addEventListener("contextmenu", dismiss)` pattern in `ProjectPanel.tsx:314` etc. still fires). |

### 5.5 Detached terminal (must remain regression-free)

| # | Scenario | Expected |
|---|---|---|
| 5.5.1 | Detach a session's terminal to its own window | Window opens. Right-click in detached terminal → WebView2 native menu (already worked, must still work). |
| 5.5.2 | `Ctrl+Shift+C` / `Ctrl+Shift+V` in detached window | Copy / paste works (these handlers also apply because TerminalView is shared). |
| 5.5.3 | Re-attach detached terminal | Terminal pane re-renders in main window. Run §5.2.1 + §5.3.1 over the re-attached terminal. |

### 5.6 Multi-session

| # | Scenario | Expected |
|---|---|---|
| 5.6.1 | Two sessions, switch between them | Selection state is per-session (xterm internal). `Ctrl+Shift+C` in session A copies A's selection; switching to B and pressing the shortcut without selection in B does NOT copy A's content. |
| 5.6.2 | Paste into session A, verify session B did not also receive | xterm's `attachCustomKeyEventHandler` is per-terminal; only the focused session's PTY receives the paste. |

### 5.7 Production build

Build (`npm run tauri build`) and re-run §5.3.1, §5.3.3 in the production binary. Verify:

- `navigator.clipboard.readText()` does NOT prompt the user for permission (it's a user gesture; should auto-grant in WebView2, but worth confirming).
- DevTools accelerators are disabled in prod, so §5.3.2 has no fallback (just nothing happens). That's fine.

---

## 6. Risks not covered by dev-webpage-ui

### 6.1 Risk: WebView2 native menu over xterm's canvas may not show Copy

**Severity: medium. Likelihood: low (empirically refuted in detached, but unverified in embedded).**

xterm.js with the WebGL renderer paints to a canvas. Canvas selections are NOT DOM selections (`window.getSelection()` is empty). The WebView2 native menu's "Copy" item is normally driven by `window.getSelection()` (or by focus on a contenteditable / textarea). xterm has a hidden helper textarea (`.xterm-helper-textarea`) used for IME and screen readers; depending on whether the right-click target resolves to the canvas, the helper textarea, or the parent div, the native menu may or may not show "Copy" / "Paste".

**Empirical evidence:** detached terminal works (per tech-lead). So WebView2 + xterm cooperate to surface working Copy/Paste somehow. We expect parity in embedded.

**Mitigation:** if the embedded native menu turns out to be missing or broken Copy/Paste, the keyboard shortcuts (`Ctrl+Shift+C` / `Ctrl+Shift+V`) ARE the primary UX, and the menu becomes cosmetic. That still satisfies the issue. But test 5.2.2 must be performed and the result reported.

**Fallback path if 5.2.2 fails:** open a follow-up to add a Solid custom context menu that uses the keyboard handler's logic. Not in scope for this PR.

### 6.2 Risk: helper textarea focus state may cause shortcut to miss on cold right-click

**Severity: low. Likelihood: low.**

When the user right-clicks the terminal without first focusing it (e.g. just opened the app, sidebar has focus), the focus event order is: contextmenu fires → focus may or may not transfer. If the user then presses `Ctrl+Shift+C` while the menu is open, the keydown might be intercepted by the menu, not by xterm's helper textarea.

**Mitigation:** standard browser behavior. Native menu consumes the next keystroke (e.g. arrow keys for navigation, Esc to dismiss). Once dismissed, focus returns. This is how all native menus work; users will learn. Not a code-level concern.

### 6.3 Risk: `e.target` in the contextmenu handler is not always an Element

**Severity: low. Likelihood: very low.**

DOM events typed as `Event` can have `target: EventTarget | null`. `EventTarget` is not necessarily an `Element`; it could be `Document`, `Window`, or `Text` node. `closest()` is only on `Element`.

**Mitigation:** guard with `instanceof Element` before calling `.closest()`. If the guard fails, fall back to current behavior (preventDefault). Already specified in §2.1.

### 6.4 Risk: `terminal.paste` behavior with disabled bracketed paste in some shells

**Severity: low. Likelihood: low.**

xterm's `terminal.paste(text)` always emits the bracketed-paste markers `\x1b[200~ ... \x1b[201~`. If the running shell does NOT have bracketed paste mode enabled (e.g. `cmd.exe`, some custom REPLs, raw `node`), it will receive the literal escape bytes as input, which appear as `^[[200~` `^[[201~` around the pasted text. This is mildly ugly but not destructive.

**Mitigation:** documented behavior of xterm; matches what Windows Terminal does; not a regression. PowerShell, bash, zsh, and `claude` all support bracketed paste. Accept as-is. If a shell visibly mangles paste, that's a shell-level config issue, not ours.

### 6.5 Risk: clipboard permission prompt on `readText()` in production WebView2

**Severity: medium. Likelihood: low (clipboard read in keypress handler is a user gesture).**

Some browsers gate `navigator.clipboard.readText()` behind a permission prompt unless the call is in a user-gesture context. Tauri/WebView2 typically grants this for desktop apps without prompt, and the shortcut IS a user gesture. But test 5.7 must confirm in production build.

**Mitigation:** if a permission prompt appears, fall back to a Tauri clipboard plugin (`tauri-plugin-clipboard-manager`). NOT planned for this PR; would be a follow-up. **Failure mode** (permission denied at runtime): per §1.1.G the `.catch` lands a `console.warn` entry in the captured log buffer, which is observable via `copyErrors()` for diagnosis. NOT a silent failure.

### 6.6 Risk: side effect on browser mode (`src/browser/App.tsx`)

**Severity: low. Likelihood: certain.**

Both changes (in `src/sidebar/App.tsx` and `src/terminal/components/TerminalView.tsx`) are in shared components used by browser mode (BrowserApp mounts both SidebarApp and TerminalApp on the same document). Therefore browser mode will also see:

- Native menu unblocked over `.terminal-host` (browser's regular context menu).
- `Ctrl+Shift+C` / `Ctrl+Shift+V` work (subject to clipboard API availability in the browser).

**Tech-lead said** browser mode is out of scope and tracked in #105. The above is an incidental fix, not a deliberate one — we are not touching `src/browser/App.tsx` or browser-specific code. The shared-component surface area is what it is.

**Decision needed by tech-lead:** is incidental browser fix acceptable, or should the new behavior be gated on `!isBrowser` (importing from `src/shared/platform.ts`)?

**My recommendation:** accept the incidental fix. Gating it adds branching for no behavior reason and slightly misaligns embedded vs browser DX. If the user dislikes the side effect in browser mode, gate later in #105.

→ See open question §7.1.

### 6.7 Risk: future `TerminalView` instances duplicating the handler

**Severity: very low. Likelihood: very low.**

`attachCustomKeyEventHandler` is per-`Terminal` instance. We register it once per `createSessionTerminal`. No risk of duplicate registration.

### 6.8 Risk: `xterm.js@6.0.0` paste-injection vulnerability (PIN + UPGRADE TRACKER) (NEW round-2, F1)

**Severity: HIGH (security; arbitrary command execution). Likelihood: ALWAYS given attacker-controlled clipboard. Mitigated in this PR via input sanitization.**

`xterm.js@6.0.0` (resolved by `package.json`'s `^6.0.0` because no patched 6.0.x exists on npm) does NOT sanitize embedded `\x1b[200~` / `\x1b[201~` markers in the payload passed to `Terminal.paste`. Attack vector: pastejacking (CVE-2019-11848 redux). An attacker who can place text on the user's clipboard (compromised webpage with copy-on-click, malicious browser extension, password manager hook, etc.) can inject literal command bytes that escape the bracketed-paste envelope and are interpreted as typed input by the shell.

**This PR mitigates** by stripping the markers in our handler before invoking `terminal.paste` (see §1.1.D and §2.2.2). The mitigation is **defense-in-depth** at the application layer; the underlying xterm bug is still present.

**Tracker:** open a follow-up issue to upgrade `@xterm/xterm` when a patched 6.0.x release lands on npm. Watch [xtermjs/xterm.js](https://github.com/xtermjs/xterm.js) for tagged releases. Patch was merged to master post-6.0.0; 6.0.x patch publication date unknown. When available:

1. Bump `@xterm/xterm` in `package.json`.
2. Verify `bracketTextForPaste` in the new version sanitizes (read its source to confirm).
3. Optionally remove the regex sanitization in our handler (would be a no-op, no harm in keeping it as belt-and-suspenders).

The plan does NOT block on this upgrade; sanitization is sufficient. But the tracker MUST exist so that we don't carry the workaround forever once a real fix is available.

### 6.9 Risk: async `clipboard.readText()` race vs session lifecycle (NEW round-2, F2 + F6)

**Severity: HIGH (silent data loss; misrouted paste). Likelihood: SOMETIMES (depends on user reflexes). Mitigated in this PR via re-check guards.**

`navigator.clipboard.readText()` resolves async (5–80 ms typical, can be longer under OS clipboard contention). In that window, the user can switch session (Ctrl+Tab, sidebar click), detach the active terminal, or destroy the session. Without guards, the paste either:

- Routes to the previously-active session's `terminal.paste`, which then hits `terminal.onData → if (activeSessionId !== sessionId) return;` and is silently dropped (no PTY input, user sees nothing).
- Calls `paste` on a disposed terminal, which may throw (xterm v6 internally accesses `coreService` that can be null after dispose).
- Lands on the wrong session if a focus shift happens but the original terminal is still alive.

**This PR mitigates** with two guards inside the `.then` (see §1.1.E and §2.2.2):

```ts
if (activeSessionId !== sessionId) return;
if (terminal.element === null) return;
```

The `terminal.element === null` check is xterm's de-facto "is disposed" sentinel — `Terminal.dispose()` sets `element` to null. This is not part of the public API contract but is stable across v6.x and used in xterm's own examples. **Caveat (per grinch F15):** if a future xterm minor version stops nulling `element` on dispose, this check breaks open. Mitigation if it happens: switch to a closure-captured `let disposed = false;` flag toggled by an `onDispose` handler (`terminal.onDispose(() => { disposed = true; })`). Not implemented here because it costs more state and the current check is sufficient for v6.0.x.

### 6.10 Risk: IME composition collision (NEW round-2, F8)

**Severity: LOW (CJK-input users only). Likelihood: SOMETIMES (during active IME composition). Mitigated in this PR via `event.isComposing` guard.**

Some Microsoft IMEs (Pinyin, Korean) map Ctrl+Shift to language-toggle and may surface chord events with `isComposing === true` while a multi-keystroke composition is in flight. Without a guard, our handler would intercept these mid-composition keystrokes (preventDefault + clipboard ops), corrupting the IME state and potentially stranding the user mid-character.

**This PR mitigates** with a first-line guard at the top of the handler body (see §1.1.F and §2.2.0):

```ts
if (event.isComposing) return true;
```

Returning `true` lets xterm's internal `_compositionHelper` process the keystroke as part of the IME flow. Cost: one bool read per keystroke; negligible.

---

## 7. Open questions (ALL RESOLVED in round 2)

### 7.1 Browser mode — accept incidental fix or gate it? **RESOLVED**

**Decision (tech-lead, round 1):** accept the incidental fix in browser mode. Both shared-component changes (`src/sidebar/App.tsx` and `src/terminal/components/TerminalView.tsx`) flow through to BrowserApp without explicit gating. #105 remains as the deliberate browser-mode work item; this PR does not pre-empt it.

### 7.2 Should `Ctrl+Shift+C` with no selection do anything else? **RESOLVED**

**Decision (tech-lead, round 1):** option (a) — do nothing. Falls through to xterm/browser handling. In `npm run dev` this means DevTools opens (intentional fallback). In production WebView2, nothing happens.

### 7.3 Should we also bind `Shift+Insert` for paste? **RESOLVED**

**Decision (tech-lead, round 1):** no. Scope is `Ctrl+Shift+V` only. `Shift+Insert` may be added in a follow-up if user requests it; not in this PR.

### 7.4 Telemetry / logging? **RESOLVED (with round-2 update)**

**Decision (tech-lead, round 1):** no telemetry. Round-2 update (per F4 + F5): clipboard call FAILURES are logged via `console.warn` (REQUIRED, see §1.1.G) — that is observability for diagnosis, not telemetry. Successful copy/paste produces no log line. The distinction matters: failure logs land in the captured ring buffer (`src/shared/console-capture.ts`) and are surfaced in `copyErrors()` exports, which is the right channel for bug reports.

---

## 8. Summary of file changes

| File | Lines touched | Net additions | Type |
|---|---|---|---|
| `src/sidebar/App.tsx` | ~55 | +3 / ~1 modified | Bug fix (scope contextmenu blocker) |
| `src/terminal/components/TerminalView.tsx` | ~154 | +~35 (round-2; round-1 was ~25) | Feature (new shortcuts + sanitization + race guards + IME guard + .catch handlers) |
| `_plans/104-copy-paste-embedded-terminal.md` | (this file) | new | Plan doc |

No new dependencies. No `tauri.conf.json` changes. No CSS changes. No backend (Rust) changes. No IPC changes. No type changes in `src/shared/types.ts`.

`@xterm/xterm@6.0.0` is NOT upgraded in this PR. Sanitization at the application layer is the round-2 mitigation for F1 / CVE-2019-11848-redux. See §6.8 for the upgrade tracker.

---

## 9. Phase tagging

Per the architect role's phase order (MVP → Full Features → Polish → Extras), this is **MVP-equivalent**: a small, user-visible bug+feature pair. No deferral of pieces; ship as one PR.

---

## 10. Handoff

**Round 1 handoff:** dev-webpage-ui (enrichment) → grinch (adversarial review). Completed; results captured in §11 and §12 as historical input.

**Round 2 handoff (current):** dev-webpage-ui round-2 review (verify the architect-applied fixes are correctly written) → grinch round-2 review (verify the security/race fixes neutralize F1, F2, F4, F5, F8). After grinch round-2 approval, hand to dev-rust / dev-webpage-ui for implementation.

Architect's round-2 changes summary (delta vs round-1):
- §1.1.C **rewritten** — switched from `event.code` to `event.key.toLowerCase()` per F3. Round-1 reasoning was inverted.
- §1.1.D **added** — sanitization regex before `terminal.paste` per F1.
- §1.1.E **added** — race guards inside `.then` per F2 + F6.
- §1.1.F **added** — `event.isComposing` early-return per F8.
- §1.1.G **added** — `.catch` REQUIRED on every clipboard call per F4 + F5.
- §2.2.0 **added** — IME guard at top of handler.
- §2.2.1 **updated** — `event.key.toLowerCase()`, REQUIRED `.catch`.
- §2.2.2 **updated** — sanitization, two race guards, REQUIRED `.catch`.
- §3.2 **updated** — line count ~35 (was ~25).
- §5.3.10–5.3.14 **added** — five new tests covering F1, F2, F3, F4+F5, F8.
- §6.5 **clarified** — `.catch` lands warning, not silent.
- §6.8, §6.9, §6.10 **added** — security pin tracker, async race, IME risk.
- §7.1–§7.4 **all marked RESOLVED**.
- §8 **updated** — round-2 line count + xterm pin note.
- §11 and §12 **untouched** (historical input per tech-lead's instruction).

— architect, round 2 (2026-05-01)

---

## 11. Dev-webpage-ui review and additions

> **NOTA** (round-2 polish O7): §11 es input round-1 histórico de dev-webpage-ui. Donde su contenido contradice §1.1 (round-2), §1.1 prevalece. Conservada como audit trail.

Tech-lead's closed inputs received and applied: §7.1 accept incidental browser fix, §7.2 do nothing on Ctrl+Shift+C without selection, §7.3 no Shift+Insert, §7.4 no telemetry. I am not re-opening any of those.

### 11.1 Verifications performed

#### 11.1.1 Refinement A — capture-phase listeners (verified, no conflict)

I grepped the entire `src/` tree for capture-phase keydown / keyup listeners (`addEventListener(..., true)`, `useCapture`, `{ capture: true }`). **Only one match**: `src/main/components/QuitConfirmModal.tsx:66` registers `document.addEventListener("keydown", onKeyDown, true)` while the modal is mounted.

That handler (lines 27–63) only acts on `Escape`, `Enter`, `Tab` — Ctrl+Shift+C/V are not handled and the event continues to bubble. Direction-of-flow: capture goes top-down BEFORE reaching xterm's helper textarea, so QuitConfirmModal sees the event first regardless. Our `event.stopPropagation()` in xterm's bubble-phase handler runs AFTER the capture phase has finished, so it cannot affect this listener.

Conversely, when the modal is open, focus is on the Cancel/Quit button and not on the xterm helper textarea, so xterm's `attachCustomKeyEventHandler` will not even fire — the event never reaches xterm. So even on Ctrl+Shift+V while the modal is open, no copy/paste happens. This is the correct behavior.

**Refinement A is safe to ship as specified.**

#### 11.1.2 Refinement B — `attachCustomKeyEventHandler` event types (verified)

xterm.js v6 invokes the custom key event handler from its helper textarea's `keydown` and `keyup` listeners. **`keypress` is NOT dispatched to this handler** (the legacy `keypress` event has been deprecated in DOM and xterm does not bridge it). The existing `Shift+Enter` branch at `src/terminal/components/TerminalView.tsx:155-162` relies on `event.type === "keydown"` and works in production today, so the contract is empirically confirmed for this codebase.

**Refinement B is correctly scoped to `keydown`. The `keyup` mirror return is needed to suppress xterm's default keyup handling (otherwise xterm's `_keyUp` may re-fire onData for held shortcuts).**

#### 11.1.3 Refinement C — `event.code` vs `event.key` (verified, with style note)

Grep for any existing `event.code` or `e.code` usage in `src/`: **zero matches**. Every keyboard handler in this codebase currently uses `event.key`:
- `src/shared/shortcuts.ts:53` → `e.key.toLowerCase() === shortcut.key`
- `src/terminal/components/TerminalView.tsx:155` → `event.key === "Enter"`
- `src/main/components/QuitConfirmModal.tsx:28,34,46` → `e.key === "Escape" / "Enter" / "Tab"`
- All sidebar modals (`OnboardingModal`, `OpenAgentModal`, `AgentPickerModal`, `NewAgentModal`, etc.) → `e.key`

Refinement C therefore introduces a new convention. **I recommend keeping it for the C/V branches** because the architect's reasoning is correct: `event.key` for letter+Shift becomes uppercase "C" / "V" and is layout-dependent on non-QWERTY layouts. Add a one-line comment in the new code explaining why this branch uses `event.code` while the adjacent `Shift+Enter` line keeps `event.key === "Enter"` (Enter has no shift-or-layout collision and matches the rest of the codebase). No need to retro-migrate other handlers in this PR.

**Refinement C is correct. Add the inline comment to prevent future "be consistent" PRs.**

#### 11.1.4 §6.1 — WebView2 native menu over xterm canvas (cannot fully verify from code; flag for testing)

xterm.js v6 mounts a real `<textarea class="xterm-helper-textarea">` (positioned offscreen / opacity 0) inside `.terminal-host` for IME and screen-reader bridging. xterm's mouse handler routes click + selection to that textarea via `terminal.select()` / its internal selection service, and on `copy` events xterm pastes `terminal.getSelection()` into the clipboard event's `clipboardData`. So when WebView2's native menu fires "Copy" (which dispatches a synthetic `copy` event), xterm's `copy` listener picks it up.

This is the most likely mechanism that makes detached-window copy work today. But: the menu's "Copy" item is only ENABLED if WebView2 thinks there's a selection in the focused element; whether that condition is satisfied with xterm's offscreen textarea (which is NOT actually focused at right-click time) is not deterministic from code alone.

**Cannot prove from code that WebView2 will show an enabled "Copy" item.** The empirical detached-mode evidence is the strongest signal. My take aligns with architect: **not a blocker**, because keyboard shortcuts are the primary UX. Promote §5.2.2 to **MUST-VERIFY for grinch / QA**, and if it fails, log it as a follow-up issue — do NOT block this PR.

#### 11.1.5 Shadow DOM check (verified, no shadow root)

xterm v6 mounts in **light DOM**. Proof from this codebase:
- `src/terminal/styles/terminal.css:238` → `.terminal-host .xterm { ... }` (descendant selector across the boundary) is applied by browser inspector on the styled terminal — would not work if xterm were in a shadow root.
- `src/terminal/styles/terminal.css:243-245` → `.terminal-host .xterm, .terminal-host .xterm * { box-sizing: content-box }` (universal descendant) — same proof.
- `src/browser/styles/browser.css:96` → `.browser-terminal .xterm-screen { ... }` — directly targets xterm internals from outer stylesheet.

If shadow DOM were in use, none of these rules would land, and the terminal would render with browser-default `box-sizing: border-box` and no padding. It does not — it renders correctly. **`closest('.terminal-host')` will resolve correctly** from any descendant (canvas, helper textarea, scrollbar div).

#### 11.1.6 `terminal.paste()` and Windows `\r\n` (no gotcha expected)

xterm.js v6's `Terminal.paste(data)` normalizes line endings internally before invoking the bracketed-paste wrapper: `\r\n` and lone `\n` are converted to `\r` (CR), matching POSIX TTY input convention. The Windows clipboard delivers `\r\n`-separated lines via `navigator.clipboard.readText()`; xterm strips the `\n` and the shell sees a single CR per line, wrapped in `\x1b[200~` … `\x1b[201~` (when the shell has bracketed-paste enabled, which bash, zsh, PowerShell 7+, and Claude Code all do).

**No special handling needed in our code.** Just call `terminal.paste(text)`. If a shell does not support bracketed paste (e.g. `cmd.exe`, raw `node`), the user sees `^[[200~` / `^[[201~` literals around the paste — mildly ugly, not destructive, matches Windows Terminal behavior. Already covered in §6.4.

#### 11.1.7 `instanceof Element` guard (low value, low cost — keep)

For `contextmenu` events fired by user right-click, `e.target` is always an `Element` in practice (right-click hits a rendered DOM node). The guard is defensive against synthetic events (`document.dispatchEvent(new Event("contextmenu"))` with `target` defaulting to `document`). No occurrence of synthetic contextmenu dispatch exists in this repo (greppable), but the guard cost is one identifier check. **Keep as architect specified.** Covers a footgun if future code synthesizes the event.

### 11.2 Additional implementation notes

- **`activeSessionId === sessionId` activeness guard (architect §2.2 said "not needed"):** I agree, but with one nuance worth a comment in the code. xterm only delivers keys to the focused terminal's helper textarea, so the handler IS only invoked on the active session in practice. However, when the user is in main and switches to a non-focused area (sidebar), the previously-focused xterm's textarea may retain focus until a click happens elsewhere. In that window, Ctrl+Shift+C still fires on the previously-active xterm — which is the correct behavior (copies whatever was last selected in that session). Just clarify in the inline comment that "active" here means "focused-in-DOM," not `activeSessionId`. No code change.

- **Closure variable for keyup mirror (architect §2.2 step 1):** the architect's two options (closure flag vs re-check) — I prefer the **re-check** (`terminal.hasSelection()` again on keyup). Reason: a closure flag would need to be per-handler-instance and per-shortcut, adding state. Re-checking is stateless and the only cost is one bool read per keyup of a Ctrl+Shift+C/V combo (negligible). Selection state can change between keydown and keyup only if the user moves the mouse during the keystroke, which is a non-issue.

- **`navigator.clipboard.writeText` rejection handling:** the architect specifies `void navigator.clipboard.writeText(...)` (fire-and-forget). Add a `.catch(() => {})` to avoid unhandled-promise-rejection warnings. The codebase already has a `console-capture` shim (`src/shared/console-capture.ts`) that swallows all `console.*`, but unhandled rejections still surface in dev tools and CI logs. One-liner cost.

- **Order of branches inside the handler matters:** put `Ctrl+Shift+C` and `Ctrl+Shift+V` BEFORE the existing `Shift+Enter` branch, so the cheap-equality checks short-circuit early for the most-frequent shortcut (`Shift+Enter` is much rarer than navigation/typing keys). Architect's §2.2 ordering is fine; just confirming the rationale.

- **Scope guard on `closest('.terminal-host')`:** per §4.1 the class is unique. I additionally checked `src/browser/styles/browser.css` and `src/main/styles/main.css` — they only reference `.terminal-host` for layout / pointer-events overrides, never re-define it as a class on a non-terminal element. Selector is safe.

### 11.3 Additional manual tests

Adding to §5 (architect's matrix already covers most). The plan's §5 matrix is solid; below are gaps I'd close:

| # | Scenario | Expected | Why added |
|---|---|---|---|
| 5.3.6 | Press `Ctrl+Shift+C` while QuitConfirmModal is open (e.g. after attempting to close main with detached sessions present) | Modal stays open; nothing copied; no error in console. Modal eats keyboard focus. | Verify the only capture-phase listener does not interact with our new handlers. |
| 5.3.7 | Type a long shell command, select part of it with `Shift+End`, press `Ctrl+Shift+C` | Selection copied (xterm selection, not DOM selection). | Confirms keyboard-driven selection (not just mouse) is supported by `terminal.getSelection()`. |
| 5.3.8 | Hold `Ctrl+Shift` and press `C` repeatedly (held-key autorepeat) | First press copies; subsequent autorepeat keydowns no-op (re-copy same selection silently OK; no PTY input). | Verify no PTY-write side effect from autorepeat. |
| 5.3.9 | Click on sidebar (focus leaves terminal), then press `Ctrl+Shift+C` | Nothing happens (xterm helper textarea no longer focused → handler not invoked). | Documents the focus-scoping behavior. |
| 5.5.4 | Detach a session, copy in detached window (Ctrl+Shift+C), re-attach, paste in attached terminal (Ctrl+Shift+V) | Pasted text matches what was copied in detached. | Cross-window clipboard (system clipboard is shared, so this should just work; but worth a sanity check.) |
| 5.6.3 | In session A select text → switch to session B (Ctrl+Tab or sidebar click) → press Ctrl+Shift+C | No copy in B (B has no selection); A's selection persists in A's xterm instance but is irrelevant because B is focused. | Confirms selection state is per-terminal-instance and not global. |
| 5.7.3 | Paste a 10MB text blob via Ctrl+Shift+V in production build | Either pastes (slow but works) or `clipboard.readText()` rejects gracefully. No crash. | Edge: oversized clipboard. xterm `paste()` is synchronous on the data, so very large pastes block the renderer briefly; not our concern but document. |

The architect's §5.4 regression list is exhaustive; nothing to add there.

### 11.4 Verdict

**Visto bueno → grinch.**

The plan is technically sound, all three architect refinements verified, no blockers, no missing implementation details. The §6.1 risk is correctly framed as "test, don't block" and the keyboard shortcut path is the primary UX so the WebView2 native menu is truly best-effort.

Concretely, what I would expect grinch to attack:
- Ordering: does `event.preventDefault()` in our handler actually arrive before `Ctrl+Shift+C` opens DevTools in dev? (My read: yes, because xterm's bubble-phase handler on the helper textarea fires before the unhandled keydown bubbles to the WebView2 chrome accelerator dispatcher.)
- Race: what if `navigator.clipboard.readText()` resolves AFTER the user has switched to another session? `terminal.paste()` would write into the new session. Mild, not destructive.
- Permission denied: should we show a user-facing toast when clipboard access fails? My read: no, silent failure matches the rest of the app (`console-capture.ts` swallows logs anyway). But grinch may push back.

Pasalo a grinch sin más vueltas con architect. Si grinch encuentra algo que requiera architect, vuelvo.

— dev-webpage-ui, 2026-05-01

---

## 12. Grinch adversarial review

Reviewed against current code on `feature/104-copy-paste-embedded-terminal`. Read every referenced file end-to-end (no skimming). Verified xterm.js v6 paste pipeline against the published v6.0.0 source on GitHub (the version `package.json` resolves to under `^6.0.0`; npm `latest` for `@xterm/xterm` is 6.0.0, no patched 6.0.x exists).

**Verdict: BLOCKER. Returns to architect for at least F1, F2, F3.** F4–F6 are also blocking by my standard but if architect/tech-lead disagree they should be acknowledged as known risks in the plan body, not silently shipped.

### F1. Bracketed-paste injection from untrusted clipboard content (CVE-class)

- **Severity: CRITICAL** (security; arbitrary command execution)
- **Reproducibility: ALWAYS** given a clipboard containing the marker bytes
- **Recommendation: BLOCK and refine plan**

**Description.** `xterm.js@6.0.0`'s `Terminal.paste(data)` wraps the input in `\x1b[200~ ... \x1b[201~` without escaping the markers themselves. I read the v6.0.0 source of `src/browser/Clipboard.ts`:

```ts
export function prepareTextForTerminal(text: string): string {
  return text.replace(/\r?\n/g, '\r');           // line endings only
}
export function bracketTextForPaste(text: string, bracketedPasteMode: boolean): string {
  if (bracketedPasteMode) {
    return '\x1b[200~' + text + '\x1b[201~';     // no sanitization
  }
  return text;
}
```

The fix landed AFTER 6.0.0 was tagged (master sanitizes by replacing `\x1b` with U+241B before bracketing). No 6.0.x patch is published on npm; `@xterm/xterm@latest` is 6.0.0 and 6.1.x is beta-only. So `npm install` resolves `^6.0.0` to literally `6.0.0` — the unpatched version.

**Concrete attack scenario.**
1. Attacker hosts a webpage with a "harmless looking" code block. CSS / clipboard JS hides the malicious tail (well-known pastejacking technique, PoCs are public).
2. User Ctrl+C from the page, switches to AgentsCommander, Ctrl+Shift+V.
3. Clipboard contains: `ls\x1b[201~rm -rf $HOME\r`.
4. Plan §2.2.2 calls `terminal.paste(text)`. xterm wraps to `\x1b[200~ls\x1b[201~rm -rf $HOME\r\x1b[201~`.
5. Bracketed-paste-aware shell (bash, zsh, PowerShell 7, Claude Code) sees: paste-begin → `ls` → paste-end. Then sees `rm -rf $HOME\r` as TYPED input → CR submits → executes.

This is CVE-2019-11848 redux. AgentsCommander is a Tauri desktop app running with the user's full FS permissions; an `rm -rf $HOME` actually deletes user data.

**Plan §6.4 covers shells WITHOUT bracketed paste support (cosmetic ugliness). It does NOT cover injection against shells WITH bracketed paste support (security). The two are opposite ends of the same axis.**

**Fix.** Sanitize before calling `terminal.paste`. Add to §2.2.2 step 2:

```ts
const sanitized = text.replace(/\x1b\[20[01]~/g, '');  // strip bracketed-paste markers
terminal.paste(sanitized);
```

Or replace with U+241B for visibility (matches xterm master's approach). Do NOT rely on `terminal.paste` to do this for us — the v6.0.0 we ship does not.

**Test addition required (§5.3.X):** paste a clipboard containing `safe\x1b[201~MALICIOUS\r` and verify NO command after the marker is executed; the literal "MALICIOUS" should appear as part of the paste payload (or be stripped), not run.

---

### F2. Race: `clipboard.readText()` resolves after the active session changed

- **Severity: HIGH** (silent data loss; user-visible UX bug)
- **Reproducibility: SOMETIMES** (depends on user reflexes; clipboard is fast but a fast user/automation can switch in the window)
- **Recommendation: BLOCK and refine plan**

**Description.** `navigator.clipboard.readText()` is async (typical 5–80 ms in WebView2, can be longer if the OS clipboard is contended). Plan §2.2.2 step 2 fires-and-awaits `.then(text => terminal.paste(text))` with a closure-captured `terminal` reference. Between keydown and resolution:

- **Case A — user clicks another session in sidebar:** `terminalStore.activeSessionId` flips, `showSessionTerminal(B)` runs, `activeSessionId = B`. Promise resolves. `terminal.paste(text)` is called on session A's xterm instance. xterm's `triggerDataEvent` fires `terminal.onData` for A. Handler at `TerminalView.tsx:165-186` checks `if (activeSessionId !== sessionId) return;` → **silently drops the paste**. PTY A doesn't receive it. PTY B doesn't receive it either. User sees nothing happen and assumes paste failed.
- **Case B — user detaches session A:** main keeps A's xterm pre-warmed (`TerminalView.tsx:259-266`). `activeSessionId` becomes null/different. Same silent drop. The detached window has its OWN xterm instance — the paste does NOT route there.
- **Case C — user destroys session A:** `disposeSessionTerminal` runs, `terminal.dispose()`. Promise resolves; `terminal.paste(text)` is invoked on a disposed Terminal. xterm v6 typically no-ops on disposed-state but is not contractually obligated to; it can throw. Plan does specify `.catch` so the throw is swallowed, but no logging means the bug is invisible during diagnosis.

**Plan does not address this. dev-webpage-ui §11.4 calls it "mild, not destructive" — disagree:** silent failure is worse than visible failure. User pastes their PR description into the wrong terminal session and never knows; or pastes a sensitive secret into a destroyed session that briefly logged the data via `pty_output` events to the cache.

**Fix.** Inside the `.then`, re-check before paste:

```ts
navigator.clipboard.readText().then(text => {
  if (!text) return;
  if (activeSessionId !== sessionId) return;  // session changed during await
  if (terminal.element === null) return;       // disposed; xterm sets element to null in dispose
  terminal.paste(sanitized(text));
}).catch(() => { /* clipboard denied */ });
```

The `terminal.element` null-check is a cheap proxy for "is this Terminal disposed". xterm does not export `_isDisposed` publicly; checking `element` is the convention in their own examples.

---

### F3. `event.code === 'KeyC'` is wrong for non-QWERTY layouts (Dvorak/Colemak/AZERTY)

- **Severity: MEDIUM** (broken for ~3–8% of users — Dvorak/Colemak/non-US layouts)
- **Reproducibility: ALWAYS** on a non-QWERTY layout
- **Recommendation: BLOCK and refine plan**

**Description.** Architect's §1.1 Refinement C asserts `event.code` is layout-independent and "matches Windows-Terminal-style binding semantics ('the physical key labeled C')". This is incorrect on two counts:

1. **`event.code === 'KeyC'` does NOT mean "the key labeled C".** Per [DOM spec](https://www.w3.org/TR/uievents-code/), `code` returns the key name that would be there on a US-QWERTY layout — it identifies the PHYSICAL POSITION (3rd row, 3rd column on ANSI), regardless of what's labeled on the keycap or what character the user's layout produces. On Dvorak, the physical QWERTY-C position is labeled "J" (Dvorak places C in QWERTY's I position). Pressing what a Dvorak user sees as the "C" key produces `event.code === 'KeyI'`, NOT `'KeyC'`. Our handler does not match. Pressing the physically-positioned QWERTY-C key (labeled "J" on Dvorak) produces `event.code === 'KeyC'` and Ctrl+Shift on that triggers our copy. Total surprise.

2. **Windows Terminal does NOT use physical key position.** WT keybindings (`"keys": "ctrl+shift+c"`) match against `WM_KEYDOWN` virtual-key codes which reflect the LAYOUT's mapping — i.e., the key labeled C in the user's active layout. So Dvorak users press Dvorak-C (QWERTY-I position) and WT copies. Our `event.code` approach is the OPPOSITE of WT.

**The codebase convention at `shortcuts.ts:52` (`e.key.toLowerCase() === shortcut.key`) is correct for international users.** Architect's "I am intentionally diverging" rationale is built on a false premise.

**Fix.** Change §1.1 Refinement C to use `event.key`:

```ts
if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'c') { ... }
```

`event.key` with Shift held returns "C" (uppercase). Lowercasing handles both Shift held / not held permutations consistently. Matches `shortcuts.ts` style. Works on every layout. The architect's worry about "uppercase vs lowercase" is solved by `.toLowerCase()`.

If keeping `event.code` for some reason (e.g. handling Caps Lock + Shift edge case), use BOTH: `event.key.toLowerCase() === 'c' || event.code === 'KeyC'`. But I see no real reason — drop `event.code`.

---

### F4. `clipboard.writeText` silent failure leaves stale clipboard content

- **Severity: MEDIUM** (user-visible UX bug; possible data leak)
- **Reproducibility: SOMETIMES** (when clipboard write fails — permission denied, focus lost, OS contention)
- **Recommendation: BLOCK and refine plan**

**Description.** Plan §2.2.2 step 1 specifies `void navigator.clipboard.writeText(...)` with "failures swallowed silently". Plan §6.5 mentions read failures but not write failures. Failure modes:

- WebView2 occasionally rejects `writeText` if the window has just lost focus (race with Alt-Tab during the keypress).
- Some Windows clipboard hooks (e.g. password managers, ditto) hold the clipboard briefly; concurrent writes throw `DOMException: Document is not focused` or `Failed to execute 'writeText' on 'Clipboard': Read permission denied`.
- Chromium-based browsers reject `writeText` if not in a user gesture context — usually fine here, but if a polyfill or `setTimeout` indirection is added later, fails silently.

**Concrete bad UX:** User has password "hunter2" in clipboard from a prior copy. Selects terminal text, presses Ctrl+Shift+C. Write fails silently. User pastes elsewhere — gets "hunter2" — possibly leaks the password into a destination they never intended (chat, command line, ticket). The user has no signal that copy failed, because the keyboard shortcut is the only feedback channel.

dev-webpage-ui §11.4 takeaway: "silent failure matches the rest of the app". I disagree — `console-capture.ts:52-55` already captures `unhandledrejection` as ERROR-level log entries, and the `copyErrors()` helper specifically exfiltrates errors. A silent rejection here generates noise in the error log without user-visible signal: worst of both worlds.

**Fix.** Two options:

1. **Minimum:** add a `.catch(err => console.warn("[paste] clipboard write failed:", err))` so it lands in the captured log but doesn't crash. NOT silent; observable for diagnosis.
2. **Better:** brief visible signal — `terminal.bell()` or a 1-second status-bar toast on failure. Out of plan scope; treat as follow-up.

Plan should require option 1 minimum and explicitly reject "silent" as the spec.

---

### F5. console-capture's `unhandledrejection` listener interacts with fire-and-forget clipboard calls

- **Severity: MEDIUM** (architect's premise wrong, affects design rationale)
- **Reproducibility: ALWAYS** if `.catch` is missed
- **Recommendation: ADD test + refine plan §11.2 wording**

**Description.** Architect §11.2 says "Add a `.catch(() => {})` to avoid unhandled-promise-rejection warnings" and notes console-capture "swallows logs anyway". This is half-right and half-wrong:

- `console.log/warn/error` are wrapped (lines 41-44 of `console-capture.ts`) and store entries in a 500-entry ring buffer.
- BUT lines 52-55 register `window.addEventListener("unhandledrejection", ...)` which capture rejected promises AS ERROR-LEVEL ENTRIES into the same buffer.

So missing `.catch(() => {})` doesn't merely produce an "unhandled rejection" devtools warning — it pollutes the captured log with permission-denied errors that show up in `copyErrors()` exports. If a user runs `copyErrors()` from a status-bar action and pastes into a bug report, we leak the names of clipboard ops they performed.

The `.catch(() => {})` is REQUIRED, not optional. Plan §11.2 should upgrade "add a `.catch`" to "REQUIRED — failure to add this leaks user activity into error logs via `console-capture.ts:52`".

**Fix.** Update §11.2 wording. Add a test (5.3.X): "press Ctrl+Shift+V with denied clipboard permission (DevTools → Application → simulate denied) and verify the error log produced by `getErrorsOnly()` does NOT contain the rejection."

---

### F6. Race: `clipboard.readText()` resolves after `terminal.dispose()` (related to F2)

- **Severity: MEDIUM** (silent state inconsistency)
- **Reproducibility: RARE** (need session-destroy within ~50ms of Ctrl+Shift+V, which can happen if peer destroys the session via API)
- **Recommendation: ADD CHECK + log**

**Description.** Same race window as F2 but a different concrete failure: `disposeSessionTerminal` calls `terminal.dispose()`. After dispose, calling `terminal.paste(text)` may throw (xterm v6 internally accesses `coreService` which is set null on dispose in some paths). The plan's `.catch(() => {})` will swallow the throw. Result: paste silently dropped, log silent (per F5 it would actually be CAPTURED, see), no diagnosis.

Worse: between keydown and resolution, another session may have been created and assigned the SAME `sessionId` (very unlikely with UUID-based IDs in `SessionManager`, but if any debug session-id reuse exists, the paste could land on the WRONG session. I checked — backend generates UUIDs, so this sub-case is theoretical).

**Fix.** Same solution as F2: re-check `terminal.element !== null` (or pick a more robust disposed-flag). The tightening is small but necessary.

---

### F7. `terminal.paste("")` and whitespace-only / CR-only clipboard

- **Severity: LOW** (cosmetic / shell-dependent)
- **Reproducibility: ALWAYS** with the trigger input
- **Recommendation: ADD test, fix on-the-fly**

**Description.** Plan §2.2.2 step 2 says `if (text)` short-circuits empty clipboard. But:

- `text === " "` (whitespace) is truthy. Calls `terminal.paste(" ")` → bracketed paste of a single space. Most shells handle. cmd.exe shows literal `^[[200~ ^[[201~` — already covered by §6.4.
- `text === "\r\n"` is truthy. `prepareTextForTerminal` collapses to `\r`. `terminal.paste(...)` wraps as `\x1b[200~\r\x1b[201~`. With bracketed paste OFF (cmd.exe, raw node), the shell sees the literal escape codes AND the CR — submits the (empty or partial) command line. Surprising.
- `text === "   "` (multiple spaces): pastes whitespace into the buffer. If shell auto-expands, fine.

**Fix.** Strengthen the guard: `if (!text || !text.trim())` — short-circuit on whitespace-only too. Or accept and add a test (5.3.X) to confirm shell behavior is benign.

---

### F8. No `event.isComposing` guard for IME composition

- **Severity: LOW** (affects CJK-input users during composition)
- **Reproducibility: SOMETIMES** (during active IME composition)
- **Recommendation: FIX on-the-fly, add test**

**Description.** When CJK IME is composing (mid-character), `event.isComposing === true`. xterm.js v6 has internal `_compositionHelper` that handles this — but `attachCustomKeyEventHandler` runs BEFORE that internal logic and our handler doesn't check `isComposing`. If user is mid-composition and accidentally chords Ctrl+Shift+C/V (some Microsoft IMEs map Ctrl+Shift to language toggle, the keys may surface as Ctrl+Shift+C with isComposing true), our handler runs preventDefault + clipboard ops. The IME composition state may corrupt or the key never reaches the IME.

**Fix.** First line of new branches: `if (event.isComposing) return true;` — let IME handle. xterm's internal composition handler then runs as usual.

**Test addition:** test 5.3.X — switch to a Microsoft Pinyin IME, start composing, press Ctrl+Shift+C — composition should not be interrupted, no copy.

---

### F9. Held-key autorepeat triggers many clipboard calls

- **Severity: LOW** (noisy, not destructive)
- **Reproducibility: ALWAYS** when shortcut is held
- **Recommendation: ACCEPT as known risk**

**Description.** dev-webpage-ui's test 5.3.8 acknowledges autorepeat. Each keydown fires `clipboard.writeText` (for copy) or `clipboard.readText` (for paste). Holding for 2 seconds at typical autorepeat rate (~30 Hz after a 500 ms initial delay) generates ~50 promises. Not destructive but:

- Paste autorepeat: pastes the same text 50 times → fills PTY buffer with 50× the bracketed content → shell receives 50 paste blocks → may execute 50 commands (one per CR if user pasted text containing CR with bracketed paste OFF).
- Write autorepeat: 50 concurrent writeText calls. Some browsers serialize, some race. Last-writer-wins on the OS clipboard — fine — but generates tail-end errors in `console-capture` from rate limiting.

**Mitigation.** Could debounce on `event.repeat === true` to skip autorepeated keydowns. One-line cost. NOT in plan but worth adding to the implementation note. Architect's plan doesn't mention `event.repeat`.

---

### F10. Last-prompt panel does not reflect pasted commands

- **Severity: LOW** (cosmetic/UX inconsistency)
- **Reproducibility: ALWAYS** with paste followed by manual Enter
- **Recommendation: ACCEPT as known**

**Description.** `TerminalView.tsx:165-186` tracks an inputBuffer and on `\r` calls `SessionAPI.setLastPrompt(sessionId, trimmed)`. The branches (`data === '\r'`, `data === '\x7f'`, `data.length === 1 && data >= ' '`, `data.length > 1 && !data.startsWith('\x1b')`) handle typed input. Pasted content arrives as one chunk starting with `\x1b[200~` → falls into the LAST branch (`data.length > 1 && !startsWith('\x1b')`)? No — paste DOES start with `\x1b`. So it falls THROUGH the chain, no buffer update.

Then user presses Enter manually. `data === '\r'` matches → `setLastPrompt(sessionId, inputBuffer.trim())` fires with the OLD/EMPTY buffer, not reflecting what was pasted. `LastPrompt` panel shows wrong text.

**Mitigation.** Out of scope. Last-prompt is heuristic — the comment at `TerminalView.tsx:165` doesn't claim correctness for pasted input. Document or accept.

---

### F11. Ordering of `event.preventDefault()` vs WebView2 DevTools accelerator

- **Severity: LOW** (only affects dev mode)
- **Reproducibility: ALWAYS** in dev (architect/dev-webpage-ui empirically verified)
- **Recommendation: ACCEPT**

**Description.** Tech-lead asked whether `preventDefault()` reliably prevents Ctrl+Shift+C from opening DevTools. xterm's `attachCustomKeyEventHandler` is called from xterm's own `_keyDown` listener attached to the offscreen `.xterm-helper-textarea`. The KeyboardEvent passed is the native event. preventDefault() marks `defaultPrevented=true` on the event. WebView2's DevTools accelerator dispatcher checks `defaultPrevented` for keys AFTER the renderer has handled them (per CDP design). So:

- Selection present → preventDefault → DevTools NOT opened. Empirically confirmed.
- No selection → return true → no preventDefault → DevTools OPENED. Empirically confirmed.

**Edge case I could not falsify in code alone:** if WebView2's accelerator runs at OS level (before renderer dispatch) for SOME shortcuts on SOME WebView2 versions (not Ctrl+Shift+C, but possible for F12 elsewhere), our preventDefault would not prevent. Empirical evidence is the only signal here. Accept; not blocker.

---

### F12. `closest('.terminal-host')` from offscreen helper textarea: confirmed safe

- **Severity: NONE — sanity-check passed**
- This isn't a finding; just confirming a tech-lead question.

dev-webpage-ui §11.1.5 verified light DOM. I additionally checked: the helper textarea is at `.terminal-host > .xterm > .xterm-helper-textarea` (xterm v6 mount path). Right-click typically lands on `.xterm-screen` (canvas) not the textarea (offscreen, opacity 0, left -9999px). Either way, `closest('.terminal-host')` resolves correctly because both ancestors include `.terminal-host`.

But — answering tech-lead's specific concern about "an overlay positioned over terminal-host": if any custom UI ever renders absolute-positioned children INSIDE `.terminal-host`, those children's right-clicks would un-block. Today: no such overlay exists. Future risk: very low; needs an explicit DOM mutation to introduce. Accept as known property of the selector design.

---

### F13. dev-webpage-ui's bubble-phase check coverage

- **Severity: NONE — confirmed by my grep**

I ran `addEventListener.*keydown` across `src/`. Bubble-phase keydown listeners on document/window:

- `src/sidebar/components/AcDiscoveryPanel.tsx:116`, `ProjectPanel.tsx` (×6), `SessionItem.tsx:208` — dismiss handlers, only act on `key === "Escape"`. No interference.
- `src/shared/shortcuts.ts:61` — only matches Ctrl+Shift+N/W/R. No interference with C/V.
- `src/shared/zoom.ts:106` — early-returns on `e.shiftKey`. No interference.
- `src/main/components/QuitConfirmModal.tsx:66` — capture phase, only Escape/Enter/Tab. dev-webpage-ui §11.1.1 verified.

**Conclusion: no bubble-phase listener intercepts Ctrl+Shift+C/V before xterm's helper textarea sees it. dev-webpage-ui's Refinement A safety claim holds.**

---

### F14. Browser-mode `BrowserApp` mounts SidebarApp without `embedded` prop

- **Severity: LOW** (alignment / scope clarity)
- **Reproducibility: ALWAYS** in browser mode
- **Recommendation: NOTE in plan; track for #105**

**Description.** `src/browser/App.tsx:59` mounts `<SidebarApp />` (no `embedded` prop). `SidebarApp.embedded` is undefined → falsy. Side effects in browser mode:

- Line 92-94: `handleRaiseTerminal` mousedown listener IS registered. But it early-returns on `!isTauri` (line 58), so harmless.
- Line 97: `blockContextMenu` IS registered. Plan §6.6 says browser-mode incidental fix is accepted. ✓
- Line 75-78: `cleanupZoom`/`cleanupGeometry` initialized. Already-existing browser-mode behavior (not new).

Plan §7.1 closed this. No new finding — just confirming tech-lead's accepted scope.

---

### F15. xterm `terminal.element` after `terminal.dispose()` — caveat

- **Severity: VERY LOW** (relevant only as part of F2/F6 fix)
- **Recommendation: USE during F2 fix**

**Description.** xterm v6's `Terminal.dispose()` sets `this.element = null` (and runs disposable disposal). Reading `terminal.element` is the public-ish way to detect disposal. NOT documented as stable API but used in xterm's own examples. If F2 fix uses this check, document the brittleness — a future xterm minor could change `element` to remain set. Mitigation: vendor a `let disposed = false; terminal.onWriteParsed(() => {})` no-op subscriber and watch its `.dispose()`-thrown error. Overkill for now; `element` check is acceptable.

---

### F16. The `if (text)` paste short-circuit allows partial-paste race

- **Severity: VERY LOW**
- **Reproducibility: NEVER REPRODUCIBLE in practice**
- **Recommendation: NONE — acknowledge only**

If the OS clipboard is being WRITTEN concurrently by another app while our `readText` reads, WebView2 typically blocks until the write settles. No partial reads. Accept.

---

### Summary of findings

| # | Severity | Action |
|---|---|---|
| F1 | CRITICAL | BLOCK — sanitize input before `terminal.paste` |
| F2 | HIGH | BLOCK — re-check active session in `.then` |
| F3 | MEDIUM | BLOCK — switch to `event.key.toLowerCase()` |
| F4 | MEDIUM | BLOCK — replace silent swallow with `console.warn` |
| F5 | MEDIUM | BLOCK — upgrade `.catch` from "nice to have" to REQUIRED |
| F6 | MEDIUM | BLOCK — covered by F2 fix |
| F7 | LOW | Strengthen guard or add test |
| F8 | LOW | Fix on-the-fly: `if (event.isComposing) return true` |
| F9 | LOW | Accept; consider `event.repeat` skip |
| F10 | LOW | Accept |
| F11 | LOW | Accept (empirically verified) |
| F12 | — | Sanity passed |
| F13 | — | Bubble-phase coverage confirmed |
| F14 | LOW | Already accepted by tech-lead |
| F15 | V.LOW | Note for F2 fix |
| F16 | V.LOW | Accept |

### Tests I would require added before merge

- **5.3.10** Paste-injection probe: clipboard contains `safe\x1b[201~MALICIOUS\r`. Verify `MALICIOUS` is NOT executed; appears as part of the paste (or stripped). (F1)
- **5.3.11** Session-switch race: paste, immediately Ctrl+Tab to another session before clipboard resolves. Verify neither session sees the paste OR original session sees it consistently. (F2)
- **5.3.12** Dvorak layout: switch keyboard layout to Dvorak via Win+Space, press the labeled C key with Ctrl+Shift held. Verify copy fires. (F3)
- **5.3.13** Permission-denied clipboard: with `Permissions-Policy: clipboard-read=()` injected (DevTools → Application), press Ctrl+Shift+V. Verify a warning lands in console-capture (F4) but no error spam (F5).
- **5.3.14** IME composition: switch to Microsoft Pinyin IME, start composing "ni hao", press Ctrl+Shift+C mid-composition. Verify composition NOT interrupted, no copy. (F8)

### Final verdict

**BLOCK and return to architect.** F1 alone is non-negotiable (security). F2 + F3 + F4 + F5 are blocking by my standard but architect/tech-lead may dispute and choose to accept. The remaining items are optional polish.

I am NOT approving as-is. If architect refines per the recommendations above, I'll re-review.

— grinch, 2026-05-01
