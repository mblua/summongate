# Plan #164 — Home screen with remote-Markdown docs

> # ⚠ SUPERSEDED — DO NOT IMPLEMENT THIS FILE ⚠
>
> **This plan has been superseded by `_plans/164-home-screen.md` (rev 2, 2026-05-07).**
>
> The architect verdict (in the canonical plan's §Architect Verdict section) explicitly rejected the design choices in this file:
> - **Frontend `fetch()`** — rejected. Backend `reqwest` via `fetch_home_markdown` Tauri command is the chosen path. Reason: every HTTP call in this codebase already goes through Rust; CSP-resilience; centralization headroom for future caching/throttling.
> - **`marked` without DOMPurify** — rejected. Sanitization is mandatory; `markdown-it` + DOMPurify is the chosen pair.
> - **`viewStore` in `src/shared/stores/`** — rejected. Home is unified-window-only (must NOT render in detached terminals); placement under `src/main/stores/` signals that scope.
>
> Adopted from this file into the canonical: the **auto-flip-back-to-Home when the last session is destroyed** UX (this file's §9 Q3) — the architect agreed it's the correct behavior over the bare "No active session" empty state.
>
> **Implementers: read `_plans/164-home-screen.md` instead.** This file is preserved for history only.

---

Issue: #164
Branch: `feature/164-home-screen-docs`
Repo: `repo-AgentsCommander`
Status: SUPERSEDED — see banner above.

---

## 1. Requirement (verbatim from tech-lead brief)

- On the initial AgentsCommander screen shown when there is no active session, add a Home experience that loads content from the web URL
  `https://github.com/mblua/AgentsCommander/blob/mblua-patch-1/docs/home.md`.
- The Markdown must render properly, not as raw text.
- Add a Home button in the sidebar action icon row, placed to the left of the existing flame/fire button.
- The Home page should explain how to start using AgentsCommander with a basic group of agents.

---

## 2. Verification of the brief against current code

Verified by Read/Grep on `main` (HEAD of `feature/164-home-screen-docs`):

| Brief claim | Verified at |
| --- | --- |
| Flame button lives inside `.action-bar-icons` in `ActionBar.tsx` | `src/sidebar/components/ActionBar.tsx:151–159` (`coord-sort-activity-btn`, glyph `🔥`) — it is the **first** button in the icons row |
| `package.json` has no Markdown renderer | Confirmed: `package.json` deps list — no `marked`, `markdown-it`, `remark`, `mdast`, etc. |
| Unified layout in `src/main/App.tsx` embeds Sidebar + Terminal | `src/main/App.tsx:213–232` |
| Sidebar renders `ActionBar`, `RootAgentBanner`, `ProjectPanel` | `src/sidebar/App.tsx:268–284` |

Additional findings the architect note did not mention but matter for the plan:

- **The current "no session" empty state is rendered by `TerminalApp`** as a Solid `<Show ... fallback={...}>` block at `src/terminal/App.tsx:204–222`. It shows an "No active session" string + a `+ New Session` button. This is the surface the Home experience must replace/augment.
- **There is also a web/browser mode** (`src/browser/App.tsx`) and a separate **Guide window** (`src/guide/App.tsx`) with a `Tutorial` tab that already lists how-to-start cards (`src/guide/components/TutorialTab.tsx`). The Home screen overlaps thematically with that Tutorial tab — call this out as an open question (§9).
- **`tauri.conf.json` has no CSP** (`src-tauri/tauri.conf.json` only contains build/identifier/bundle keys). So WebView2 can `fetch()` `https://raw.githubusercontent.com` without CSP edits.
- **Capabilities** (`src-tauri/capabilities/default.json`) are window/event/dialog only — no `http:` permission needed for direct `fetch()` from the WebView (the http permission is only required if we route the request through `@tauri-apps/plugin-http`).
- **Memory rule:** every feature build bumps `tauri.conf.json` version so the user can confirm they are running the new build. The plan must include that bump in the implementation step.
- **Memory rule:** WG-only deploys go to `_wg-20.exe` only — never the bare `agentscommander_standalone.exe`. (Affects shipper, not this repo's source code, but flag for the implementer step.)

---

## 3. UI/navigation state flow

### Goal
Home is treated as a top-level **view mode** of the terminal pane, not a modal. It replaces the existing "No active session" fallback as the **default** empty state, and the new Home button can re-show it without destroying the current session.

### New state: `viewMode`
Add a single `viewMode: "home" | "session"` signal. Recommended location: a small new shared store `src/shared/stores/view.ts` (parallel to `settingsStore`). Reasoning: both Sidebar (Home button active state) and Terminal pane need to read it, and bolting it onto `terminalStore` would mix view chrome with session state.

```ts
// src/shared/stores/view.ts
const [viewMode, setViewMode] = createSignal<"home" | "session">("home");
export const viewStore = {
  get mode() { return viewMode(); },
  showHome() { setViewMode("home"); },
  showSession() { setViewMode("session"); },
};
```

### Initial value
`"home"`. The first paint with no active session naturally shows Home. If a session already exists at boot (project restored), the user sees Home until they click a session — which feels right for a "home screen".

### Transitions

| Event | Mode change |
| --- | --- |
| App boot | → `"home"` (default) |
| User clicks the **Home** button in ActionBar | → `"home"` (no-op if already there; do not touch `SessionAPI.switch`) |
| User clicks a session in the sidebar (`onSessionSwitched`) | → `"session"` |
| User creates a new session (`onSessionCreated` and the session becomes active) | → `"session"` |
| Active session is destroyed and there's no replacement | leave as-is in `"session"` (the existing "No active session" fallback already covers this) **OR** auto-flip back to `"home"` — see §9 open question Q3 |

### Render decision (in `TerminalApp`)

Replace the current `<Show when={terminalStore.activeSessionId} fallback={<empty/>}>` so it becomes a 3-way:

```tsx
<Show
  when={viewStore.mode === "session" && terminalStore.activeSessionId}
  fallback={viewStore.mode === "home" ? <HomeView/> : <NoSessionEmptyState/>}
>
  <TerminalView lockedSessionId={props.lockedSessionId} />
</Show>
```

`<NoSessionEmptyState/>` is the current empty `<div class="terminal-empty">…</div>` extracted into a named local block (or a tiny component) so we don't lose it — there's still a path where `mode === "session"` but there's no active id (a session was just destroyed).

### Detached-window behavior

`TerminalApp` is also rendered in detached windows (`props.lockedSessionId` set, `props.detached` true). In that case **do not render Home** — those windows must always show the locked session's terminal. Gate Home rendering on `!props.lockedSessionId`. Detached windows simply ignore `viewMode`.

### Web/Browser mode

`BrowserApp` mounts the same `TerminalApp` (un-embedded). Home should work there too. The only difference is fetch behavior (cross-origin) — see §5.

---

## 4. Files to create / modify

### Create

| File | Purpose |
| --- | --- |
| `src/shared/stores/view.ts` | `viewStore` with `mode` signal + `showHome()` / `showSession()`. |
| `src/terminal/components/HomeView.tsx` | The Home component: fetch → render → loading/error/retry. |
| `src/terminal/styles/home.css` | Markdown-element styling (h1/h2/h3, p, ul/ol, code, pre, a, blockquote, hr) — all using existing CSS variables. Imported by `HomeView.tsx` (Solid + Vite handles it). |
| `src/shared/markdown.ts` *(only if §6 picks the no-dep path)* | Tiny Markdown subset renderer + HTML escaper. |

### Modify

| File | Change |
| --- | --- |
| `src/sidebar/components/ActionBar.tsx` | Insert a new Home button **before** the existing flame button (currently first child of `.action-bar-icons`, line 152). Click → `viewStore.showHome()`. Active state when `viewStore.mode === "home"`. |
| `src/terminal/App.tsx` | Replace the `<Show ... fallback>` block at lines 204–222 with the 3-way render described in §3. Import `viewStore` + `HomeView`. Add a listener so `onSessionSwitched` and `onSessionCreated` set `viewStore.showSession()` (the listeners already exist on lines 130–164 — append a one-line call). Gate Home with `!props.lockedSessionId`. |
| `src/terminal/styles/terminal.css` | Optional: small layout tweak only if `home.css` doesn't fully encapsulate. Default plan = no change here. |
| `src/shared/constants.ts` | Add `HOME_DOC_URL` constant (raw URL — see §5). |
| `package.json` | If §6 picks `marked`: add `"marked": "^14"` to `dependencies` (and types if needed). If no-dep path: untouched. |
| `src-tauri/tauri.conf.json` | **Bump `version`** (e.g. `0.8.5` → `0.8.6`) so the user can visually confirm a fresh build. Per memory rule. |

No Rust changes.
No `tauri.conf.json` CSP changes (none currently set).
No `capabilities/default.json` changes.

---

## 5. Frontend fetch vs Tauri command — and the URL

### Recommendation: **frontend `fetch()` in HomeView**, no Tauri command.

Reasons:
- Tauri 2 default config has no CSP, so the WebView can fetch any HTTPS origin.
- `raw.githubusercontent.com` returns `access-control-allow-origin: *`, so even Browser mode (`BrowserApp` served by `agentscommander_standalone`) works without a server-side proxy.
- Adding a Rust command would require capability + `reqwest` plumbing for what is a single GET. Not worth the surface area.

If we later want offline caching, that's the moment to add a Tauri command. Not now.

### URL

The page link in the brief is the GitHub web view (HTML wrapper). The raw Markdown URL is:

```
https://raw.githubusercontent.com/mblua/AgentsCommander/mblua-patch-1/docs/home.md
```

Stored in `src/shared/constants.ts` as `HOME_DOC_URL`. The transformation rule, for posterity:
`github.com/<owner>/<repo>/blob/<branch>/<path>` → `raw.githubusercontent.com/<owner>/<repo>/<branch>/<path>`.

### Fetch contract

```ts
const res = await fetch(HOME_DOC_URL, { cache: "no-cache", redirect: "follow" });
if (!res.ok) throw new Error(`HTTP ${res.status}`);
const md = await res.text();
```

- `cache: "no-cache"` so users get fresh docs after we update the upstream file. (We could relax to `default` later — leaving the trade-off for follow-up.)
- A 5 s `AbortController` timeout to avoid spinner-forever on a slow link.
- In-memory cache on the module so revisits during the same session are instant (no refetch).

---

## 6. Markdown rendering — library vs no-dep

### Recommendation: **add `marked` (`^14`)**, no sanitizer beyond `marked`'s own escaping.

#### Rationale
- `marked` is ~12 KB gzipped, zero deps, MIT, actively maintained, used by GitHub-flavored markdown sites.
- The doc is curated content we own (our own repo's `docs/home.md`), so untrusted-input sanitization is not the primary concern. The trust boundary is "TLS + GitHub" — same trust we already extend to `npm install`.
- A homegrown 150-line subset parser will silently drop tables / task lists / GFM features and rot the moment someone adds them to `home.md`.

#### Mitigations we still apply
- Configure `marked` with `breaks: true, gfm: true`.
- Post-process the output once: walk anchor tags and force `target="_blank" rel="noopener noreferrer"` so links open in the user's external browser, not inside the WebView. Implementation: a `DOMParser`-based pass on the rendered HTML string, then `innerHTML` assignment. Keep this in `HomeView.tsx`.
- We do **not** add `DOMPurify` (extra ~20 KB). If we ever start fetching user-authored Markdown (e.g. WG-shared briefs), revisit.

#### Alternative (kept on the table for the implementer)
**No-dep path:** write `src/shared/markdown.ts` covering headings, bold, italic, inline + fenced code, links, ul/ol, paragraphs, line breaks, hr; HTML-escape source first. ~150 LoC. Pros: zero deps, full control. Cons: brittle, no tables/blockquotes/GFM, future maintenance.

If the team prefers strict zero-dep, fall back to this. **Default of this plan is `marked`.**

#### Rendering call
```tsx
import { marked } from "marked";
marked.setOptions({ breaks: true, gfm: true });
const html = marked.parse(md) as string; // sync mode
const safeHtml = rewriteLinks(html); // anchor target=_blank pass
elRef.innerHTML = safeHtml;
```

`elRef` is the `<div>` that hosts the rendered content. SolidJS's `innerHTML` prop is fine here because the content is non-reactive HTML.

---

## 7. Loading / error / retry / fallback behavior

### States in `HomeView`
- `idle` (pre-mount, vanishingly short)
- `loading` — show a centered spinner + "Loading Home…" string. Use `--text-muted` color.
- `success` — render the parsed HTML.
- `error` — show:
  - one-line message: "Couldn't load Home. Check your internet connection."
  - the underlying error (`HTTP 404`, `TimeoutError`, etc.) in muted text.
  - a **Retry** button (re-runs the fetch).
  - a **fallback** block: a tiny static "How to start" snippet (3–4 bullet points: Open / New Project, click Open Agent, talk to your agent, open Guide for details). This guarantees the screen never feels broken offline.

### Retry mechanics
- `Retry` resets state to `loading` and re-runs `fetchHome()`.
- No automatic retry-with-backoff (avoid burning network on persistent failures).
- The 5 s timeout from §5 enforces a hard ceiling per attempt.

### Lifecycle
- `onMount` triggers the first fetch.
- Module-level in-memory cache short-circuits subsequent renders within the same app session.
- No `onCleanup` work needed (fetch promise doesn't dangle past component once the AbortController fires).

---

## 8. Tests / build checks

### Mandatory before reporting done
1. `npx tsc --noEmit` in `repo-AgentsCommander` — no TS errors.
2. `npm install` (only if §6 picks `marked`).
3. `npm run build` — Vite build succeeds, bundle size sanity check (`marked` should add ~12 KB gzipped).

### Visual / manual
4. `npm run tauri dev` (or the WG-20 build path used by the team).
5. App boots → Home view is the default. Markdown renders (h1/h2/p/lists/code/links).
6. ActionBar icons row: **Home button is left of flame**. Tooltip "Home". Active state when in Home view.
7. Click **Home** while a session is active → terminal pane shows Home; session is **not** destroyed; clicking the session in the sidebar returns to its terminal.
8. Click a session → mode flips to `"session"`; clicking Home flips back.
9. Disconnect network, click Home, observe error state + Retry. Reconnect, click Retry → success.
10. Detached window: Home does not appear (locked session always shows terminal).
11. Browser mode (if used): same fetch path works (CORS check).

### Type-checking detail
- `marked.parse` is synchronous when called without async extensions; cast to `string`. If TS complains, use `marked.parse(md, { async: false })`.

### No automated tests added
The existing repo uses Vitest sparingly. Adding a unit test for the link-rewriter would be useful but is non-blocking; defer unless code review insists.

---

## 9. Risks and open questions

### Q1 — Does `mblua-patch-1` branch + `docs/home.md` exist upstream right now?
The implementer should `curl -sI` the raw URL once before starting. If it 404s, either (a) push the doc first or (b) hardcode a temporary URL the team controls. The fallback static block in §7 mitigates user-facing impact, but a 404 in the happy path would look broken.

### Q2 — Branch durability of `mblua-patch-1`
If that branch is later merged + deleted, the URL breaks. Options:
- (a) Move the doc to `main` after PR merge and update `HOME_DOC_URL` in the same PR.
- (b) Keep `mblua-patch-1` as a long-lived "docs" branch (anti-pattern).
- (c) Use a GitHub release asset URL (more stable but heavier).
**Recommendation: (a).** Track as follow-up in the issue.

### Q3 — Auto-flip back to Home when a session is destroyed?
Today, when the active session is destroyed and there is no replacement, the user sees the bare "No active session" empty state. Should we auto-flip `viewMode` to `"home"` so they always land on Home when nothing is running?
**Recommendation: yes**, in `onSessionDestroyed` when no sessions remain. Subtle, but avoids a near-empty terminal pane after closing the last session. Confirm with tech-lead before implementing.

### Q4 — Overlap with Guide window's Tutorial tab
`src/guide/components/TutorialTab.tsx` already has hand-written cards explaining the same thing. We are not removing or refactoring it in this issue — Home is a separate surface that pulls from a remote doc. Long-term we may want to make the Guide window also load `home.md`, but that is out of scope.

### Q5 — Theming of rendered Markdown
The `home.css` file must use CSS variables only (`var(--bg-primary)`, `var(--text-muted)`, `var(--accent)`, etc.) so theme switching works. No hardcoded colors. Industrial-Dark aesthetic: minimal borders, separation by background-shift only, animations 150–200 ms.

### Q6 — Font for code blocks
Use `var(--font-mono)` (Cascadia Code → JetBrains Mono fallback) per the role doc. If the variable isn't defined for code, define it inline using the same fallback chain.

### Q7 — `marked` security posture
We trust GitHub-served Markdown from our own repo. We do not currently add DOMPurify. If the threat model ever expands (user-submitted MDX / arbitrary URL configurable by user), revisit and add it. Document this trade-off in a comment in `HomeView.tsx`.

### Q8 — Version bump policy
Bump `tauri.conf.json` from `0.8.5` → `0.8.6` as part of the implementation commit (per memory). Confirms freshness in the user's running build.

---

## 10. Implementation order (for the next session)

1. Confirm Q1 (raw URL returns 200).
2. Add `marked` to `package.json` and `npm install`.
3. Create `src/shared/stores/view.ts` and `src/shared/constants.ts` entry.
4. Create `src/terminal/components/HomeView.tsx` + `home.css`.
5. Wire `TerminalApp` to render Home / NoSession / TerminalView 3-way.
6. Wire `ActionBar` to insert the Home button left of flame.
7. Hook `onSessionSwitched` / `onSessionCreated` in `TerminalApp` to call `viewStore.showSession()`. (Optional Q3) hook `onSessionDestroyed` to flip back to home when sessions list is empty.
8. Bump version in `tauri.conf.json`.
9. `npx tsc --noEmit` + `npm run build`.
10. Visual run-through per §8.
11. Commit on `feature/164-home-screen-docs`. Never `main`.

End of plan.
