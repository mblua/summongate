# Plan: Highlight selected replica/coordinator with active background (Issue #97)

**Branch:** `feature/97-replica-item-active-highlight`
**Status:** READY FOR IMPLEMENTATION

<!-- dev-webpage-ui round-2-approval -->
**Round-3 final consensus pass — APPROVED by dev-webpage-ui (2026-04-30).**

All three round-1 concerns remain satisfied: (1) `classList` reactivity strategy at
`ProjectPanel.tsx:489` is unchanged from round-1 — no JSX/store handling
edits in rounds 2 or 3; (2) Arctic Ops light values `(80, 140, 220, 0.32)` /
`(20, 80, 180, 0.75)` in §4.3.2/§4.3.3 are unchanged; (3) my B8–B13 and
QA4–QA6 test additions are intact, with one legitimate B10 rewording per
grinch G11 (cascade-win semantics, not visual layering — grinch was right;
I was using imprecise wording).

I accept grinch's G3 correction to my round-1 R7 (I claimed
`.session-item-status` had `transition: all 200ms` — it does not; the dot
flips instantly). I missed the G1 noir-minimal blocker in round-1 (per-theme
cascade analysis with shorthand resets was a blind spot). All round-2/3
edits look implementable as-written; no new substantive concerns.

**Ready to receive Step 6 implementation handoff.**
<!-- /dev-webpage-ui round-2-approval -->

---

## 1. Requirement

When a coordinator (or any replica) has its session set as the **active** one
(`sessionsStore.activeId === replicaSession(wg, replica).id`), its row in the
sidebar must show a distinctive background + accent border, mirroring the
existing `.session-item.active` styling.

The single shared renderer `renderReplicaItem` in
`src/sidebar/components/ProjectPanel.tsx` paints rows in **both** target
sections — `.coord-quick-access` (coord-only quick-access) and the workgroup
content (`.ac-wg-subgroup .replica-item` inside `.ac-wg-group`). One JSX edit
covers both.

Today `.replica-item` has **no** active styling; selection is invisible in
both sections. This plan adds it.

In addition, this plan tunes the **Arctic Ops light-mode** active highlight,
which is currently barely distinguishable from non-active rows for both
`.session-item.active` and the new `.replica-item.active`.

Out of scope (carried verbatim from the issue):

- Offline replicas with no session yet (pre-existing limitation).
- Optimistic-feedback signal during session spawn.
- Project-level "Agents" matrix offline rows (`ProjectPanel.tsx` line 854 fallback).
- Any Rust/backend change.

---

## 2. Affected files

| File | Type of change | Purpose |
|---|---|---|
| `src/sidebar/components/ProjectPanel.tsx` | JSX edit (1 attribute added) | Apply `.active` class conditionally on the shared `<div class="replica-item">` |
| `src/sidebar/styles/sidebar.css` | New rules + 1 modified block | Base `.replica-item.active`, per-theme overrides, coord-quick-access override, Arctic Ops light contrast bump |

No new imports, no new types, no new dependencies. No Rust changes.

---

## 3. JSX change — `src/sidebar/components/ProjectPanel.tsx`

### Context (current code, lines 487–496)

```tsx
return (
  <div
    class="replica-item"
    onClick={() => handleReplicaClick(replica, wg)}
    onContextMenu={(e) => {
      const s = session();
      if (s) handleReplicaContextMenu(e, s);
    }}
    title={replica.path}
  >
```

Note: `session` is already declared at **line 410** as
`const session = () => replicaSession(wg, replica);` and reused throughout the
closure. Reuse it — do not re-call `replicaSession(...)`.

### Change — add a single `classList` attribute at line 489 (after `class="replica-item"`)

**Replace** lines 487–496 with:

```tsx
return (
  <div
    class="replica-item"
    classList={{ active: session()?.id === sessionsStore.activeId }}
    onClick={() => handleReplicaClick(replica, wg)}
    onContextMenu={(e) => {
      const s = session();
      if (s) handleReplicaContextMenu(e, s);
    }}
    title={replica.path}
  >
```

### Reactivity rationale

- `session()` reads from `sessionsStore` (via `findSessionByName`) — tracked.
- `sessionsStore.activeId` is a getter on the store — tracked.
- When `activeId` changes (initial mount, `onSessionSwitched` event, click-driven
  `SessionAPI.switch(...)`), `classList` re-evaluates and the class toggles.
- When the row's session does not exist yet (`session()` returns `undefined`),
  `undefined?.id === sessionsStore.activeId` is `false` (any value vs undefined
  is false; `null === undefined` is also false), so `.active` is correctly NOT
  applied. No extra null guard needed.

<!-- dev-webpage-ui round-1 -->
### Reactivity confirmation (dev-webpage-ui review)

**Verdict: the architect's reactivity approach is correct.** Specifics traced
end-to-end in the current code:

1. **`classList={{ active: <expr> }}` is the canonical Solid pattern** for
   boolean class toggles. The `babel-plugin-jsx-dom-expressions` compiler
   wraps the property value in a reactive computation; the expression is
   re-evaluated whenever any tracked dependency inside it changes. There is
   no need to wrap in `() => ...` or `createMemo(...)` — the compiler does it.

2. **Tracked dependencies of `session()?.id === sessionsStore.activeId`:**
   - `state.sessions` array membership (via `state.sessions.find(...)` in
     `findSessionByName` at `src/sidebar/stores/sessions.ts:403–405`).
   - Each iterated session's `.name` field (read by the `find` predicate).
   - The matched session's `.id` (read after the find resolves).
   - `state.activeId` (read via the `get activeId()` getter at
     `src/sidebar/stores/sessions.ts:247–249`).

3. **Trigger paths confirmed in code:**
   - **Initial mount:** `sidebar/App.tsx:132–133` calls
     `SessionAPI.getActive()` and `sessionsStore.setActiveId(activeId)`.
   - **Backend-driven switch:** `sidebar/App.tsx:179–183` —
     `onSessionSwitched(({ id }) => sessionsStore.setActiveId(id))`.
   - **Click → switch:** `handleReplicaClick` at `ProjectPanel.tsx:111` calls
     `SessionAPI.switch(existing.id)`. **Note:** this is **indirect** — the
     click does NOT call `setActiveId` directly. The backend processes the
     switch and emits `onSessionSwitched`, which then calls `setActiveId`.
     Latency is typically <50ms but exists. Same indirection that today's
     `.session-item.active` already lives with — no new risk.
   - **Session created (offline coord first click):** `handleReplicaClick`
     calls `SessionAPI.create(...)` at `ProjectPanel.tsx:135`, then
     `SessionAPI.switch(newSession.id)`. The backend emits
     `onSessionCreated` (which adds the session to `state.sessions` via
     `App.tsx:137–143`) followed by `onSessionSwitched` (which calls
     `setActiveId`). Two-step settle — first the row's `session()` resolves
     (because the session is now in the store), then `.active` lights up.

4. **`classList` precedent in the codebase confirms the pattern works:**
   `ProjectPanel.tsx` already uses `classList={{ collapsed: ... }}` at lines
   597, 625, 726, 750, 834, 1005, 1027, and `classList={{ attached: ... }}`
   at line 558. None require explicit `createMemo` wrapping.

5. **Stale closure: not a concern.** `session = () => replicaSession(wg, replica)`
   is defined inside `renderReplicaItem`, which is called fresh by the `<For>`
   block for each row. Solid's `<For>` is keyed by reference identity — when
   the same `replica` reference re-renders, the closure persists; when the
   `replica` is removed/added, `<For>` rebuilds the row with a new closure.
   No stale-reference bug surfaces.

