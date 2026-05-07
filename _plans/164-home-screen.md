# Plan: Home Screen with Remote Markdown Docs

**Branch:** `feature/164-home-screen-docs`
**Issue:** #164
**Scope:** new sidebar action button, new in-pane Home view in unified main window (rendered as a CSS overlay so the active session keeps its scrollback), new backend command to fetch remote markdown, new shared store, new npm deps for Markdown rendering + sanitization, new dev-dep for DOM-environment vitest
**Status:** Ready for implementation (rev 2 — Grinch findings 1–8 resolved 2026-05-07)
**Supersedes:** `_plans/164-home-screen-docs.md` (dev-webpage-ui draft) — see §Supersession at the end of this file.

---

## Requirement

- On the initial AgentsCommander screen shown when there is **no active session**, render a **Home view** that displays remote Markdown documentation explaining how to start using AgentsCommander with a basic group of agents.
- Source URL: `https://github.com/mblua/AgentsCommander/blob/mblua-patch-1/docs/home.md` (UI URL — must be converted to the raw URL for fetch, see §Design D2).
- Markdown must render **as formatted HTML** (headings, code blocks, lists, links), not as raw text.
- Add a **Home button** in `ActionBar`'s `.action-bar-icons` row, positioned **immediately to the left of the existing flame button** (`coord-sort-activity-btn`).
- Home button **toggles** Home visibility. Active state mirrors the flame button pattern (`.active` CSS class).
- Detached terminal windows MUST NOT render Home (they are locked to a specific session).
- Browser/web mode (non-Tauri) is out of scope for this issue (see §Constraints).

---

## Design Summary

Five architectural decisions, each justified inline below.

### D1. Where Home state lives

New singleton store at **`src/main/stores/home.ts`** holding:
- `visible: boolean` — whether Home pane is shown in the terminal pane area.
- `content: string | null` — fetched Markdown source.
- `loading: boolean` — request in flight.
- `error: string | null` — last fetch error.

Methods: `toggle()`, `show()`, `hide()`, `setInitialVisibility(hasActiveSession)`, `fetch()`.

**Why a store, not a Tauri event?**
- The unified `MainApp` embeds `SidebarApp` and `TerminalApp` in the **same JS context** (`MainApp.tsx:213-232` mounts `<SidebarApp embedded />` and `<TerminalApp embedded />` side-by-side), so the Home button (sidebar) and the Home view (terminal pane) can share a singleton SolidJS store directly — no IPC needed.
- Detached terminal windows are separate WebviewWindows but MUST NOT render Home, so cross-window event broadcast is unnecessary.
- The store-singleton pattern is already in use across the app: `terminalStore` (`src/terminal/stores/terminal.ts`), the various sidebar stores (`src/sidebar/stores/*`), and shared cross-window stores (`src/shared/stores/settings.ts`). Home reuses that pattern, just under a new directory (see below).

**Why `src/main/stores/` (a NEW location), not `src/sidebar/` or `src/terminal/` or `src/shared/stores/`?**
- This directory does **not exist yet** (verified — `src/main/` currently contains only `App.tsx`, `entry.tsx`, `components/`, `styles/`). This change creates `src/main/stores/`. It is a NEW convention, not a continuation of an existing one.
- Cross-pane stores (read by Sidebar AND Terminal panes within the unified window) belong under `src/main/` because `MainApp` owns the unified layout that hosts both panes. Putting the store under `src/sidebar/` or `src/terminal/` would force the other pane to reach into a sibling's namespace, which the existing per-pane stores deliberately avoid.
- Putting it under `src/shared/stores/` would imply cross-window sharing (settings does that). Home is unified-window-only — detached/standalone terminals MUST NOT render it. Co-locating with `MainApp` signals that scope.
- Future cross-pane state should also land in `src/main/stores/`. This plan establishes the convention.

**Initial visibility rule:**
- On `MainApp.onMount`, after `SessionAPI.getActive()` resolves, call `homeStore.setInitialVisibility(activeId !== null)`.
- If no active session at boot → `visible = true` (Home shows by default — replaces "+ New Session" empty state).
- If active session exists at boot → `visible = false` (terminal renders normally; user may click Home button to overlay).
- After boot, `visible` is changed only by the Home button **and** by the auto-hide rule below.

**Auto-hide rule:**
- On `onSessionCreated`, call `homeStore.hide()` once. Rationale: creating a session implies user wants to use it; Home is dismissed so the new session is immediately visible. Subsequent `onSessionSwitched` does NOT auto-hide — the user may want to keep Home open while idly switching.

**Auto-show rule (last-session-destroyed flip-back):**
- On `onSessionDestroyed`, after the existing `loadActiveSession()` flow has settled, if `terminalStore.activeSessionId === null` (i.e. there are no remaining sessions to fall back to), call `homeStore.show()`.
- **Why:** without this, destroying the last session leaves the user staring at the bare "No active session" empty state — a worse landing surface than Home, which is the curated entry point. Auto-flipping back when (and only when) there is *nothing else to show* respects the user's earlier hide intent (they still see the terminal pane normally as long as a session exists) while preventing the "empty pane" UX dead-end.
- **Where it is wired:** §B3 — see the `onSessionDestroyed` listener appended in `MainApp.onMount`. Fire only in the unified main window (not in detached terminals, which have their own destroy handling).
- This is the ONLY session-lifecycle event that reopens Home. Switching sessions, renaming, etc., never touch Home visibility.

### D2. Backend fetch via Tauri command, not frontend `fetch()`

New Rust command **`fetch_home_markdown`** in `src-tauri/src/commands/config.rs` (existing module — already houses miscellaneous app-level commands). Returns `Result<String, String>` with the raw Markdown body.

