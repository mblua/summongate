# Plan: Per-Project Coding Agents Configuration

**Branch:** `feature/per-project-coding-agents`
**Status:** Draft
**Created:** 2026-04-09

---

## Problem Statement

Coding Agents (Claude Code, Codex, Gemini CLI, custom) are currently configured globally in `settings.json` and apply to ALL projects. Users need the ability to override which agents are available on a per-project basis — e.g., Project A uses only Claude Code while Project B uses Claude Code + Codex.

## Solution Overview

1. **Per-project settings file** stored at `<project>/.ac-new/project-settings.json`
2. **New context menu option** "Coding Agents" in ProjectPanel's right-click menu
3. **Modal UI** reusing the existing Coding Agents tab pattern from SettingsModal
4. **Visual badge** on project header when custom agents are configured
5. **Resolution logic**: project-level agents **fully replace** global agents (no merge)

---

## 1. Data Model Changes

### 1.1 New File: `<project>/.ac-new/project-settings.json`

```json
{
  "agents": [
    {
      "id": "agent_1712678400000_0",
      "label": "Claude Code",
      "command": "claude",
      "color": "#d97706",
      "gitPullBefore": false,
      "excludeGlobalClaudeMd": true
    }
  ]
}
```

**Design decisions:**
- Lives inside `.ac-new/` (already exists for every AC project, already gitignored via `wg-*/` pattern)
- Uses the same `AgentConfig` schema as global settings — identical fields
- File is optional: absence means "use global agents"
- Empty `agents: []` means "no agents available for this project" (valid state, distinct from absent file)
- camelCase keys (matching existing `config.json` pattern in `.ac-new/`)

### 1.2 Rust Struct: `ProjectSettings`

**File:** `src-tauri/src/config/project_settings.rs` (new)

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
}
```

Reuses existing `AgentConfig` from `settings.rs`. No new types needed.

> **[DEV-RUST] Serde note:** `AgentConfig` in `settings.rs` already has `#[serde(rename_all = "camelCase")]` and `#[serde(default)]` on bool fields (`git_pull_before`, `exclude_global_claude_md`). This means project-settings.json will serialize/deserialize correctly with camelCase keys matching the frontend. The `#[serde(default)]` on `agents: Vec<AgentConfig>` ensures that a JSON file with `{}` (no `agents` key) deserializes to an empty vec — important for forward-compatibility if we add more fields to `ProjectSettings` later.

### 1.3 Frontend Type: `ProjectSettings`

**File:** `src/shared/types.ts` — add:

```typescript
export interface ProjectSettings {
  agents: AgentConfig[];
}
```

### 1.4 Extend `ProjectState` (frontend store)

**File:** `src/sidebar/stores/project.ts` — add field:

```typescript
interface ProjectState {
  path: string;
  folderName: string;
  workgroups: AcWorkgroup[];
  agents: AcAgentMatrix[];
  teams: AcTeam[];
  projectSettings: ProjectSettings | null;  // NEW — null = no override, use global
}
```

---

## 2. Backend Changes (Rust)

### 2.1 New Module: `src-tauri/src/config/project_settings.rs`

Functions:
- `project_settings_path(project_path: &str) -> PathBuf` — returns `<project>/.ac-new/project-settings.json`
- `load_project_settings(project_path: &str) -> Option<ProjectSettings>` — reads and parses; returns `None` if file missing or invalid
- `save_project_settings(project_path: &str, settings: &ProjectSettings) -> Result<(), String>` — writes JSON (pretty-printed)
- `delete_project_settings(project_path: &str) -> Result<(), String>` — removes the file (revert to global)

Register module in `src-tauri/src/config/mod.rs`.

> **[DEV-RUST] File I/O details and edge cases:**
>
> 1. **Path validation:** `project_path` originates from the frontend (user-controlled). Before constructing `.ac-new/project-settings.json`, validate that the project path is a real existing directory AND that `.ac-new/` exists inside it. This prevents the backend from creating arbitrary directories on the filesystem if the frontend sends a malicious or garbled path. Pattern:
>    ```rust
>    fn validated_settings_path(project_path: &str) -> Result<PathBuf, String> {
>        let base = Path::new(project_path);
>        if !base.is_dir() {
>            return Err(format!("Project path is not a directory: {}", project_path));
>        }
>        let ac_dir = base.join(".ac-new");
>        if !ac_dir.is_dir() {
>            return Err(format!("Not an AC project (no .ac-new/): {}", project_path));
>        }
>        Ok(ac_dir.join("project-settings.json"))
>    }
>    ```
>    Use this in all three functions instead of raw `project_settings_path()`.
>
> 2. **`load_project_settings` graceful fallback:** Must return `None` on *any* read/parse failure (not just missing file). This matches test case E7 and the pattern in `settings.rs:load_settings()` which returns `AppSettings::default()` on parse errors. Use:
>    ```rust
>    pub fn load_project_settings(project_path: &str) -> Option<ProjectSettings> {
>        let path = validated_settings_path(project_path).ok()?;
>        if !path.exists() { return None; }
>        let content = std::fs::read_to_string(&path).ok()?;
>        serde_json::from_str(&content).ok()
>    }
>    ```
>    Log a warning on parse failure (`serde_json::from_str` returns `Err`) so corrupted files are visible in logs but don't crash the UI.
>
> 3. **`delete_project_settings` idempotent:** `std::fs::remove_file` returns `Err` if the file doesn't exist. Handle `ErrorKind::NotFound` as success (idempotent delete). The frontend may call delete on a project that never had custom settings.
>
> 4. **`save_project_settings` uses `serde_json::to_string_pretty`:** Consistent with `save_settings()` in `settings.rs:244`. The `.ac-new/` directory already exists (validated above), so no `create_dir_all` needed — but keeping it as a safety net is fine.
>
> 5. **Encoding:** `serde_json` produces and expects UTF-8. `fs::write`/`fs::read_to_string` handle UTF-8 natively on all platforms. No BOM issues on Windows since we're writing JSON, not TOML.

### 2.2 New Tauri Commands: `src-tauri/src/commands/project_settings.rs`

```rust
#[tauri::command]
pub async fn get_project_settings(project_path: String) -> Result<Option<ProjectSettings>, String>
// Reads project-settings.json. Returns None if file doesn't exist.

#[tauri::command]
pub async fn update_project_settings(project_path: String, settings: ProjectSettings) -> Result<(), String>
// Writes project-settings.json. Creates .ac-new/ if needed.

#[tauri::command]
pub async fn delete_project_settings(project_path: String) -> Result<(), String>
// Deletes project-settings.json. Reverts project to global agents.

#[tauri::command]
pub async fn resolve_agents_for_project(
    project_path: String,
    settings: State<'_, SettingsState>,
) -> Result<Vec<AgentConfig>, String>
// Resolution logic:
//   1. Try load project-settings.json
//   2. If exists and has agents array → return those
//   3. Otherwise → return global settings.agents
```