6. **Performance budget (also confirms architect's R6):** Per `setActiveId`
   call, every `.replica-item` row re-evaluates `classList` once. Cost per
   row = one `state.sessions.find()` (O(M) over M sessions). Total =
   O(R × M) per active-id change, where R = visible replicas. For typical
   R<100 and M<50, this is sub-millisecond. Even at R=500 and M=200 (large
   workgroup) it's under 5ms — well below a 60fps frame budget.

7. **No measurable difference vs. SessionItem precedent.** `SessionItem.tsx:271`
   uses `class={\`session-item ${props.isActive ? "active" : ""} ...\`}` —
   a string-concat pattern that also tracks `props.isActive` reactively.
   Both patterns produce the same outcome. The plan's `classList` form is
   cleaner for boolean toggles and matches Solid's style guide. **Adopt
   this pattern; do not refactor SessionItem.tsx in this PR — out of scope.**

**No reactive-system changes needed.** The architect's expression compiles
and runs correctly as written.

<!-- /dev-webpage-ui round-1 -->

### Why this single edit covers both sections

`renderReplicaItem` is invoked from:

- **Coord quick-access** loop at line 709: `renderReplicaItem(item.replica, item.wg, item.wg.name, runningPeers)`
- **Workgroup content** loop at line 762: `renderReplicaItem(replica, wg)`

Both produce the same `<div class="replica-item">`. Adding the `classList`
once on that div applies to **both**. When the same coordinator appears in
both sections (workgroup expanded), both DOM rows reactively gain `.active`
because they share the same `session()` lookup keyed by `wg.name/replica.name`.

### What we do NOT touch in JSX

- The offline-fallback `<div class="replica-item">` at lines 853–865 (project-level
  "Agents" matrix offline rows) is **not** modified — explicitly out of scope.

<!-- architect round-2 -->
**Round 2 — additional notes per grinch G7/G8:**

- `AcDiscoveryPanel.tsx` (lines 233 and 290) renders `.replica-item` rows but
  is **dead code** — the file is not imported anywhere in the current bundle.
  No JSX edit needed in this PR. If the file is ever revived, it will require
  the same `classList={{ active: <session getter>?.id === sessionsStore.activeId }}`
  treatment on each `<div class="replica-item">` it renders. (Tracked as a
  comment here so future readers don't re-investigate.)
- The offline-fallback `<div class="replica-item">` at `ProjectPanel.tsx:853–865`
  (project-level "Agents" matrix, offline rows only — live agents in that
  section already render via `<SessionItem>` which has working `.active`
  styling) **does inherit** the §4.1 base CSS changes (transparent
  `border-left`, extended transition list). It picks up a 3px transparent
  border-left in themes that don't override it (see updated R1). Since
  selection logic is keyed by session id and these offline rows have no
  session, they never visually toggle to `.active` — but the layout
  footprint changes. Visual review during implementation should confirm
  the offline rows still render acceptably with the new transparent border.
<!-- /architect round-2 -->

---

## 4. CSS changes — `src/sidebar/styles/sidebar.css`

All line numbers below are **re-verified against the current branch**
(`feature/97-replica-item-active-highlight`, base `13539de`). The findings file
referenced offsets that drifted by a handful of lines after 24 commits on
`main`; the values below are current.

### 4.1 Base `.replica-item` — add transparent left border (lines 2314–2321)

**Current:**

```css
.replica-item {
  display: flex;
  align-items: center;
  padding: 4px var(--spacing-md) 4px calc(var(--spacing-md) + 6px);
  cursor: pointer;
  position: relative;
  transition: background var(--transition-fast);
}
```

**Replace with:**

```css
.replica-item {
  display: flex;
  align-items: center;
  padding: 4px var(--spacing-md) 4px calc(var(--spacing-md) + 6px);
  cursor: pointer;
  position: relative;
  transition: background var(--transition-fast), border-left-color var(--transition-fast);
  border-left: 3px solid transparent;
}
```

- Add `border-left: 3px solid transparent` for layout stability when
  `.active` colors the border.
- Add `border-left-color` to the `transition` list so the color change
  animates with the same 150ms ease-out as the existing background fade.

<!-- architect round-2 -->
**Round 2 — corrections per grinch G6:**

- The previous draft cited `.session-item` line 407 as a model that already
  animates `border-left-color`. **That was wrong.** `.session-item` line 407
  declares `transition: background var(--transition-fast), transform
  var(--transition-fast);` — no `border-left-color`. `.session-item.active`
  flips its border color **instantly** today; the eye doesn't notice
  because the bg fade carries the perception. The base `.replica-item`
  rule edited in §4.1 is the **first** rule in the codebase to animate
  `border-left-color` on a row.
- The `border-left-color var(--transition-fast)` we add to base
  `.replica-item` in §4.1 **does not take effect** in three themes that
  declare their own `.replica-item` `transition` property at higher
  specificity (0,2,0):
  - **deep-space** (line 3010): `transition: background 200ms, box-shadow 200ms;`
  - **obsidian-mesh** (line 3433): `transition: background 150ms;`
  - **neon-circuit** (line 3702): `transition: background 150ms, box-shadow 150ms;`
  Without explicit `border-left-color` in these lists, the color flips
  instantly on activation in those three themes. To keep the animation
  consistent with the base, the per-theme transition lists are extended
  in-place — see the round-2 edits inside §4.3.1, §4.3.4, and §4.3.5.
- Themes that **already** animate border colors correctly (no edit
  needed):
  - **noir-minimal** (line 2722): `transition: border-color 150ms,
    background 150ms;` — `border-color` shorthand covers
    `border-left-color`. ✓
  - **arctic-ops** (line 3238): `transition: background 150ms,
    border-color 150ms;` — same. ✓
- Themes that inherit base (`card-sections`, `command-center`) do not
  declare their own `.replica-item` `transition` and inherit the base
  edit cleanly. ✓
<!-- /architect round-2 -->

### 4.2 Add base `.replica-item.active` rule — directly after `.replica-item:hover` (after line 2325)

**Insert (between current lines 2325 and 2327):**

```css
.replica-item.active {
  background: var(--sidebar-active);
  border-left-color: var(--sidebar-accent);
}
```

This mirrors `.session-item.active` (lines 417–420) byte-for-byte except
for the selector. Specificity = (0,2,0).

### 4.3 Per-theme overrides

The themes that already override `.session-item.active` need a parallel
override for `.replica-item.active`. Specificity callouts in §4.4 explain why
some themes need an additional `:has(...)` / `:not(:has(...))` variant.

#### 4.3.1 Deep Space — insert after the existing `.session-item.active` block

**After line 3050, insert:**

```css
[data-sidebar-style="deep-space"] .replica-item.active {
  background: rgba(30, 50, 110, 0.35);
  border-left-color: rgba(80, 160, 255, 0.7);
  box-shadow: inset 0 0 20px rgba(60, 130, 255, 0.08);
}

/* Active coordinator beacon — beats the (0,4,0) coord-beacon rule at line 3072.
   Specificity of this selector: (0,5,0). Tints the existing amber gradient
   toward active-blue while preserving the beacon shape. */
[data-sidebar-style="deep-space"] .replica-item.active:has(.ac-discovery-badge.coord) {
  background: linear-gradient(
    135deg,
    rgba(80, 160, 255, 0.32) 0%,
    rgba(60, 130, 255, 0.18) 100%
  );
  border-color: rgba(120, 180, 255, 0.55);
  box-shadow: inset 0 0 24px rgba(60, 130, 255, 0.18);
}
```

Light-theme deep-space coord-beacon (line 3100) is at (0,6,1) — but the
**non-coord** active variant is fine with (0,3,0) since there is no
light-deep-space `.session-item.active` light override (line 3046 is the
dark variant only). Skip a light variant for non-coord deep-space active —
the dark tone reads acceptably on light bg too. **However**, the
`:has(.coord)` light variant IS needed because its dark base at (0,5,0)
loses to the light coord-beacon at (0,6,1):

**Also after line 3050 (or grouped with the rule above), insert:**

```css
html.light-theme[data-sidebar-style="deep-space"] .replica-item.active:has(.ac-discovery-badge.coord) {
  background: linear-gradient(
    135deg,
    rgba(40, 100, 200, 0.28) 0%,
    rgba(30, 80, 180, 0.16) 100%
  );
  border-color: rgba(20, 80, 180, 0.5);
}
```

Specificity: `html.light-theme[data-sidebar-style="deep-space"] .replica-item.active:has(.ac-discovery-badge.coord)` <!-- architect round-2: corrected per G2 --> = **(0,6,1)**. Beats light coord-beacon **(0,5,1)**.

<!-- architect round-2 -->
**Round 2 — extend the deep-space `.replica-item` transition list (per G6):**

Modify the existing rule at `sidebar.css:3006–3011`:

**Current:**

```css
[data-sidebar-style="deep-space"] .replica-item {
  padding: 5px 12px 5px 18px;
  border-radius: 4px;
  margin: 1px 4px;
  transition: background 200ms, box-shadow 200ms;
}
```

**Replace with:**

<!-- architect round-3: border-left-color → border-color per grinch G14 (round 2). The active coord beacon at sidebar.css:3072 sets `border-color` (4-side shorthand), so animating only `border-left-color` produces 3 instant + 1 animated; `border-color` covers all 4 sides at the same 200ms. Same character count, same specificity. -->

```css
[data-sidebar-style="deep-space"] .replica-item {
  padding: 5px 12px 5px 18px;
  border-radius: 4px;
  margin: 1px 4px;
  transition: background 200ms, box-shadow 200ms, border-color 200ms;
}
```

Specificity (0,2,0) — wins over the base `.replica-item` transition added
in §4.1.

<!-- architect round-3 -->
**Round 3 — corrected transition shorthand per grinch G14:**

The round-2 draft used `border-left-color 200ms` here, which animated
only the LEFT border color. But §4.3.1's active coord rule at
`sidebar.css:3072+` sets `border-color: rgba(120, 180, 255, 0.55)` — a
**4-side shorthand** that flips all four border colors of the 1px coord
beacon. Animating only `border-left-color` produced a "3 instant + 1
animated" mismatch on the beacon's 1px border.

**Fix:** use `border-color 200ms` (a 4-side shorthand transition) instead.
Same character count, same `(0,2,0)` specificity. Now all four sides of
the coord beacon tween in lockstep at 200ms when activated. Non-coord
deep-space rows still use the base 3px `border-left` (only the left side
ever changes color, so transitioning `border-color` shorthand still
correctly animates the left side — the other three sides are
`transparent` and stay `transparent`, no visible animation).
<!-- /architect round-3 -->
<!-- /architect round-2 -->

#### 4.3.2 Arctic Ops — insert after the existing `.session-item.active` light block

**After line 3286, insert:**

```css
[data-sidebar-style="arctic-ops"] .replica-item.active {
  background: rgba(160, 200, 255, 0.08);
  border-left-color: rgba(100, 180, 255, 0.6);
}

html.light-theme[data-sidebar-style="arctic-ops"] .replica-item.active {
  background: rgba(80, 140, 220, 0.32);
  border-left-color: rgba(20, 80, 180, 0.75);
}
```

#### 4.3.3 Arctic Ops — light-mode contrast bump for the EXISTING `.session-item.active`

**Modify lines 3283–3286 (current values are too subtle on light surface):**

**Current:**

```css
html.light-theme[data-sidebar-style="arctic-ops"] .session-item.active {
  background: rgba(180, 215, 255, 0.25);
  border-left-color: rgba(40, 100, 200, 0.5);
}
```

**Replace with:**

```css
html.light-theme[data-sidebar-style="arctic-ops"] .session-item.active {
  background: rgba(80, 140, 220, 0.32);
  border-left-color: rgba(20, 80, 180, 0.75);
}
```

#### 4.3.3.1 Color rationale — Arctic Ops light contrast bump

| Sample | rgba | Approx. solid blend on `--sidebar-bg` `#f5f5f7` | Lightness drop vs bg |
|---|---|---|---|
| Old `.session-item.active` bg | `rgba(180, 215, 255, 0.25)` | ≈ `#e8edf6` | ~3% |
| New `.session-item.active` bg | `rgba(80, 140, 220, 0.32)` | ≈ `#c1d7f6` | ~9% |
| Old border accent | `rgba(40, 100, 200, 0.5)` | ≈ `#7a99c8` | (border) |
| New border accent | `rgba(20, 80, 180, 0.75)` | ≈ `#3361b2` | (border) |

- `(80, 140, 220)` and `(20, 80, 180)` are **inside the Arctic Ops blue
  family** already used by the theme (compare hover at line 3247 which uses
  `rgba(180, 210, 255, 0.2)` on bg and `rgba(40, 100, 200, 0.4)` on border).
  Same hue, deeper alpha and saturation.
- The new `--sidebar-accent` light-theme value is `#0066cc` =
  `rgba(0, 102, 204, 1)`; the new border `(20, 80, 180, 0.75)` is a near-equivalent
  with slightly more violet to keep the existing Arctic visual character.
- New active background sits ~6% darker than the existing hover background
  (`rgba(180, 210, 255, 0.2)` ≈ `#e3ebf5`), so hover and active are clearly
  distinct.
- All values stay within the existing palette (Arctic blues), no random colors.

<!-- dev-webpage-ui round-1 -->
**Palette confirmation (dev-webpage-ui review): the proposed values are
palette-correct.** Cross-checked against every Arctic Ops light rule:

| Existing rule | rgba | Family |
|---|---|---|
| `.project-panel` border (line 3165) | `(140, 180, 220, 0.3)` | mid-saturation blue |
| `.ac-wg-group` border (line 3205) | `(140, 180, 220, 0.2)` | same |
| `.project-title` color (line 3188) | `(30, 60, 120, 0.8)` | deep blue text |
| `.ac-wg-name` color (line 3227) | `(40, 80, 150, 0.5)` | deep blue text |
| `.replica-item:hover` bg (line 3248) | `(180, 210, 255, 0.2)` | pale icy blue |
| `.replica-item:hover` border (line 3249) | `(40, 100, 200, 0.4)` | mid blue |
| Old `.session-item.active` bg (line 3284) | `(180, 215, 255, 0.25)` | **same family as hover** ← root cause of the invisibility |
| Old `.session-item.active` border (line 3285) | `(40, 100, 200, 0.5)` | **same RGB as hover, only alpha +0.1** |
| **NEW** `.session-item.active` bg | `(80, 140, 220, 0.32)` | between `(140, 180, 220)` and `(40, 80, 150)` |
| **NEW** `.session-item.active` border | `(20, 80, 180, 0.75)` | ≈ `--sidebar-accent` light (`#0066cc` = `(0, 102, 204)`) shifted slightly violet |

**Why the old values were invisible:** the bg `(180, 215, 255, 0.25)` and
hover bg `(180, 210, 255, 0.2)` share the same base RGB and differ only by
0.05 alpha — effectively imperceptible. Border `(40, 100, 200, 0.5)` vs
hover border `(40, 100, 200, 0.4)` share the same base RGB with only 0.1
alpha difference — also barely distinguishable. The user's complaint is
spot-on; the fix MUST shift the base RGB, not just the alpha.

**Why the new values work:**

1. **Base RGB shift, not alpha shift** — `(80, 140, 220)` is genuinely
   different from `(180, 210, 255)` in saturation (more chroma) and
   lightness (~50% darker). At 0.32 alpha on `--sidebar-bg #f5f5f7` it
   blends to ~`#c1d7f6`, which is ~9% darker than the bg vs the old ~3%.
2. **Border `(20, 80, 180)` is essentially `--sidebar-accent`** — the
   light theme `--sidebar-accent` is `#0066cc` = `(0, 102, 204)`. The
   proposed `(20, 80, 180)` is the same accent shifted slightly violet to
   match the existing `(30, 60, 120)` / `(40, 80, 150)` text-color family.
   This is consistent with how `.session-item.active` in the default
   theme uses `var(--sidebar-accent)` directly. ✓
3. **Stacking inside nested containers stays distinct.** The replica row
   sits inside `.project-panel` (light bg `(220, 235, 255, 0.5)` →
   ≈ `#e8f0fb` blended) and often inside `.ac-wg-group` (light bg
   `(220, 235, 255, 0.35)` → ≈ `#e4eefc` blended). New active bg blended
   on those: ~`#b7d0f1` to `#b5cff2` — clearly distinct against the
   near-white containers.
4. **Hover and active stay distinct.** Hover blends to ≈ `#e3ebf5`
   (almost-white bluish), new active to ≈ `#c1d7f6` (clearly bluer,
   ~6% darker). Reading hover and active side-by-side, the difference
   is obvious.
5. **No collision with `.session-item-status.active` glow at line 3289**
   (`box-shadow: 0 0 6px rgba(100, 180, 255, 0.5)`) — that's a status-dot
   shadow, applied via a sibling selector, completely independent.

**Verdict: ship the proposed values as-is.** I would not propose alternatives.

<!-- /dev-webpage-ui round-1 -->

#### 4.3.4 Obsidian Mesh — insert after the existing `.session-item.active` block

**After line 3479, insert:**

```css
/* Active coord rows — beats coord-beacon at (0,4,0). Specificity (0,5,0). */
[data-sidebar-style="obsidian-mesh"] .replica-item.active:has(.ac-discovery-badge.coord) {
  background: rgba(255, 140, 60, 0.18);
  border-left-color: rgba(255, 180, 80, 0.85);
}

/* Active worker rows — beats worker-recede at (0,4,0). Specificity (0,5,0).
   `opacity: 1` overrides the (0,4,0) `opacity: 0.75` from line 3518 so the
   active worker is fully legible. */
[data-sidebar-style="obsidian-mesh"] .replica-item.active:not(:has(.ac-discovery-badge.coord)) {
  background: rgba(220, 160, 40, 0.1);
  border-left-color: rgba(220, 160, 40, 0.6);
  opacity: 1;
}
```

<!-- architect round-2 -->
**Round 2 — apply tech-lead's decision on the light-theme gap (option (b),
per dev-webpage-ui round-1 finding):**

The dev-webpage-ui round-1 enrichment (below) flagged that an active coord
row in **light obsidian-mesh** is repainted by the light coord-beacon at
`sidebar.css:3506–3508` (specificity `(0,5,1)`) — the dark active rule
above sits at `(0,5,0)` and loses on the type-selector tiebreak. Tech-lead
chose **option (b)**: add a light-theme variant. Specificity-validated by
grinch round-1.

**After the obsidian-mesh dark active block above, insert:**

```css
/* Light-theme coord active — beats the light coord-beacon at (0,5,1).
   Specificity (0,6,1). Color values are saturated variants of the
   existing light orange-coord palette at sidebar.css:3506–3508 so the
   active state reads as "selected" while staying in the theme's
   visual language. */
html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item.active:has(.ac-discovery-badge.coord) {
  background: rgba(220, 130, 30, 0.18);
  border-left-color: rgba(180, 90, 20, 0.85);
}
```

The light **worker** active rule does **not** need a separate variant —
there is no existing `html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.coord))`
rule, so the dark-mode worker active rule (specificity `(0,5,0)`) reaches
the row in light mode unobstructed and paints the worker recede correctly.

**Round 2 — extend the obsidian-mesh `.replica-item` transition list (per G6):**

Modify the existing rule at `sidebar.css:3431–3435`:

**Current:**

```css
[data-sidebar-style="obsidian-mesh"] .replica-item {
  padding: 3px 10px 3px 14px;
  transition: background 150ms;
  border-left: 2px solid transparent;
}
```

**Replace with:**

```css
[data-sidebar-style="obsidian-mesh"] .replica-item {
  padding: 3px 10px 3px 14px;
  transition: background 150ms, border-left-color 150ms, opacity 150ms;
  border-left: 2px solid transparent;
}
```

Adds `border-left-color` and `opacity` to the transition list. The
`opacity` tween already exists for the worker `:not(:has(.coord))` block at
line 3519 — including it on the base block keeps the transition declared
once at the parent rule and lets both coord and non-coord rows animate
opacity if it ever changes.
<!-- /architect round-2 -->

<!-- architect round-3 -->
**Round 3 — additionally extend the obsidian-mesh WORKER transition list (per grinch G13):**

The round-2 fix above updates the COORD-row transition at sidebar.css line
3433 (specificity `(0,2,0)`). But obsidian-mesh's WORKER rule at line 3519
declares its own `transition` at higher specificity `(0,4,0)`, which
overrides the line 3433 transition for non-coord rows. The round-2 edit
left line 3519 untouched — so worker active rows in obsidian-mesh flip
border-left-color **instantly** while coord active rows tween over 150ms.
Internal inconsistency within the theme.

**Fix — also modify the existing rule at `sidebar.css:3516–3520`:**

**Current:**

```css
[data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.ac-discovery-badge.coord)) {
  padding: 2px 8px 2px 14px;
  opacity: 0.75;
  transition: opacity 150ms, background 150ms;
}
```

**Replace with:**

```css
[data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.ac-discovery-badge.coord)) {
  padding: 2px 8px 2px 14px;
  opacity: 0.75;
  transition: opacity 150ms, background 150ms, border-left-color 150ms;
}
```

Same `(0,4,0)` specificity, same line position, just appends one transition
target. After the edit, both coord and worker active rows in obsidian-mesh
animate `border-left-color` over 150ms — the theme is internally
consistent.
<!-- /architect round-3 -->



<!-- dev-webpage-ui round-1 -->
**GAP — light-mode obsidian-mesh active coord rows will be invisible (dev-webpage-ui review)**

The plan adds dark-only rules for obsidian-mesh active. But the existing CSS
has a **light-theme** coord-beacon at `sidebar.css:3506–3508`:

```css
html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item:has(.ac-discovery-badge.coord) {
  background: rgba(200, 100, 20, 0.06);
  border-left-color: rgba(200, 100, 20, 0.35);
}
```

Specificity of that rule: `(0,5,1)` — `html` (c=1) + `.light-theme` (b=1) +
`[data-sidebar-style]` (b=1) + `.replica-item` (b=1) + `:has(.coord)` (b=2).

The plan's new dark active-coord rule is `(0,5,0)` (no `html` type selector).
**`(0,5,1)` beats `(0,5,0)`** because the `c` component breaks the tie when
the `b` count is equal. So in **light-mode obsidian-mesh**, an active coord
row will be repainted by the light coord-beacon — the active highlight will
not appear.

**Two options:**

(a) **Confirm obsidian-mesh is dark-only and document the constraint.** §5.3
of the test plan already says "n/a (this theme is dark-only AFAICT —
confirm)". If the project policy is "obsidian-mesh light is unsupported,"
no fix is needed; just remove the light-theme coord-quick-access override
in §4.5 too (it's vestigial), and add a comment in this section. *I cannot
confirm this on my own — the user toggle exists in `variables.css` (line
54: `html.light-theme { ... }`) but the project may simply not maintain
obsidian-mesh's light tones.*

(b) **Add a light-theme variant** to keep parity with deep-space §4.3.1.
Insert after the dark obsidian-mesh active block:

```css
/* Light-theme coord active — beats light coord-beacon at (0,5,1).
   Specificity (0,6,1). Tints the existing light orange-coord visuals
   slightly more saturated so the active state reads as "selected". */
html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item.active:has(.ac-discovery-badge.coord) {
  background: rgba(220, 130, 30, 0.18);
  border-left-color: rgba(180, 90, 20, 0.85);
}
```

The light worker active rule at (0,5,0) is **fine** as-is — there is no
existing `html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.coord))`
rule, so the dark active rule reaches the row in light mode without
override (just with default-light-theme containers around it).

**Recommendation:** go with (a) if the team agrees obsidian-mesh is
dark-only (per §5.3's hedge). Otherwise (b) is a 4-line addition.

<!-- /dev-webpage-ui round-1 -->

#### 4.3.5 Neon Circuit — insert after the existing `.session-item.active` light block

**After line 3747, insert:**

```css
[data-sidebar-style="neon-circuit"] .replica-item.active {
  background: rgba(255, 0, 180, 0.05);
  border-left-color: rgba(255, 0, 180, 0.5);
}

html.light-theme[data-sidebar-style="neon-circuit"] .replica-item.active {
  background: rgba(255, 0, 180, 0.06);
  border-left-color: rgba(200, 0, 140, 0.45);
}
```

These mirror the existing `.session-item.active` values for the same theme
(lines 3739–3746). Neon Circuit's base `.replica-item` (line 3698) has no
border-left override, so it inherits the new base `border-left: 3px solid
transparent` from §4.1.

<!-- architect round-2 -->
**Round 2 — extend the neon-circuit `.replica-item` transition list (per G6):**

Modify the existing rule at `sidebar.css:3698–3703`:

**Current:**

```css
[data-sidebar-style="neon-circuit"] .replica-item {
  padding: 4px 12px 4px 16px;
  margin: 1px 2px;
  border-radius: 3px;
  transition: background 150ms, box-shadow 150ms;
}
```

**Replace with:**

```css
[data-sidebar-style="neon-circuit"] .replica-item {
  padding: 4px 12px 4px 16px;
  margin: 1px 2px;
  border-radius: 3px;
  transition: background 150ms, box-shadow 150ms, border-left-color 150ms;
}
```
<!-- /architect round-2 -->

<!-- architect round-2 -->
#### 4.3.6 Noir Minimal — per-theme override (G1 BLOCKER FIX)

**Why this exists (per grinch G1):** noir-minimal declares
`border-left: 2px solid transparent` on `.replica-item` at
`sidebar.css:2719–2723` (specificity `(0,2,0)`). The base
`.replica-item.active` rule from §4.2 has equal specificity `(0,2,0)` but
appears **earlier** in source order (~line 2326 vs 2721). With equal
specificity and noir-minimal coming later, noir-minimal's
`border-left: 2px solid transparent` shorthand resets `border-left-color`
to transparent — **killing the active accent.** Result: active rows in
noir-minimal show only a barely-visible bg shift, no accent border.
This was a blocker.

**Fix — insert after `[data-sidebar-style="noir-minimal"] .replica-item:hover`
at sidebar.css line 2727:**

```css
[data-sidebar-style="noir-minimal"] .replica-item.active {
  background: var(--sidebar-active);
  border-left-color: var(--sidebar-accent);
}
```

Specificity `(0,3,0)` — beats both the noir-minimal `.replica-item` rule
`(0,2,0)` and the noir-minimal `.replica-item:hover` rule at line 2725
`(0,3,0)` via source order (the new rule comes later). Width stays at
2px (set by noir-minimal's `.replica-item` shorthand at `(0,2,0)` since
the `.active` rule only sets `border-left-color`, not the full shorthand).

**Default vars are noir-minimal-tuned.** `--sidebar-active` (`#161628`)
and `--sidebar-accent` (`#00d4ff`) at `variables.css:6, 8` are the noir
defaults — using them keeps the active state visually consistent with
`.session-item.active` in this theme. No new color values introduced.

**Note on noir-minimal's transition:** the existing rule at line 2722
declares `transition: border-color 150ms, background 150ms;` —
`border-color` shorthand covers `border-left-color`, so the active accent
animates correctly with no transition-list edit needed.

**Source-order placement matters.** Insert the rule AFTER the
`:hover` rule at line 2725, NOT before. The `:hover` rule at
`(0,3,0)` would otherwise win at the same specificity. With the new rule
placed after `:hover`, the active state wins source-order on the rare
"hovered while active" combination — which is harmless because
`:hover` sets the same `border-left-color: var(--sidebar-accent)` value
anyway (visually identical).

<!-- /architect round-2 -->

### 4.4 Specificity rationale (why the extra `:has(...)` rules exist)

<!-- architect round-2: corrected per G2 — `html` counts as a type selector (0,0,1), not a class. Light-theme rows previously listed at (0,6,1) / (0,7,1) are recomputed to (0,5,1) / (0,6,1). New entries added for noir-minimal (G1 fix) and the coord-quick-access neutralizer pair. Relative ordering and cascade behavior unchanged — only the numerical labels were off. -->

**Convention used in this table:** specificity is written `(a, b, c)` per
the W3C definition where `a` = ID selectors, `b` = class / attribute /
pseudo-class selectors (including each `:has()` argument's contribution),
`c` = type / element selectors and pseudo-elements. `html` counts as a
type selector and contributes to `c`, not `b`.

| Selector | Specificity | Purpose |
|---|---|---|
| `.replica-item.active` | (0,2,0) | Base default-theme highlight |
| `[data-sidebar-style="noir-minimal"] .replica-item.active` (new, §4.3.6 G1 fix) | (0,3,0) | Required to beat noir-minimal's `.replica-item` shorthand at line 2721 (which sits later in source order at the same `(0,2,0)` and resets `border-left-color: transparent`) |
| `[data-sidebar-style="X"] .replica-item.active` | (0,3,0) | Per-theme tuned highlight (arctic-ops, deep-space, obsidian-mesh, neon-circuit) |
| `[data-sidebar-style="deep-space"] .replica-item:has(.ac-discovery-badge.coord)` (existing line 3072) | (0,4,0) | Existing coord beacon — beats the per-theme `.active` rule above |
| `[data-sidebar-style="deep-space"] .replica-item.active:has(.ac-discovery-badge.coord)` (new) | (0,5,0) | Required to beat coord-beacon for active coord rows |
| `[data-sidebar-style="obsidian-mesh"] .replica-item:has(.ac-discovery-badge.coord)` (existing line 3488) | (0,4,0) | Same problem in obsidian-mesh — coord beacon |
| `[data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.ac-discovery-badge.coord))` (existing line 3516) | (0,4,0) | Worker recede (`opacity: 0.75`) — beats per-theme `.active` |
| `[data-sidebar-style="obsidian-mesh"] .replica-item.active:has(...)` / `:not(:has(...))` (new) | (0,5,0) | Required to beat both above |
| **Coord-quick-access neutralizer pair (§4.5)** | | |
| `.coord-quick-access .replica-item:has(.ac-discovery-badge.coord)` (existing line 3765) | (0,4,0) | Dark-theme coord-quick-access neutralizer |
| `.coord-quick-access .replica-item.active:has(.ac-discovery-badge.coord)` (new) | (0,5,0) | Required to beat the dark neutralizer |
| **Light-theme rules** (note `html` adds `c=1` to all of these) | | |
| `html.light-theme[data-sidebar-style="deep-space"] .replica-item:has(.ac-discovery-badge.coord)` (existing line 3100) | **(0,5,1)** | Light deep-space coord-beacon (corrected from prior draft) |
| `html.light-theme[data-sidebar-style="deep-space"] .replica-item.active:has(...)` (new) | **(0,6,1)** | Required to beat light coord-beacon (corrected from prior draft) |
| `html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item:has(.ac-discovery-badge.coord)` (existing line 3506) | **(0,5,1)** | Light obsidian-mesh coord-beacon |
| `html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item.active:has(...)` (new, §4.3.4 round-2 option (b)) | **(0,6,1)** | Required to beat light obsidian-mesh coord-beacon |
| `html.light-theme[data-sidebar-style="deep-space"] .coord-quick-access .replica-item:has(.coord)`, `html.light-theme[data-sidebar-style="obsidian-mesh"] .coord-quick-access .replica-item:has(.coord)` (existing line 3782, combined selector) | (0,6,1) | Light coord-quick-access neutralizer (correctly numbered) |
| `html.light-theme[data-sidebar-style="deep-space"] .coord-quick-access .replica-item.active:has(.coord)` (new, §4.5) | (0,7,1) | Required to beat the light combined neutralizer (correctly numbered) |

**Cascade behavior is unchanged from round 1.** Every relative ordering
(active-variant beats existing-rule for the same coord/worker variant) is
still correct. Only the numerical labels in the previous draft were off
by 1 in the light-theme rows — grinch G2 catch.

Themes **without** `.replica-item:has(.ac-discovery-badge.coord)` or
`:not(:has(...))` rules (Default/Noir, card-sections, command-center,
arctic-ops, neon-circuit) need only the simple per-theme `(0,3,0)`
override (or in noir-minimal's case, the §4.3.6 fix at `(0,3,0)` to
beat the shorthand-reset trap); the per-theme rule wins unobstructed.
The existing CSS confirms this — `arctic-ops` and `neon-circuit` set
their own `.replica-item` rules but neither declares a `:has()` variant.

### 4.5 Coord-quick-access neutralizer override

The existing rule at **line 3765**:

```css
.coord-quick-access .replica-item:has(.ac-discovery-badge.coord) {
  background: transparent;
  border: none;
  ...
}
```

— is at specificity (0,4,0) and applies in **every** theme that enables
coord-quick-access (noir-minimal, arctic-ops, deep-space, obsidian-mesh,
neon-circuit). It zeroes out the coord-beacon visuals because the
`.coord-quick-access` container provides its own framing.

But it ALSO zeroes out **anything** the `.replica-item.active` rules (0,2,0)
or per-theme `.replica-item.active` rules (0,3,0) try to set. Without an
override, an active coord row inside `.coord-quick-access` would have NO
visible highlight in any theme.

#### Add — directly after the existing dark-theme neutralizer (after line 3778)

```css
/* Active highlight inside coord-quick-access — beats the dark-theme
   neutralizer at (0,4,0). Specificity (0,5,0). Uses CSS vars so each theme
   gets its own light/dark `--sidebar-active` and `--sidebar-accent` tones.
   `box-shadow: none` neutralizes the per-theme deep-space active glow
   (§4.3.1, `inset 0 0 24px ...`) which is at lower specificity (0,5,0) but
   a separate selector — it leaks into coord-quick-access without this
   reset. See round-2 G4 fix below. */
.coord-quick-access .replica-item.active:has(.ac-discovery-badge.coord) {
  background: var(--sidebar-active);
  border-left: 3px solid var(--sidebar-accent);
  box-shadow: none;
}
```

#### Add — directly after the existing light-theme neutralizer (after line 3797)

```css
/* Light-theme deep-space + obsidian-mesh have their own neutralizer at
   (0,6,1) which the (0,5,0) rule above cannot beat. Higher-specificity
   override needed for those two. Specificity (0,7,1). `box-shadow: none`
   matches the dark-mode rule for visual parity. */
html.light-theme[data-sidebar-style="deep-space"] .coord-quick-access .replica-item.active:has(.ac-discovery-badge.coord),
html.light-theme[data-sidebar-style="obsidian-mesh"] .coord-quick-access .replica-item.active:has(.ac-discovery-badge.coord) {
  background: var(--sidebar-active);
  border-left: 3px solid var(--sidebar-accent);
  box-shadow: none;
}
```

<!-- architect round-2 -->
**Round 2 — added `box-shadow: none` to both rules above (per grinch G4).**

The previous draft set only `background` and `border-left` on the
coord-quick-access active rules. Grinch caught a leak: the per-theme
deep-space active coord rule from §4.3.1 sets
`box-shadow: inset 0 0 24px rgba(60, 130, 255, 0.18)` at specificity
`(0,5,0)`. Inside `.coord-quick-access`, the existing dark neutralizer
at `(0,4,0)` already has `box-shadow: none`, but the §4.3.1 rule's
`(0,5,0)` beats it — so the deep-space blue inset glow leaks into
coord-quick-access on active deep-space coord rows. The result was
default cyan bg + default cyan border + DEEP-SPACE blue glow — mixed
visual language.

Adding `box-shadow: none` to the new `(0,5,0)` rule (matching neutralizer
intent) at higher specificity than the §4.3.1 rule's `(0,5,0)` via
**source order** (the new coord-quick-access rule is placed AFTER §4.3.1's
deep-space active rule in `sidebar.css`) — wins the cascade and zeroes
the glow. Same reasoning for the light-theme override at `(0,7,1)`.

**Trade-off boundary (per tech-lead, R4 + G4):** inside `.coord-quick-access`,
an active row is **fully neutralized to default accent** — no theme-specific
glow, gradient, beacon styling, or other ornamental treatment. Theme tuning
applies only to the workgroup-section row, not the coord-quick-access twin.
This is the architectural boundary: the container provides its own framing,
and inside the container the active state uses var-driven defaults.
<!-- /architect round-2 -->

#### Trade-off acknowledged

Inside `.coord-quick-access`, an active coord row uses **default**
`--sidebar-active` / `--sidebar-accent` values (cyan/light-blue accent),
not the per-theme tuned colors used in the workgroup section below. This
is intentional — preserving per-theme tuning would require 5 additional
per-theme rules at (0,5,0)/(0,7,1) levels just for the quick-access
container. The user requirement (#97 acceptance criterion: "Selecting a
coordinator in coord-quick-access highlights its row with a clear background
+ accent border") is met by the var-based approach. If user feedback later
asks for per-theme tuning inside coord-quick-access, that's a follow-up.

### 4.6 Summary of CSS line ranges touched

<!-- architect round-3: added obsidian-mesh worker transition row at line 3519 per G13. Also reflected G14 in the deep-space transition row label (`border-color` instead of `border-left-color`). -->

| Action | Existing lines | New rules count | Approx. new lines |
|---|---|---|---|
| Modify base `.replica-item` | 2314–2321 | – | +1 (border-left), +1 (transition arg) |
| Insert base `.replica-item.active` | after 2325 | 1 rule | +4 |
| **Insert noir-minimal `.replica-item.active`** (G1 fix) | after 2727 | 1 rule | +4 |
| **Modify deep-space `.replica-item` transition** (G6 + G14 r3 — `border-color`) | 3006–3011 | – (in-place) | 0 net |
| Insert deep-space overrides | after 3050 | 3 rules | +25 |
| Insert arctic-ops overrides | after 3286 | 2 rules | +10 |
| Modify arctic-ops light `.session-item.active` | 3283–3286 | – (in-place edit) | 0 net |
| **Modify obsidian-mesh `.replica-item` (coord) transition** (G6) | 3431–3435 | – (in-place) | 0 net |
| Insert obsidian-mesh overrides (incl. light option (b)) | after 3479 | **3 rules** | +20 |
| **Modify obsidian-mesh `.replica-item:not(:has(.coord))` (worker) transition** (G13 r3) | 3516–3520 | – (in-place) | 0 net |
| **Modify neon-circuit `.replica-item` transition** (G6) | 3698–3703 | – (in-place) | 0 net |
| Insert neon-circuit overrides | after 3747 | 2 rules | +10 |
| Insert coord-quick-access dark override (incl. `box-shadow: none`) | after 3778 | 1 rule | +6 |
| Insert coord-quick-access light override (incl. `box-shadow: none`) | after 3797 | 1 rule | +6 |

Total: ~85 new lines, **6 selector blocks modified in place** (base
`.replica-item`, arctic-ops light `.session-item.active`, deep-space
`.replica-item` transition, obsidian-mesh `.replica-item` (coord)
transition, **obsidian-mesh `.replica-item:not(:has(.coord))` (worker)
transition** [round 3], neon-circuit `.replica-item` transition).
Single file: `src/sidebar/styles/sidebar.css`.

---

## 5. Manual test plan

The dev should run all of the following in a development build before
declaring the implementation complete. Tests cover both target sections, both
sidebar window modes (unified + detached, since both render the same sidebar
bundle per the role doc), and every themed acceptance-criterion combination.

### 5.1 Pre-test setup

1. Start `pnpm tauri dev` from `repo-AgentsCommander`.
2. Ensure at least one project is loaded with **at least 2 workgroups**, each
   containing at least 1 coordinator and 1 non-coord agent.
3. Open Settings → Appearance and verify the seven sidebar styles are
   selectable.

### 5.2 Behavior tests (core acceptance — run in default Noir theme first)

| # | Action | Expected |
|---|---|---|
| B1 | Click a coordinator row in the workgroup section | The clicked row gains the `.active` class; background switches to `--sidebar-active`; left border-color switches to `--sidebar-accent`; transition is ~150ms |
| B2 | While B1 is selected, expand the same workgroup so the coord also appears in `.coord-quick-access` | Both DOM rows (the one in `.coord-quick-access` and the one inside `.ac-wg-subgroup`) show the active highlight simultaneously |
| B3 | Click a non-coord agent in a workgroup | Previously-active coord row LOSES `.active`; the agent row GAINS it. Highlight transitions cleanly with no flash |
| B4 | Click a coordinator in `.coord-quick-access` | Both that row and its workgroup-section twin gain `.active`; the previous active row loses it |
| B5 | Reload the window (Ctrl+R) while a row is active | After mount, `SessionAPI.getActive()` resolves and the same row returns to active state. Briefly there is no highlight (~50–200ms) — acceptable per the issue's scope |
| B6 | Destroy the active session (right-click → Close, or X button) | The active highlight disappears as the session is removed; the row reverts to offline if no other replica is active |
| B7 | Open the Terminal window separately and switch sessions there | Sidebar reflects the same `activeId`; the `.replica-item.active` class follows the backend's `onSessionSwitched` event in real time |

### 5.3 Per-theme visual verification

For **each** of the seven themes, repeat steps B1 + B2 + B3. Switch dark
↔ light where supported. Take a screenshot of an active state in each.

| Theme | Dark mode active row | Light mode active row | Notes |
|---|---|---|---|
| `noir-minimal` | <!-- architect round-2: corrected per G12 -->per-theme override (§4.3.6, G1 fix) — `var(--sidebar-active)` bg + `var(--sidebar-accent)` 2px left border | n/a (Noir is dark only) | **Critical (G1 blocker check):** confirm the 2px LEFT BORDER actually paints in the cyan accent color when active. Specifically: an active row should be visually **distinct from a non-active row** by both bg shift AND a clearly visible left-border accent. If the left border is invisible / transparent on an active noir-minimal row, the §4.3.6 override didn't fire — back-debug specificity (the new rule must be at line >2725 to win source order over `:hover`). |
| `card-sections` | base `.replica-item.active` | base `.replica-item.active` | Theme overrides padding (line 2810) — confirm border-left renders inside the card |
| `command-center` | base `.replica-item.active` | n/a | Theme overrides padding (line 2878) |
| `deep-space` | per-theme rule (line 3046+) — non-coord active uses blue gradient; coord active overrides the amber beacon with blue gradient | per-theme rule for coord active only | Critical: confirm an active coord row STILL READS as a beacon (gradient + border-radius preserved) but tinted blue |
| `arctic-ops` | per-theme rule | **REGRESSION FIX TARGET** — verify `.session-item.active` and `.replica-item.active` are now clearly distinguishable from non-active rows | Hover and active should be visually distinct (active is darker/saturated) |
| `obsidian-mesh` | per-theme rule (coord and worker variants) — coord-beacon amber tint, worker `opacity: 1` regained | n/a (this theme is dark-only AFAICT — confirm) | Critical: confirm an active worker row jumps from `opacity: 0.75` to `opacity: 1` visually |
| `neon-circuit` | per-theme rule (pink/magenta) | per-theme rule (pink/magenta, slightly toned for light) | Confirm hover (which uses `box-shadow: inset 2px`) and active (which uses `border-left-color`) read as distinct treatments |

### 5.4 Coord-quick-access specific tests

| # | Action | Expected |
|---|---|---|
| QA1 | In each theme that enables `.coord-quick-access` (noir-minimal, arctic-ops, deep-space, obsidian-mesh, neon-circuit), click a coordinator | The row inside `.coord-quick-access` shows the active highlight using `var(--sidebar-active)`/`var(--sidebar-accent)` |
| QA2 | In light deep-space and light obsidian-mesh, repeat QA1 | Same — confirm the higher-specificity light-theme override fires (otherwise the highlight is invisible due to the `(0,6,1)` light neutralizer) |
| QA3 | Confirm hover still works on the same row when it's not active | Hover background applies; active background continues to apply when active |

### 5.5 Edge cases

| # | Action | Expected |
|---|---|---|
| E1 | Click an offline coord (no session exists) — session is created asynchronously | The row stays unstyled until `SessionAPI.create` resolves and `setActiveId` fires (small delay acceptable per the issue) |
| E2 | Same coord rendered in two workgroups via cross-project name (`name@originProject`) | If `replicaSession(wg, replica)` resolves to the same session id in both wgs, both rows light up. If resolution differs, only the one whose `replicaSession()` matches `activeId` lights up |
| E3 | Detach the active session to its own window | Per-replica row in the unified sidebar should still reflect `activeId` updates as the backend emits `onSessionSwitched` |

<!-- dev-webpage-ui round-1 -->
### 5.6 Additional tests (dev-webpage-ui review)

| # | Action | Expected |
|---|---|---|
| B8 | Rapidly click 5 different replica rows in <1 second | The `.active` class follows the **last** click. No flicker, no row stuck in active state. Confirms there's no race in the `SessionAPI.switch` → `onSessionSwitched` round trip. |
| B9 | While a row is `.active`, toggle dark↔light theme via Settings | The active highlight redraws using the new theme's `--sidebar-active` / `--sidebar-accent` (or per-theme tuned values). Brief visual transition acceptable; no flash of un-styled content (FOUC). |
| B10 | While a row is `.active`, hover over it | <!-- architect round-2: rewording per G11 — hover doesn't paint at all when active wins the cascade -->Active background **wins the cascade** (equal `(0,2,0)` specificity vs `.replica-item:hover`, decided by source order — active rule is placed AFTER hover in §4.2). Hover background does NOT paint while `.active` is set, in **any** theme. On hover-leave the active styling persists with no flash, no flicker. (If the active bg ever flickers to hover bg, a per-theme `:hover` rule has higher specificity than the active rule for that theme — flag and re-check the §4.4 table.) |
| B11 | Active session is on **screen A**; click a row that resolves to a session on **screen B** in a different workgroup | Active class moves cleanly between rows; previous row's class removal animates over `--transition-fast` (150ms). |
| B12 | In dev tools, programmatically remove the `.active` class from the DOM and force-refresh layout | Solid's reactive system re-applies `.active` on the next reactive tick (because the underlying expression is still truthy). Confirms the class is **declarative**, not imperative — no manual classList management. |
| B13 | Open the discovery view, expand 5+ workgroups so 50+ `.replica-item` rows are visible, then rapidly switch sessions | All visible rows update reactively in the same frame. No row lags behind. Confirms R6 (reactive over-trigger) is not a perceivable issue. |

### 5.7 Coord-quick-access pitfalls (dev-webpage-ui review)

| # | Action | Expected |
|---|---|---|
| QA4 | In **dark** obsidian-mesh, click a coordinator | The row in `.coord-quick-access` lights up with `var(--sidebar-active)` (the noir-default `#161628`) — distinctly **different from** the workgroup-section row's amber-tinted active. **Per §4.5 R4 trade-off this is intentional**, but confirm the visual is acceptable to the user; if not, raise it post-merge. |
| QA5 | In **light** deep-space, click a coordinator | The row in `.coord-quick-access` lights up with the light-theme default (`#dcdce4` bg, `#0066cc` border) — verify the `(0,7,1)` light override fires. (Common failure mode: if specificity is wrong, the row is repainted to look like a normal hovered row, not active.) |
| QA6 | While a `.coord-quick-access` row is active, scroll the workgroup section so its workgroup-section twin scrolls into view | Both rows show `.active` simultaneously without any layout reflow caused by the new transparent border-left. |

<!-- /dev-webpage-ui round-1 -->

---

## 6. Risk callouts

### R1 — 3px content shift in themes that don't override `.replica-item` border-left

<!-- architect round-2: re-enumerated per grinch G5 — the previous list contained two errors. -->

Adding `border-left: 3px solid transparent` to base `.replica-item` (§4.1)
shifts content right by 3px in **only the themes that do not already override
`border-left` on `.replica-item`**. Re-verified list (round 2):

**Themes that DO shift by 3px** (no `border-left` override on
`.replica-item`):

- `card-sections` (`.replica-item` at line 2810): no `border-left` override → +3px shift
- `command-center` (`.replica-item` at line 2878): no `border-left` override → +3px shift
- `deep-space` non-coord rows (`.replica-item` at line 3006): no `border-left` override → +3px shift
- `neon-circuit` (`.replica-item` at line 3698): no `border-left` override → +3px shift

**Themes that already had their own `border-left` override and DO NOT
shift by 3px** (corrected from round 1):

- `noir-minimal` (`.replica-item` at line 2721): **already declares**
  `border-left: 2px solid transparent` → no new shift introduced by §4.1.
  (Round 1 incorrectly listed noir-minimal as shifting; grinch G5 caught
  this.)
- `arctic-ops` (line 3239): `border-left: 2px solid transparent` → no shift.
- `obsidian-mesh` (line 3434): `border-left: 2px solid transparent` → no shift.
- `deep-space` **coord rows** (line 3072): the coord-beacon block sets
  `border: 1px solid rgba(255, 180, 40, 0.12)` (shorthand including
  `border-left: 1px solid ...`) at specificity `(0,4,0)`. This **wins**
  over the base `(0,1,0)` rule — coord rows already have a 1px left
  border (4-side, amber). They do **not** acquire a new 3px transparent
  border on top. (Round 1 implicitly claimed deep-space shifts uniformly;
  grinch G5 caught this — only non-coord deep-space rows shift.)

**Project-level "Agents" matrix offline-fallback rows
(`ProjectPanel.tsx:853–865`) inherit §4.1 changes**. They render a plain
`<div class="replica-item">` with no theme-specific override, so they
also pick up the 3px transparent border in unfilled themes (same list as
above). Live agents in that section render via `<SessionItem>` and are
unaffected. (Round 1 missed this — grinch G8.)

**Why we accept it:** the shift is < 1 character width, only visible if
the user A/B compares an active row vs a non-active row pixel-perfectly,
and is the same trade-off the existing `.session-item` rule made (line
409). Tech-lead's directive in the request message explicitly chose this
approach over `box-shadow`-based alternatives.

**If post-merge feedback says it's too noticeable**, the follow-up is to
reduce `padding-left` by 3px in each non-overriding theme's `.replica-item`
rule — four small edits (card-sections, command-center, deep-space
non-coord rows, neon-circuit). Out of scope for this plan.

### R2 — Active class application requires session existence

The match expression `session()?.id === sessionsStore.activeId` relies on a
session existing for the replica. Replicas without a session cannot be
selected; this is a pre-existing limitation called out in Issue #97 as
out of scope. No new risk introduced.

### R3 — Theme-specific coord beacon visual continuity

In deep-space and obsidian-mesh, an active coord row uses a NEW `:has(.coord)`
override that re-paints the beacon. Manual review during implementation must
confirm the active variant still **reads as a beacon** (not flat) — i.e.,
the gradient and rounded corners are preserved. Visual review is required;
the unit-test budget for this is "look at it in the dev build, capture a
screenshot, confirm it reads correctly."

### R4 — Coord-quick-access uses default accent vs theme accent

§4.5 trade-off: an active coord row inside `.coord-quick-access` uses
`var(--sidebar-active)`/`var(--sidebar-accent)` (cyan in dark / blue in
light), not the per-theme tuned colors. Visually, this means a coord row
that's active in arctic-ops will show in the workgroup section below with
the arctic-ops blue active color, but in the `.coord-quick-access` container
above with the default cyan active color.

**Why we accept it:** preserving per-theme tuning inside `.coord-quick-access`
requires 5–7 additional rules at (0,5,0) / (0,7,1) specificity for each
themed combination, multiplying the CSS-rule count for marginal visual gain.
The current approach satisfies the acceptance criterion ("clearly visible
background + accent border") with minimal new CSS. Follow-up if user feedback
asks for it.

### R5 — Specificity surprise on future theme additions

Anyone adding a new theme that introduces a `:has(.ac-discovery-badge.coord)`
or `:not(:has(...))` rule on `.replica-item` MUST also add a matching
`.replica-item.active:has(...)` / `:not(:has(...))` variant, otherwise active
highlighting will be silently invisible in that theme. Document this in a
short comment above the deep-space active block (§4.3.1) so future
theme-authors notice.

### R6 — Reactive over-trigger of `classList`

Solid's reactive system will re-evaluate `classList` for every row whenever
`sessionsStore.activeId` changes. With N replicas across all expanded
workgroups, each `activeId` change triggers N `classList` evaluations.
For typical N < 100 this is sub-millisecond and not a concern. Confirmed by
inspection of the existing `SessionItem.tsx` line 247 pattern, which uses
the same idiom on a comparable scale.

<!-- dev-webpage-ui round-1 -->
### R7 — Status-dot color change is layered with the new active highlight

`sessionsStore.setActiveId` (at `sessions.ts:289–303`) flips two things on
the relevant sessions: it sets `activeId` (which the new `.active` class
reads) AND it sets the new active session's `status` to `"active"` plus
the previously-active session's `status` back to `"running"`.

The dot in `.replica-item` reads `replicaDotClass(wg, replica)` (function
at `ProjectPanel.tsx:58–66`), which renders `session-item-status active`
or `session-item-status running` based on `session.status`. So when a
replica becomes active:
1. Its `.replica-item.active` class is added (background + border-left
   transition over 150ms).
2. Its `.session-item-status` element switches class from `running` to
   `active` (color flips **instantly** — see correction below).

<!-- architect round-2: corrected per grinch G3 -->
**Round 2 correction (per grinch G3):** the previous draft claimed
`.session-item-status` has `transition: all 200ms`. **That was wrong.**
`.session-item-status` (`sidebar.css:498–503`) has **no** `transition`
property declared:

```css
.session-item-status {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
```

The dot color flips **instantly** when status changes — no animation.
This is consistent with current behavior: today, when a session changes
status from "running" → "active" (or vice versa), the dot color changes
in the same paint frame as the status update. The user already sees this
behavior; no regression introduced by this PR. No CSS change needed.

**Tonal consistency:** the row bg/border animates over 150–200ms while
the dot flips instantly. This is a visually cleaner pattern than animating
both — the dot acts as a hard discrete signal while the row tweens
smoothly. **No new risk** and **no change required.**

### R8 — Obsidian-mesh worker `opacity` jump animates correctly

The new rule `[data-sidebar-style="obsidian-mesh"] .replica-item.active:not(:has(.coord)) { opacity: 1; }`
overrides `opacity: 0.75` from line 3518. The transition list at line 3519
already includes `opacity 150ms`, so the jump from 0.75 → 1.0 animates
over 150ms. ✓ No additional CSS work needed.

**However**, the architect's `transition: background var(--transition-fast),
border-left-color var(--transition-fast)` in §4.1 (the BASE `.replica-item`
rule) does NOT include `opacity`. For obsidian-mesh, the per-theme rule at
line 3519 already covers `opacity` for non-coord rows. For coord rows
(line 3488 onward), there is no `opacity` transition — but coord rows
also do not change opacity on activation. Confirmed safe.

<!-- architect round-3 -->
**Round 3 update (per G13):** the round-3 edit to line 3519 appends
`border-left-color 150ms` to the worker rule's transition list, alongside
the existing `opacity 150ms` and `background 150ms`. Workers in
obsidian-mesh now tween bg, opacity, and border-left-color **all in
parallel** over 150ms when activated — visually consistent with coord
rows in the same theme. The original `opacity 150ms` is preserved
unchanged; this R8 narrative remains correct as written.
<!-- /architect round-3 -->

### R9 — `.replica-item.active` does not inherit the existing `.session-item.active` glow effects

`.session-item-status.active` rules in deep-space (line 3052), arctic-ops
(line 3288), obsidian-mesh (line 3476), neon-circuit (line 3749) add
`box-shadow` glows on the status dot. The new `.replica-item.active`
parent class does NOT need to coordinate with these — they are scoped to
`.session-item-status.active`, a child element whose `.active` modifier is
driven by `replicaDotClass()` reading `session.status === "active"`,
which DOES become true for the active replica's session (per
`setActiveId` at line 293). So the dot glow lights up correctly **without
any change to the new `.replica-item.active` rule**. Verify visually
during implementation, but no extra CSS needed.

<!-- /dev-webpage-ui round-1 -->

---

## 7. Files summary

<!-- architect round-3: added line 3516–3520 obsidian-mesh worker transition to the in-place modify list per G13 -->

| File | Lines touched | Type |
|---|---|---|
| `src/sidebar/components/ProjectPanel.tsx` | line 489 (1 attribute added) | JSX edit |
| `src/sidebar/styles/sidebar.css` | **modify in place:** 2314–2321 (base `.replica-item`), 3283–3286 (arctic-ops light `.session-item.active`), 3006–3011 (deep-space `.replica-item` transition — round-3 uses `border-color`), 3431–3435 (obsidian-mesh `.replica-item` coord transition), **3516–3520 (obsidian-mesh worker `:not(:has(.coord))` transition — round-3 G13)**, 3698–3703 (neon-circuit `.replica-item` transition). **Inserts after:** 2325, **2727 (new)**, 3050, 3286, 3479, 3747, 3778, 3797 | CSS edits + ~85 new lines |

No Rust changes. No new types. No new dependencies.

---

## 8. Pending review steps (per tech-lead workflow)

1. ~~dev-webpage-ui — frontend enrichment: confirm SolidJS reactivity behavior,
   confirm exact color values for arctic-ops light bump are within the
   theme's visual language, flag any animation/timing concerns.~~ **DONE — round 1.**
   See `<!-- dev-webpage-ui round-1 -->` blocks throughout. Summary:
   - Reactivity confirmed sound (§3 enrichment).
   - Arctic Ops light tones confirmed palette-correct (§4.3.3.1 enrichment).
   - **GAP flagged:** light-mode obsidian-mesh active coord rows will be
     invisible due to existing light coord-beacon at higher specificity
     (§4.3.4 enrichment — needs decision: confirm dark-only or add fix).
   - Test plan additions: §5.6, §5.7.
   - Risk additions: R7 (dot animation overlay), R8 (opacity transition),
     R9 (status-dot glow inheritance).
2. ~~dev-rust-grinch — adversarial review: specificity math
   double-check, hunt for breakage in themes I didn't enumerate, challenge
   the §4.5 trade-off and §6 risk callouts.~~ **DONE — round 1.** Verdict
   **NEEDS-CHANGES**. See `## Grinch Review (round 1)` section below for
   findings. Summary: 1 blocker (noir-minimal active border invisibility),
   5 important issues (specificity-table errors, R7 factual error,
   coord-quick-access box-shadow leakage, R1 enumeration inaccuracies,
   per-theme transition list gaps), and several minor doc/scope nits.
3. ~~Architect (this file) — incorporate round-1 review feedback.~~ **DONE — round 2.**
   See `<!-- architect round-2 -->` blocks throughout. Summary in §10.
4. ~~dev-rust-grinch round 2 — confirm round-2 fixes don't introduce new
   specificity gaps.~~ **DONE — round 2.** Verdict **APPROVE** with 3 minor
   nits (G13/G14/G15). See `## Grinch Review (round 2)` section below.
5. ~~Architect — apply G13/G14 fixes (Path A, per tech-lead). G15 remains
   out of scope.~~ **DONE — round 3.** See `<!-- architect round-3 -->`
   blocks. Summary in §10.
6. dev-webpage-ui — final consensus pass. Tech-lead routes round-3 plan
   directly to dev-webpage-ui for implementation.
7. Implementation by dev-webpage-ui.

---

<!-- architect round-2 -->
## 9. Future work (out of scope for this PR — track post-merge)

These items were raised during review but tech-lead deferred them to
follow-up issues. Listed here so they aren't lost.

### 9.1 — A11y: `aria-current` on the active replica row (per grinch G9)

`.replica-item` rows expose **no selection state to assistive tech**:
no `aria-current`, no `role`, no `tabindex`. Adding
`aria-current={session()?.id === sessionsStore.activeId ? "true" : undefined}`
would announce the active row to screen readers and would be a 1-line
addition. Pre-existing gap (also affects `.session-item`, which has the
same omission today). **Tech-lead will open a follow-up issue post-merge.**
Do NOT add it in this PR.

### 9.2 — `handleReplicaClick` swallows errors from `SessionAPI.switch` (per grinch G10)

`ProjectPanel.tsx:111` calls `await SessionAPI.switch(existing.id);`
without a try/catch. If the backend rejects, the failure is silent and
`activeId` does not update — the user sees nothing change but the row
stays unhighlighted. Pre-existing — not introduced by this PR. Tech-lead
flagged as out of scope for #97; track separately.

### 9.3 — `AcDiscoveryPanel.tsx` is dead code (per grinch G7)

The file at `src/sidebar/components/AcDiscoveryPanel.tsx` is not
imported anywhere in the current bundle. It renders `.replica-item`
rows at lines 233 and 290. If ever revived, it will need the same
`classList={{ active: <session getter>?.id === sessionsStore.activeId }}`
treatment on each row to participate in this feature. Captured here as
a reminder; no action in this PR.

### 9.4 — Per-theme tuning inside `.coord-quick-access` (R4 + G4 trade-off)

§4.5 deliberately uses `var(--sidebar-active)` / `var(--sidebar-accent)`
inside the quick-access container, foregoing per-theme tuning to keep
the new CSS-rule count minimal. If post-merge feedback says the active
state inside coord-quick-access feels visually disconnected from the
workgroup-section twin in deep-space, obsidian-mesh, arctic-ops, or
neon-circuit, the follow-up is 5–7 additional `(0,5,0)` / `(0,7,1)` rules
per theme. Track separately if reported.

<!-- /architect round-2 -->

---

<!-- architect round-2 -->
## 10. Round-2 changelog (architect)

Summary of edits made in response to grinch round-1 review and tech-lead
binding decisions in `messaging/20260430-185941-wg5-tech-lead-to-wg5-architect-plan-97-round2-incorporate-grinch.md`:

| ID | Change | Location |
|---|---|---|
| **G1** | Added §4.3.6 noir-minimal per-theme override (BLOCKER fix) — `[data-sidebar-style="noir-minimal"] .replica-item.active { ... }` at `(0,3,0)`, inserted after sidebar.css line 2727 | §4.3.6 (new) |
| **G2** | Recomputed §4.4 specificity table: `html` is type `(0,0,1)` not class. Light-theme deep-space coord-beacon corrected to `(0,5,1)` (was `(0,6,1)`); active variant to `(0,6,1)` (was `(0,7,1)`). Same correction for obsidian-mesh light. Added rows for noir-minimal `(0,3,0)` fix and the coord-quick-access neutralizer pair. Cascade behavior unchanged | §4.4 (in-place) and §4.3.1 (corrected inline claim) |
| **G3** | Removed false claim that `.session-item-status` has `transition: all 200ms`. Updated R7 narrative — dot color flips instantly (consistent with existing behavior, no code change) | §6 R7 |
| **G4** | Added `box-shadow: none` to both new coord-quick-access active rules. Documented trade-off boundary: inside coord-quick-access, active is fully neutralized to default accent (no theme-specific glow, gradient, or beacon leak) | §4.5 (in-place) |
| **G5** | Re-enumerated R1 affected themes: noir-minimal does NOT shift (line 2721 has its own 2px transparent), deep-space coord rows do NOT shift (line 3072 sets a 1px 4-side border at higher specificity), and the project-level offline fallback at `ProjectPanel.tsx:853–865` was added to the inheritance list | §6 R1, §3 |
| **G6** | Added per-theme transition extensions for deep-space (line 3010), obsidian-mesh (line 3433), neon-circuit (line 3702) — each gains `border-left-color` so the active accent animates in those themes. Corrected the misclaim that `.session-item:407` includes `border-left-color`. Confirmed noir-minimal (line 2722) and arctic-ops (line 3238) already animate `border-color` and need no edit | §4.1, §4.3.1, §4.3.4, §4.3.5 |
| **G7** | Documented `AcDiscoveryPanel.tsx` as dead code in §3 and §9.3 — no JSX edit needed in this PR | §3, §9.3 |
| **G8** | Documented project-level offline fallback inheritance of §4.1 changes | §3, §6 R1 |
| **G9** | Added §9.1 future-work entry for `aria-current` follow-up (tech-lead opens post-merge) | §9.1 |
| **G10** | Added §9.2 future-work entry for silent `SessionAPI.switch` errors (pre-existing, separate track) | §9.2 |
| **G11** | Reworded B10: active wins the cascade (equal specificity, source order), hover does not paint while `.active` is set | §5.2 B10 |
| **G12** | Reworded §5.3 noir-minimal test row to explicitly check the 2px cyan accent border-left (catches the G1 blocker visually) | §5.3 |
| **Obsidian-mesh option (b)** | Tech-lead chose option (b); included light coord-active rule cleanly in §4.3.4 body, no longer a "pending decision" | §4.3.4 |

**Items not changed (per tech-lead's "items grinch dismissed" list and
"do not revisit" directive):** R6, B8, initial-mount race, `activeId
=== undefined`, R9 status-dot glow, noir-minimal hover, deep-space
4-side border behavior on active coord rows, obsidian-mesh option (b)
specificity validation. All confirmed sound by grinch and not modified.

**No substantive design changes** were required by the round-1 findings
beyond what tech-lead outlined. Specifically: G4, G6 option (a), and
the obsidian-mesh option (b) are CSS additions (not redesigns); G5/G8
are doc completeness fixes; G2/G3/G11/G12 are narrative corrections.
G1 is a small per-theme override addition, not a redesign.

<!-- /architect round-2 -->

<!-- architect round-3 -->
### Round-3 changelog (architect)

Summary of edits made in response to grinch round-2 review (G13/G14/G15)
and tech-lead's binding decisions in
`messaging/20260430-192421-wg5-tech-lead-to-wg5-architect-plan-97-round3-g13-g14.md`
(Path A — apply the fixes).

| ID | Change | Location |
|---|---|---|
| **G13** | Added round-3 edit in §4.3.4 to extend the obsidian-mesh worker rule transition. Modify `[data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.ac-discovery-badge.coord))` at `sidebar.css:3516–3520` — append `, border-left-color 150ms` to the existing `transition: opacity 150ms, background 150ms`. Both coord and worker active rows in obsidian-mesh now animate `border-left-color` consistently over 150ms. | §4.3.4 (new round-3 block) |
| **G14** | Updated §4.3.1's round-2 deep-space transition edit at `sidebar.css:3010` from `border-left-color 200ms` → `border-color 200ms`. Same character count, same `(0,2,0)` specificity. The active coord beacon at `sidebar.css:3072+` sets `border-color` (4-side shorthand) on activation; transitioning the shorthand `border-color` animates all 4 sides at the same 200ms cadence, eliminating the round-2 "3 instant + 1 animated" mismatch on the 1px coord beacon. | §4.3.1 (in-place CSS update + round-3 explanation block) |
| **G15** | Marked as **out of scope, per tech-lead.** Acknowledged trade-off: `.session-item.active` flips border instantly, `.replica-item.active` animates. Fixing would require modifying `.session-item.active` which is outside the scope of issue #97. | Status note added to grinch round-2 G15 entry |
| Status notes | Added inline status notes to the grinch round-2 G13/G14/G15 entries — "Status: addressed in round 3" / "Status: out of scope, per tech-lead." Audit trail preserved. | `## Grinch Review (round 2)` section |
| §6 R8 narrative | Added round-3 update note acknowledging that worker rows in obsidian-mesh now also animate `border-left-color` alongside the existing `opacity` and `background` tweens. Confirmed R8 narrative remains correct as written. | §6 R8 |
| §4.6 line ranges | Added the new line 3516–3520 worker transition modification row. Updated count: now 6 in-place modified blocks (was 5). Updated label of the deep-space row to reflect `border-color` instead of `border-left-color`. | §4.6 (in-place table edit) |
| §7 files summary | Added line 3516–3520 to the in-place modify list. | §7 (in-place edit) |
| §8 pending review steps | Added round-3 entry, noted that next step is final consensus pass with dev-webpage-ui. | §8 (in-place edit) |

**Net delta for round 3:** 2 transition-list edits in `sidebar.css`,
~5 character net change in the actual CSS (the `border-color`/`border-left-color`
swap is character-equal; the obsidian-mesh worker append is `+ ", border-left-color 150ms"`).
No new rules, no specificity changes, no scope expansion.

**Items dismissed** (per tech-lead's "do not expand beyond G13 + G14"
scope guardrail):
- Grinch's strengthening suggestion to add an explicit CSS comment in
  §4.5 noting "must come AFTER §4.3.1" — the round-2 narrative already
  documents this constraint. Tech-lead did not include it in round-3
  scope. Skipped.
- Source-order strengthening for §4.5 box-shadow neutralization — same
  reason as above.

<!-- /architect round-3 -->

---

<!-- dev-rust-grinch round-1 -->
## Grinch Review (round 1)

**Verdict: NEEDS-CHANGES.**

I independently verified the plan against the current branch
(`feature/97-replica-item-active-highlight`, base `13539de`). All cited line
numbers match. Reactivity claim by dev-webpage-ui in §3 is sound.
`SessionAPI.switch` round-trip race in B8 is bounded by Tauri's ordered
event channel — no new risk. Coord-quick-access dark/light source-order
analysis works as the plan describes.

But the plan ships a per-theme override only for arctic-ops, deep-space,
obsidian-mesh, and neon-circuit. It does **not** ship one for noir-minimal,
and noir-minimal's `.replica-item` rule sets a `border-left` shorthand at
equal specificity to the new base `.replica-item.active`. Source order
makes noir-minimal win. That kills the accent border in the most
common theme. Details below.

### Blocker

#### G1. Noir-minimal active rows lose the accent border-left

- **What:** In noir-minimal, an active replica row will show NO visible
  accent border-left. Only the background changes (and the bg change is
  subtle — `--sidebar-active` `#161628` against `--sidebar-bg` `#0a0a0f`
  differs by ~12 in R/G and ~25 in B; visible but easy to miss without
  the border accent).
- **Why:** The plan adds the base `.replica-item.active` rule at
  specificity (0,2,0) with `border-left-color: var(--sidebar-accent)`.
  But `[data-sidebar-style="noir-minimal"] .replica-item` at
  `sidebar.css:2719–2723` is also (0,2,0) and declares
  `border-left: 2px solid transparent` — a SHORTHAND that resets
  `border-left-color` back to transparent. Equal specificity → source
  order decides. Noir-minimal's rule sits at line 2719; the new active
  rule will be inserted around line 2326 (per §4.2). **2719 > 2326, so
  noir-minimal wins the cascade and `border-left-color` stays transparent
  on active rows.**
- **Confirmation:** I verified the same pattern is NOT a problem for
  arctic-ops or obsidian-mesh, because the plan adds per-theme
  `.replica-item.active` overrides at (0,3,0) / (0,5,0) for those —
  beating their (0,2,0) base. Noir-minimal has no such override.
- **Why dev-webpage-ui missed it:** §3's reactivity audit didn't extend
  to per-theme cascade analysis; §4.4's specificity table only enumerates
  themes that have `:has(...)` / `:not(:has(...))` complications, not the
  simpler "theme overrides border-left via shorthand" case.
- **Fix:** Add a per-theme override:
  ```css
  [data-sidebar-style="noir-minimal"] .replica-item.active {
    background: var(--sidebar-active);
    border-left-color: var(--sidebar-accent);
  }
  ```
  Specificity (0,3,0) — beats noir-minimal's (0,2,0) base. Insert
  alongside the noir-minimal `:hover` rule at sidebar.css:2725 area.
  Document in §4.4 that this case (theme overrides
  `.replica-item` border-left via shorthand) requires per-theme parity.

### Important

#### G2. Specificity table in §4.4 has off-by-1 errors in light-theme rows

- **What:** The architect's table claims:
  - `html.light-theme[data-sidebar-style="deep-space"] .replica-item:has(.ac-discovery-badge.coord)` (line 3100) is **(0,6,1)**.
  - The new `html.light-theme[data-sidebar-style="deep-space"] .replica-item.active:has(...)` is **(0,7,1)**.
- **Correct values:**
  - Line 3100: `html` (c=1) + `.light-theme` (b=1) + `[data-sidebar-style]` (b=1) + `.replica-item` (b=1) + `:has(.ac-discovery-badge.coord)` (b=2, takes specificity of `.ac-discovery-badge.coord`) = **(0,5,1)**.
  - New light deep-space active coord: same as above + `.active` (b=1) = **(0,6,1)**.
- **Why:** dev-webpage-ui already flagged "absolute numbers off by 1 in
  some light-theme rows" but did not correct the table. The relative
  ordering is preserved — new (0,6,1) still beats existing (0,5,1) — so
  cascade behavior is correct. But the table is a documentation hazard:
  a future maintainer adding a (0,6,1) selector will believe it beats
  the light coord-beacon, and it WILL — but they'll be reasoning from a
  table that says they need (0,7,1). Misleads.
- **Same off-by-1 applies to:** the obsidian-mesh light coord variant
  added under tech-lead's option (b). dev-webpage-ui's enrichment block
  for §4.3.4 has the correct (0,5,1) and (0,6,1). The §4.4 table does
  not.
- **Fix:** Correct the §4.4 table:
  - Line 3100: `(0,5,1)` not `(0,6,1)`.
  - New light deep-space active coord: `(0,6,1)` not `(0,7,1)`.
  - Add a row for the obsidian-mesh light variant: existing line 3506 is
    `(0,5,1)`, new active light coord is `(0,6,1)`.
  - Cross-check the §4.5 light coord-quick-access override (called
    `(0,7,1)` in §4.5 prose) — it uses
    `html.light-theme[data-sidebar-style="deep-space"] .coord-quick-access .replica-item.active:has(...)`,
    which by my count is `html`(c=1) + `.light-theme`(b=1) + `[data-sidebar-style]`(b=1) + `.coord-quick-access`(b=1) + `.replica-item`(b=1) + `.active`(b=1) + `:has(...)`(b=2) = **(0,7,1)**. **§4.5 is actually correct.** Table in §4.4 is the only inconsistency.

#### G3. dev-webpage-ui's R7 makes a factually incorrect transition claim

- **What:** R7 says: "the dot has its own transition — `.session-item-status`
  at `sidebar.css` does have a `transition: all 200ms` per the existing
  rules, so the color animates."
- **Reality:** `sidebar.css:498–503` defines `.session-item-status` with
  `width`, `height`, `border-radius: 50%`, `flex-shrink: 0` — and **no
  `transition` property**. The status-dot color flip on activation is
  INSTANT, not a 200ms ease-out.
- **Why it matters:** When `setActiveId` fires, the row's bg/border
  animate over 150ms while the dot snaps to its active color
  immediately. Probably fine UX-wise but the plan's narrative says the
  two transitions "have visually compatible timing" — they don't have
  compatible timing because one timing is missing.
- **Fix:** Either correct R7 to say the dot flips instantly (which is
  acceptable behavior consistent with `.session-item.active` today), or
  if simultaneous animation is desired, propose adding `transition:
  background var(--transition-fast)` to `.session-item-status` — though
  this is scope creep and probably not worth it.

#### G4. Coord-quick-access trade-off (§4.5) leaks per-theme box-shadow into the neutralized container

- **What:** §4.5's new active rule sets only `background` and
  `border-left`. But the deep-space active coord rule (§4.3.1) sets
  `box-shadow: inset 0 0 24px rgba(60, 130, 255, 0.18)`. Both at
  specificity (0,5,0). Source order: §4.5 inserts later (~line 3779+),
  so it wins for the props it sets. **But it doesn't touch box-shadow.**
- **Cascade trace for an active deep-space coord row inside coord-quick-access:**
  - `.coord-quick-access .replica-item:has(.coord)` (0,4,0) at line 3765:
    `box-shadow: none`.
  - `[data-sidebar-style="deep-space"] .replica-item.active:has(.coord)`
    (0,5,0) (new): `box-shadow: inset 0 0 24px rgba(60, 130, 255, 0.18)`.
  - `.coord-quick-access .replica-item.active:has(.coord)` (0,5,0) (new):
    no box-shadow declared.
  - Winner for `box-shadow`: deep-space active coord rule (0,5,0) > neutralizer (0,4,0). The blue inset glow appears.
- **Result:** an active coord in deep-space's coord-quick-access shows
  default cyan bg + default cyan border-left + DEEP-SPACE blue inset
  glow. Mixed visual language. The §4.5 trade-off documents the bg/border
  divergence but not the box-shadow.
- **Fix options:**
  - (a) Document this in §4.5's trade-off section so the implementer
    knows to expect it visually.
  - (b) Add `box-shadow: none` to the new coord-quick-access active rules
    (both dark and light) so the neutralization is complete.
- **Recommendation:** (b) — preserves the neutralizer's stated intent
  ("the container provides its own framing") for ALL declared properties,
  not just bg/border.

#### G5. R1 layout-shift enumeration is incomplete and partially inaccurate

- **What R1 claims:** noir-minimal, card-sections, command-center,
  deep-space, neon-circuit all shift by 3px (no border-left override).
  arctic-ops and obsidian-mesh remain unaffected (already 2px transparent).
- **What's actually true:**
  - noir-minimal `.replica-item` at line 2721 ALREADY has
    `border-left: 2px solid transparent`. It does NOT shift by 3px on
    activation; it shifts by 2px (consistent with arctic-ops/obsidian-mesh).
    R1 incorrectly lumps noir-minimal with the "shifts by 3px" group.
  - deep-space coord rows do NOT shift by 3px on activation, because the
    coord-beacon rule at line 3072 sets `border: 1px solid ...` —
    specificity (0,4,0), wins the border-left-width cascade. Coord rows
    keep 1px border on activate; non-coord rows shift by 3px.
  - The project-level "Agents" matrix offline-fallback `.replica-item`
    at `ProjectPanel.tsx:853–865` will ALSO pick up the new
    `border-left: 3px transparent` and shift by 3px in non-overriding
    themes. Plan §3 explicitly says it's not modified in JSX, but §4.1's
    base CSS edit affects it anyway. R1 doesn't enumerate this.
- **Why it matters:** R1 is the section testers will read to know what
  to verify visually. Inaccurate enumeration = missed regressions.
- **Fix:** Re-enumerate in R1:
  - "Already 2px transparent (no shift)": noir-minimal, arctic-ops, obsidian-mesh.
  - "Shifts 3px on workgroup-section non-coord rows": card-sections,
    command-center, deep-space (non-coord only), neon-circuit.
  - Add a paragraph noting the project-level offline-fallback row
    (`ProjectPanel.tsx:853`) inherits the same shift.

#### G6. Per-theme transition lists in deep-space/obsidian-mesh/neon-circuit don't animate `border-left-color`

- **What:** §4.1 adds `border-left-color` to the BASE `.replica-item`
  transition. Plan rationale says this matches `.session-item` line 407
  — but `.session-item` line 407 is `transition: background, transform`,
  with no `border-left-color`. The plan accidentally adds animation that
  `.session-item` doesn't have.
- **Cascade reality:** Per-theme `.replica-item` rules with their own
  `transition` declaration override the base entirely (transition is not
  cumulative across rules; each `transition` assignment replaces the
  previous one). Per-theme transition lists:
  - deep-space line 3010: `transition: background 200ms, box-shadow 200ms` — no border-left-color.
  - obsidian-mesh line 3433: `transition: background 150ms` — no border-left-color.
  - neon-circuit line 3702: `transition: background 150ms, box-shadow 150ms` — no border-left-color.
- **Result:** In those three themes, the border-left color flips
  INSTANTLY on activate. Background still animates. Visual inconsistency
  vs noir-minimal (which has `transition: border-color 150ms, background 150ms`)
  and arctic-ops (which has `transition: background 150ms, border-color 150ms`).
- **Why it matters:** Minor — a tester who notices it might file a
  follow-up. Functionally fine. But the plan's claim "animates with the
  same 150ms ease-out as the existing background fade" is false in three
  themes.
- **Fix:** Either (a) accept the inconsistency and update §4.1's claim
  to "matches the BASE `.session-item` rule, not all per-theme overrides",
  or (b) extend deep-space/obsidian-mesh/neon-circuit `.replica-item`
  transition lists to include `border-left-color`. Out-of-scope creep
  for (b) — recommend (a) plus a brief note in §6.

### Minor

#### G7. AcDiscoveryPanel.tsx is dead code with `.replica-item` rows; not enumerated

- `src/sidebar/components/AcDiscoveryPanel.tsx` lines 233 and 290 render
  `<div class="replica-item">`. The component is exported but NOT
  imported anywhere else in the codebase (verified via
  `grep -rn AcDiscoveryPanel src/`). It is currently dead code.
- If revived in the future, those rows will pick up the new
  `border-left: 3px transparent` from §4.1 (3px shift) but won't get
  `.active` because no `classList` edit. Whoever revives it must also
  edit those two sites.
- **Fix:** Add a one-line note in §3 ("What we do NOT touch in JSX")
  warning that `AcDiscoveryPanel.tsx` is dead but renders `.replica-item`
  rows; if revived, it needs the same `classList` treatment.

#### G8. Project-level Agents matrix offline fallback (line 853–865) inherits §4.1 changes

- The offline `.replica-item` div at `ProjectPanel.tsx:853–865` has no
  `classList`, no `session()` lookup. It will never have `.active`. ✓
- BUT it picks up the new `border-left: 3px transparent` and the new
  `transition` from §4.1. Layout shift applies to this row too.
- §3's "What we do NOT touch in JSX" mentions this row is excluded from
  JSX changes, but doesn't note that the §4.1 CSS still affects it
  visually.
- **Fix:** Add a sentence to §3 or §6 R1 acknowledging this.

#### G9. A11y: replica rows expose no selection state to assistive tech

- `.replica-item` divs have no `role`, no `tabindex`, no `aria-current`,
  no `aria-selected`. Screen reader users cannot tell which replica is
  active. This is a PRE-EXISTING gap (`.session-item.active` has the
  same issue per `grep` results — only `Titlebar.tsx:121` uses ARIA in
  the sidebar).
- The plan does not regress this. But adding `aria-current="true"` on
  the active row would be a 1-line a11y win. **NOT a blocker** —
  recommend as a follow-up issue.

#### G10. `handleReplicaClick` does not catch errors from `SessionAPI.switch`

- `ProjectPanel.tsx:111`: `await SessionAPI.switch(existing.id);` is not
  wrapped in try/catch. If the backend rejects (e.g., session was
  destroyed between click and dispatch), the click handler errors
  silently. The active highlight stays on the previous row. User sees
  no feedback.
- This is PRE-EXISTING and unrelated to the plan's scope. Not a
  blocker for this plan, but worth a separate issue.

#### G11. Test plan B10 is ambiguously worded

- §5.6 B10: "While a row is .active, hover over it. Hover styling layered
  on top — active background should remain visible. (Hover bg should NOT
  mask the active state in any theme.)"
- This is correct in outcome but the phrase "hover styling layered on
  top" suggests visual layering, when actually the active rule's
  bg/border-color WIN the cascade and hover's are not painted at all.
- **Fix:** Rephrase: "Active styling should remain visible while
  hovering. In themes where the active rule's specificity matches the
  hover rule's, source order ensures active wins."

#### G12. Test plan §5.3 won't catch the noir-minimal blocker (G1)

- §5.3's noir-minimal row says "verify base rule paints". Vague.
  A tester confirming the `.active` class is in the DOM would pass this
  step, but the missing `border-left-color` would only be caught by
  side-by-side visual comparison.
- §5.3 also references "3px transparent border doesn't visibly shift the
  row" — but noir-minimal's existing 2px border-left means there's no
  3px transparent border in this theme. The note is for the wrong theme.
- **Fix:** Once G1 is fixed by adding the noir-minimal per-theme
  override, restate §5.3's noir-minimal expectation: "Verify accent
  border-left appears on activate AND that the row does NOT shift
  (already 2px)."

### Items considered and dismissed

- **R6 reactive over-trigger:** dev-webpage-ui's analysis is sound;
  Solid only re-runs the `classList` expression when a tracked
  dependency changes. With R<100 replicas and M<50 sessions, sub-ms
  cost. No issue.
- **Double-click race (B8):** `SessionAPI.switch` round-trips through
  Tauri's ordered event channel. As long as the backend processes
  switches FIFO, the final `setActiveId` reflects the last click. ✓ No
  new race introduced.
- **Initial-mount race:** App.tsx awaits `SessionAPI.list()` (line 129)
  before `SessionAPI.getActive()` (line 132). By the time `setActiveId`
  fires, sessions are loaded. ✓
- **`activeId === undefined` collision with `session()?.id === undefined`:**
  Verified `SessionsState.activeId` is typed `string | null`, never
  undefined. The expression `undefined === null` is false → safe. ✓
- **Status dot stuck on stale active row:** When `setActiveId(newId)`
  fires, line 295 reverts the previously-active session's status from
  "active" to "running". Dot CSS class follows. ✓ R9 is correct.
- **Coord-quick-access source-order edge case for hover-on-active:**
  Plan §4.5 inserts the new active rule AFTER the existing :hover rule
  on the neutralizer. Equal specificity (0,5,0). Active wins on source
  order. ✓
- **noir-minimal :hover overriding :active border-left-color:** Yes,
  noir-minimal `.replica-item:hover` (0,3,0) at line 2725 sets
  `border-left-color: var(--sidebar-accent)`. When user hovers an
  active row, hover's color wins (but it's the same color as active
  anyway). When NOT hovered, only the (0,2,0) noir base wins (G1
  blocker). The fix in G1 (adding (0,3,0) per-theme active) makes
  hover and active have equal specificity AND equal color, so source
  order is irrelevant. Either wins, both produce the same visual. ✓
- **deep-space active coord `border-color: rgba(120, 180, 255, 0.55)` is a 4-side shorthand:** Yes; correctly overrides the (0,4,0) coord beacon's `border: 1px solid amber`. ✓ All four sides retain 1px solid, with new blue color. The 4-side blue tint reads as "active" on the beacon shape.

<!-- /dev-rust-grinch round-1 -->

---

<!-- dev-rust-grinch round-2 -->
## Grinch Review (round 2)

**Verdict: APPROVE** (with 3 minor follow-up nits — none blocking).

I independently re-verified all round-2 changes against the current
codebase. Every numerical correction, source-order assertion, and
transition-list edit is correct or operates as the plan claims. The
G1 blocker fix is sound. The G2 specificity table now matches my
independent recompute. The G4 box-shadow neutralization and source-order
guarantee are valid as written.

The plan is implementable as-is. The remaining nits (G13–G15 below) are
minor visual-consistency issues that the architect can address now (3-line
total CSS change) or punt to a post-merge follow-up. They do not block
implementation correctness.

### Round-2 verifications (PASS)

#### G1 fix (§4.3.6) — VERIFIED CORRECT
- Insertion location after `sidebar.css:2727` is the right spot. Lines 2728+ are unrelated (`.session-item`, `.coord-quick-access`, `.root-agent-banner`) — no risk of re-override.
- New `[data-sidebar-style="noir-minimal"] .replica-item.active` at (0,3,0) beats both noir-minimal `.replica-item` (0,2,0) and noir-minimal `.replica-item:hover` (0,3,0) via source order.
- Hover-active interaction trace:
  - Plain hover (no active): (0,3,0) hover wins. `border-left-color: var(--sidebar-accent)`. Base `.replica-item:hover` (0,1,0) `background: var(--sidebar-hover)` paints. ✓
  - Plain active (no hover): (0,3,0) new active wins. `background: var(--sidebar-active)`, `border-left-color: var(--sidebar-accent)`. ✓
  - Hover + active: equal (0,3,0). Active later in source → wins. Both rules set the same `border-left-color: var(--sidebar-accent)`, so visually identical regardless. Active sets bg=`var(--sidebar-active)`; hover doesn't set bg, so base hover (0,1,0) loses to active. Final: bg=active, border-left-color=accent. ✓
- Width preserved at 2px (set by noir-minimal's (0,2,0) shorthand at line 2721 since the (0,3,0) active rule only touches `border-left-color`, not the full shorthand). ✓

#### G2 specificity table (§4.4) — VERIFIED CORRECT
Independently recomputed every entry. All 6 corrections match my counts:

| Selector | Plan claims | Verified |
|---|---|---|
| `[data-sidebar-style="noir-minimal"] .replica-item.active` | (0,3,0) | ✓ |
| `html.light-theme[data-sidebar-style="deep-space"] .replica-item:has(.coord)` | (0,5,1) | ✓ |
| `html.light-theme[data-sidebar-style="deep-space"] .replica-item.active:has(.coord)` | (0,6,1) | ✓ |
| `html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item:has(.coord)` | (0,5,1) | ✓ |
| `html.light-theme[data-sidebar-style="obsidian-mesh"] .replica-item.active:has(.coord)` | (0,6,1) | ✓ |
| `.coord-quick-access .replica-item:has(.coord)` | (0,4,0) | ✓ |
| `.coord-quick-access .replica-item.active:has(.coord)` | (0,5,0) | ✓ |
| Combined `html.light-theme[data-sidebar-style="X"] .coord-quick-access .replica-item:has(.coord)` | (0,6,1) | ✓ |
| Combined active variant of above | (0,7,1) | ✓ |

Every other entry in §4.4 also independently verified.

#### G4 source-order — VERIFIED CORRECT (with one strengthening suggestion)
- §4.3.1 inserts deep-space active rules after `sidebar.css:3050`.
- §4.5 inserts coord-quick-access dark active rule after `sidebar.css:3778`.
- §4.5 inserts coord-quick-access light active rule after `sidebar.css:3797`.
- Original line numbers: 3050 < 3778 < 3797. Coord-quick-access rules will physically be placed AFTER the deep-space active block in the resulting file regardless of insertion order chosen by the implementer (line anchors point to original-file positions). Source order is naturally enforced. ✓
- The architect documented the constraint inline in §4.5's round-2 explanation block ("the new coord-quick-access rule is placed AFTER §4.3.1's deep-space active rule in `sidebar.css`"). Adequate.
- **Strengthening suggestion (not a blocker):** Add a one-line callout in §4.5 above the `box-shadow: none` rules: `/* INSERT-ORDER NOTE: this block must come AFTER §4.3.1 in sidebar.css for box-shadow:none to win source-order tiebreak over the deep-space active coord (0,5,0) glow. */`. Makes the constraint immune to a future architect re-organizing the plan.

#### G6 transition-list edits — VERIFIED, but with two partial-fix gaps (see G13/G14 below)
- No double-tween: each rule's `transition` declaration is a complete shorthand assignment. The cascade picks ONE rule per element (per specificity). No element receives two transition declarations simultaneously. ✓
- The added `opacity 150ms` on obsidian-mesh line 3433 (0,2,0) does NOT conflict with line 3519's existing `opacity 150ms` (0,4,0). Workers get line 3519's transition (which still includes opacity); coords get line 3433's transition (which now also has opacity declared, though no rule actually changes coord opacity, so it's a no-op declaration). No animation race. ✓
- Deep-space line 3010, neon-circuit line 3702 transition extensions verified. The `border-left-color X` longhand correctly applies to the cascaded `border-left-color`. ✓

#### G3 R7 narrative correction — VERIFIED CORRECT
The corrected narrative now accurately states the dot color flips
instantly. Cross-checked at `sidebar.css:498–503` — no `transition`
property declared on `.session-item-status`. ✓

#### G5 R1 enumeration — VERIFIED CORRECT
- noir-minimal correctly removed from "shifts" list (already 2px transparent at line 2721).
- deep-space coord rows correctly noted as not shifting (1px solid border from line 3072 wins (0,4,0) > base (0,1,0)).
- Project-level offline-fallback at `ProjectPanel.tsx:853–865` correctly enumerated.
✓

### Remaining minor issues (3 total — all G6-related, none blocking)

#### G13 — Obsidian-mesh worker active rows have INSTANT border-left-color flip while coord rows animate

<!-- architect round-3 -->
**Status: addressed in round 3** (Path A per tech-lead). The proposed
edit was applied in §4.3.4's round-3 block — line 3519's transition now
reads `transition: opacity 150ms, background 150ms, border-left-color 150ms;`.
Both coord and worker active rows in obsidian-mesh now animate
`border-left-color` consistently over 150ms.
<!-- /architect round-3 -->

- **What:** The architect's round-2 fix added `border-left-color 150ms` to `[data-sidebar-style="obsidian-mesh"] .replica-item` at line 3433 (specificity (0,2,0)). But the **worker** rule at line 3519 — `[data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.coord))` — has its own `transition: opacity 150ms, background 150ms` at higher specificity (0,4,0), which OVERRIDES the line 3433 transition for non-coord rows.
- **Result:** Obsidian-mesh COORD active rows animate border-left-color over 150ms (line 3433 wins). Obsidian-mesh WORKER active rows flip border-left-color INSTANTLY (line 3519 wins, no border-left-color in its transition list).
- **Why it matters:** Internal inconsistency within obsidian-mesh — coord activate animates smoothly, worker activate snaps. Probably barely perceptible (workers shift opacity 0.75 → 1 over 150ms simultaneously, which dominates the visual), but technically a regression vs round-1's claim "G6 fully addressed."
- **Fix:** Append `, border-left-color 150ms` to line 3519's transition list:
  ```css
  [data-sidebar-style="obsidian-mesh"] .replica-item:not(:has(.ac-discovery-badge.coord)) {
    padding: 2px 8px 2px 14px;
    opacity: 0.75;
    transition: opacity 150ms, background 150ms, border-left-color 150ms;
  }
  ```
- **Severity:** Minor. Easy 3-word edit. Architect may include now or punt.

#### G14 — Deep-space active coord `border-color` 4-side shorthand: only border-LEFT-color animates; top/right/bottom flip instantly

<!-- architect round-3 -->
**Status: addressed in round 3** (Path A per tech-lead, fix option (a)).
The proposed edit was applied in §4.3.1's round-3 block — line 3010's
transition now reads `transition: background 200ms, box-shadow 200ms,
border-color 200ms;`. All four sides of the deep-space coord beacon now
tween in lockstep at 200ms when activated.
<!-- /architect round-3 -->

- **What:** §4.3.1's active coord rule sets `border-color: rgba(120, 180, 255, 0.55)` — a 4-side color shorthand that changes all four border colors. The round-2 transition update at line 3010 only adds `border-left-color 200ms`, NOT `border-color 200ms` (or `border-top-color`, `border-right-color`, `border-bottom-color`).
- **Result:** When a deep-space coord row activates, three of its four 1px borders snap to blue instantly while the LEFT border tweens from amber to blue over 200ms. Subtle visual mismatch.
- **Why it matters:** The coord beacon is small (1px borders) so the asymmetric tween is hard to notice. Pre-round-2, all 4 sides flipped instantly (consistent). Post-round-2, the asymmetry is introduced as a side effect of fixing G6 for the bg+box-shadow case.
- **Fix options:**
  - (a) Change line 3010 transition to `transition: background 200ms, box-shadow 200ms, border-color 200ms` — animates all 4 sides over 200ms. Most consistent.
  - (b) Accept the asymmetry; document in R3 (theme-specific coord beacon visual continuity) that the tween is left-only.
- **Severity:** Minor. Recommend (a) — it's the same number of characters and produces a cleaner visual.

#### G15 — `.session-item.active` border-left-color still flips instantly while `.replica-item.active` will animate

<!-- architect round-3 -->
**Status: out of scope, per tech-lead.** Already documented in §4.1
round-2 as "the first rule in the codebase to animate `border-left-color`;
`.session-item` does not." Tech-lead's round-3 instruction confirms the
asymmetry is acknowledged as an acceptable trade-off — fixing it would
require modifying `.session-item.active` which is outside the scope of
issue #97. Track separately if the asymmetry becomes user-visible enough
to report.
<!-- /architect round-3 -->

- **What:** This is documented in the architect's §4.1 round-2 correction: `.session-item` line 407 doesn't include `border-left-color` in its transition. So `.session-item.active` flips border color instantly today. The new `.replica-item.active` will animate border-left-color over 150ms (in default theme + arctic-ops + noir-minimal + deep-space + obsidian-mesh + neon-circuit per the round-2 transition extensions).
- **Asymmetry:** A user clicking a `.session-item` row (e.g., in the project-level Agents matrix where live agents render via `<SessionItem>`) sees the border snap; clicking a `.replica-item` row in a workgroup sees a smooth fade. Inconsistent UX.
- **Fix options:**
  - (a) Out-of-scope; live with the asymmetry.
  - (b) Extend `.session-item` line 407's transition list. Would touch `.session-item.active` behavior — out of scope per the plan's stated boundaries.
- **Severity:** Minor. The architect explicitly documents this in §4.1 round-2 ("the base `.replica-item` rule is the **first** rule in the codebase to animate `border-left-color` on a row"). Acceptable trade-off.

### Items considered and dismissed

- **`.replica-item-name` color contrast in light obsidian-mesh active state:** existing `(0,6,1)` rule at sidebar.css:3511 sets `color: rgba(160, 70, 0, 0.9)`. New (0,6,1) light obsidian-mesh active coord rule sets bg `rgba(220, 130, 30, 0.18)`. Blended bg ≈ #f0e0d0; text ≈ #a85515. Lightness ratio ~3:1 — passes WCAG AA Large. Acceptable. ✓
- **Light-mode deep-space active coord row in workgroup section keeps the dark `box-shadow` (inset blue glow):** The (0,5,0) dark deep-space active coord rule sets `box-shadow`; the (0,6,1) light variant does NOT override box-shadow. So in light mode + workgroup section, the inset blue glow appears on active deep-space coord rows. Verified by cascade trace. **Probably intentional and consistent with deep-space's "beacon recolored" visual language**, but worth a visual eye-test during implementation. Not a bug — just a visual nuance the test plan should verify.
- **Implementer applies multi-step inserts in the wrong order causing line-anchor drift:** The plan's line numbers are pre-edit references. Standard plan convention; dev-webpage-ui handles this. Source order in the resulting file is determined by ORIGINAL line numbers, so even if the implementer applies inserts in different orders, the final structure is consistent. ✓
- **Hover+active stack on neon-circuit coord rows:** neon-circuit `.replica-item:hover` (line 3705, (0,3,0)) sets `box-shadow: inset 2px 0 0 rgba(255, 0, 180, 0.3)`. The new `.replica-item.active` for neon-circuit at (0,3,0) doesn't touch box-shadow. Equal specificity, source order: hover at 3705, active at 3748+. Active later → wins for the props it sets (bg, border-left-color). Hover's box-shadow inset is not overridden by active → both visible simultaneously when hover+active. The 3px accent border-left + the 2px inset hover shadow on the LEFT edge: visually overlapping but slightly different colors (border-left = `rgba(255, 0, 180, 0.5)`, box-shadow = `rgba(255, 0, 180, 0.3)`). Probably reads as "intensified left edge" — acceptable. ✓ (Same analysis as round-1; architect did not change this and it's out of scope.)
- **Coord-quick-access trade-off boundary (per round-2 §4.5 documentation):** "Inside `.coord-quick-access`, an active row is fully neutralized to default accent — no theme-specific glow, gradient, beacon styling, or other ornamental treatment." Verified via cascade trace for deep-space and obsidian-mesh. ✓
- **Round-2 changelog claim "G6 fully addressed":** technically off — see G13. The base claim is correct for COORD rows in obsidian-mesh; only WORKER rows are missed.
- **Source-order strengthening for §4.5:** documented but only inline in the round-2 explanation block. Consider adding a CSS comment in the actual rule (suggestion only, not a blocker).

### Summary

**1 blocker fixed (G1).** All 5 important issues addressed (G2–G6). All 6 minor issues handled (G7–G12). Round 2 successfully resolves every round-1 finding.

**3 new minor nits surfaced from the round-2 changes themselves (G13–G15)**, all in the G6 category. None blocking. The architect can address them in a 3-line follow-up edit or punt to post-merge polish.

The plan is **ready to implement** as written.

<!-- /dev-rust-grinch round-2 -->