**Why backend, not frontend `fetch()`?**
- This codebase has **zero frontend `fetch()` calls** today. Every outbound HTTP request goes through `reqwest` in Rust (see `commands/voice.rs:146`, `commands/telegram.rs:97`, `telegram/bridge.rs:498`). Established convention.
- Tauri 2 does not enforce a CSP unless `app.security.csp` is configured (it isn't in `tauri.conf.json`), so frontend `fetch()` would currently work — but if a CSP is added later, frontend fetch would silently break. Backend reqwest is immune.
- Backend can centralize timeout, user-agent, error mapping, and (later) caching without touching the frontend.
- The `reqwest` crate is already a dependency — no new Rust crate needed.

**URL handling:**
- The user-facing URL `https://github.com/mblua/AgentsCommander/blob/mblua-patch-1/docs/home.md` is a GitHub UI URL; fetching it returns the GitHub website HTML, not the Markdown source. The command MUST fetch the raw URL: `https://raw.githubusercontent.com/mblua/AgentsCommander/mblua-patch-1/docs/home.md`.
- Hardcode the raw URL as a `const` at the top of `config.rs`. Do not parameterize via the frontend — the source URL is a feature decision, not a per-call parameter.

**Network policy:**
- 5-second total request timeout (`reqwest::Client::builder().timeout(...)`).
- User-agent string: `agentscommander/{CARGO_PKG_VERSION}` (avoid GitHub raw rate-limit edge cases).
- Non-2xx HTTP status maps to `Err("Server returned status {n}")`.
- Network/IO errors map to `Err("Network error: {e}")`.
- Empty body maps to `Err("Server returned empty response")`.
- Body size cap: refuse responses larger than **256 KB** to avoid pathological inputs (`Err("Response too large")`).

### D3. Markdown rendering

Use **`markdown-it`** (parser) + **`dompurify`** (sanitizer). Both as new `dependencies` in `package.json`.

**Why these two?**
- `markdown-it` is mature, fast, ESM-friendly with Vite, has TypeScript types via `@types/markdown-it`, and crucially supports `html: false` which **disables raw HTML in the source**. That alone removes most XSS surface for trusted Markdown.
- `dompurify` is the industry-standard HTML sanitizer. Even with `html: false`, applying DOMPurify to the parser output is defense-in-depth: if the source is ever rewritten, or if a future markdown-it plugin emits a vector, DOMPurify catches it.
- Combined gzipped size is roughly ~50 KB — acceptable for a Tauri desktop app.

**Why not `solid-markdown` or `marked`?**
- `solid-markdown` adds an additional abstraction over an existing JS markdown lib, has fewer maintainers, and is overkill — we render one document into a static container.
- `marked` works too, but `marked` does NOT sanitize by default and its built-in sanitizer was deprecated. Pairing `marked` with DOMPurify is fine but `markdown-it`'s `html: false` is a stronger primary defence than `marked`'s default behavior.

**Configuration:**
```ts
const md = MarkdownIt({
  html: false,        // do not allow raw HTML in source
  linkify: true,      // auto-link bare URLs
  typographer: false, // no smart-quote replacement (preserves code)
  breaks: false,      // standard CommonMark line-break behavior
});
```

**Sanitization:**
```ts
const dirty = md.render(source);
const clean = DOMPurify.sanitize(dirty, {
  USE_PROFILES: { html: true },
});
```

`ADD_ATTR: ["target", "rel"]` is intentionally NOT passed: DOMPurify's HTML profile already permits `target` and `rel` on anchors, so it is redundant. More importantly, this plan does NOT post-process anchors to add `target=_blank`, because (see Link handling below) the click is intercepted before the browser ever uses those attributes. Adding them would be dead code and forces a costly DOM round-trip per render.

**Link handling:**
- The DOMPurify output is rendered directly via the `innerHTML` prop on the container — no DOM round-trip via a `<div>` scratchpad.
- A single delegated `click` handler on the container (`onContainerClick`) intercepts ALL anchor clicks, calls `event.preventDefault()`, and dispatches via `WindowAPI.openExternal(url)` (see §D5). This is the ONLY external-open path; `target=_blank` is irrelevant because the browser never gets to handle the click.
- **Why no post-process pass?** A `tmp.innerHTML = clean; tmp.querySelectorAll('a').forEach(...); return tmp.innerHTML;` round-trip parses then re-serializes the HTML (potentially up to 256 KB) on every memo recompute, is brittle to whitespace/attribute-ordering changes, and adds zero security value given click delegation. Drop it.

### D4. Loading / error / retry / fallback

Four UI states inside the Home view, all rendered by `HomeView.tsx`:

1. **Loading** — `loading === true && content === null && error === null`
   - Centered spinner + text "Loading Home…".
2. **Error** — `error !== null && content === null`
   - Error message (`error` text) + a "Try again" button that calls `homeStore.fetch()`.
3. **Success** — `content !== null`
   - Rendered Markdown in a scrollable `.home-markdown` container.
4. **Initial idle** — `content === null && loading === false && error === null`
   - Same UX as Loading; first call to `fetch()` happens on first mount when `visible` becomes `true`.

**Caching:**
- Once `content !== null`, cache it in the store for the rest of the app session. Toggling Home off then back on does NOT re-fetch.
- Optional manual refresh: a small ↻ icon button in the top-right of the Home view that clears `content` and re-runs `fetch()`. Implement this in this plan — it is the supported retry path while content is already loaded.

**No auto-retry:**
- A failed fetch does NOT retry on its own. The "Try again" button is the only recovery path. Rationale: silent retries can mask real outages; explicit user action is clearer and avoids retry storms during long network failures.

**Lifecycle:**
- On mount of `HomeView`, if `content === null && error === null && !loading`, call `homeStore.fetch()`.
- `homeStore.fetch()` is idempotent (early-return when `loading === true`).

### D5. External link opening

New Rust command **`open_external_url`** in `src-tauri/src/commands/window.rs` (next to the existing `open_in_explorer`). Validates the scheme is `http://` or `https://` and dispatches via the existing `open` crate (already a Cargo dep, already used by `open_in_explorer` — no new crate).

```rust
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("Refusing to open non-http(s) URL: {}", url));
    }
    open::that_detached(trimmed).map_err(|e| format!("Failed to open URL: {}", e))
}
```

**Scheme check is case-insensitive (RFC 3986 §3.1):**
- `lower` is used ONLY for the validation; the original (trimmed) URL is passed to `open::that_detached`. This accepts `HTTP://example.com`, `Https://...`, and similar mixed-case schemes that linkifiers occasionally emit, while still rejecting `mailto:`, `file:`, `javascript:`, etc.
- `to_ascii_lowercase` (not `to_lowercase`) is intentional — schemes are pure ASCII per spec; ASCII-lowercase is faster and cannot turn legitimate input into something else via Unicode case-folding edge cases.
- `trim()` is defensive — markdown sometimes wraps URLs with leading/trailing whitespace from sloppy autolinking. Same trimmed string is passed to `open` so the OS receives a clean URL.

**Why a dedicated command and not `tauri-plugin-shell`?**
- `tauri-plugin-shell` is in `package.json` but is NOT registered in `src-tauri/Cargo.toml`, NOT initialized in `lib.rs`, and NOT in `capabilities/default.json`. Wiring it up is more invasive than reusing the existing `open` crate.
- The `open` crate is already used by `open_in_explorer` (window.rs:283) — same primitive, different argument.

---

## Affected Files

### A. Frontend — new files

#### A1. `src/main/stores/home.ts` (NEW)

Create a singleton store implementing the contract in §D1.

```ts
import { createSignal } from "solid-js";
import { HomeAPI } from "../../shared/ipc";

const [visible, setVisible] = createSignal(false);
const [content, setContent] = createSignal<string | null>(null);
const [loading, setLoading] = createSignal(false);
const [error, setError] = createSignal<string | null>(null);

export const homeStore = {
  get visible() { return visible(); },
  get content() { return content(); },
  get loading() { return loading(); },
  get error() { return error(); },

  // Called once from MainApp.onMount after SessionAPI.getActive resolves.
  // After boot, visibility is user-controlled.
  setInitialVisibility(hasActiveSession: boolean) {
    setVisible(!hasActiveSession);
  },

  toggle() { setVisible((v) => !v); },
  show() { setVisible(true); },
  hide() { setVisible(false); },

  // Idempotent. Sets loading + error appropriately.
  async fetch() {
    if (loading()) return;
    setLoading(true);
    setError(null);
    try {
      const text = await HomeAPI.fetchMarkdown();
      setContent(text);
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg);
    } finally {
      setLoading(false);
    }
  },

  // Manual refresh — re-runs fetch but does NOT wipe currently-displayed content.
  // If the refetch fails, the user keeps seeing the last-good content; the
  // error surfaces only via the (transient) error signal, which the view
  // shows non-destructively.
  async refresh() {
    setError(null);
    await this.fetch();
  },
};

// Test-only reset (gated to NODE_ENV === "test"). Resets every signal so
// vitest tests do not leak state across cases. Safe to expose because the
// gate prevents prod surface and the export is a no-op outside tests.
export function __resetHomeStoreForTests() {
  if (process.env.NODE_ENV !== "test") return;
  setVisible(false);
  setContent(null);
  setLoading(false);
  setError(null);
}
```

**Notes:**
- Use plain SolidJS `createSignal`, not `createStore`. The state is flat and there is no nested reactivity to manage.
- Do NOT persist Home visibility to settings. It is per-session UI state only. Persisting would surprise users who dismissed Home weeks ago.
- `refresh()` deliberately does NOT call `setContent(null)` first. Wiping content before a refetch causes a visible regression to the empty state when the refetch fails (corporate proxy hiccup, transient 5xx, etc.) — the user loses perfectly-good content for no benefit, since `fetch()` already overwrites `content` on success. Clearing `error` at the start is safe (the spinner takes over), but `content` must be preserved across the refetch. View-side rule: when `loading === true && content !== null`, show the existing content with a non-blocking refresh indicator (e.g. spinner on the ↻ button), NOT the centered "Loading Home…" status.

#### A2. `src/main/components/HomeView.tsx` (NEW)

```tsx
import { Component, Show, createMemo, onMount } from "solid-js";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import { homeStore } from "../stores/home";
import { WindowAPI } from "../../shared/ipc";

const md = MarkdownIt({
  html: false,
  linkify: true,
  typographer: false,
  breaks: false,
});

const HomeView: Component = () => {
  onMount(() => {
    if (homeStore.content === null && homeStore.error === null && !homeStore.loading) {
      homeStore.fetch();
    }
  });

  const html = createMemo(() => {
    const src = homeStore.content;
    if (!src) return "";
    return DOMPurify.sanitize(md.render(src), {
      USE_PROFILES: { html: true },
    });
  });

  const onContainerClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    const anchor = target.closest("a") as HTMLAnchorElement | null;
    if (!anchor) return;
    const href = anchor.getAttribute("href") ?? "";
    if (!href) return;
    e.preventDefault();
    WindowAPI.openExternal(href).catch((err) => console.error("openExternal failed:", err));
  };

  return (
    <div class="home-view">
      <div class="home-toolbar">
        <button
          class="home-refresh-btn"
          title="Refresh"
          disabled={homeStore.loading}
          onClick={() => homeStore.refresh()}
        >
          ↻
        </button>
      </div>
      <Show when={homeStore.loading && homeStore.content === null}>
        <div class="home-status">Loading Home…</div>
      </Show>
      <Show when={homeStore.error && homeStore.content === null}>
        <div class="home-status home-status-error">
          <p>Could not load Home: {homeStore.error}</p>
          <button class="home-retry-btn" onClick={() => homeStore.fetch()}>
            Try again
          </button>
        </div>
      </Show>
      <Show when={homeStore.content !== null}>
        <div
          class="home-markdown"
          onClick={onContainerClick}
          // eslint-disable-next-line solid/no-innerhtml
          innerHTML={html()}
        />
      </Show>
    </div>
  );
};

export default HomeView;
```

**Notes:**
- `innerHTML` is the only way to render a string of HTML in SolidJS. The lint rule `solid/no-innerhtml` is intentionally suppressed here because the source is sanitized by DOMPurify at the boundary. Do NOT bypass DOMPurify.
- The toolbar lives at the top of the Home view because the Home button in `ActionBar` only toggles visibility, not refresh.
- The `<div class="home-view">` element is the **overlay container** — see §B2 for the `position: absolute; inset: 0` placement that lets it sit on top of the still-mounted `TerminalView` without unmounting it.

#### A3. `src/main/styles/main.css` (EDIT — append at end)

Add styling for the Home view. Match existing CSS variables (used by `.terminal-empty`, `.toolbar-gear-btn`, etc.).

```css
/* Home view (issue #164)
 * Architectural note: .home-view is rendered as an overlay sibling of the
 * terminal/empty <Show> block (see §B2). It is absolutely positioned inside
 * .terminal-content-area so TerminalView remains mounted underneath. */

.terminal-content-area {
  position: relative;       /* containing block for .home-view absolute */
  flex: 1;
  min-height: 0;            /* allow inner flex children to shrink */
  display: flex;
  flex-direction: column;
}

.home-view {
  position: absolute;
  inset: 0;
  z-index: 10;              /* above .terminal-host */
  display: flex;
  flex-direction: column;
  background: var(--terminal-bg, #0a0e14);
  color: var(--statusbar-fg);
  overflow: hidden;
}

.home-toolbar {
  display: flex;
  justify-content: flex-end;
  padding: var(--spacing-xs) var(--spacing-sm);
  border-bottom: 1px solid var(--sidebar-border);
  flex-shrink: 0;
}

.home-refresh-btn {
  width: 28px;
  height: 28px;
  border: none;
  background: transparent;
  color: var(--sidebar-fg-dim);
  cursor: pointer;
  border-radius: var(--radius-md);
  font-size: 16px;
  transition: background var(--transition-fast), color var(--transition-fast);
}
.home-refresh-btn:hover { background: var(--sidebar-hover); color: var(--sidebar-fg); }
.home-refresh-btn:disabled { opacity: 0.4; cursor: not-allowed; }

.home-status {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  flex: 1;
  gap: var(--spacing-md);
  font-size: var(--font-size-md);
  padding: var(--spacing-lg);
  text-align: center;
}
.home-status-error { color: #ff6b6b; }
.home-retry-btn {
  padding: var(--spacing-sm) var(--spacing-md);
  border: 1px solid rgba(0, 212, 255, 0.3);
  background: rgba(0, 212, 255, 0.08);
  color: var(--statusbar-accent);
  font-family: var(--font-ui);
  font-size: var(--font-size-sm);
  cursor: pointer;
  border-radius: 6px;
  transition: background var(--transition-fast);
}
.home-retry-btn:hover { background: rgba(0, 212, 255, 0.15); }

.home-markdown {
  flex: 1;
  overflow-y: auto;
  padding: var(--spacing-lg) calc(var(--spacing-lg) * 1.5);
  line-height: 1.55;
  font-family: var(--font-ui);
  font-size: var(--font-size-md);
  color: var(--sidebar-fg);
}
.home-markdown h1, .home-markdown h2, .home-markdown h3 {
  margin-top: 1.4em;
  margin-bottom: 0.5em;
  color: var(--sidebar-fg);
}
.home-markdown h1 { font-size: 1.6em; border-bottom: 1px solid var(--sidebar-border); padding-bottom: 0.3em; }
.home-markdown h2 { font-size: 1.3em; }
.home-markdown h3 { font-size: 1.1em; }
.home-markdown p, .home-markdown ul, .home-markdown ol { margin: 0.6em 0; }
.home-markdown ul, .home-markdown ol { padding-left: 1.6em; }
.home-markdown li { margin: 0.2em 0; }
.home-markdown a { color: var(--statusbar-accent); text-decoration: underline; }
.home-markdown a:hover { color: var(--sidebar-accent); }
.home-markdown code {
  font-family: var(--font-mono, monospace);
  background: rgba(255,255,255,0.06);
  padding: 0.1em 0.35em;
  border-radius: 4px;
  font-size: 0.92em;
}
.home-markdown pre {
  background: rgba(0,0,0,0.4);
  padding: var(--spacing-md);
  border-radius: 6px;
  overflow-x: auto;
  margin: 0.8em 0;
}
.home-markdown pre code { background: transparent; padding: 0; font-size: 0.9em; }
.home-markdown blockquote {
  border-left: 3px solid var(--sidebar-border);
  padding-left: var(--spacing-md);
  color: var(--sidebar-fg-dim);
  margin: 0.8em 0;
}
.home-markdown img { max-width: 100%; height: auto; }

/* Light theme overrides */
.light-theme .home-markdown code { background: rgba(0,0,0,0.06); }
.light-theme .home-markdown pre { background: rgba(0,0,0,0.05); }
```

**Notes:**
- If `--terminal-bg`, `--font-mono`, or `--font-ui` are not defined in `variables.css`, the fallbacks above keep the styling readable. Verify variable names against `src/sidebar/styles/variables.css` before committing.
- Do NOT introduce new CSS variables in `variables.css` for this feature unless an existing one is missing for an essential property — keep blast radius minimal.

### B. Frontend — edits to existing files

#### B1. `src/sidebar/components/ActionBar.tsx`

**Current anchor:** `.action-bar-icons` block, line 151–202. The flame button is the **first** child (lines 152–159).

**Change:** Insert the Home button BEFORE the flame button (so it renders to the left in DOM order).

Add to imports at the top of the file:
```ts
import { homeStore } from "../../main/stores/home";
```

Insert at line 152 (immediately after `<div class="action-bar-icons">`, before the flame `<button>`):

```tsx
<button
  class={`toolbar-gear-btn home-toggle-btn ${homeStore.visible ? "active" : ""}`}
  onClick={() => homeStore.toggle()}
  title={homeStore.visible ? "Hide Home" : "Show Home"}
  aria-label={homeStore.visible ? "Hide Home" : "Show Home"}
  aria-pressed={homeStore.visible}
>
  &#x1F3E0;
</button>
```

**Rules:**
- Use the `toolbar-gear-btn` base class so the visual matches the other icon buttons.
- The `home-toggle-btn` modifier is reserved for any Home-specific styling (none required initially, but kept for parity with `coord-sort-activity-btn` and `sounds-mute-btn`).
- `&#x1F3E0;` is 🏠 (HOUSE BUILDING). HTML-entity form vs. raw-emoji literal is a wash — the same file mixes both (`&#x1F525;` line 158, `🔊`/`🔇` line 167). Either form is fine; the entity form was chosen here for greppability of all home-button references.
- Do NOT add any other props or import side-effects.

#### B2. `src/terminal/App.tsx`

**Current anchor:** lines 197–228, the `<div class="terminal-layout">` JSX with the active-session/empty `<Show>` block at lines 204–225.

**Change (CSS-overlay approach — load-bearing):** Render `HomeView` as an **absolutely-positioned overlay** that sits on top of the existing terminal content area. The existing `<Show when={terminalStore.activeSessionId}>` block is preserved **unchanged** — `TerminalView` stays mounted while Home is visible, so xterm.js scrollback, the `onPtyOutput` listener, and the WebGL context survive Home toggling.

This replaces the rev-1 design (which wrapped the inner Show and unmounted TerminalView). That approach was a regression: TerminalView's `onCleanup` (`src/terminal/components/TerminalView.tsx:340-349`) unregisters `onPtyOutput`, calls `terminal.dispose()` on every xterm instance, and removes the container from the DOM. Subsequent re-mounts construct a fresh empty xterm — scrollback gone, output that arrived during the Home view also gone (no listener active). The CSS-overlay approach avoids all of this because TerminalView is never unmounted.

Add to imports at the top of the file:
```ts
import { homeStore } from "../main/stores/home";
import HomeView from "../main/components/HomeView";
```

Replace the existing JSX inside `<div class="terminal-layout">` (lines 197–228) with:

```tsx
return (
  <div class="terminal-layout">
    <Show when={!props.embedded}>
      <Titlebar detached={props.detached} lockedSessionId={props.lockedSessionId} />
    </Show>
    <WorkgroupBrief />
    <LastPrompt sessionId={props.lockedSessionId} />
    <div class="terminal-content-area">
      <Show
        when={terminalStore.activeSessionId}
        fallback={
          <div class="terminal-empty">
            <span>
              {props.detached
                ? "Session closed"
                : "No active session"}
            </span>
            <Show when={!props.detached}>
              <button
                class="terminal-empty-btn"
                onClick={() => SessionAPI.create()}
              >
                + New Session
              </button>
            </Show>
          </div>
        }
      >
        <TerminalView lockedSessionId={props.lockedSessionId} />
      </Show>
      <Show when={props.embedded && !props.detached && !props.lockedSessionId && homeStore.visible}>
        <HomeView />
      </Show>
    </div>
    <StatusBar detached={props.detached} />
  </div>
);
```

**What changed structurally:**
- Introduce a new wrapper `<div class="terminal-content-area">` between the context panels and the StatusBar. It is `position: relative; flex: 1; min-height: 0; display: flex; flex-direction: column;` (see §A3 CSS additions).
- The existing terminal/empty `<Show>` is moved INSIDE the wrapper, completely unchanged.
- `<HomeView />` is a sibling inside the same wrapper, rendered conditionally. CSS positions it `position: absolute; inset: 0; z-index: 10` so it visually covers the terminal/empty without unmounting it.

**Rules:**
- The visibility guard `props.embedded && !props.detached && !props.lockedSessionId && homeStore.visible` is load-bearing. Detached terminal windows, the legacy standalone terminal route, and locked-session contexts MUST NOT render Home — they go straight to TerminalView/empty. The first three flags are static for a given window; only `homeStore.visible` flips at runtime.
- `TerminalView` MUST stay mounted while Home is visible. This is the entire point of the rewrite. Do NOT re-introduce a wrapping `<Show when={!homeStore.visible}>` around `<TerminalView />` — that would unmount it.
- Do NOT add a separate `display: none` toggle on `TerminalView`. xterm.js handles its own visibility (`terminal-instance` containers use `hidden` flags managed by `showSessionTerminal`). Letting CSS hide the host could disrupt fit-addon sizing on re-show; the overlay approach sidesteps this entirely because TerminalView is still rendered, just visually behind the overlay.
- The `.terminal-content-area` wrapper is required for the absolute-positioning containing block. Without it, `position: absolute` on `.home-view` would resolve to the document root.

**Why this is safer than CSS `display: none` on TerminalView:**
- An overlay layer is a pure visual change. TerminalView's React lifecycle is untouched; `createEffect` on `terminalStore.activeSessionId` continues to drive `showSessionTerminal`; PTY output keeps streaming into the existing xterm buffer.
- Toggling Home is O(1) — no xterm construction, no WebGL context allocation, no fit-addon re-sizing race. The 16-context WebGL budget noted at `TerminalView.tsx:32-33` is unaffected.

#### B3. `src/main/App.tsx`

**Current anchor:** the `onMount` block at line 145–190. Specifically, `SettingsAPI.get()` (line 155) returns `appSettings`, but the **active session** isn't queried in MainApp today — `TerminalApp` does that itself in its own onMount.

**Change:** Set Home initial visibility based on whether an active session exists at boot.

Add to imports at the top of the file:
```ts
import { SessionAPI, onSessionCreated, onSessionDestroyed } from "../shared/ipc";
import { homeStore } from "./stores/home";
```

Inside `onMount`, after the existing `try { ... settings = await SettingsAPI.get() ...}` block (around line 165) and before `window.addEventListener("resize", ...)` (line 167), add:

```ts
// Home initial visibility — true when no active session at boot.
try {
  const activeId = await SessionAPI.getActive();
  homeStore.setInitialVisibility(activeId !== null);
} catch (e) {
  console.error("[home] Failed to read initial active session:", e);
  homeStore.setInitialVisibility(false);
}

// Auto-hide Home when a session is created (user wants to use the new session).
unlisteners.push(
  await onSessionCreated(() => {
    homeStore.hide();
  })
);

// Auto-show Home when the LAST session is destroyed (no remaining sessions
// to fall back to). Avoids dropping the user on the bare "No active session"
// empty state. We re-query active state because TerminalApp's own destroy
// handler may still be in flight; a small delay (microtask) gives it time
// to settle. If a fallback session exists, leave Home alone.
unlisteners.push(
  await onSessionDestroyed(async () => {
    // Yield once so TerminalApp's onSessionDestroyed → loadActiveSession()
    // can update terminalStore.activeSessionId before we read it.
    await Promise.resolve();
    try {
      const remaining = await SessionAPI.list();
      if (remaining.length === 0) {
        homeStore.show();
      }
    } catch (e) {
      console.error("[home] Failed to query session list after destroy:", e);
    }
  })
);
```

**Rules:**
- This is the ONLY place Home auto-hide AND auto-show are wired. Do not add similar logic to `SidebarApp.onMount` or `TerminalApp.onMount` — duplicate listeners would fire twice (harmless but wasteful and harder to reason about).
- The `SessionAPI.getActive()` call here is in addition to the one already inside `TerminalApp.onMount`. Both windows reading active state is a known existing pattern (see SidebarApp:150) and is cheap.
- The auto-show uses `SessionAPI.list().length === 0` rather than `SessionAPI.getActive() === null`, because `getActive()` can transiently be null during a switch even when sessions remain. The list-empty signal is the unambiguous "no fallback" condition.
- Do NOT auto-hide on `onSessionSwitched`. Switching sessions while Home is open is a valid intent.
- Do NOT auto-show on `onSessionDestroyed` when `remaining.length > 0`. The user still has sessions to look at; the existing terminal-pane logic will switch to one of them, and Home should not crowd the screen.

### C. Frontend — typed IPC wrappers

#### C1. `src/shared/ipc.ts`

**Current anchors:**
- `GuideAPI` block: lines 449–452 (template for new typed API).
- Existing `WindowAPI` (search the file for `export const WindowAPI`).

**Change:** Add two new typed APIs.

After the existing `WindowAPI` definition, add the `openExternal` method to `WindowAPI` (don't create a new `ExternalAPI`):

```ts
// Inside WindowAPI = { ... existing ..., 
openExternal: (url: string) => transport.invoke<void>("open_external_url", { url }),
```

If a `WindowAPI` doesn't exist yet (verify by grep before editing), add `openExternal` to the most natural existing API namespace; if none fit, create a small `ExternalAPI`. **Verify before edit.**

Add a new `HomeAPI` block immediately after `GuideAPI` (line 452):

```ts
// Home content (issue #164)
export const HomeAPI = {
  fetchMarkdown: () => transport.invoke<string>("fetch_home_markdown"),
};
```

**Rules:**
- Both commands take/return primitives — no new TypeScript interface needed.
- No event listener helpers required (Home is request/response only).

### D. Backend — Rust

#### D1. `src-tauri/src/commands/config.rs`

**Change:** Add the `fetch_home_markdown` command.

Add at the top of the file (after existing imports):
```rust
const HOME_MARKDOWN_URL: &str =
    "https://raw.githubusercontent.com/mblua/AgentsCommander/mblua-patch-1/docs/home.md";

const HOME_MARKDOWN_MAX_BYTES: usize = 256 * 1024; // 256 KB
const HOME_MARKDOWN_TIMEOUT_SECS: u64 = 5;
```

Add the command at the end of the file:

```rust
/// Fetch the Home screen Markdown source from the public docs URL.
/// Returns the raw Markdown body as a String.
/// Errors are returned as user-facing strings; the frontend renders them in
/// the Home view's error state.
#[tauri::command]
pub async fn fetch_home_markdown() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(HOME_MARKDOWN_TIMEOUT_SECS))
        .user_agent(concat!("agentscommander/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let resp = client
        .get(HOME_MARKDOWN_URL)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Server returned status {}", resp.status().as_u16()));
    }

    // Use bytes() so we can length-check before allocating a String.
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    if bytes.is_empty() {
        return Err("Server returned empty response".to_string());
    }
    if bytes.len() > HOME_MARKDOWN_MAX_BYTES {
        return Err("Response too large".to_string());
    }

    String::from_utf8(bytes.to_vec())
        .map_err(|_| "Response is not valid UTF-8".to_string())
}
```

**Rules:**
- Use `concat!("agentscommander/", env!("CARGO_PKG_VERSION"))` so the user-agent stays in lockstep with `Cargo.toml`'s version (and therefore with the `tauri.conf.json` bump in §F).
- Do NOT introduce caching at the Rust layer for MVP. Frontend-store caching (§D4) is sufficient.
- Do NOT add any `serde` types — the command returns a plain `String`.

#### D2. `src-tauri/src/commands/window.rs`

**Current anchor:** `open_in_explorer` at line 277.

**Change:** Add `open_external_url` immediately after `open_in_explorer`.

```rust
/// Open an http/https URL in the user's default browser.
/// Refuses any other scheme to prevent the frontend from invoking arbitrary
/// shell handlers via crafted URLs. Scheme check is case-insensitive
/// (RFC 3986 §3.1) but the original URL is passed to `open::that_detached`.
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("Refusing to open non-http(s) URL: {}", url));
    }
    open::that_detached(trimmed).map_err(|e| format!("Failed to open URL: {}", e))
}
```

**Rules:**
- The scheme check is a security boundary. Do NOT loosen it (e.g. allowing `mailto:` or `file:`) without a separate review. The Home doc's audience is hyperlinked GitHub docs and HTTP resources only.
- Use `open::that_detached`, matching `open_in_explorer`. Do NOT introduce `tauri-plugin-shell` to keep the dependency surface unchanged.

#### D3. `src-tauri/src/lib.rs`

**Current anchor:** the `invoke_handler` registration block at lines 800–843.

**Change:** Add the two new commands to `tauri::generate_handler![...]`.

Insert after `commands::window::open_guide_window,` (line 809):
```rust
commands::window::open_external_url,
```

Insert near the other `commands::config::*` lines (around line 818–823):
```rust
commands::config::fetch_home_markdown,
```

**Rules:**
- Order does not affect runtime behavior, but keeping additions adjacent to module-mates aids future readers.

#### D4. `src-tauri/src/web/commands.rs`

**Current anchor:** the no-op stub block for browser-mode at lines 319–329.

**Change:** Decide explicit behavior for browser/web mode.

Add `open_external_url` to the no-op stub list at line 324–329:
```rust
"detach_terminal"
| "attach_terminal"
| "set_detached_geometry"
| "open_in_explorer"
| "focus_main_window"
| "open_guide_window"
| "open_external_url" => Ok(json!(null)),
```

For `fetch_home_markdown`, in browser mode the frontend can hit GitHub directly via the WebSocket-relayed invoke. Since browser mode is **out of scope** for this issue (per §Constraints), return a stub error that the frontend can display:
```rust
"fetch_home_markdown" => Err("Home is not available in browser mode".to_string()),
```

Place the `fetch_home_markdown` arm anywhere in the existing match block — there is no requirement on adjacency.

**Rules:**
- Returning `Err` causes `HomeAPI.fetchMarkdown()` to throw, which `homeStore.fetch()` already catches and renders in the error state. The user sees "Home is not available in browser mode" with the Try-again button. This is acceptable degraded behavior for v1.
- The Home button should still toggle in browser mode — we do not gate the button by `isTauri`. This keeps the codepath uniform.

### E. Tests

#### E1. `src/shared/path-extractors.test.ts` is the only existing frontend test file. The vitest configuration in `vitest.config.ts` is set up for the frontend.

Add **two new test files**:

##### E1a. `src/main/stores/home.test.ts`

Cover the store contract. **MUST use `__resetHomeStoreForTests()`** (added in §A1) in `beforeEach` so signals don't leak between tests.

```ts
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../../shared/ipc", () => ({
  HomeAPI: { fetchMarkdown: vi.fn() },
}));

import { homeStore, __resetHomeStoreForTests } from "./home";
import { HomeAPI } from "../../shared/ipc";

describe("homeStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    __resetHomeStoreForTests();
  });

  it("setInitialVisibility(false) → visible=true (no active session)", () => {
    homeStore.setInitialVisibility(false);
    expect(homeStore.visible).toBe(true);
  });

  it("setInitialVisibility(true) → visible=false (active session at boot)", () => {
    homeStore.setInitialVisibility(true);
    expect(homeStore.visible).toBe(false);
  });

  it("toggle flips visibility", () => {
    homeStore.hide();
    homeStore.toggle();
    expect(homeStore.visible).toBe(true);
    homeStore.toggle();
    expect(homeStore.visible).toBe(false);
  });

  it("fetch sets content on success", async () => {
    (HomeAPI.fetchMarkdown as any).mockResolvedValue("# Hello\n");
    await homeStore.fetch();
    expect(homeStore.content).toBe("# Hello\n");
    expect(homeStore.error).toBeNull();
    expect(homeStore.loading).toBe(false);
  });

  it("fetch records error on failure and content stays null when no prior content", async () => {
    (HomeAPI.fetchMarkdown as any).mockRejectedValue(new Error("Network error: down"));
    await homeStore.fetch();
    expect(homeStore.content).toBeNull();
    expect(homeStore.error).toContain("Network error");
    expect(homeStore.loading).toBe(false);
  });

  it("refresh on success replaces existing content", async () => {
    (HomeAPI.fetchMarkdown as any).mockResolvedValueOnce("# v1");
    await homeStore.fetch();
    expect(homeStore.content).toBe("# v1");
    (HomeAPI.fetchMarkdown as any).mockResolvedValueOnce("# v2");
    await homeStore.refresh();
    expect(homeStore.content).toBe("# v2");
  });

  it("refresh failure preserves prior content (does NOT wipe to null)", async () => {
    (HomeAPI.fetchMarkdown as any).mockResolvedValueOnce("# v1");
    await homeStore.fetch();
    expect(homeStore.content).toBe("# v1");
    (HomeAPI.fetchMarkdown as any).mockRejectedValueOnce(new Error("offline"));
    await homeStore.refresh();
    expect(homeStore.content).toBe("# v1"); // critical: content survives a failed refresh
    expect(homeStore.error).toContain("offline");
  });

  it("concurrent fetch is idempotent", async () => {
    let resolveFn: (v: string) => void;
    (HomeAPI.fetchMarkdown as any).mockReturnValue(
      new Promise<string>((r) => { resolveFn = r; })
    );
    const p1 = homeStore.fetch();
    const p2 = homeStore.fetch();
    resolveFn!("ok");
    await Promise.all([p1, p2]);
    expect((HomeAPI.fetchMarkdown as any).mock.calls.length).toBe(1);
  });
});
```

**Notes:**
- The reset helper is mandatory — without it, `content` and `error` from prior tests leak into the next case (e.g. the "refresh on success" test would otherwise pollute the "concurrent fetch" assertion).
- The "refresh failure preserves prior content" test is the regression guard for Grinch finding #3. Do NOT remove it.
- This file runs under the default vitest `node` environment (no DOM needed). Do NOT add a `// @vitest-environment` directive here.

##### E1b. `src/main/components/HomeView.sanitize.test.ts`

Verify XSS defence. **The first line of the file MUST be the per-file vitest environment directive** — DOMPurify imports `window`/`document`, which the global `node` environment in `vitest.config.ts` does not provide.

```ts
// @vitest-environment jsdom
import { describe, it, expect } from "vitest";
import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";

const md = MarkdownIt({ html: false, linkify: true, typographer: false, breaks: false });
const render = (src: string) =>
  DOMPurify.sanitize(md.render(src), { USE_PROFILES: { html: true } });

describe("HomeView Markdown sanitization", () => {
  it("escapes raw HTML in Markdown source (html:false)", () => {
    const out = render('Hello <script>alert(1)</script> world');
    expect(out).not.toContain("<script>");
  });

  it("strips javascript: URLs in links", () => {
    const out = render('[click](javascript:alert(1))');
    // DOMPurify removes the dangerous href; the anchor tag may remain but href is gone or sanitized.
    expect(out).not.toContain("javascript:");
  });

  it("preserves http(s) anchors", () => {
    const out = render('[ok](https://example.com)');
    expect(out).toContain('href="https://example.com"');
  });

  it("renders code blocks", () => {
    const out = render("```\nlet x = 1;\n```");
    expect(out).toMatch(/<pre><code>.*let x = 1.*<\/code><\/pre>/s);
  });
});
```

**Why this works without changing `vitest.config.ts`:**
- The repo's `vitest.config.ts` sets `environment: 'node'` globally. Vitest honours per-file `// @vitest-environment <name>` directives, so this file runs under jsdom while all other tests stay on node.
- `jsdom` MUST be in `devDependencies` (see §Dependencies). Without it, vitest emits `Cannot find module 'jsdom'` at test load.
- The existing `src/shared/path-extractors.test.ts` is pure logic with no DOM dependency, so it stays on the default `node` env unaffected. **Verify** by running `npm run test` after adding `jsdom`: both files (the existing and the new) must pass.

**Why a per-file directive instead of switching the global env to jsdom:**
- Switching globally adds DOM-construction overhead to every test file in the repo, even pure-logic ones. Per-file scoping keeps the cost paid only by tests that need it.
- Future test files that incidentally touch DOM globals would silently start working under a global jsdom env without anyone noticing — fragile. Explicit per-file directives keep the dependency intentional.

#### E2. Manual validation

1. Build runs cleanly: `cargo check` and `npx tsc --noEmit`.
2. `npm run test` passes (frontend) — both new test files green.
3. Launch the app with no existing sessions → terminal pane shows Home content (rendered Markdown), Home button has `.active` styling.
4. Click Home button → Home hides, terminal pane shows the existing "+ New Session" empty state.
5. Click "+ New Session" → session is created, Home stays hidden (Home button is no longer `.active`).
6. While the new session is active, click Home button → Home replaces the terminal view.
7. Click Home button again → Home overlay disappears, the terminal view (which was never unmounted) is visible again with **complete scrollback intact**. To verify: in step 5 above, before opening Home, run a command that produces ≥50 lines of output (`Get-ChildItem -Recurse C:\Windows\System32 | Select-Object -First 100 | Format-Table` is fine on Windows). Open Home (output is now hidden behind the overlay), then close Home. All 50+ lines must still be scrollable. If any output is missing, the overlay design has regressed to the rev-1 unmount-on-show approach — investigate before merging.

7a. (Live-output test) Inside an active session, kick off `for ($i=1; $i -le 30; $i++) { $i; Start-Sleep -Milliseconds 200 }` and immediately open Home while it is still emitting. Wait for the loop to finish (≈6 s), then close Home. Numbers 1–30 must all be present in the terminal scrollback. This proves the `onPtyOutput` listener stayed registered while Home was visible.
8. Click a hyperlink in the rendered Home content → opens in OS default browser.
9. Disconnect network, click ↻ refresh → error state shows; click Try again → still errors. Reconnect, click Try again → loads.
10. Detach a terminal window — Home is **not** rendered there; only the locked session view appears.
11. Restart the app while a session is active and Home was previously visible → on restart, Home is hidden (initial visibility rule), since there is an active session.
12. Verify size: response from GitHub raw is well under 256 KB. (Today, the doc is small; if it ever grows past 256 KB, raise the cap in `config.rs` — not in the frontend.)

---

## Dependencies

### New npm dependencies (add to `package.json` under `"dependencies"`)
- `markdown-it` (latest stable; pin to `^14.x` at time of writing).
- `dompurify` (`^3.x`).

### New npm devDependencies (add to `"devDependencies"`)
- `@types/markdown-it` (`^14.x`).
- `@types/dompurify` (`^3.x`).
- `jsdom` (`^25.x`) — required by the per-file `// @vitest-environment jsdom` directive in `src/main/components/HomeView.sanitize.test.ts`. DOMPurify needs DOM globals (`window`, `document`); jsdom provides them in the test environment. Pin a major version to avoid surprises across upgrades. (Choose `happy-dom` only if jsdom proves slow — for a single-file directive use, the speed difference is negligible.)

### Rust dependencies
None. `reqwest`, `open`, and `tauri` are already in `Cargo.toml`.

### Capabilities
None. The two new Tauri commands ride on `core:default` (no plugin permissions required for invocations of app-defined commands).

### Version bump
- `tauri.conf.json`: `0.8.5` → `0.8.6`.
- `src-tauri/Cargo.toml`: `version = "0.8.5"` → `version = "0.8.6"`.
- `package.json`: `"version": "0.8.5"` → `"0.8.6"`.

The version bump must accompany this feature to make a fresh build visually distinguishable from prior installs (per project convention).

---

## Constraints / Notes for the Dev

1. **Do NOT** wire `tauri-plugin-shell` into the backend. Use the existing `open` crate via `open_external_url` (§D2). Adding a plugin requires Cargo dep + `lib.rs` init + capability JSON, which is broader than this issue needs.
2. **Do NOT** allow raw HTML in the Markdown render (`markdown-it`'s `html: false`). Even with DOMPurify, defense-in-depth requires the parser to reject raw HTML at source. If a future Home doc relies on inline HTML, that is a separate scope discussion.
3. **Do NOT** persist Home visibility to settings. Per-session UI state only.
4. **Do NOT** add a CSP to `tauri.conf.json` as part of this issue. Adding one is a global-impact change that needs its own plan; this feature deliberately routes HTTP via the backend so CSP is not a blocker either way.
5. **Do NOT** add caching at the Rust layer. The frontend store caches in-memory for the session, which is sufficient for MVP.
6. **Do NOT** auto-retry failed fetches. Explicit "Try again" button only.
7. **Do NOT** broadcast Home state via Tauri events. Same JS context shares the store directly.
8. **Do NOT** render Home in detached/locked terminal windows. The `props.embedded && !props.detached && !props.lockedSessionId` guard in TerminalApp is load-bearing.
9. **Do NOT** loosen the `http(s)://` scheme check in `open_external_url`. Tightening the scheme list later is fine; loosening requires a security review.
10. **Browser/web mode is out of scope.** The web/commands.rs stub returns an error string that the frontend Home view will render. Full browser-mode Home support (frontend `fetch()` + relaxed CORS) belongs in a follow-up issue.
11. **Windows specifics**: `open::that_detached(url)` on Windows uses `ShellExecute` under the hood; the OS resolves the default browser. No path-separator concerns. Verified the same primitive is in production use via `open_in_explorer`.
12. **innerHTML usage** is intentional and limited to the post-DOMPurify HTML string in `HomeView.tsx`. Do NOT introduce `innerHTML` anywhere else as part of this issue.
13. **Do NOT** unmount `TerminalView` when Home is shown. The CSS-overlay approach in §B2 is load-bearing — the previous design (rev 1) wrapped the terminal `<Show>` in a Home check that unmounted TerminalView; that destroyed scrollback, the WebGL context, and the `onPtyOutput` listener. If you find yourself rewriting §B2 to put Home where TerminalView used to be, stop and re-read Grinch finding #1.
14. **Do NOT** drop the `__resetHomeStoreForTests()` helper or skip it in `beforeEach`. The store is a module singleton; without explicit reset, signal state from prior tests leaks into the next case.
15. **Do NOT** wipe `homeStore.content` at the start of `refresh()`. The user keeps seeing the last-good content while a refetch is in flight; only `error` is cleared. This is the regression guard for Grinch finding #3 — there is a test asserting the behavior (§E1a "refresh failure preserves prior content").

---

## Risks and Edge Cases

1. **GitHub raw rate limiting** — anonymous requests to `raw.githubusercontent.com` are subject to soft rate limits. The 5s timeout + manual retry pattern is the right behavior; if a user spam-clicks ↻, they'll see errors. Acceptable.
2. **GitHub branch rename / file move** — the URL is hardcoded. If `mblua-patch-1` is renamed or `docs/home.md` moves, fetch will 404. The frontend renders the error cleanly. A follow-up could read the URL from settings or the build profile, but not in this issue.
3. **Proxy / corporate network** — `reqwest` respects environment proxy variables (`HTTPS_PROXY`, etc.) by default. No special configuration needed.
4. **Rendered link to a non-http scheme** — DOMPurify strips `javascript:` URLs in links, and `open_external_url` rejects non-http(s) schemes server-side. Two layers.
5. **Markdown content containing `<img src="...">`** — `html: false` means raw `<img>` in the source is escaped. CommonMark image syntax (`![alt](url)`) is rendered normally and, after DOMPurify, only safe schemes pass. `crossorigin` is not needed since images render via the WebView's own GET (subject to default CSP, which is unset).
6. **Theme switch while Home is open** — the `.home-markdown` styles include a `.light-theme` variant. Toggling the theme while Home is showing will repaint correctly because the styles use existing CSS variables.
7. **Window resize / splitter drag while Home is open** — Home's `.home-view { height: 100% }` means it follows the pane size. No special handling needed.
8. **Memory growth from cached `content`** — content is bounded to 256 KB by the backend cap, kept once per app session in a single signal. Trivial.
9. **Race between `MainApp.onMount` and `TerminalApp.onMount`** — both query `SessionAPI.getActive()`. The order doesn't matter because Home visibility is set BEFORE the user can click anything, and `TerminalApp` reads `terminalStore.activeSessionId`, not Home state.

---

## Validation Checklist

- [ ] `cargo check` — backend compiles.
- [ ] `npx tsc --noEmit` — TypeScript clean.
- [ ] `npm run test` — both new test files pass.
- [ ] `npm run build` — Vite bundle produced.
- [ ] Manual scenarios 3–11 in §E2.
- [ ] Version visible in About / titlebar suffix shows `0.8.6`.

---

## Dev / Grinch Review

**Status:** Rev 2 — Grinch findings 1–8 resolved 2026-05-07. Awaiting re-review.

---

## Architect Verdict (rev 2 — 2026-05-07)

The dev-webpage-ui agent proposed an alternative design (`_plans/164-home-screen-docs.md`): frontend `fetch()` + `marked` (no sanitizer), shared `viewStore` in `src/shared/stores/`, no Rust command. Tech-lead asked the architect to choose between the two competing approaches and resolve the Grinch findings on the canonical plan. Decisions:

### V1. Fetch approach — **backend `reqwest` via `fetch_home_markdown` Tauri command** (NOT frontend `fetch()`)

The dev's "frontend fetch is simpler" argument has merit (no Rust surface, cross-origin works under default no-CSP) but is outweighed by:

- **Codebase convention.** Every outbound HTTP call in this codebase already goes through `reqwest` in Rust: `commands/voice.rs:146`, `commands/telegram.rs:97`, `telegram/bridge.rs:498`. There are **zero** frontend `fetch()` calls today. Introducing one would force every future contributor to wonder which side of the IPC boundary HTTP belongs on. We have a clean rule today; keep it.
- **CSP-resilience.** `tauri.conf.json` has no CSP today (verified). If one is ever added — and CSP is a normal hardening step for Tauri apps — frontend `fetch()` to `raw.githubusercontent.com` would silently start failing. Backend reqwest is immune. The dev's plan acknowledges this risk in passing but accepts it; we don't.
- **Surface-area math is not as one-sided as the dev plan claims.** The dev's "Adding a Rust command would require capability + reqwest plumbing" sentence is misleading: `reqwest` is already a Cargo dep (no new crate), Tauri capabilities are not required for app-defined commands (verified — `capabilities/default.json` has `core:default` only and that suffices), and the command body is ~30 LoC of straightforward async code with the timeout/UA/size-cap hardening already specified in §D2. The IPC wrapper in §C1 is 3 lines. That is well below the cost of breaking a clean codebase rule.
- **Centralization headroom.** Putting the fetch in Rust gives one place to add caching (TTL on disk), a fallback URL, telemetry, or per-host throttling later, without touching the frontend. The dev plan would force any of those to be done twice (once in Rust for everything else, once in JS for Home).

What we adopted from the dev plan:
- The dev's "raw URL transformation rule" is good documentation; the canonical plan already hardcodes the raw URL but the rule itself is worth preserving in §D2 for future readers (not adding now — comment in §D2 already covers it).
- The dev's auto-flip-back-to-Home-on-last-session-destroyed (Q3) is the right UX call. **Adopted** — see new §D1 "Auto-show rule" and §B3 wiring.

What we rejected:
- Skipping DOMPurify. `marked` does not sanitize by default; even for content from "our own repo", the trust boundary is GitHub + TLS, and a successful spoof or branch hijack would be game-over without sanitization. The cost (one ~20 KB dep) is trivial.
- `src/shared/stores/view.ts` location. The store is unified-window-only; cross-window placement would mislead future readers into emitting from detached windows.
- Long-term GitHub-branch durability concern (Q2): real but not in scope. The plan documents the risk under §Risks; the URL is a single-line constant change if `mblua-patch-1` is ever retired.

### V2. Auto-flip Home back when last session is destroyed — **YES, adopt the dev's recommendation**

Rationale lives in §D1's new "Auto-show rule" subsection. Wiring is in §B3. The architect's rev-1 stance ("destruction does not affect Home's purpose") was wrong: when there are zero remaining sessions, the alternative landing surface is the bare "No active session" empty state — strictly worse than the curated Home doc. Auto-show fires only when `SessionAPI.list().length === 0`, so the user is not crowded by Home as long as they have any session left.

### V3. Open Q1 (raw URL existence) — verified

Tech-lead reported `GET https://raw.githubusercontent.com/mblua/AgentsCommander/mblua-patch-1/docs/home.md` returns HTTP 200 with content `Initial home`. The fetch path is unblocked; whoever updates the upstream doc to a richer "how to start with a basic group of agents" content can do so independently of this implementation. The plan does not gate on the doc content being final.

---

## Supersession

`_plans/164-home-screen-docs.md` (the dev-webpage-ui draft) is **superseded by this file** for issue #164. Reasons summarized in §Architect Verdict V1 above. The dev plan file should be left in place for history but treated as non-authoritative — a `> SUPERSEDED` banner has been added to its header. Implementers must follow this file (`_plans/164-home-screen.md`); the dev plan's `viewStore`, frontend-fetch, and `marked`-without-sanitizer guidance is NOT to be implemented.

---

## Rev 2 changelog (2026-05-07)

Resolutions to the Grinch review (full review preserved at the end of this file for traceability):

| # | Severity | Resolution |
|---|---|---|
| 1 | BLOCKER | §B2 rewritten to use a CSS overlay (`.home-view { position: absolute; inset: 0 }` inside a new `.terminal-content-area` wrapper). `TerminalView` stays mounted while Home is visible — scrollback, `onPtyOutput`, and the WebGL context all preserved. New §A3 CSS adds the `.terminal-content-area` rule. New constraint #13 prohibits reverting to the unmount approach. New validation steps 7 and 7a explicitly assert scrollback preservation and live-output preservation. |
| 2 | BLOCKER | §Dependencies adds `jsdom (^25.x)` as a devDependency. §E1b mandates `// @vitest-environment jsdom` as the first line of `HomeView.sanitize.test.ts` and explains why a per-file directive is preferred over flipping the global env. |
| 3 | HIGH | §A1 `refresh()` no longer calls `setContent(null)`. Only `error` is cleared at the start; `content` is preserved across a failed refetch. §E1a adds an explicit regression test asserting this. New constraint #15. |
| 4 | HIGH | §D5 and §D2 use `to_ascii_lowercase()` on a trimmed copy for the scheme check; the original (trimmed) URL is passed to `open::that_detached`. Accepts mixed-case `HTTP://`, `Https://`, etc. |
| 5 | HIGH | §D1 rationale corrected to acknowledge `src/main/stores/` is a NEW directory and explain why it's the right place (cross-pane stores live with `MainApp` which owns the unified layout). |
| 6 | MEDIUM | §A2 `HomeView` `createMemo` returns DOMPurify output directly — no `tmp.innerHTML` round-trip, no anchor post-process. §D3 explanation updated; `ADD_ATTR: ["target","rel"]` removed (redundant). Click delegation is the sole external-open path. |
| 7 | LOW | §B1 emoji-convention rationale rewritten to acknowledge the existing file is mixed; HTML-entity choice is now justified by greppability rather than a non-existent convention. |
| 8 | LOW | §A1 exports `__resetHomeStoreForTests()` (NODE_ENV-gated). §E1a `beforeEach` calls it. The misleading "leaves content null" test is renamed and its assertion is now explicit. |

Plus two architectural decisions added (V1 backend reqwest, V2 auto-flip on last-session-destroyed) and the dev plan marked as superseded.

---

## Grinch Review

**Verdict: NOT APPROVED — implementation must NOT proceed until findings 1–5 are resolved.**

Verified against current code at `feature/164-home-screen-docs` (last commit `3770977`). Anchor line numbers, file existence, deps, vitest config, capabilities, and CSP all checked.

### 1. BLOCKER — `TerminalView` is fully unmounted when Home is shown; scrollback and live PTY output are LOST

- **What:** §B2 wraps the inner `<Show>` so that `<HomeView />` replaces the entire `<TerminalView />` subtree. SolidJS unmounts `TerminalView`, which fires its `onCleanup` (`src/terminal/components/TerminalView.tsx:340-349`):
  - Unregisters `onPtyOutput` listener — no listener captures PTY output while Home is visible.
  - Iterates `terminals.keys()` and calls `disposeSessionTerminal` for each, which calls `terminal.dispose()` and removes the container element from the DOM. xterm.js scrollback is destroyed.
- **Why it matters (concrete failure):** User runs a long command that produces 200 lines of output, then opens Home, then closes Home. On Home dismissal, `TerminalView` re-mounts; `terminalStore.activeSessionId` is unchanged so `createEffect` triggers `showSessionTerminal` → `createSessionTerminal` builds a brand-new xterm.Terminal in an empty container. Result: terminal pane is **blank**. Output that arrived during the Home view is also lost (no listener was active). This contradicts §B2 Rules ("the xterm.js terminal will re-mount and reattach to its session buffer") and §E2 manual step 7 ("with the active session's PTY output preserved (no scrollback loss)") — there is no buffer-replay path in main-window mode. The pre-warm logic at `TerminalView.tsx:314-321` only handles `terminal_detached` events, not Home toggling.
- **Compounding issue:** Each Home toggle re-creates an xterm.Terminal which consumes a WebGL context. Per the comment at `TerminalView.tsx:32-33`, the document budget is ~16 contexts. Rapid toggling across multiple sessions can exhaust the budget and silently fall back to canvas (degraded perf).
- **Fix:** Render Home as an **overlay** sibling that sits on top of (or alongside) `TerminalView`, toggled via CSS (e.g. `display: none` / `visibility: hidden`) based on `homeStore.visible`. `TerminalView` MUST stay mounted while Home is visible. Concrete approach for §B2: keep the existing inner `<Show when={terminalStore.activeSessionId} fallback={...}>` block exactly as-is; render `<HomeView />` as an additional sibling (or absolutely-positioned overlay inside `.terminal-layout`) gated only by `props.embedded && !props.detached && !props.lockedSessionId && homeStore.visible`. The plan's §B2 Rule "Do NOT introduce a CSS overlay approach" is incorrect — the empty-state-fallback comparison does not apply because the empty state only fires when there is no active session (no scrollback to preserve), whereas Home toggling fires while a session is active.

### 2. BLOCKER — `HomeView.sanitize.test.ts` cannot run because vitest is configured for `node` and `jsdom` is not installed

- **What:** `vitest.config.ts` has `environment: 'node'` and includes `src/**/*.test.ts`. §E1b imports `dompurify` and calls `DOMPurify.sanitize(...)`. DOMPurify requires DOM globals (`window`, `document`); in pure Node it errors at import or on first sanitize. Neither `jsdom` nor `happy-dom` is in `package.json` devDependencies (the matches in `package-lock.json` are transitive optional peers from vitest's nested deps, not installed).
- **Why it matters:** Validation Checklist item "`npm run test` — both new test files pass" cannot succeed. The plan acknowledges uncertainty in an §E1b "Notes" subsection ("if not, add `// @vitest-environment jsdom` at the top of this test file. **Verify** before committing.") but does NOT explicitly mandate the directive AND does NOT add `jsdom` to `devDependencies`. This is hand-waving on a real blocker.
- **Fix:** Plan must explicitly:
  - Add `jsdom` (or `happy-dom`) to §Dependencies → new devDependencies, with a pinned version.
  - Mandate `// @vitest-environment jsdom` (or `happy-dom`) at the top of `src/main/components/HomeView.sanitize.test.ts`.
  - Verify the existing `src/shared/path-extractors.test.ts` doesn't break under the new env (it shouldn't — it's pure logic, but a global override risks regressions).

### 3. HIGH — `homeStore.refresh()` wipes content BEFORE re-fetching → user loses visible content if refresh fails

- **What:** §A1 implements `refresh()` as `setContent(null); await this.fetch();`. If the network is down when the user clicks ↻, `content` becomes `null`, then `fetch()` sets `error`. The user — who had perfectly valid Home content displayed — now sees only an error message until a successful refetch.
- **Why it matters:** Network failures during retry are common (corporate proxies, momentary disconnects). Every failed refresh causes a visible regression of state. There is no benefit to wiping content first; `fetch()` already overwrites `content` on success.
- **Fix:** Drop the `setContent(null)` line in `refresh()`. Implementation:
  ```ts
  async refresh() { await this.fetch(); }
  ```
  Optional: also clear `error` at the start of `refresh()` so the error banner disappears while loading.

### 4. HIGH — `open_external_url` scheme check is case-sensitive and rejects legitimate URIs

- **What:** §D5 uses `url.starts_with("http://") || url.starts_with("https://")`. URI schemes are case-insensitive per RFC 3986. A markdown link `[click](HTTP://example.com)` (legal, sometimes generated by linkifiers) is rejected. Same for `Https://...`, leading whitespace from sloppy markdown, etc.
- **Why it matters:** Real-world markdown contains case-varied schemes. The user clicks a link, frontend sees a backend error in the console, link does nothing. No security loss, but a UX failure on perfectly valid input.
- **Fix:** Normalize for the check, but pass the original URL to `open::that_detached`:
  ```rust
  let trimmed = url.trim();
  let lower = trimmed.to_ascii_lowercase();
  if !(lower.starts_with("http://") || lower.starts_with("https://")) {
      return Err(format!("Refusing to open non-http(s) URL: {}", url));
  }
  open::that_detached(trimmed).map_err(|e| format!("Failed to open URL: {}", e))
  ```

### 5. HIGH — Plan claim that `src/main/stores/` "matches existing in-process state patterns" is false

- **What:** §D1 rationale states: "Matches existing in-process state patterns: `terminalStore`, `sessionsStore`, `bridgesStore`." Verified: `src/main/stores/` does **not exist**. `terminalStore` lives in `src/terminal/stores/terminal.ts`; sidebar stores live in `src/sidebar/stores/`; shared stores live in `src/shared/stores/`. There is no precedent for `src/main/stores/`.
- **Why it matters:** Not a functional blocker (file creation auto-creates the directory). But the rationale is misleading. The dev is creating a NEW location convention, not following an existing one. A future reader who trusts the rationale may make wrong assumptions about where cross-pane state should live.
- **Fix:** Replace the misleading sentence with: "This is a NEW location — no `src/main/stores/` exists yet; this change creates it. Cross-pane stores belong here because the unified `MainApp` owns the layout that hosts both panes. Existing per-window stores live in `src/sidebar/stores/`, `src/terminal/stores/`, and shared cross-window stores in `src/shared/stores/`."

### 6. MEDIUM — `HomeView.html()` createMemo does pointless DOM round-trip; anchor post-process is redundant given click delegation

- **What:** §A2 builds `tmp.innerHTML = clean; tmp.querySelectorAll("a").forEach(setAttribute target/rel); return tmp.innerHTML`. With click delegation via `onContainerClick` already calling `preventDefault()` + `WindowAPI.openExternal(href)`, the `target="_blank"` and `rel="noopener noreferrer"` attributes are **never used by the browser** — the click is intercepted before navigation.
- **Why it matters:** Wasted CPU on every memo recompute (parse + serialize a potentially 256 KB HTML string). For ASCII-heavy markdown this is fast but it's pointless work and adds a synchronous block on first render. Also, re-serializing via `tmp.innerHTML` is lossy (e.g. attribute ordering, whitespace) and brittle if anyone later adds plugins emitting custom attributes.
- **Fix:** Drop the temp-div round-trip and the anchor attribute pass entirely. Just pass the DOMPurify output through directly:
  ```ts
  const html = createMemo(() => {
    const src = homeStore.content;
    if (!src) return "";
    return DOMPurify.sanitize(md.render(src), {
      USE_PROFILES: { html: true },
    });
  });
  ```
  The click delegation handler already enforces external-open; the missing `target`/`rel` is irrelevant because the browser never gets to handle the click. (`ADD_ATTR: ["target", "rel"]` is also redundant — DOMPurify allows both by default.)

### 7. LOW — `&#x1F3E0;` HTML-entity convention is inconsistent in the same file

- **What:** §B1 says "Keep the HTML entity form to match the existing button conventions in the same file." But `ActionBar.tsx:167` uses raw emoji literals (`{isSoundsEnabled() ? "🔊" : "🔇"}`). The convention in the file is mixed.
- **Why it matters:** Cosmetic only. Pick one and call it out, OR don't justify with a non-existent convention.
- **Fix:** Drop the rationale sentence; either form is fine.

### 8. LOW — §E1a test reset is incomplete; "leaves content null" comment is misleading

- **What:** `beforeEach` only calls `homeStore.hide()`. `content` and `error` from a prior test persist into the next. The test "fetch records error on failure and leaves content null" does not actually assert `content === null` — it only checks `error` and `loading`. Comment is misleading.
- **Fix:** Either add a `__resetForTests()` method (gated to `process.env.NODE_ENV === "test"` if you care about prod surface) that resets all four signals, OR rename the test to drop the misleading "leaves content null" wording and explicitly assert what is being tested.

---

### Notes for the dev (non-blocking observations, useful context)

- §D2 `String::from_utf8` will pass through a UTF-8 BOM at the start of the response body. GitHub raw doesn't normally emit BOM but if it ever does, the resulting markdown will render with an invisible `﻿` at the top. Optional: strip it with `bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes[..])` before `String::from_utf8`.
- §D3 lib.rs anchor verified: `commands::window::open_guide_window,` is at line 809; `commands::config::*` block is at lines 791-797 and 818-823. Plan's insertion guidance is accurate.
- §D4 web/commands.rs no-op block is at lines 324-329 (the comment header at 319-323). Plan said 319-329; close enough.
- Capabilities (`src-tauri/capabilities/default.json`) has `core:default` only — confirmed; new app-defined commands need no capability changes. ✓
- `tauri.conf.json` has no CSP set — confirmed. Backend-fetch decision is sound. ✓
- `tauri-plugin-shell` is in `package.json` but NOT in `src-tauri/Cargo.toml` — confirmed. Plan's §D5 rationale is accurate. ✓
- Plan's §F version-bump triple (`tauri.conf.json`, `Cargo.toml`, `package.json`) all currently `0.8.5` — confirmed. ✓

### Required actions before implementation can proceed

1. Fix finding #1: redesign §B2 to keep `TerminalView` mounted (CSS-overlay approach).
2. Fix finding #2: add `jsdom` (or `happy-dom`) devDependency in §Dependencies AND mandate the per-file `// @vitest-environment` directive in §E1b.
3. Fix finding #3: simplify `homeStore.refresh()` to not wipe content.
4. Fix finding #4: case-insensitive scheme check in `open_external_url`.
5. Fix finding #5: correct the rationale in §D1.
6. Address findings #6–#8 in the same revision.

When the plan has been revised, request a re-review.