Register commands in `lib.rs` invoke handler (NOT `main.rs`).

> **[DEV-RUST] Command registration specifics:**
>
> 1. **Registration is in `lib.rs`, not `main.rs`:** The `generate_handler![]` macro is in `src-tauri/src/lib.rs:570`. The plan says `main.rs` but the actual registration point is `lib.rs`. Add the 4 new commands after the existing `commands::entity_creation::*` block (line ~627).
>
> 2. **No Tauri capability/permission files exist:** The plan's section 5 mentions updating `src-tauri/capabilities/*.json` — those files don't exist in this project. Tauri 2's ACL is not configured; all commands registered in `generate_handler![]` are implicitly callable. So step A6 in the plan only needs `cargo check`, no permission files to update. **Remove the capabilities row from the "Tauri Permissions" table in section 5 to avoid confusion.**
>
> 3. **`resolve_agents_for_project` needs `State<'_, SettingsState>`:** This is already managed via `.manage()` in `lib.rs:241`. No new state registration needed — just add the `State<>` param to the command signature. Pattern matches `get_settings` in `commands/config.rs:24`.
>
> 4. **Error pattern:** All existing commands return `Result<T, String>` with `.map_err(|e| format!(...))`. Stay consistent — no `thiserror` in the commands layer even though CLAUDE.md mentions it for internal code. The commands are the boundary layer and Tauri requires `String` errors.
>
> 5. **Commands module registration:** Add `pub mod project_settings;` to `src-tauri/src/commands/mod.rs` (currently has: ac_discovery, agent_creator, config, entity_creation, phone, pty, repos, session, telegram, voice, window).

### 2.3 Extend Discovery (optional enhancement)

In `ac_discovery.rs`, the `AcDiscoveryResult` could include a `has_project_settings: bool` flag per project. However, since the frontend already loads project settings separately, this is **optional** and can be deferred. The badge can be driven by the frontend store instead.

> **[DEV-RUST] Recommendation: defer discovery integration.** Adding `has_project_settings` to `AcDiscoveryResult` would require modifying the `discover_project` command and its return type, which ripples into the frontend discovery store. Since the frontend can check project settings with a separate `get_project_settings` call (already planned), this adds complexity for no functional gain. The badge visibility can be derived from the `projectSettings` field in the frontend store. Defer to Phase E or beyond.

---

## 3. Frontend Changes

### 3.1 New IPC Wrappers: `src/shared/ipc.ts`

Add to existing API exports:

```typescript
export const ProjectSettingsAPI = {
  get: (projectPath: string) => 
    transport.invoke<ProjectSettings | null>("get_project_settings", { projectPath }),
  update: (projectPath: string, settings: ProjectSettings) => 
    transport.invoke<void>("update_project_settings", { projectPath, settings }),
  delete: (projectPath: string) => 
    transport.invoke<void>("delete_project_settings", { projectPath }),
  resolveAgents: (projectPath: string) => 
    transport.invoke<AgentConfig[]>("resolve_agents_for_project", { projectPath }),
};
```

### 3.2 Update Project Store: `src/sidebar/stores/project.ts`

- On `loadProject(path)` and `reloadProject(path)`, also call `ProjectSettingsAPI.get(path)` and store result in `projectSettings` field
- Add helper: `hasCustomAgents(path: string): boolean` — checks if `projectSettings !== null`
- Add helper: `getProjectSettings(path: string): ProjectSettings | null`

> **[DEV-WEBPAGE-UI] Store reactivity implementation details:**
>
> The project store uses `createSignal<ProjectState[]>` (not `createStore`). Every mutation replaces the array element via `setProjects()`. The existing pattern is in `reloadProject()` (project.ts:117-121).
>
> **Fetch sequencing:** `ProjectSettingsAPI.get()` and `ProjectAPI.discover()` are independent I/O — run them in parallel via `Promise.all` to avoid sequential latency:
> ```typescript
> // In loadProject:
> const [result, projectSettings] = await Promise.all([
>   ProjectAPI.discover(path),
>   ProjectSettingsAPI.get(path),
> ]);
> setProjects((prev) => [
>   ...prev,
>   { path, folderName, workgroups: result.workgroups, agents: result.agents, teams: result.teams, projectSettings },
> ]);
> ```
>
> ```typescript
> // In reloadProject — same parallel pattern, spread projectSettings into the update:
> const [result, projectSettings] = await Promise.all([
>   ProjectAPI.discover(path),
>   ProjectSettingsAPI.get(path),
> ]);
> setProjects((prev) =>
>   prev.map((p) =>
>     normalizePath(p.path) === normalized
>       ? { ...p, workgroups: result.workgroups, agents: result.agents, teams: result.teams, projectSettings }
>       : p
>   )
> );
> ```
>
> **Helper accessors** — add to the `projectStore` object as derived reads (NOT new signals). They read from `projects()` which auto-tracks in any reactive context (JSX, `createMemo`, `createEffect`):
> ```typescript
> hasCustomAgents(path: string): boolean {
>   const norm = normalizePath(path);
>   return projects().some((p) => normalizePath(p.path) === norm && p.projectSettings != null);
> },
> getResolvedAgents(path: string): AgentConfig[] | null {
>   const norm = normalizePath(path);
>   const proj = projects().find((p) => normalizePath(p.path) === norm);
>   return proj?.projectSettings?.agents ?? null;  // null = use global
> },
> ```

### 3.3 New Component: `ProjectAgentsModal.tsx`

**File:** `src/sidebar/components/ProjectAgentsModal.tsx`

A modal dialog that reuses the Coding Agents UI pattern from `SettingsModal.tsx` (lines ~319-448). Key differences from the global settings modal:

**Props:**
```typescript
{
  projectPath: string;
  projectName: string;
  initialSettings: ProjectSettings | null;
  onClose: () => void;
  onSaved: () => void;  // triggers project reload
}
```

**UI Structure:**
```
Modal Overlay
  Modal Container
    Header: "Coding Agents — {projectName}"
    
    Toggle: "Use custom agents for this project" (checkbox/switch)
      - OFF (default when no project-settings.json): shows message "Using global agents"
      - ON: shows agent editor (same as SettingsModal Coding Agents tab)
    
    When ON:
      [Copy from Global] button — copies current global agents as starting point
      
      Agent list (same card UI as SettingsModal):
        For each agent:
          - Label input
          - Command input  
          - Color picker + hex
          - gitPullBefore checkbox
          - excludeGlobalClaudeMd checkbox
          - Remove button
      
      Add buttons:
        - Preset buttons (Claude Code, Codex, Gemini CLI) — from AGENT_PRESETS
        - Custom Agent button
    
    Footer:
      [Cancel] [Save]
      If toggle ON → save calls ProjectSettingsAPI.update()
      If toggle OFF → save calls ProjectSettingsAPI.delete() (revert to global)
```

**Key behaviors:**
- "Copy from Global" loads `SettingsAPI.get().agents` into the local editor — one-time copy, not a link
- Reuses `AGENT_PRESETS`, `AGENT_PRESET_MAP`, `newAgentId()` from `src/shared/agent-presets.ts`
- Same validation logic as SettingsModal (check for `--continue` / `-c` flags in Claude commands)
- On save, calls `onSaved()` which triggers `projectStore.reloadProject(path)`

> **[DEV-WEBPAGE-UI] ProjectAgentsModal — detailed implementation guidance:**
>
> **1. State management — use `createStore`, not `createSignal`:**
> SettingsModal uses `createStore<{ data: AppSettings | null }>` for agent editing (SettingsModal.tsx:42). This enables granular path-based mutations like `setSettings("data", "agents", index, "label", value)` without recreating the entire object. ProjectAgentsModal must follow the same pattern:
> ```typescript
> const [localAgents, setLocalAgents] = createStore<{ list: AgentConfig[] }>({ list: [] });
> const [customEnabled, setCustomEnabled] = createSignal(props.initialSettings !== null);
> const [saving, setSaving] = createSignal(false);
> const [saveError, setSaveError] = createSignal<string | null>(null);
> ```
> Use `createStore` for the agents list (allows `setLocalAgents("list", index, "label", value)`), and `createSignal` for scalar UI state (toggle, saving, error). This matches the SettingsModal pattern exactly.
>
> **2. Initialization from `initialSettings` prop:**
> On mount, if `props.initialSettings` is non-null, deep-clone its agents into the store (with new references so edits don't mutate the prop):
> ```typescript
> onMount(() => {
>   if (props.initialSettings) {
>     setLocalAgents("list", props.initialSettings.agents.map(a => ({ ...a })));
>   }
> });
> ```
> Do NOT use `createEffect` to watch `props.initialSettings` — the modal opens with a snapshot and edits locally. Reactivity to prop changes would discard user edits if the store reloads.
>
> **3. "Copy from Global" must generate fresh IDs:**
> When copying global agents, generate new IDs to prevent any theoretical collision in log/audit trails:
> ```typescript
> const handleCopyFromGlobal = async () => {
>   const settings = await SettingsAPI.get();
>   const copied = settings.agents.map(a => ({ ...a, id: newAgentId() }));
>   setLocalAgents("list", copied);
> };
> ```
>
> **4. Validation — replicate `validateAgents()` from SettingsModal (lines 141-153):**
> Must check for `--continue` / `-c` flags in Claude commands. Extract this logic into a shared utility in `src/shared/agent-presets.ts` or duplicate it. Since SettingsModal already has it inline, duplicating is acceptable for now:
> ```typescript
> const validateAgents = (): string | null => {
>   for (const agent of localAgents.list) {
>     const cmd = agent.command.toLowerCase();
>     if (cmd.includes("claude")) {
>       const flags = cmd.split(/\s+/);
>       if (flags.includes("--continue") || flags.includes("-c")) {
>         return `Agent "${agent.label || "Unnamed"}": Claude commands must not include --continue or -c`;
>       }
>     }
>   }
>   return null;
> };
> ```
>
> **5. Save flow with toggle-aware logic:**
> ```typescript
> const handleSave = async () => {
>   if (customEnabled()) {
>     const err = validateAgents();
>     if (err) { setSaveError(err); return; }
>     setSaving(true);
>     try {
>       await ProjectSettingsAPI.update(props.projectPath, { agents: localAgents.list });
>       props.onSaved();
>     } catch (e) { setSaveError(String(e)); }
>     finally { setSaving(false); }
>   } else {
>     // Toggle OFF → delete project settings file
>     setSaving(true);
>     try {
>       await ProjectSettingsAPI.delete(props.projectPath);
>       props.onSaved();
>     } catch (e) { setSaveError(String(e)); }
>     finally { setSaving(false); }
>   }
> };
> ```
>
> **6. Modal structure — reuse existing CSS classes, NOT new ones:**
> The modal should use the existing `modal-overlay`, `modal-container`, `modal-header`, `modal-title`, `modal-close`, `modal-body`, `modal-footer` classes from sidebar.css (lines 790-850). No new container styles needed. The content section reuses `settings-section`, `settings-button-card`, `settings-field`, `settings-input`, etc. from the SettingsModal styles.
>
> **7. Portal usage — the CALLER wraps in Portal, NOT the modal itself:**
> Consistent with ProjectPanel's pattern (lines 397-421): the modal renders its overlay/container directly. The caller in ProjectPanel.tsx wraps it in `<Portal>`. The modal component should NOT import or use Portal internally.
>
> **8. UX states to handle:**
> - **Loading**: Brief — show disabled Save button while `saving()` is true. The initial data comes from `props.initialSettings` (already loaded), so no initial loading spinner needed.
> - **Error on save**: Display `saveError()` in the footer area, same as SettingsModal (line ~668): `<Show when={saveError()}><span class="modal-save-error">{saveError()}</span></Show>`
> - **Empty agents list with toggle ON**: Show the preset buttons and "Custom Agent" button (same as SettingsModal when no agents exist). The `<For each={localAgents.list}>` will render nothing, and the action buttons below it let the user add agents.
> - **Toggle OFF state**: Show a clear message: "Using global agents" with a subtle info style. Consider showing the global agent count: "Using global agents (3 configured)".

### 3.4 Update ProjectPanel: Context Menu + Badge

**File:** `src/sidebar/components/ProjectPanel.tsx`

#### 3.4.1 New Signal

```typescript
const [showProjectAgents, setShowProjectAgents] = createSignal(false);
```

#### 3.4.2 Context Menu — Add "Coding Agents" Option

Insert before the separator (between "New Workgroup" and the separator):

```tsx
<div class="context-separator" />
<button
  class="session-context-option"
  onClick={() => { setShowCtxMenu(false); setShowProjectAgents(true); }}
>
  Coding Agents
</button>
```

Position: After "New Workgroup", before the existing separator + "Remove Project". This groups creation actions together and puts configuration in its own section.

#### 3.4.3 Modal Render

Below the existing modals (after NewWorkgroupModal), add:

```tsx
{showProjectAgents() && (
  <Portal>
    <ProjectAgentsModal
      projectPath={proj.path}
      projectName={proj.folderName}
      initialSettings={proj.projectSettings}
      onClose={() => setShowProjectAgents(false)}
      onSaved={() => {
        setShowProjectAgents(false);
        projectStore.reloadProject(proj.path);
      }}
    />
  </Portal>
)}
```

#### 3.4.4 Badge on Project Header

In the project header button (line ~342-351), add a badge after the title:

```tsx
<button class="project-header" ...>
  <span class="ac-discovery-chevron" ...>▾</span>
  <span class="project-title">Project: {proj.folderName}</span>
  {proj.projectSettings && (
    <span class="project-custom-agents-badge" title="Custom Coding Agents configured">
      ⚙ Custom Agents
    </span>
  )}
</button>
```

**CSS for badge** (add to project panel styles):
```css
.project-custom-agents-badge {
  font-size: 0.65em;
  padding: 1px 6px;
  border-radius: 3px;
  background: rgba(255, 255, 255, 0.08);
  color: var(--text-secondary, rgba(255, 255, 255, 0.5));
  margin-left: 8px;
  white-space: nowrap;
  letter-spacing: 0.02em;
}
```

Subtle, non-intrusive — matches the industrial-dark aesthetic. No bright colors; uses opacity for hierarchy.

> **[DEV-WEBPAGE-UI] Badge and context menu implementation details:**
>
> **Badge reactivity:** The `proj.projectSettings` access in JSX is reactive because `proj` comes from `projectStore.projects` (a signal). When `reloadProject()` updates `projectSettings`, the badge `<Show>` will auto-toggle. However, the plan uses `{proj.projectSettings && (...)}` — this should be `<Show when={proj.projectSettings}>` for consistent SolidJS idiom. The `&&` pattern works but `<Show>` is the established pattern in this codebase for conditional rendering with signals.
>
> **Badge CSS adjustment:** The proposed badge uses `font-size: 0.65em` which is relative. Since the parent `.project-header` font size varies, prefer an absolute `font-size: 9px` to match other small badges in the codebase (e.g., `.titlebar-dev-badge` at 8px, `.status-bar` at 10px). Also add `vertical-align: middle` for alignment within the flex parent, and use `pointer-events: none` so the badge doesn't interfere with the header button's click/context-menu handlers.
>
> **Context menu position:** The plan places "Coding Agents" after "New Workgroup" with a separator above it. This is correct. However, the separator creates a visual section: creation actions (New Agent, New Team, New Workgroup) | configuration (Coding Agents) | destructive (Remove Project). This grouping is clear and follows the existing pattern in team/workgroup context menus where destructive actions are separated.
>
> **Context menu dismiss:** The `showProjectAgents` signal must be set to `false` when the context menu dismisses. The plan already does this: `onClick={() => { setShowCtxMenu(false); setShowProjectAgents(true); }}`. But also ensure `setShowProjectAgents(false)` is called in the `cleanupCtx` function if the user dismisses the context menu without clicking (Escape key, click-away). Since the modal opens via a separate signal, this is already handled — dismissing the context menu doesn't affect the modal signal.

### 3.5 Update Agent Resolution Points

These components currently read agents from `SettingsAPI.get().agents` (global). They need to use the resolution logic instead:

#### 3.5.1 `AgentPickerModal.tsx` (line 23)

This modal is shown when picking an agent for a session. It needs to know which project the session belongs to.

**Change:** Accept optional `projectPath` prop. On mount:
```typescript
onMount(async () => {
  if (props.projectPath) {
    const resolved = await ProjectSettingsAPI.resolveAgents(props.projectPath);
    setAgents(resolved);
  } else {
    const settings = await SettingsAPI.get();
    setAgents(settings.agents);
  }
});
```

The caller must pass `projectPath` when the session/agent belongs to a project. Check all call sites and thread the project path through.

#### 3.5.2 `NewAgentModal.tsx` (line 39)

Same pattern — accept optional `projectPath`, resolve agents accordingly.

#### 3.5.3 `OpenAgentModal.tsx` (line 20)

Same pattern.

#### 3.5.4 `SessionItem.tsx` (line 143)

Currently checks `settingsStore.current?.agents` for agent availability. Should check resolved agents for the session's project instead. This may require the session to know which project it belongs to (trace the session → project relationship).

> **[DEV-WEBPAGE-UI] Resolution points — detailed threading analysis and missing items:**
>
> ### 3.5.1 AgentPickerModal — `projectPath` threading is straightforward
>
> The single call site is ProjectPanel.tsx line 1232. The `proj` object is in scope (the `<For each={projectStore.projects}>` iterator). Thread `proj.path`:
> ```tsx
> <AgentPickerModal
>   sessionName={pendingLaunch()!.sessionName}
>   projectPath={proj.path}      // ADD THIS
>   onSelect={async (agent) => { ... }}
>   onClose={() => setPendingLaunch(null)}
> />
> ```
>
> **Empty state message update:** AgentPickerModal line 61 shows "No agents configured. Add agents in Settings." when the agent list is empty. With project resolution, this message should be context-aware:
> ```tsx
> fallback={
>   <div class="agent-modal-empty">
>     {props.projectPath
>       ? "No agents configured for this project. Edit in project Coding Agents settings."
>       : "No agents configured. Add agents in Settings."}
>   </div>
> }
> ```
>
> ### 3.5.2 NewAgentModal — NOTE: this is NOT the same as `NewEntityAgentModal`
>
> The plan references `NewAgentModal.tsx` but ProjectPanel.tsx imports and uses `NewEntityAgentModal` (line 399). These are **different components**:
> - `NewEntityAgentModal` — creates an agent entity (folder) within a project; already receives `projectPath` prop
> - `NewAgentModal` — creates a new top-level agent from the sidebar header; does NOT receive `projectPath`
>
> `NewAgentModal` loads agents at line 37-40 via `SettingsAPI.get()`. It's invoked from outside project context (sidebar-level action). For project-aware resolution, the caller would need to know which project is active. **Recommendation: defer this — `NewAgentModal` is a global action and should continue showing global agents. Only `AgentPickerModal` (launched within project context) needs project resolution.**
>
> ### 3.5.3 OpenAgentModal — already has partial context via `initialRepo`
>
> `OpenAgentModal` accepts `initialRepo?: RepoMatch` (line 6). When `initialRepo` is set, the working directory is known, which means the project path can be derived. However, `OpenAgentModal` is used for two flows:
> - With `initialRepo` (from SessionItem context menu) — project path derivable
> - Without `initialRepo` (global "Open Agent" action) — no project context
>
> The derivation from `initialRepo.path` to `projectPath` requires extracting the project root (everything before `.ac-new`). This is the same logic as `extractProjectName` in the terminal titlebar. **Add a shared utility:**
> ```typescript
> // src/shared/utils.ts
> export function extractProjectPath(workDir: string): string | null {
>   const norm = workDir.replace(/\\/g, '/');
>   const idx = norm.indexOf('/.ac-new');
>   return idx > 0 ? norm.substring(0, idx) : null;
> }
> ```
> Then in `OpenAgentModal.onMount`:
> ```typescript
> const projectPath = props.initialRepo ? extractProjectPath(props.initialRepo.path) : null;
> const agents = projectPath
>   ? await ProjectSettingsAPI.resolveAgents(projectPath)
>   : (await SettingsAPI.get()).agents;
> setAgents(agents);
> ```
>
> ### 3.5.4 SessionItem `hasClaude()` — needs project-aware resolution
>
> `SessionItem.tsx` line 142-145 reads `settingsStore.current?.agents` (global). The session's `workingDirectory` is available but the session doesn't directly know its project path. Two approaches:
>
> **Option A (recommended): Resolve at session creation time.** When a session is created via `SessionAPI.create()`, the `PendingLaunch.path` is the agent/replica path within a project. The selected agent config is already known at that point. The `hasClaude()` check could be based on the session's own `agentId` — if the session was launched with an agent whose command includes "claude", then `hasClaude` is true. This avoids runtime resolution entirely.
>
> **Option B: Derive project path from session cwd.** Use `extractProjectPath(session.workingDirectory)` to get the project path, then look up `projectStore.getResolvedAgents(projectPath)`. But this runs a lookup on every render of every session item, which is wasteful.
>
> **Recommendation: Option A.** The `hasClaude()` function is used to show/hide Claude-specific context menu options (like "Send CLAUDE.md"). Since the session already has an `agentId` (or `preferredAgentId`), check the agent config for that specific session rather than scanning all agents. This is more correct anyway — a session launched with Codex shouldn't show Claude options even if Claude is configured globally.
>
> ### 3.5.5 MISSING: `NewEntityAgentModal` — already receives `projectPath`, needs agent resolution
>
> `NewEntityAgentModal` is called from ProjectPanel.tsx line 399 with `projectPath={proj.path}`. It likely loads agents via `SettingsAPI.get()` internally. This component needs the same resolution treatment as AgentPickerModal: use `ProjectSettingsAPI.resolveAgents(props.projectPath)` instead of global settings. **Add to Phase D as step D3b.**
>
> ### 3.5.6 MISSING: `EditTeamModal` — may show agent assignment UI
>
> If `EditTeamModal` allows assigning agents to teams, it reads from global agents. Check if it needs project-aware resolution. (Lower priority — verify during Phase D.)

---

## 4. Implementation Sequence

### Phase A: Backend Foundation (no UI changes yet)

| Step | File | What |
|------|------|------|
| A1 | `src-tauri/src/config/project_settings.rs` | New module: `ProjectSettings` struct, load/save/delete functions |
| A2 | `src-tauri/src/config/mod.rs` | Register `project_settings` module |
| A3 | `src-tauri/src/commands/project_settings.rs` | New commands: get, update, delete, resolve |
| A4 | `src-tauri/src/commands/mod.rs` | Register module |
| A5 | `src-tauri/src/lib.rs` | Register commands in `generate_handler![]` (line ~570) |
| A6 | Verify | `cargo check` passes |

### Phase B: Frontend Types & IPC

| Step | File | What |
|------|------|------|
| B1 | `src/shared/types.ts` | Add `ProjectSettings` interface |
| B2 | `src/shared/ipc.ts` | Add `ProjectSettingsAPI` object |
| B3 | `src/sidebar/stores/project.ts` | Extend `ProjectState`, load settings on discovery |
| B4 | Verify | `npx tsc --noEmit` passes |

> **[DEV-WEBPAGE-UI] Phase B implementation notes:**
>
> - **B1**: Place `ProjectSettings` interface adjacent to `AgentConfig` in `types.ts` (they share the same domain). Import `AgentConfig` from the same file — no cross-file dependency.
> - **B2**: Add `ProjectSettingsAPI` after the existing `ProjectAPI` export in `ipc.ts`. Import `ProjectSettings` from `types.ts`. All 4 methods use `transport.invoke` — no event listeners needed.
> - **B3**: Import `ProjectSettingsAPI` in `project.ts`. Add `ProjectSettings` to the import from `types.ts`. The `Promise.all` pattern means no additional error handling beyond the existing try/catch in `loadProject`/`reloadProject` — if `ProjectSettingsAPI.get()` fails, it returns `null` (backend returns `None` on any error), so the project still loads fine.
> - **B3 edge case**: If `ProjectSettingsAPI.get()` rejects (network error, Tauri IPC failure), the `Promise.all` will reject the whole pair. Wrap the settings call with a fallback: `ProjectSettingsAPI.get(path).catch(() => null)`.

### Phase C: Modal UI

| Step | File | What |
|------|------|------|
| C1 | `src/sidebar/components/ProjectAgentsModal.tsx` | New modal component (reuse SettingsModal agent tab pattern) |
| C2 | CSS file for modal styles | Styles for the modal (or add to existing project panel CSS) |
| C3 | Verify | Modal renders correctly with test data |

> **[DEV-WEBPAGE-UI] Phase C implementation notes:**
>
> **C1 — Component skeleton (~150-200 lines estimated):**
> The modal reuses SettingsModal's agent card rendering (lines 321-448) almost verbatim. The main structural differences are:
> - No tab bar (single-purpose modal, not multi-tab like SettingsModal)
> - Added toggle switch at the top
> - "Copy from Global" button in the action bar
> - Smaller scope — only agents, no Telegram/integrations/general tabs
>
> **C2 — CSS approach:**
> - **Do NOT create a new CSS file.** Add styles to `src/sidebar/styles/sidebar.css` where all other modal styles live (lines 790+).
> - New CSS needed is minimal: only the toggle switch (`.project-agents-toggle`) and the "Using global agents" info message (`.project-agents-global-info`). Everything else reuses existing classes:
>   - Modal shell: `.modal-overlay`, `.modal-container`, `.modal-header`, `.modal-title`, `.modal-close`, `.modal-body`, `.modal-footer`
>   - Agent cards: `.settings-button-card`, `.settings-button-card-header`, `.settings-color-dot`, `.settings-agent-remove`
>   - Form fields: `.settings-field`, `.settings-label`, `.settings-input`, `.settings-color-row`, `.settings-color-picker`, `.settings-input-sm`
>   - Checkboxes: `.settings-checkbox-field`, `.settings-checkbox`
>   - Action buttons: `.settings-agent-actions`, `.settings-preset-btn`, `.settings-add-btn`
>   - Footer: `.modal-footer`, `.modal-save-error`, `.modal-btn-primary`, `.modal-btn-secondary`
>
> **Toggle switch CSS** — keep it minimal, consistent with the industrial-dark aesthetic:
> ```css
> .project-agents-toggle {
>   display: flex;
>   align-items: center;
>   gap: 8px;
>   padding: var(--spacing-sm) 0;
>   margin-bottom: var(--spacing-md);
>   border-bottom: 1px solid var(--sidebar-border);
> }
> .project-agents-toggle input[type="checkbox"] {
>   /* reuse .settings-checkbox styling */
> }
> .project-agents-global-info {
>   padding: var(--spacing-md);
>   color: var(--sidebar-fg-dim);
>   font-size: var(--font-size-sm);
>   text-align: center;
>   opacity: 0.7;
> }
> ```

### Phase D: Integration

| Step | File | What |
|------|------|------|
| D1 | `src/sidebar/components/ProjectPanel.tsx` | Add context menu option + modal trigger + badge |
| D2 | `src/sidebar/components/AgentPickerModal.tsx` | Accept `projectPath`, use resolution logic |
| D3 | `src/sidebar/components/NewAgentModal.tsx` | Same resolution update |
| D4 | `src/sidebar/components/OpenAgentModal.tsx` | Same resolution update |
| D3b | `src/sidebar/components/NewEntityAgentModal.tsx` | Use `projectPath` prop for agent resolution (already receives it) |
| D5 | `src/sidebar/components/SessionItem.tsx` | Use resolved agents for display logic |
| D6 | Verify | Full flow works: set project agents → picker shows only those agents |

> **[DEV-WEBPAGE-UI] Phase D implementation notes:**
>
> **D1 — ProjectPanel changes are contained:** Only 3 additions: (1) new signal, (2) context menu button, (3) modal render with Portal. All within the existing `<For each={projectStore.projects}>` iterator where `proj` is in scope. No prop threading needed — `proj.path`, `proj.folderName`, and `proj.projectSettings` are all directly accessible.
>
> **D2 — AgentPickerModal minimal change:** Add `projectPath?: string` to the props interface. Change `onMount` to branch on `props.projectPath`. The existing keyboard navigation, highlight index, and overlay click logic are unaffected. ~5 lines changed.
>
> **D3/D3b — NewAgentModal vs NewEntityAgentModal clarification:**
> - `NewAgentModal.tsx`: Global sidebar action — keep using global agents. No change needed (see 3.5.2 analysis above).
> - `NewEntityAgentModal.tsx`: Already receives `projectPath` prop from ProjectPanel. Change its internal `SettingsAPI.get()` call to `ProjectSettingsAPI.resolveAgents(props.projectPath)`. This ensures that when creating an entity agent within a project that has custom agents, only the project's agents are shown.
>
> **D4 — OpenAgentModal:** Add `extractProjectPath()` utility (see 3.5.3). Derive project path from `initialRepo.path` when available. Fallback to global when no repo context.
>
> **D5 — SessionItem `hasClaude()` — recommend Option A from 3.5.4:** Check the session's own agent config rather than scanning all agents. This is more semantically correct and avoids the project-path lookup complexity. The session's `agentId` is available — match it against the resolved agents for display.
>
> **D6 — Verification flow:**
> 1. Open project context menu → "Coding Agents" → modal opens
> 2. Toggle ON → "Copy from Global" → agents appear → modify one → Save
> 3. Right-click an agent entity → launch → AgentPickerModal shows only project agents
> 4. Badge appears on project header
> 5. Re-open modal → toggle OFF → Save → badge disappears, picker shows global agents again

### Phase E: Validation & Edge Cases

| Step | What |
|------|------|
| E1 | Test: project with custom agents → only those appear in picker |
| E2 | Test: project without custom agents → global agents appear |
| E3 | Test: toggle custom agents off → file deleted, reverts to global |
| E4 | Test: "Copy from Global" populates correctly |
| E5 | Test: badge appears/disappears correctly |
| E6 | Test: empty agents array `[]` → no agents available (valid state) |
| E7 | Test: malformed/corrupted project-settings.json → graceful fallback to global |

---

## 5. Files Changed Summary

### New Files
| File | Purpose |
|------|---------|
| `src-tauri/src/config/project_settings.rs` | Rust: ProjectSettings struct, load/save/delete |
| `src-tauri/src/commands/project_settings.rs` | Rust: Tauri commands for project settings |
| `src/sidebar/components/ProjectAgentsModal.tsx` | Frontend: modal for editing project agents |

### Modified Files
| File | Change |
|------|--------|
| `src-tauri/src/config/mod.rs` | Register project_settings module |
| `src-tauri/src/commands/mod.rs` | Register project_settings commands module |
| `src-tauri/src/lib.rs` | Register 4 new commands in `generate_handler![]` |
| `src/shared/types.ts` | Add `ProjectSettings` interface |
| `src/shared/ipc.ts` | Add `ProjectSettingsAPI` |
| `src/sidebar/stores/project.ts` | Extend ProjectState, load project settings |
| `src/sidebar/components/ProjectPanel.tsx` | Context menu option + badge + modal trigger |
| `src/sidebar/components/AgentPickerModal.tsx` | Accept projectPath, use resolution |
| `src/sidebar/components/NewEntityAgentModal.tsx` | Use existing `projectPath` prop for agent resolution |
| `src/sidebar/components/OpenAgentModal.tsx` | Accept projectPath (derived from `initialRepo`), use resolution |
| `src/sidebar/components/SessionItem.tsx` | Use session's own agent config for `hasClaude()` check |
| `src/sidebar/styles/sidebar.css` | Badge + toggle switch + global-info styles |
| `src/shared/utils.ts` | Add `extractProjectPath()` utility |

### Tauri Permissions

> **[DEV-RUST] No capability files exist.** This project does not use Tauri 2's ACL/capabilities system. There are no `src-tauri/capabilities/*.json` files. Commands are implicitly allowed by being listed in `generate_handler![]` in `lib.rs`. No permission changes needed — just add the 4 commands to the handler macro.

---

## 6. Risk Assessment

| Risk | Mitigation |
|------|-----------|
| Breaking existing agent picker behavior | Resolution command falls back to global — same behavior as today when no project settings exist |
| Stale project settings after reload | `reloadProject()` already re-fetches all data; just add project settings to that flow |
| File permissions on `.ac-new/` | Directory already exists and is writable (used by discovery) |
| Session-project association unclear | Sessions are created within project context; trace the project path from where the session is spawned |
| AgentConfig ID collisions between global and project | IDs use timestamp + counter (`newAgentId()`), collisions extremely unlikely; and they're independent namespaces anyway |

> **[DEV-RUST] Additional risks identified:**
>
> | Risk | Mitigation |
> |------|-----------|
> | Path traversal via `project_path` param | Validate `.ac-new/` exists within the path before any write (see validated_settings_path above) |
> | Concurrent write from multiple windows | Acceptable — same pattern as `save_settings()` in settings.rs. Both sidebar windows writing simultaneously is extremely unlikely since the modal blocks interaction |
> | Stale `SettingsState` in `resolve_agents_for_project` | The command reads from `State<SettingsState>` which is the in-memory live copy (updated by `update_settings`). No stale-file risk — it's always current |
>
> **[DEV-WEBPAGE-UI] Additional frontend risks identified:**
>
> | Risk | Mitigation |
> |------|-----------|
> | Modal edits lost on accidental overlay click | SettingsModal already has `handleOverlayClick` that checks `e.target === overlay` — same pattern prevents misclicks on inner content. Consider adding a "discard changes?" confirmation if edits were made, but defer to Phase E (not MVP). |
> | `Promise.all` in `loadProject` rejects if either call fails | Wrap `ProjectSettingsAPI.get()` with `.catch(() => null)` so project discovery succeeds even if settings fetch fails. Project loads with `projectSettings: null` (uses global). |
> | Agent preset dedup across global↔project | The `hasAgentByCommand()` check in SettingsModal hides preset buttons if that agent type already exists. ProjectAgentsModal must implement the same check against `localAgents.list`, not global settings. If a project copies from global (which has Claude), the "+ Claude Code" preset button should be hidden. |
> | Stale `initialSettings` prop if another tab edits the file | Not a real risk — project-settings.json is only edited through this modal, and only one modal instance exists at a time. The prop is a snapshot from the store, which is reloaded on `onSaved()`. |

---

## 7. Adversarial Review [GRINCH]

### [GRINCH] G1 — CRITICAL: `project-settings.json` is NOT gitignored

**Problem:** Section 1.1 claims the file "Lives inside `.ac-new/` (already exists for every AC project, already gitignored via `wg-*/` pattern)". This is FALSE. The `.ac-new/.gitignore` only contains the pattern `wg-*/` (see `ensure_ac_new_gitignore()` in `ac_discovery.rs:700-727`). A file at `.ac-new/project-settings.json` is NOT matched by `wg-*/` and WILL be tracked by git.

**Impact:** Users' per-project agent configurations (including shell commands, potentially containing paths or sensitive flags) will be committed to the repository and visible to anyone with access. This may be intentional (shared config), but the plan explicitly claims it's gitignored, which is wrong. If it IS meant to be shared, the plan should say so and document the implications. If it IS meant to be private, the gitignore needs updating.

**Severity:** CRITICAL — the assumption is factually wrong and drives design decisions.

**Fix:** Decide the intent:
- **Option A (private):** Add `project-settings.json` to the `ensure_ac_new_gitignore()` function as a second required pattern. Update the plan text to reflect this.
- **Option B (shared/committed):** Remove the "already gitignored" claim. Document that the file is intentionally tracked by git so teams share the same agent configuration. Consider security implications of committed `command` fields.

---

### [GRINCH] G2 — HIGH: Duplicate and conflicting resolution logic — backend command vs frontend store helper

**Problem:** The plan defines TWO independent resolution mechanisms:
1. **Backend:** `resolve_agents_for_project` Tauri command (section 2.2) — reads file from disk + in-memory `SettingsState`
2. **Frontend:** `projectStore.getResolvedAgents(path)` (section 3.2, DEV-WEBPAGE-UI enrichment) — reads from the already-loaded `projectSettings` in the SolidJS store

These can diverge. The backend reads the file fresh from disk; the frontend reads from a snapshot loaded at project-load time. If the file is modified externally (or by another window), the two sources give different answers.

Worse, different call sites use different sources:
- `AgentPickerModal`, `OpenAgentModal` → call the backend `resolveAgents` (IPC round-trip)
- `SessionItem.hasClaude()` needs a synchronous answer → must use the store helper
- The badge → uses `proj.projectSettings` from the store

**Severity:** HIGH — two sources of truth leads to inconsistent UI.

**Fix:** Pick ONE authoritative source. Recommended: the **frontend store** is the single source of truth. Remove `resolve_agents_for_project` backend command entirely. The frontend already has `projectSettings` loaded in the store. Resolution logic is trivial: `projectSettings?.agents ?? globalAgents`. All call sites use the store. This eliminates the extra IPC call AND the divergence risk. The backend only needs `get`, `update`, `delete` — not `resolve`.

---

### [GRINCH] G3 — HIGH: Section 3.5.5 (DEV-WEBPAGE-UI) is WRONG — `NewEntityAgentModal` does NOT load agents

**Problem:** The DEV-WEBPAGE-UI enrichment in 3.5.5 says: "NewEntityAgentModal [...] likely loads agents via `SettingsAPI.get()` internally. This component needs the same resolution treatment as AgentPickerModal."

This is factually wrong. I read the component (`NewEntityAgentModal.tsx`). It is a form for creating a named agent entity (folder) via `EntityAPI.createAgentMatrix()`. It has fields for `name` and `description`. It does NOT load, display, or reference coding agents (`AgentConfig[]`) anywhere. It doesn't import `SettingsAPI`. The word "likely" in the enrichment reveals this was a guess, not a verified claim.

**Impact:** If implemented as written, a developer would waste time trying to add agent resolution to a component that has nothing to do with agents. Step D3b in the implementation sequence is based on this false premise.

**Severity:** HIGH — incorrect analysis that would misdirect implementation effort.

**Fix:** Remove section 3.5.5 entirely. Remove step D3b from Phase D. `NewEntityAgentModal` needs zero changes for this feature.

---

### [GRINCH] G4 — HIGH: `hasClaude()` in SessionItem — reactivity constraint unaddressed

**Problem:** `SessionItem.tsx:142-145` currently reads `settingsStore.current?.agents` synchronously (reactive — auto-tracks in JSX). The plan's Option A (section 3.5.4 DEV-WEBPAGE-UI enrichment) proposes checking "the session's own agent config" instead.

Two issues:
1. **Semantic change:** Current `hasClaude()` checks if ANY configured agent is Claude-based. Option A changes it to check only the session's specific agent. This changes behavior — a session launched with Codex would lose Claude-specific context menu items even though Claude agents exist. This may or may not be desired, but the plan doesn't acknowledge the behavioral change.
2. **Reactivity:** If we switch to project-aware resolution using the store, the code becomes: `extractProjectPath(session.workingDirectory)` → `projectStore.getResolvedAgents(path)` → fallback to global. This is synchronous and reactive (good), but the plan doesn't spell out this full chain. Option B (async lookup) is correctly flagged as breaking reactivity, but the recommended Option A sidesteps the real question.

**Severity:** HIGH — unclear spec leads to either a behavior regression or an incomplete implementation.

**Fix:** Clarify the desired semantics:
- **If intent is "does this session's agent support Claude features?"** → Use `session.preferredAgentId` to look up the specific agent config. But ensure the agent config is available synchronously (store-based).
- **If intent is "are Claude agents available for this project?"** → Use `extractProjectPath()` + store-based resolution. Document the full reactive chain.

---

### [GRINCH] G5 — MEDIUM: Frontend resolution call sites lack version-mismatch fallback

**Problem:** Section B3 (DEV-WEBPAGE-UI enrichment) correctly identifies that `ProjectSettingsAPI.get()` should be wrapped in `.catch(() => null)` in `loadProject`/`reloadProject`. But the same problem exists in OTHER call sites that the plan doesn't protect:
- `AgentPickerModal.onMount` (section 3.5.1) — calls `ProjectSettingsAPI.resolveAgents()` without catch
- `OpenAgentModal.onMount` (section 3.5.3) — same
- `ProjectAgentsModal.handleCopyFromGlobal` — calls `SettingsAPI.get()` (existing pattern, but still)

If a user updates the frontend but has an old backend (or vice versa during development), these commands will fail with "command not found". The modals will show an empty agent list with no error feedback.

**Severity:** MEDIUM — affects development workflow and edge-case deployment scenarios.

**Fix:** Every `ProjectSettingsAPI.*` call outside the project store should have a `.catch()` fallback. For `resolveAgents`, fallback to `SettingsAPI.get().then(s => s.agents)`. For the modal save/delete, show the error in `saveError()` (already handled). Add a note in Phase D steps.

---

### [GRINCH] G6 — MEDIUM: Empty `{}` file creates semantic ambiguity with `#[serde(default)]`

**Problem:** Section 1.1 defines clear semantics: "File absent = use global" vs "Empty `agents: []` = no agents". But `ProjectSettings` has `#[serde(default)] pub agents: Vec<AgentConfig>`. This means a file containing `{}` (no `agents` key) deserializes to `ProjectSettings { agents: [] }` — identical to a file with `{"agents": []}`.

So a manually created `{}` file (or a future version of `project-settings.json` that adds new fields but not `agents`) would be interpreted as "no agents" rather than "use global", with no way to distinguish.

**Severity:** MEDIUM — edge case for manual edits or future schema evolution.

**Fix:** The resolution function `load_project_settings` should check if the file exists AND has an `agents` key present (not just rely on serde default). Alternatively, use `Option<Vec<AgentConfig>>` instead of `Vec<AgentConfig>` for the `agents` field: `None` means "field absent, use global", `Some([])` means "explicitly no agents". This is cleaner.

---

### [GRINCH] G7 — MEDIUM: "Copy from Global" with zero global agents — silent empty result

**Problem:** If the user clicks "Copy from Global" but global settings has zero agents, the result is silently setting `localAgents.list` to `[]`. The user sees no agents appear and has no feedback about why. This is confusing — they expected to copy something but nothing happened.

**Severity:** MEDIUM — poor UX, not a bug.

**Fix:** Before copying, check `settings.agents.length`. If zero, show a brief message: "No global agents configured. Add agents in Settings first." Disable the "Copy from Global" button or show it grayed out. Alternative: show a toast/inline notice.

---

### [GRINCH] G8 — MEDIUM: Missing resolution point — `OnboardingModal.tsx`

**Problem:** Section 3.5 lists resolution points but misses `OnboardingModal.tsx`, which calls `SettingsAPI.get()` at lines 32 and 63 to read and write global agents. The onboarding modal adds a user's first agent to global settings.

Analysis: this modal runs on first launch, before any project exists. It configures global settings. So it correctly uses global agents and does NOT need project-aware resolution.

**Severity:** MEDIUM (documentation gap) — the plan should explicitly list `OnboardingModal` as a NON-resolution point and explain why, to prevent a future developer from "fixing" it.

**Fix:** Add a note to section 3.5: "`OnboardingModal.tsx` reads/writes global settings — intentionally NOT project-aware. It configures the global agent list during first-run setup."

---

### [GRINCH] G9 — LOW: `resolve_agents_for_project` reads two sources non-atomically

**Problem:** The backend command reads from disk (`load_project_settings`) and from in-memory state (`SettingsState`) as two separate operations. Between the two reads, `update_settings` could modify the in-memory global agents. For a single-user desktop app this is extremely unlikely, but it's a logical gap.

**Severity:** LOW — theoretical race, practically irrelevant. Becomes moot if G2 fix is adopted (remove backend resolve command).

---

### [GRINCH] G10 — LOW: Corrupted `project-settings.json` fails silently — no user visibility

**Problem:** `load_project_settings` returns `None` on parse error, falling back to global agents. The user has no idea their project settings file is corrupt. They might think they never configured custom agents, or wonder why their settings "disappeared".

**Severity:** LOW — edge case (corruption requires external file modification or disk error).

**Fix:** When `serde_json::from_str` fails but the file exists, emit a Tauri event (e.g., `project-settings-corrupt`) that the frontend can display as a non-blocking notification: "project-settings.json is corrupt, using global agents. Re-save to fix."
