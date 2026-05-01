# Plan: Issue #120 — Settings/General toggle to inject RTK PreToolUse hook into agent replicas

- **Issue**: https://github.com/mblua/AgentsCommander/issues/120
- **Branch**: `feature/120-rtk-hook-injection-toggle` (cut from `main` at `4e85a32`)
- **Scope**: `repo-AgentsCommander` only — Rust backend (settings, hook helper, sweep, startup detection) and SolidJS frontend (Settings/General checkbox, startup banner). One new cargo dep: `which`.
- **Anchored against**: the branch tip at the time of writing. Re-verify line numbers if `main` advances before implementation lands.

---

## 1. Overview

A new Settings/General checkbox `Inject RTK hook into agent replicas` (default `false`). When enabled, AgentsCommander writes the canonical RTK PreToolUse hook (the same `node -e "..."` rewriter currently embedded in `repo-AgentsCommander/.claude/settings.json`) into every AC-managed agent directory's `.claude/settings.local.json`. When disabled, AC removes only the entries whose `command` matches the canonical rewriter byte-for-byte; other PreToolUse entries are untouched.

Three behaviors shipped together:

1. **Toggle ON sweep.** All existing AC-managed agent dirs are visited and the hook is merged. New replica/matrix creation also injects.
2. **Toggle OFF sweep.** All existing AC-managed agent dirs are visited and our hook entry is removed.
3. **Startup detection.** On every boot AC probes for `rtk` in `PATH`:
   - `rtk` found AND `injectRtkHook=false` AND `rtkPromptDismissed=false` → non-blocking banner with `[Enable]` / `[Don't ask again]`.
   - `rtk` missing AND `injectRtkHook=true` → AC auto-disables the toggle, runs an OFF-sweep to clean up the broken hooks, and shows a non-blocking banner explaining the auto-disable.
   - Otherwise: silent.

**Architectural decisions (all closed in §9 — round 2; round-3 mechanical fixes in §17; phase-B-bug fix in §18):**

- The rewriter command lives as `pub const RTK_REWRITER_COMMAND: &str` in `claude_settings.rs`. It carries an immutable substring `RTK_HOOK_MARKER = "@ac-rtk-marker-v1"`, embedded as a leading JS string-expression statement (`'@ac-rtk-marker-v1';...`) inside the `node -e "..."` argument. Idempotency on insert AND filtering on remove both match by **marker substring**, not by byte-equality of the whole command. Pre-ship change locked here so all future-shipped hooks are forward-compatible against upstream rtk evolution (grinch M10).
- The hook merge helper is `ensure_rtk_pretool_hook(dir: &Path, enabled: bool) -> Result<(), String>` in the same module as `ensure_claude_md_excludes`. **It bails with `log::warn!` and returns Ok(()) on any malformed JSON or wrong-shape value** — never overwrites user data we cannot semantically interpret (grinch H1, H2). UTF-8 BOM is stripped before parsing on both ON and OFF read paths (grinch M11).
- The replica enumerator uses `symlink_metadata` to skip Unix symlinks-to-dir, plus a Windows `FILE_ATTRIBUTE_REPARSE_POINT` check via `MetadataExt::file_attributes()` to skip NTFS junctions, plus canonical-path dedupe. The sweep cannot escape the user's `project_paths` set even when junctions are present (grinch M7).
- The sweep covers BOTH `_agent_*` matrices AND `__agent_*` replicas. Both already receive `.claude/settings.local.json` via `ensure_claude_md_excludes` (4 callers); keeping the two helpers wired in lockstep avoids per-caller asymmetry. The checkbox label keeps the issue's exact wording ("agent replicas") but the code path applies symmetrically — closed in §9 Q1.
- Three new Tauri commands: `set_inject_rtk_hook(value)` and `set_rtk_prompt_dismissed(value)` (narrow setters that hold the `SettingsState` write lock through `save_settings`, eliminating the IPC-level read-modify-write race; grinch H3), and `sweep_rtk_hook(enabled)` (the retroactive sweep). The setting itself can ALSO flow through the existing `update_settings` command via serde for the Settings/General checkbox path.
- A new `RtkSweepLockState = Arc<tokio::sync::Mutex<()>>` is registered in `lib.rs::setup`. Acquired by the sweep, the startup auto-disable sweep, and the four `ensure_claude_md_excludes` + `ensure_rtk_pretool_hook` call sites — eliminates the in-process race where `sweep_rtk_hook` and `entity_creation::create_workgroup` interleave on the same file (grinch M8).
- The startup auto-disable persists `injectRtkHook=false` while **holding the write lock through the `save_settings` call**, mirroring `update_settings` semantics — closes the disk/memory-divergence race (grinch H4).
- The startup sweep runs unconditionally (with the current `injectRtkHook` value) so a mid-sweep crash heals automatically on next boot. No new "dirty" state field needed; idempotency does the work.
- **The boot-time RTK startup mode is cached in `RtkStartupModeState = Arc<std::sync::OnceLock<String>>`** registered in `lib.rs::setup`. The setup task computes the mode and writes it to the cache **before** running side effects; `get_rtk_startup_status` reads the cache instead of recomputing from current state. This guarantees that the listener (boot-time emit) and the getter (late-mount snapshot) always agree on the mode — required because the `auto-disabled` mode mutates settings as a side effect, breaking any naïve recompute path (§18 amendment, surfaced by dev-webpage-ui in Phase B integration).

---

## 2. Files to touch

| File | Phase | Change |
|---|---|---|
| `src-tauri/Cargo.toml` | A | Add `which = "7"` dependency. |
| `src-tauri/src/config/settings.rs` | A | Two new boolean fields on `AppSettings` (`inject_rtk_hook`, `rtk_prompt_dismissed`); initialize in `Default`; round-trip serde tests. |
| `src-tauri/src/config/claude_settings.rs` | A | Add `RTK_HOOK_MARKER` and `RTK_REWRITER_COMMAND` constants (marker pre-baked); add `ensure_rtk_pretool_hook(dir, enabled)` with bail-on-malformed (H1) and bail-on-wrong-shape (H2) and BOM strip (M11); add `enumerate_managed_agent_dirs(project_paths)` with symlink/junction filtering (M7); extend the doc-block at top of file to list the rtk callers alongside the existing exclude callers; new unit-test module covering the merge/remove matrix + new shape/BOM/junction tests. |
| `src-tauri/src/commands/config.rs` | A | New Tauri commands: `set_inject_rtk_hook(value)`, `set_rtk_prompt_dismissed(value)` (narrow setters, write-lock-held, H3); `sweep_rtk_hook(settings, sweep_lock, enabled) -> RtkSweepResult` (acquires `RtkSweepLockState`, M8); `get_rtk_startup_status()` (sync getter for late-mounting frontend). |
| `src-tauri/src/commands/agent_creator.rs` | A | Add `settings` and `sweep_lock` State injections to `write_claude_settings_local`; acquire `RtkSweepLockState` around the helper sequence (M8). |
| `src-tauri/src/cli/create_agent.rs` | A | Inside the `--launch` block, mirror the `ensure_rtk_pretool_hook` call alongside the existing exclude call. **No lock acquisition** — CLI runs out-of-process and cannot share the in-process tokio Mutex with a running AC instance (cross-process race documented in §7.4). |
| `src-tauri/src/commands/entity_creation.rs` | A | Read `inject_rtk_hook` from the same `SettingsState` snapshot that already gates the exclude write; acquire `RtkSweepLockState` around the helper sequence in `create_agent_matrix` and around the per-replica loop in `create_workgroup` (M8). |
| `src-tauri/src/lib.rs` | A | Register `set_inject_rtk_hook`, `set_rtk_prompt_dismissed`, `sweep_rtk_hook`, `get_rtk_startup_status` in `invoke_handler!`; build & `manage(RtkSweepLockState)`; build & `manage(RtkStartupModeState)` (boot-mode cache, §18); add the startup detection task in `setup()` (with H4-fixed write-lock-through-save, M8 lock acquisition for the auto-disable + active-recovery sweeps, and §18 mode-cache write before running side effects). |
| `repo-AgentsCommander/.claude/settings.json` | A | Update line 9 `command` field: prefix `'@ac-rtk-marker-v1';` to the JS source so unit test #14 (source-of-truth check) passes. One-time pre-ship change. |
| `src/shared/types.ts` | B | Add `injectRtkHook` and `rtkPromptDismissed` to the `AppSettings` interface. |
| `src/shared/ipc.ts` | B | Extend `SettingsAPI` with `setInjectRtkHook(value)`, `setRtkPromptDismissed(value)`, `sweepRtkHook(enabled)`, `getRtkStartupStatus()`. |
| `src/sidebar/components/SettingsModal.tsx` | B | Add the checkbox in the General tab; **fire the sweep from `handleSave`** (when `injectRtkHook` changed vs. the initial snapshot), not from `onChange` — see §5.3 for the corrected pattern; log per-dir errors via `console.error` (M6); UI gate the Save button while a sweep is in flight (M9). |
| `src/main/components/RtkBanner.tsx` | B | NEW non-blocking banner with two modes (`prompt-enable` / `auto-disabled`). Uses the narrow setters (H3); subscribes BEFORE snapshotting the initial mode (M5); logs per-dir sweep errors (M6); UI gate while sweep in flight (M9). |
| `src/main/App.tsx` | B | Mount `<RtkBanner />` between `<Titlebar />` and `<div class="main-body">`; subscribe to the `rtk_startup_status` event. |
| `src/main/styles/main.css` | B | Banner styles (color, padding, dismiss-button). |

**Files NOT to touch:**

- `src-tauri/src/config/teams.rs`, `commands/ac_discovery.rs` — no replica enumeration through these. The sweep uses the new dedicated `enumerate_managed_agent_dirs` helper (§4.3) — it returns paths only, no identity/repo resolution; `discover_ac_agents` is too heavy and pulls Tauri DI we do not need here.
- `src-tauri/src/commands/window.rs`, `commands/session.rs`, `commands/pty.rs` — out of scope.
- `src/sidebar/App.tsx`, `src/terminal/App.tsx` — banner mounts in main only (the unified window is the canonical surface in 0.8.0; a banner inside the embedded sidebar pane is wrong because it would not be visible when the sidebar pane is narrow).

---

## 3. Phase split

Two phases so dev-rust can ship Phase A independently.

- **Phase A — Backend.** §4.1–§4.7. Builds clean (`cargo check` green) at every commit. Frontend ignores the new settings fields gracefully (camelCase serde defaults to `false`).
- **Phase B — Frontend.** §5.1–§5.5. Activates the new settings fields, the checkbox, the banner, and the sweep round-trip. Requires Phase A merged.

Phase A includes registration of the Tauri command in `invoke_handler!` so Phase B can call it on day one of the merge.

---

## 4. Phase A — Backend

### 4.1 `src-tauri/src/config/settings.rs` — new fields

**Anchor:** the `AppSettings` struct currently spans lines 47–152 (last field is `log_level: Option<String>`). The `Default` impl spans lines 186–230.

#### 4.1.1 Insert the two new fields at the end of the struct

Add **immediately after** `pub log_level: Option<String>,` (line 151), inside `AppSettings`:

```rust
    /// When true, AC writes the RTK PreToolUse rewriter hook into every managed
    /// agent dir's `.claude/settings.local.json` (matrices + workgroup replicas).
    /// Toggled from Settings/General. See issue #120.
    #[serde(default)]
    pub inject_rtk_hook: bool,
    /// When true, the startup banner offering to enable `inject_rtk_hook` is
    /// suppressed for the lifetime of this settings file. Set by the `[Don't
    /// ask again]` button on the banner. See issue #120.
    #[serde(default)]
    pub rtk_prompt_dismissed: bool,
```

`#[serde(default)]` is **mandatory**: older `settings.json` files written by 0.8.0 must deserialize cleanly with both fields defaulting to `false`.

#### 4.1.2 Initialize the fields in the `Default` impl

Add the two field assignments **immediately after** `log_level: None,` (line 227), keeping declaration order:

```rust
            log_level: None,
            inject_rtk_hook: false,
            rtk_prompt_dismissed: false,
```

#### 4.1.3 Add round-trip serde tests

Append inside the existing `mod tests` block (after the last test, before the closing `}` of the module). Two tests, mirror of `coord_sort_by_activity_round_trips_through_serde` (line 554):

```rust
    #[test]
    fn inject_rtk_hook_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.inject_rtk_hook);
        s.inject_rtk_hook = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"injectRtkHook\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.inject_rtk_hook);
    }

    #[test]
    fn rtk_prompt_dismissed_round_trips_through_serde() {
        let mut s = AppSettings::default();
        assert!(!s.rtk_prompt_dismissed);
        s.rtk_prompt_dismissed = true;
        let json = serde_json::to_string(&s).expect("serialize");
        assert!(json.contains("\"rtkPromptDismissed\":true"));
        let back: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back.rtk_prompt_dismissed);
    }
```

The existing "defaults when missing from JSON" tests (e.g. `coord_sort_by_activity_defaults_when_missing_from_json` at line 731) already cover the missing-key path implicitly; no extension required, since adding new fields to that JSON literal is unrelated to deserialization defaults — those tests assert "OLD JSON deserializes". Leave them untouched.

### 4.2 `src-tauri/src/config/claude_settings.rs` — the rewriter constant + the merge helper

**Anchor:** the file currently has 1–7 doc-comment header lines listing the four callers of `ensure_claude_md_excludes`, then `use std::path::Path;` (line 8), then `ensure_claude_md_excludes` (lines 16–69).

#### 4.2.1 Extend the doc-block at top of file

Replace lines 1–7 with:

```rust
// Callers of `ensure_claude_md_excludes` and `ensure_rtk_pretool_hook` (must be
// kept in sync with any new agent-creation flow — see issue #84 for the original
// `ensure_claude_md_excludes` miss and issue #120 for the rtk extension):
//   - commands/agent_creator.rs::write_claude_settings_local (Tauri cmd; frontend: NewAgentModal.tsx + SessionItem.tsx ctx-menu)
//   - cli/create_agent.rs (CLI `create-agent --launch <id>`)
//   - commands/entity_creation.rs::create_agent_matrix
//   - commands/entity_creation.rs::create_workgroup (per-replica)
//
// The retroactive sweep (issue #120) is `commands/config.rs::sweep_rtk_hook`,
// which reuses `enumerate_managed_agent_dirs` below.
```

#### 4.2.2 Add `RTK_HOOK_MARKER` and `RTK_REWRITER_COMMAND` constants

**Insert after** the doc-block, **before** `use std::path::Path;` (current line 8). Two related constants:

- `RTK_HOOK_MARKER` — an immutable substring embedded in every AC-injected rewriter command. OFF-sweep removes any PreToolUse hook whose `command` contains this string; ON-sweep idempotency check uses the same. This protects users from stale hooks injected by an older AC version when the upstream rtk rewriter command evolves between AC releases (closes grinch M10).
- `RTK_REWRITER_COMMAND` — the canonical injected command. The marker is a JS string-expression statement at the very start of the `node -e "..."` argument: harmless to node (string literal in statement position), persists across upstream rtk-rewriter evolution. Mirrors `repo-AgentsCommander/.claude/settings.json` byte-for-byte (the source-of-truth file MUST be updated to embed the same marker, see §2 Files to touch and unit test #14 in §4.2.5).

```rust
/// Immutable substring embedded in every AC-injected rewriter command.
/// OFF-sweep removes any PreToolUse hook whose `command` contains this
/// string, regardless of whether the rest of the command matches the
/// current `RTK_REWRITER_COMMAND` byte-for-byte. ON-sweep also uses the
/// substring to skip insertion if any marker-bearing entry already
/// exists (preserves user customizations of the rewriter body across
/// AC upgrades).
///
/// MUST NEVER CHANGE. If the marker space ever needs to be retired,
/// bump to `@ac-rtk-marker-v2` AND keep `v1` in a new `RTK_LEGACY_MARKERS`
/// constant for OFF-sweep cleanup. v1 ships pre-launch — see §10
/// (migration) and grinch finding M10.
pub const RTK_HOOK_MARKER: &str = "@ac-rtk-marker-v1";

/// Canonical RTK PreToolUse rewriter command. The leading
/// `'@ac-rtk-marker-v1';` is a JS string-literal expression statement —
/// node treats it as a no-op (string in statement position). The marker
/// is never executed and never affects rewriter behavior; it exists
/// solely to identify "this hook is AC-injected" across AC upgrades
/// (see `RTK_HOOK_MARKER`).
///
/// Mirrors `repo-AgentsCommander/.claude/settings.json` (project-level
/// hook). Must stay byte-identical to that file; unit test #14 in §4.2.5
/// loads the source `.claude/settings.json` at test time and asserts
/// equality.
pub const RTK_REWRITER_COMMAND: &str = r#"node -e "'@ac-rtk-marker-v1';const s=JSON.parse(require('fs').readFileSync(0,'utf8'));const c=s?.tool_input?.command;if(!c){process.exit(0)}if(/^rtk\s/.test(c)||/&&\s*rtk\s/.test(c)){process.exit(0)}const skip=/^(cd |mkdir |echo |cat <<|source |export |\.|set )/.test(c);if(skip){process.exit(0)}const parts=c.split(/\s*(&&|\|\||;)\s*/);const out=parts.map((p,i)=>{if(i%2===1)return p;if(/^rtk\s/.test(p))return p;return 'rtk '+p}).join(' ');if(out!==c){console.log(JSON.stringify({decision:'modify',tool_input:{...s.tool_input,command:out}}))}else{process.exit(0)}""#;
```

`serde_json::Value::String` will JSON-escape both `"` and `\` on serialization; the on-disk encoding will match the source `.claude/settings.json` byte-for-byte. Unit test #14 (§4.2.5) locks this contract and catches drift in CI.

**Source-of-truth update — required.** Edit `repo-AgentsCommander/.claude/settings.json` line 9 to prefix the JS source with `'@ac-rtk-marker-v1';` so the file matches the constant. Without this edit, unit test #14 fails. The change is JS-inert (a no-op string-expression statement); rtk continues to function unchanged for any user already running Claude Code in that repo.

#### 4.2.3 Add `ensure_rtk_pretool_hook(dir, enabled)`

**Insert after** `ensure_claude_md_excludes` (after line 69).

**Contract (round 2).** The helper is **non-destructive on every parse failure or wrong-shape encounter**: it logs a warning and returns `Ok(())` without writing. It never overwrites a value whose meaning we cannot interpret. UTF-8 BOM is stripped before parsing on both ON and OFF read paths.

- ON-path:
  - Creates `.claude/` if missing.
  - Reads + BOM-strips + parses. If parse fails OR result is non-object → log + bail (no write). [grinch H1]
  - In `merge_rtk_hook`, if `hooks` exists and is non-object, OR `PreToolUse` exists and is non-array, OR a Bash-matcher's inner `hooks` exists and is non-array → log + bail (no write). [grinch H2]
  - **Idempotency by marker**: if any existing PreToolUse entry contains an inner hook whose `command` contains `RTK_HOOK_MARKER`, treat as already-applied — preserves user customizations of the rewriter body across AC upgrades.
  - Otherwise: locate a `Bash` matcher (or push a new one), append our `{type:"command", command:RTK_REWRITER_COMMAND}`.
- OFF-path:
  - If file missing → no-op.
  - Reads + BOM-strips + parses. If parse fails OR result is non-object → log + bail.
  - In `remove_rtk_hook`, filter every `hooks.PreToolUse[*].hooks` array, dropping entries whose `command` contains `RTK_HOOK_MARKER`. Wrong-shape branches inside the tree are skipped with a log warn (no destruction); other shapes are left untouched.
  - Cascade cleanup: empty inner-hooks → drop matcher entry; empty PreToolUse → drop key; empty hooks → drop key. File is NEVER deleted (other top-level keys like `claudeMdExcludes` may live alongside).

```rust
/// Merges (`enabled=true`) or removes (`enabled=false`) the RTK PreToolUse
/// rewriter hook in `<dir>/.claude/settings.local.json`. See module-level
/// docs and §4.2.3 in the plan for the full contract.
///
/// Non-destructive on every malformed input: bails with `log::warn!` and
/// returns `Ok(())` without modifying the file.
pub fn ensure_rtk_pretool_hook(dir: &Path, enabled: bool) -> Result<(), String> {
    if !dir.exists() {
        return Err(format!("Directory does not exist: {}", dir.display()));
    }

    let claude_dir = dir.join(".claude");
    let settings_path = claude_dir.join("settings.local.json");

    // OFF-path early exit: nothing to remove if file is missing.
    if !enabled && !settings_path.exists() {
        return Ok(());
    }

    // Read + BOM-strip + parse. On any parse failure or non-object root,
    // log + bail without modifying the file (grinch H1, M11).
    let mut obj: serde_json::Value = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)
            .map_err(|e| format!("Failed to read existing settings.local.json: {}", e))?;
        let cleaned = raw.strip_prefix('\u{feff}').unwrap_or(raw.as_str());
        match serde_json::from_str::<serde_json::Value>(cleaned) {
            Ok(v) if v.is_object() => v,
            _ => {
                log::warn!(
                    "[rtk] Skipping {} for {}: file is not a JSON object (preserved as-is)",
                    if enabled { "ON-sweep" } else { "OFF-sweep" },
                    settings_path.display()
                );
                return Ok(());
            }
        }
    } else {
        // ON-path with missing file — start with an empty doc.
        serde_json::json!({})
    };

    // For ON-path, ensure .claude/ exists before we eventually write back.
    if enabled && !claude_dir.exists() {
        std::fs::create_dir_all(&claude_dir)
            .map_err(|e| format!("Failed to create .claude directory: {}", e))?;
    }

    // Mutate. Both helpers return `true` if `obj` was changed (caller should
    // write back) and `false` if the call was a structural no-op or a
    // wrong-shape bail (caller must NOT write — keep user's file untouched).
    let mutated = if enabled {
        merge_rtk_hook(&mut obj, &settings_path)
    } else {
        remove_rtk_hook(&mut obj, &settings_path)
    };

    if !mutated {
        return Ok(());
    }

    let content = serde_json::to_string_pretty(&obj)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&settings_path, format!("{}\n", content))
        .map_err(|e| format!("Failed to write settings.local.json: {}", e))?;

    Ok(())
}

/// Idempotent merge by marker (preserves user customizations of the
/// rewriter body across AC upgrades — see grinch M10). Returns `true` iff
/// `obj` was modified (caller should write).
///
/// Bails with `log::warn!` and returns `false` on any wrong-shape value
/// (grinch H2). The user's file is preserved verbatim by the outer caller.
fn merge_rtk_hook(obj: &mut serde_json::Value, settings_path: &Path) -> bool {
    use serde_json::Value;

    let map = match obj.as_object_mut() {
        Some(m) => m,
        None => {
            // Outer guard already enforced object-ness; defensive only.
            log::warn!(
                "[rtk] ON-sweep: top-level value in {} is not an object; bailing",
                settings_path.display()
            );
            return false;
        }
    };

    // 'hooks' (if present) must be an object.
    if let Some(existing) = map.get("hooks") {
        if !existing.is_object() {
            log::warn!(
                "[rtk] ON-sweep: 'hooks' in {} is {} (expected object); bailing — preserving user data",
                settings_path.display(),
                discriminant_label(existing),
            );
            return false;
        }
    }
    let hooks_obj = map
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .expect("just inserted or pre-checked object");

    // 'PreToolUse' (if present) must be an array.
    if let Some(existing) = hooks_obj.get("PreToolUse") {
        if !existing.is_array() {
            log::warn!(
                "[rtk] ON-sweep: 'hooks.PreToolUse' in {} is {} (expected array); bailing",
                settings_path.display(),
                discriminant_label(existing),
            );
            return false;
        }
    }
    let pretool_arr = hooks_obj
        .entry("PreToolUse".to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("just inserted or pre-checked array");

    // Idempotency: ANY existing inner hook whose command contains the marker
    // means "already applied". This includes user-customized variants — we
    // do NOT overwrite their tweaks.
    for entry in pretool_arr.iter() {
        if let Some(inner) = entry.get("hooks").and_then(|v| v.as_array()) {
            for h in inner {
                if let Some(cmd) = h.get("command").and_then(|c| c.as_str()) {
                    if cmd.contains(RTK_HOOK_MARKER) {
                        return false; // already-applied no-op
                    }
                }
            }
        }
    }

    // Find a Bash matcher entry. If multiple exist (rare; user-created),
    // use the FIRST and leave the rest untouched.
    let bash_idx = pretool_arr.iter().position(|entry| {
        entry
            .get("matcher")
            .and_then(|m| m.as_str())
            .map(|s| s == "Bash")
            .unwrap_or(false)
    });

    let our_hook = serde_json::json!({
        "type": "command",
        "command": RTK_REWRITER_COMMAND,
    });

    match bash_idx {
        Some(idx) => {
            // Inner 'hooks' (if present) must be an array.
            if let Some(existing) = pretool_arr[idx].get("hooks") {
                if !existing.is_array() {
                    log::warn!(
                        "[rtk] ON-sweep: 'hooks.PreToolUse[{}].hooks' in {} is {} (expected array); bailing",
                        idx,
                        settings_path.display(),
                        discriminant_label(existing),
                    );
                    return false;
                }
            }
            let entry_obj = pretool_arr[idx]
                .as_object_mut()
                .expect("matcher entry is object (validated by the matcher::as_str above)");
            let inner = entry_obj
                .entry("hooks".to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("just inserted or pre-checked array");
            inner.push(our_hook);
        }
        None => {
            pretool_arr.push(serde_json::json!({
                "matcher": "Bash",
                "hooks": [our_hook],
            }));
        }
    }
    true
}

/// Removes every PreToolUse hook whose `command` contains `RTK_HOOK_MARKER`.
/// Returns `true` iff `obj` was modified (caller should write).
///
/// Wrong-shape branches inside the tree are SKIPPED with a log warn — never
/// destroyed. Other shapes (e.g. a non-Bash matcher entry, an entry with
/// no `hooks` key) are left untouched.
fn remove_rtk_hook(obj: &mut serde_json::Value, settings_path: &Path) -> bool {
    let map = match obj.as_object_mut() {
        Some(m) => m,
        None => return false,
    };

    if !map.contains_key("hooks") {
        return false;
    }
    let hooks_obj = match map.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        Some(o) => o,
        None => {
            log::warn!(
                "[rtk] OFF-sweep: 'hooks' in {} is non-object (preserved as-is)",
                settings_path.display()
            );
            return false;
        }
    };

    if !hooks_obj.contains_key("PreToolUse") {
        return false;
    }
    let pretool_arr = match hooks_obj.get_mut("PreToolUse").and_then(|v| v.as_array_mut()) {
        Some(a) => a,
        None => {
            log::warn!(
                "[rtk] OFF-sweep: 'hooks.PreToolUse' in {} is non-array (preserved as-is)",
                settings_path.display()
            );
            return false;
        }
    };

    let mut any_removed = false;
    for (idx, entry) in pretool_arr.iter_mut().enumerate() {
        if let Some(existing) = entry.get("hooks") {
            if !existing.is_array() {
                log::warn!(
                    "[rtk] OFF-sweep: 'hooks.PreToolUse[{}].hooks' in {} is non-array (preserved as-is for this entry)",
                    idx,
                    settings_path.display(),
                );
                continue;
            }
        } else {
            continue;
        }
        let inner = entry.get_mut("hooks").and_then(|v| v.as_array_mut()).unwrap();
        let before = inner.len();
        inner.retain(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|s| !s.contains(RTK_HOOK_MARKER))
                .unwrap_or(true) // keep entries that don't expose a string command
        });
        if inner.len() != before {
            any_removed = true;
        }
    }

    if !any_removed {
        return false;
    }

    // Drop matcher entries whose inner `hooks` is now empty.
    pretool_arr.retain(|entry| {
        entry
            .get("hooks")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(true) // keep entries with no `hooks` key (we didn't touch them)
    });

    // Cascade: empty PreToolUse → drop key. Empty hooks → drop key.
    if pretool_arr.is_empty() {
        hooks_obj.remove("PreToolUse");
    }
    if hooks_obj.is_empty() {
        map.remove("hooks");
    }

    true
}

/// Maps a `serde_json::Value` to a short discriminant label for log messages.
fn discriminant_label(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}
```

**MSRV note (grinch L15).** The `entry().or_insert_with(...).as_object_mut().expect(...)` chain compiles cleanly on rustc ≥ 1.71 (NLL borrow-check is sufficient). If the project's MSRV is older, dev-rust can split each chain into:

```rust
let needs_init = !map.get("hooks").map(|v| v.is_object()).unwrap_or(false);
if needs_init {
    map.insert("hooks".to_string(), Value::Object(serde_json::Map::new()));
}
let hooks_obj = map.get_mut("hooks").and_then(|v| v.as_object_mut()).unwrap();
```

Same shape for the `PreToolUse` and inner `hooks` blocks. Verify MSRV before writing the patch (also flagged in §11).

#### 4.2.4 Add `enumerate_managed_agent_dirs(project_paths)` — symlink / junction-aware

Append at the end of the file. Two correctness rules added vs. round 1 (closes grinch M7):

1. **Use `symlink_metadata` for the dir-check.** A symlink-to-dir has `metadata().is_dir() == true` (follows the link), but `symlink_metadata().is_dir() == false` AND `symlink_metadata().file_type().is_symlink() == true`. Rejecting symlinks here means a sweep cannot escape `project_paths` via Unix symlinks.
2. **Detect Windows NTFS junctions explicitly.** On stable Rust, `FileType::is_symlink()` does **not** detect NTFS junctions (`mklink /J`). Junction targets typically resolve to legitimate directories elsewhere on disk. We additionally check `FILE_ATTRIBUTE_REPARSE_POINT` via `MetadataExt::file_attributes()` to filter junctions explicitly.
3. **Canonicalize and dedupe.** Even after skipping symlinks/junctions, duplicate hardlinks or bind-mount-style filesystem layouts can yield two distinct paths pointing to the same file. We canonicalize each candidate and dedupe by canonical form so the sweep visits each underlying file exactly once.

```rust
/// Walks every `<project>/.ac-new/` and returns absolute paths to every
/// `_agent_*` matrix and every `__agent_*` replica (inside `wg-*` dirs).
///
/// Filters applied (grinch M7):
///   - `symlink_metadata` — Unix symlinks-to-dir are NOT followed.
///   - Windows NTFS junctions (FILE_ATTRIBUTE_REPARSE_POINT) are filtered.
///   - Canonical-path dedupe — duplicates resolved.
///
/// Skips silently: missing project paths, non-directory entries, unreadable
/// directories, paths that fail to canonicalize.
///
/// Order is filesystem-iteration order minus duplicates; not sorted. Callers
/// should not rely on stable ordering — sweep is idempotent per dir.
pub fn enumerate_managed_agent_dirs(project_paths: &[String]) -> Vec<std::path::PathBuf> {
    use std::collections::HashSet;
    use std::path::PathBuf;

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();

    let push_if_new = |raw: PathBuf, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>| {
        // Reject if the path is a symlink or junction.
        let md = match std::fs::symlink_metadata(&raw) {
            Ok(m) => m,
            Err(_) => return,
        };
        if md.file_type().is_symlink() {
            return;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return;
            }
        }
        if !md.is_dir() {
            return;
        }

        // Canonicalize; if it fails (race, permissions), skip.
        let canonical = match std::fs::canonicalize(&raw) {
            Ok(c) => c,
            Err(_) => return,
        };
        if seen.insert(canonical) {
            out.push(raw);
        }
    };

    for project in project_paths {
        let ac_new = std::path::Path::new(project).join(".ac-new");
        if !ac_new.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&ac_new) {
            Ok(e) => e,
            Err(e) => {
                log::warn!(
                    "[rtk-sweep] Cannot read {} for replica enumeration: {}",
                    ac_new.display(),
                    e
                );
                continue;
            }
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            if name.starts_with("_agent_") {
                push_if_new(p, &mut out, &mut seen);
                continue;
            }

            if name.starts_with("wg-") {
                // Re-check wg-* itself isn't a junction.
                let md = match std::fs::symlink_metadata(&p) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if md.file_type().is_symlink() {
                    continue;
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
                    if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        continue;
                    }
                }
                if !md.is_dir() {
                    continue;
                }

                let wg_entries = match std::fs::read_dir(&p) {
                    Ok(e) => e,
                    Err(e) => {
                        log::warn!(
                            "[rtk-sweep] Cannot read workgroup {} for replica enumeration: {}",
                            p.display(),
                            e
                        );
                        continue;
                    }
                };
                for wg_entry in wg_entries.flatten() {
                    let rp = wg_entry.path();
                    let rname = match rp.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if rname.starts_with("__agent_") {
                        push_if_new(rp, &mut out, &mut seen);
                    }
                }
            }
        }
    }
    out
}
```

The closure-based dedupe keeps each filter rule local and DRY. dev-rust may refactor into a private free function if borrow-checker friction arises with the dual `&mut` captures.

#### 4.2.5 Unit tests

Append a `#[cfg(test)] mod tests { ... }` block at the end of the file. Each `ensure_rtk_pretool_hook` test creates a tempdir, writes a starting `.claude/settings.local.json` (or skips for missing-file cases), invokes `ensure_rtk_pretool_hook(dir, enabled)`, and asserts the resulting JSON via `serde_json::Value` **structural** equality (grinch L12 — not byte-equality, since `to_string_pretty` re-formats whitespace).

Required cases:

1. `enabled=true`, no file → file created with the canonical hook tree.
2. `enabled=false`, no file → file NOT created (no-op).
3. `enabled=true`, file `{}` → adds the full hook tree, preserves no other keys.
4. `enabled=true`, file `{"claudeMdExcludes":["x"]}` → adds hook tree, preserves `claudeMdExcludes`.
5. `enabled=true`, file with PreToolUse entry where `matcher=="Read"` → new Bash entry pushed alongside, Read entry preserved.
6. `enabled=true`, file with PreToolUse Bash matcher containing OTHER hooks (e.g. some custom rewriter) → our entry appended to inner hooks, other entries preserved.
7. `enabled=true`, file already containing a marker-bearing entry → no-op (no duplicate appended). Asserts via structural `Value` equality (grinch L12).
8. `enabled=false`, file with our entry alongside an unrelated Bash hook → our entry removed, unrelated preserved, structure untouched.
9. `enabled=false`, file with ONLY our entry → matcher entry dropped, PreToolUse key dropped, hooks key dropped; remaining file is `{}` (or `{"claudeMdExcludes":...}` if it was there).
10. **(INVERTED — was destructive in round 1, see grinch H1)** `enabled=true`, file with malformed JSON (`{ invalid`) → file content **unchanged** on disk; function returns `Ok(())`; a `log::warn!` was emitted (verify via the `log` crate's testing facade or by capturing stderr).
11. **Byte-equality test on the constant payload**: write hook with `enabled=true` to an empty dir, parse the resulting JSON, assert `RTK_REWRITER_COMMAND` appears verbatim as the `command` field's decoded string. Locks the contract that future reformatting of the constant cannot break the marker substring.

Tests for `enumerate_managed_agent_dirs`:

12. Build a tempdir with `proj/.ac-new/{_agent_one, wg-1-team/{__agent_two, __agent_three, repo-x}, _team_team}`; assert exactly `[_agent_one, __agent_two, __agent_three]` are returned. Confirms `repo-*`, `_team_*`, and non-dir entries are filtered.

New tests added in round 2:

13. **(grinch §12.3.1, OFF + malformed)** `enabled=false`, file with malformed JSON → file content unchanged on disk; function returns `Ok(())`; a `log::warn!` was emitted.
14. **(grinch §12.3.5, source-of-truth)** Read `repo-AgentsCommander/.claude/settings.json` at test time (relative to `CARGO_MANIFEST_DIR`); JSON-decode `hooks.PreToolUse[0].hooks[0].command`; assert byte-equal to `RTK_REWRITER_COMMAND`. Locks both files in lockstep — catches drift if either is edited without the other. Code skeleton given in §12.3.5. **Round-3 N4 implementation note:** the `RTK_REWRITER_COMMAND` constant edit AND the `repo-AgentsCommander/.claude/settings.json` marker prefix edit MUST land in the **same commit**. Splitting them across commits breaks `cargo test` on the intermediate commit and harms bisectability.
15. **(grinch H2, wrong-shape `hooks`)** `enabled=true`, file `{"hooks":null}` → file unchanged on disk; warn logged. Repeat with `{"hooks":"string"}` and `{"hooks":42}`.
16. **(grinch H2, wrong-shape `PreToolUse`)** `enabled=true`, file `{"hooks":{"PreToolUse":"string"}}` → file unchanged on disk; warn logged. Repeat with `{"hooks":{"PreToolUse":{}}}`.
17. **(grinch H2, wrong-shape inner `hooks`)** `enabled=true`, file `{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":"string"}]}}` → file unchanged on disk; warn logged.
18. **(grinch H2 OFF mirror)** `enabled=false`, file `{"hooks":{"PreToolUse":"string"}}` → file unchanged; warn logged. **Same precision matters even on OFF**: we never destroy a wrong-shape value, even in the absence of a marker-bearing entry to remove.
19. **(grinch M7, junction/symlink)** Build a tempdir on Windows with a junction (`mklink /J target source`) from `wg-1-team/__agent_X` to a target outside `project_paths`; on Unix use a symlink. Assert `enumerate_managed_agent_dirs` does NOT include the linked path. (Skip the test gracefully on platforms where the link cannot be created; this is conditional via `#[cfg]` and a runtime check.)
20. **(grinch M7, dedupe)** Build a tempdir with two paths that canonicalize to the same target (e.g. via UNC/short-name vs. long-name on Windows, or by passing the same project twice in `project_paths`); assert each canonical path is returned exactly once.
21. **(grinch M10, marker idempotency across constant evolution)** Manually write a `.claude/settings.local.json` containing `{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"node -e \"'@ac-rtk-marker-v1'; /* OLDER REWRITER BODY */ \""}]}]}}` — i.e. a hook with the marker but a different body. Run `enabled=true`: assert no-op (idempotency by marker). Run `enabled=false`: assert the older entry is removed (filter by marker substring).
22. **(grinch M11, BOM)** `enabled=true`, file content is `\u{feff}{"claudeMdExcludes":[]}` (with a leading UTF-8 BOM) → ON-sweep adds the rtk hook successfully; the BOM is dropped on write (acceptable — file remains valid UTF-8). Repeat for `enabled=false` with a marker-bearing entry — entry is removed, BOM dropped.

The full test count is 22 (10 ON, 6 OFF semantics + 6 enumerator/cross-cutting).

### 4.3 `src-tauri/Cargo.toml` — add `which`

**Anchor:** the `[dependencies]` block ends at line 31 (`tauri-plugin-dialog = "2.6.0"`). The `[target.'cfg(windows)'.dependencies]` block begins line 33.

Add **immediately before** line 33, inside `[dependencies]`:

```toml
which = "7"
```

The `which` crate (v7) is portable across Windows/macOS/Linux: on Windows it walks `%PATH%` and tries `.exe`/`.cmd`/`.bat` extensions; on Unix it stat-checks executable bits. This avoids OS-specific shell-out (`where rtk` on Windows vs `which rtk` on Unix) and the related quoting / `creation_flags` ceremony.

### 4.4 `src-tauri/src/commands/config.rs` — narrow setters + sweep + getter

**Anchor:** the file currently ends at line 142. All new code appends after `get_instance_label` (line 138–141).

#### 4.4.1 Imports + result types

**Add** alongside the existing imports at the top:

```rust
use crate::config::claude_settings::{enumerate_managed_agent_dirs, ensure_rtk_pretool_hook};
use crate::{RtkSweepLockState, RtkStartupModeState};
```

**Add** the result types (after the `use` block, before the first command):

```rust
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtkSweepResult {
    pub total: u32,
    pub succeeded: u32,
    pub errors: Vec<RtkSweepError>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtkSweepError {
    pub path: String,
    pub error: String,
}
```

#### 4.4.2 Narrow setters — `set_inject_rtk_hook`, `set_rtk_prompt_dismissed` (closes grinch H3 + H4)

These commands hold the `SettingsState` write lock through the `save_settings` call, eliminating the IPC-level read-modify-write race that exists in any frontend `get` + `update` round-trip when two writers run concurrently. The banner uses these instead of `get` + `update` (§5.4).

```rust
/// Narrow setter — flips ONLY `inject_rtk_hook`. **Holds the SettingsState
/// write lock through `save_settings`** so a concurrent `update_settings`
/// from the SettingsModal cannot overwrite the change at the IPC boundary
/// (grinch H3 + N1). The explicit `drop(s)` after `save_settings` keeps the
/// guard scope visually unambiguous: lock-then-write-then-release. Caller
/// is responsible for triggering `sweep_rtk_hook` if disk side-effects on
/// replicas are desired.
#[tauri::command]
pub async fn set_inject_rtk_hook(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.inject_rtk_hook = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(())
}

/// Narrow setter — flips ONLY `rtk_prompt_dismissed`. Same lock-held-through-save
/// pattern as `set_inject_rtk_hook` (grinch H3 + N1).
#[tauri::command]
pub async fn set_rtk_prompt_dismissed(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.rtk_prompt_dismissed = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(())
}
```

**Note for grinch awareness, not in scope here:** the existing `update_settings` reads `root_token` outside the write lock (line 38 of `commands/config.rs`). That is a pre-existing race separate from #120 — flagged here for tech-lead's awareness; addressing it is a follow-up that does not affect the rtk fix.

#### 4.4.3 The sweep command (acquires `RtkSweepLockState` — closes grinch M8)

Append at end of file. The lock is held for the entire duration of the per-dir loop, ensuring that no concurrent `entity_creation::create_workgroup`, `agent_creator::write_claude_settings_local`, or peer sweep can interleave a read-modify-write on the same `.claude/settings.local.json`.

```rust
/// Sweep every AC-managed agent directory and apply
/// `ensure_rtk_pretool_hook(dir, enabled)`. Best-effort per directory:
/// per-dir failures are logged + appended to `errors` and the sweep
/// continues. Reads `project_paths` from the live `SettingsState` (avoids a
/// disk-read race against `save_settings`).
///
/// Acquires `RtkSweepLockState` for the entire loop — eliminates the
/// in-process race vs. concurrent `ensure_claude_md_excludes` /
/// `ensure_rtk_pretool_hook` calls from `entity_creation` /
/// `agent_creator` (grinch M8). Cross-process races (two AC instances)
/// remain documented in §7.4.
///
/// Frontend contract: see §6 behavior matrix.
#[tauri::command]
pub async fn sweep_rtk_hook(
    settings: State<'_, SettingsState>,
    sweep_lock: State<'_, RtkSweepLockState>,
    enabled: bool,
) -> Result<RtkSweepResult, String> {
    let _guard = sweep_lock.lock().await;

    let project_paths: Vec<String> = {
        let s = settings.read().await;
        s.project_paths.clone()
    };

    let dirs = enumerate_managed_agent_dirs(&project_paths);
    let total = dirs.len() as u32;
    let mut succeeded: u32 = 0;
    let mut errors: Vec<RtkSweepError> = Vec::new();

    for dir in dirs {
        match ensure_rtk_pretool_hook(&dir, enabled) {
            Ok(()) => {
                succeeded += 1;
            }
            Err(e) => {
                log::warn!(
                    "[rtk-sweep] Failed to apply (enabled={}) to {}: {}",
                    enabled,
                    dir.display(),
                    e
                );
                errors.push(RtkSweepError {
                    path: dir.to_string_lossy().to_string(),
                    error: e,
                });
            }
        }
    }

    log::info!(
        "[rtk-sweep] enabled={} total={} succeeded={} errors={}",
        enabled,
        total,
        succeeded,
        errors.len()
    );

    Ok(RtkSweepResult {
        total,
        succeeded,
        errors,
    })
}
```

### 4.5 `src-tauri/src/lib.rs` — register the commands + `RtkSweepLockState` + startup detection

#### 4.5.1 Register the four new commands

**Anchor:** `invoke_handler!` block at lines 701–760. The settings cluster is `commands::config::get_settings, commands::config::update_settings,` (lines 713–714).

**Insert** right after line 714 (before `commands::repos::search_repos,`):

```rust
            commands::config::set_inject_rtk_hook,
            commands::config::set_rtk_prompt_dismissed,
            commands::config::sweep_rtk_hook,
            commands::config::get_rtk_startup_status,
```

#### 4.5.1b Define and register `RtkSweepLockState` (closes grinch M8) and `RtkStartupModeState` (closes §18 / phase-B-bug)

Two new State types share this section because they have identical wiring shape (define alias, initialize, clone for setup, manage).

**Add** at the top of `lib.rs` (alongside other `pub type` State aliases, e.g. `SettingsState`, `DetachedSessionsState`):

```rust
pub type RtkSweepLockState = Arc<tokio::sync::Mutex<()>>;

/// Cached boot-time RTK startup mode. Set ONCE by the setup task in
/// `lib.rs` (§4.5.2) after computing the mode + running side effects.
/// Read by `get_rtk_startup_status` (§4.5.3) so the getter returns the
/// boot decision instead of recomputing from the post-side-effect state
/// (which would mismatch the listener for the `auto-disabled` mode —
/// see §18 amendment).
pub type RtkStartupModeState = Arc<std::sync::OnceLock<String>>;
```

**Initialize** alongside other state objects in `run()` (near line 256 where `voice_tracking` is built):

```rust
let rtk_sweep_lock: RtkSweepLockState = Arc::new(tokio::sync::Mutex::new(()));
let rtk_sweep_lock_for_setup = Arc::clone(&rtk_sweep_lock);

let rtk_startup_mode: RtkStartupModeState = Arc::new(std::sync::OnceLock::new());
let rtk_startup_mode_for_setup = Arc::clone(&rtk_startup_mode);
```

**Manage** both states on the Tauri builder (alongside other `.manage(...)` calls near line 270):

```rust
.manage(rtk_sweep_lock)
.manage(rtk_startup_mode)
```

The `_for_setup` clones are captured by the startup detection task (§4.5.2) — without them, the task cannot reach either state since the originals are moved into `.manage`.

**Why `std::sync::OnceLock<String>` (and not `tokio::sync::OnceCell` or `RwLock<Option<…>>`).** OnceLock matches the "set once at boot, read many times for the lifetime of the process" lifecycle exactly. `set()` is sync and infallible-after-first-call (returns `Err` on subsequent calls, which we ignore). `get()` is sync and free of any locking — fast path for every banner mount and every `get_rtk_startup_status` invocation. No contention, no async, no panic risk.

#### 4.5.2 Startup detection task

**Anchor:** the `setup(move |app| { ... })` closure begins at line 276. The first action inside is storing `AppHandle` in a `OnceLock` (line 281). The web-server boot is at lines 309–331.

Add a new block **after** line 331 (after the web-server boot) and **before** the existing session-restoration block. The block spawns a `tokio` task that probes for `rtk` in PATH, reads the latest settings, and emits one of three Tauri events. It then conditionally re-applies the sweep if `injectRtkHook` is true (so a mid-sweep crash on a previous boot heals automatically).

Code to insert. The closure captures `rtk_sweep_lock_for_setup` (cloned in §4.5.1b). Two grinch fixes are baked in:

- **H4 fix.** The auto-disable persists `inject_rtk_hook=false` while **holding the SettingsState write lock through `save_settings`**, mirroring `update_settings`. Disk and memory cannot disagree even if a concurrent `update_settings` from the user fires during the boot window.
- **M8 fix.** Both the auto-disable sweep and the active-recovery sweep acquire `RtkSweepLockState` for the duration of the loop, blocking concurrent `entity_creation` / `agent_creator` / `sweep_rtk_hook` writers on the same files.

```rust
            // RTK startup detection (issue #120). Probes PATH for `rtk`, then:
            //   - rtk found AND inject_rtk_hook=false AND rtk_prompt_dismissed=false
            //       → emit "rtk_startup_status" with mode="prompt-enable"
            //   - rtk found AND inject_rtk_hook=true
            //       → emit "rtk_startup_status" with mode="active"
            //         + run a sweep with enabled=true (idempotent recovery).
            //   - rtk missing AND inject_rtk_hook=true
            //       → persist inject_rtk_hook=false (write lock held through save —
            //         grinch H4); sweep with enabled=false (RtkSweepLock held —
            //         grinch M8); emit "rtk_startup_status" with mode="auto-disabled".
            //   - otherwise: emit "rtk_startup_status" with mode="silent"
            //         (frontend treats as no-op; emitted for observability).
            // Detached so the rest of setup is not blocked by disk I/O.
            {
                let app_handle_for_rtk = app.handle().clone();
                let sweep_lock = Arc::clone(&rtk_sweep_lock_for_setup);
                let mode_cache = Arc::clone(&rtk_startup_mode_for_setup);
                tauri::async_runtime::spawn(async move {
                    use crate::config::claude_settings::{enumerate_managed_agent_dirs, ensure_rtk_pretool_hook};

                    let rtk_present = which::which("rtk").is_ok();

                    let settings_state = app_handle_for_rtk
                        .state::<crate::config::settings::SettingsState>();

                    let (inject_enabled, prompt_dismissed) = {
                        let s = settings_state.read().await;
                        (s.inject_rtk_hook, s.rtk_prompt_dismissed)
                    };

                    let mode: &'static str = match (rtk_present, inject_enabled, prompt_dismissed) {
                        (true, false, false) => "prompt-enable",
                        (true, true, _) => "active",
                        (false, true, _) => "auto-disabled",
                        _ => "silent",
                    };

                    // Cache the boot decision BEFORE running side effects (§18
                    // amendment). The getter `get_rtk_startup_status` reads from
                    // this cache instead of recomputing from current state — so
                    // a banner mounting after the auto-disable side-effect still
                    // sees "auto-disabled" rather than the recomputed "silent".
                    // `set` returns Err if already set; we ignore (idempotent).
                    let _ = mode_cache.set(mode.to_string());

                    if mode == "auto-disabled" {
                        // H4 + N1 fix: hold the SettingsState write lock through
                        // save_settings so a concurrent update_settings cannot
                        // land between our in-memory flip and the disk persist.
                        // The lock is released explicitly via drop(s) AFTER the
                        // save returns, mirroring the narrow-setter pattern in
                        // commands/config.rs.
                        let mut s = settings_state.write().await;
                        s.inject_rtk_hook = false;
                        let snapshot = s.clone();
                        if let Err(e) = crate::config::settings::save_settings(&snapshot) {
                            log::warn!("[rtk-startup] Failed to persist auto-disable: {}", e);
                        }
                        let project_paths = snapshot.project_paths.clone();
                        drop(s); // explicit; lock released AFTER the disk write

                        // M8 fix: hold RtkSweepLock through the OFF-sweep loop.
                        let _guard = sweep_lock.lock().await;
                        for dir in enumerate_managed_agent_dirs(&project_paths) {
                            if let Err(e) = ensure_rtk_pretool_hook(&dir, false) {
                                log::warn!(
                                    "[rtk-startup] auto-disable sweep failed for {}: {}",
                                    dir.display(),
                                    e
                                );
                            }
                        }
                    } else if mode == "active" {
                        // M8 fix: hold RtkSweepLock through the ON-sweep loop.
                        let project_paths = {
                            let s = settings_state.read().await;
                            s.project_paths.clone()
                        };
                        let _guard = sweep_lock.lock().await;
                        for dir in enumerate_managed_agent_dirs(&project_paths) {
                            if let Err(e) = ensure_rtk_pretool_hook(&dir, true) {
                                log::warn!(
                                    "[rtk-startup] active recovery sweep failed for {}: {}",
                                    dir.display(),
                                    e
                                );
                            }
                        }
                    }

                    let _ = tauri::Emitter::emit(
                        &app_handle_for_rtk,
                        "rtk_startup_status",
                        serde_json::json!({ "mode": mode }),
                    );

                    log::info!(
                        "[rtk-startup] mode={} rtkPresent={} injectEnabled={} promptDismissed={}",
                        mode,
                        rtk_present,
                        inject_enabled,
                        prompt_dismissed
                    );
                });
            }
```

**Notes:**

- The task is detached: `setup()` returns immediately, the rest of the app boots in parallel. The frontend banner subscribes to `rtk_startup_status` AND immediately calls `get_rtk_startup_status` (§5.4) so a fast-emit before-mount cannot orphan the message.
- `which::which` is sync and cheap; running it inside `tauri::async_runtime::spawn` is fine.
- The two `_guard` bindings drop at end-of-block. Total lock-hold time is bounded by the sweep latency (typically sub-second; see grinch L14 for an mtime-skip follow-up).

#### 4.5.3 Add a sync getter for late-mounting frontend (reads cached boot mode — §18)

Append to `src-tauri/src/commands/config.rs`:

```rust
/// Returns the BOOT-TIME RTK startup decision computed by the setup task in
/// `lib.rs` (§4.5.2) and cached in `RtkStartupModeState`. This is the SAME
/// value the setup task emitted via `rtk_startup_status` — so the listener
/// (M5 fix in §5.4) and the getter always agree, even after the auto-disable
/// side-effect mutates settings (§18 amendment).
///
/// If called BEFORE the setup task has finished (extremely narrow boot
/// window — `which::which` resolve + a state read), returns "silent".
/// The listener will fire shortly after with the actual mode; combined with
/// idempotent `setMode` on the frontend, the banner self-corrects.
///
/// Pure read — does NOT auto-disable, does NOT sweep, does NOT probe PATH.
#[tauri::command]
pub async fn get_rtk_startup_status(
    mode_cache: State<'_, RtkStartupModeState>,
) -> Result<String, String> {
    Ok(mode_cache
        .get()
        .cloned()
        .unwrap_or_else(|| "silent".to_string()))
}
```

Registration of all four new commands (`set_inject_rtk_hook`, `set_rtk_prompt_dismissed`, `sweep_rtk_hook`, `get_rtk_startup_status`) is in §4.5.1.

**Why a cache instead of recomputing.** The setup task COMPUTES the mode from then-current settings, then RUNS SIDE EFFECTS that mutate those settings (e.g. auto-disable persists `inject_rtk_hook=false`). A naïve recompute-on-read getter would return a different mode after the side effects than the one the listener received — the bug described in §18. The cache pins the boot decision so listener and getter always agree.

### 4.6 Wire the helper into the four existing call sites — with `RtkSweepLockState` (closes grinch M8)

Pattern: in every in-process site that calls `ensure_claude_md_excludes(&dir)`, **acquire `RtkSweepLockState` for the entire helper sequence** (both `ensure_claude_md_excludes` and `ensure_rtk_pretool_hook`). The lock blocks any concurrent sweep / banner-driven write / peer entity-creation flow from interleaving on the same file. CLI is the one exception — it runs out-of-process and cannot share the in-process tokio Mutex (see §7.4).

Source value of `inject_rtk_hook` differs per site (in-memory `SettingsState` vs. `load_settings()` for the CLI). Error policy is uniform: `log::warn!`, never propagate.

#### 4.6.1 `src-tauri/src/commands/agent_creator.rs::write_claude_settings_local`

**Current** (lines 59–62):

```rust
#[tauri::command]
pub async fn write_claude_settings_local(agent_path: String) -> Result<(), String> {
    crate::config::claude_settings::ensure_claude_md_excludes(&PathBuf::from(&agent_path))
}
```

**After** — acquires `RtkSweepLockState` around the helper sequence (M8). Add `settings: State<'_, SettingsState>` AND `sweep_lock: State<'_, RtkSweepLockState>`. Tauri auto-injects both — the frontend `invoke()` call stays unchanged.

```rust
#[tauri::command]
pub async fn write_claude_settings_local(
    settings: tauri::State<'_, crate::config::settings::SettingsState>,
    sweep_lock: tauri::State<'_, crate::RtkSweepLockState>,
    agent_path: String,
) -> Result<(), String> {
    let dir = PathBuf::from(&agent_path);
    let inject = settings.read().await.inject_rtk_hook;
    let _guard = sweep_lock.lock().await;
    crate::config::claude_settings::ensure_claude_md_excludes(&dir)?;
    if let Err(e) = crate::config::claude_settings::ensure_rtk_pretool_hook(&dir, inject) {
        log::warn!(
            "[agent_creator] Failed to apply rtk hook (enabled={}) to {}: {}",
            inject,
            dir.display(),
            e
        );
    }
    Ok(())
}
```

#### 4.6.2 `src-tauri/src/cli/create_agent.rs`

**Current** (lines 134–140):

```rust
match agent_config {
    Some(agent) => {
        // Auto-generate .claude/settings.local.json if the agent has the flag
        if agent.exclude_global_claude_md {
            if let Err(e) = config::claude_settings::ensure_claude_md_excludes(&agent_dir) {
                eprintln!("Warning: failed to write claude settings: {}", e);
            }
        }
```

**After** — insert the rtk call right after the exclude block. `settings` is already in scope (line 123). Use `settings.inject_rtk_hook` directly.

```rust
match agent_config {
    Some(agent) => {
        // Auto-generate .claude/settings.local.json if the agent has the flag
        if agent.exclude_global_claude_md {
            if let Err(e) = config::claude_settings::ensure_claude_md_excludes(&agent_dir) {
                eprintln!("Warning: failed to write claude settings: {}", e);
            }
        }
        // Issue #120 — apply the rtk hook based on the global toggle.
        if let Err(e) = config::claude_settings::ensure_rtk_pretool_hook(
            &agent_dir,
            settings.inject_rtk_hook,
        ) {
            eprintln!("Warning: failed to apply rtk hook: {}", e);
        }
```

**Locking note (M8 scope).** The CLI flow runs **out-of-process** and cannot share the in-process `RtkSweepLockState` with a running AC instance. The cross-process race between a CLI `create-agent --launch` and the running app's sweep is structurally identical to the existing race documented in §7.4 (`ensure_claude_md_excludes` cross-process). Closing it requires file-based locking (e.g. `fs2::FileExt::lock_exclusive` on `.claude/settings.local.json` itself), which is **out of scope for #120** — flagged in §11 as a follow-up. dev-rust does NOT acquire `RtkSweepLockState` here; the CLI binary cannot reach the running app's Tauri State anyway.

#### 4.6.3 `src-tauri/src/commands/entity_creation.rs::create_agent_matrix`

**Current** (lines 244–261, the "Issue #84" block at end of function):

```rust
    let exclude_claude_md = {
        let s = settings.read().await;
        s.agents.iter().any(|a| a.exclude_global_claude_md)
    };
    if exclude_claude_md {
        if let Err(e) = ensure_claude_md_excludes(&agent_dir) {
            log::warn!(
                "[entity_creation] Failed to write .claude/settings.local.json for {}: {}",
                agent_dir.display(),
                e
            );
        }
    }
```

**After** — extend the snapshot read to pull `inject_rtk_hook`, **acquire `RtkSweepLockState`** around the helper sequence (M8), then call both helpers.

Add `sweep_lock: State<'_, crate::RtkSweepLockState>` to the function signature alongside the existing `settings: State<...>`.

```rust
    let (exclude_claude_md, inject_rtk_hook) = {
        let s = settings.read().await;
        (
            s.agents.iter().any(|a| a.exclude_global_claude_md),
            s.inject_rtk_hook,
        )
    };
    {
        let _guard = sweep_lock.lock().await;
        if exclude_claude_md {
            if let Err(e) = ensure_claude_md_excludes(&agent_dir) {
                log::warn!(
                    "[entity_creation] Failed to write .claude/settings.local.json for {}: {}",
                    agent_dir.display(),
                    e
                );
            }
        }
        // Issue #120 — apply the rtk hook based on the global toggle. Called
        // unconditionally; the helper no-ops when enabled=false and the file
        // does not exist.
        if let Err(e) = crate::config::claude_settings::ensure_rtk_pretool_hook(
            &agent_dir,
            inject_rtk_hook,
        ) {
            log::warn!(
                "[entity_creation] Failed to apply rtk hook for matrix {}: {}",
                agent_dir.display(),
                e
            );
        }
    }
```

#### 4.6.4 `src-tauri/src/commands/entity_creation.rs::create_workgroup`

**Current** (lines 569–576, the snapshot-once-before-the-loop block):

```rust
    let exclude_claude_md = {
        let s = settings.read().await;
        s.agents.iter().any(|a| a.exclude_global_claude_md)
    };
```

**After** — extend the snapshot:

```rust
    let (exclude_claude_md, inject_rtk_hook) = {
        let s = settings.read().await;
        (
            s.agents.iter().any(|a| a.exclude_global_claude_md),
            s.inject_rtk_hook,
        )
    };
```

**Inside the per-replica loop** (current lines 600–609 — the "Issue #84" block):

```rust
        // Issue #84 — write .claude/settings.local.json if any agent has the flag.
        if exclude_claude_md {
            if let Err(e) = ensure_claude_md_excludes(&replica_dir) {
                log::warn!(
                    "[entity_creation] Failed to write .claude/settings.local.json for replica {}: {}",
                    replica_dir.display(),
                    e
                );
            }
        }
```

**After** — wrap each replica's helper sequence in a scoped `RtkSweepLock` guard (M8). Releasing the lock between replicas keeps contention low while still serializing per-file work against any concurrent sweep.

Add `sweep_lock: State<'_, crate::RtkSweepLockState>` to the function signature alongside the existing `settings: State<...>`.

```rust
        {
            let _guard = sweep_lock.lock().await;
            // Issue #84 — write .claude/settings.local.json if any agent has the flag.
            if exclude_claude_md {
                if let Err(e) = ensure_claude_md_excludes(&replica_dir) {
                    log::warn!(
                        "[entity_creation] Failed to write .claude/settings.local.json for replica {}: {}",
                        replica_dir.display(),
                        e
                    );
                }
            }
            // Issue #120 — apply the rtk hook based on the global toggle.
            if let Err(e) = crate::config::claude_settings::ensure_rtk_pretool_hook(
                &replica_dir,
                inject_rtk_hook,
            ) {
                log::warn!(
                    "[entity_creation] Failed to apply rtk hook for replica {}: {}",
                    replica_dir.display(),
                    e
                );
            }
        }
```

**Locking granularity.** The lock is acquired per-replica, not over the entire `for agent_path in &team_agents` loop. Per-replica scoping keeps the critical section short (~one file's read+write) so a long workgroup creation does not block a concurrent sweep for the entire duration. The trade-off: in the same workgroup, two replicas cannot be processed in parallel — which is fine because the loop is sequential anyway.

The snapshot-once-before-the-loop pattern is intentional: any mid-loop toggle of `injectRtkHook` is ignored, mirroring the same decision documented for `exclude_claude_md` in the Issue #84 plan.

### 4.7 Phase A summary

After Phase A:

- `cargo check` and `cargo test` are green.
- New settings fields persist via existing `update_settings`.
- `sweep_rtk_hook` and `get_rtk_startup_status` Tauri commands are callable from the frontend.
- 4 existing creation flows wire the helper.
- Startup task probes rtk, runs auto-disable / recovery sweep, emits `rtk_startup_status`.
- Frontend receives the new fields silently (no UI consumer yet).

---

## 5. Phase B — Frontend

**dev-webpage-ui round-2 reading list.** Three grinch findings are addressed in this section's components (closed pre-implementation; marked inline so you know where each one lives):

- **M5 — banner mount snapshot-then-listen ordering.** §5.4: subscribe to `rtk_startup_status` BEFORE calling `getRtkStartupStatus`. Ordering matters because the backend setup task may emit during your `onMount`.
- **M6 — per-replica sweep errors silently dropped.** §5.3 (handleSave) and §5.4 (banner Enable) both inspect `RtkSweepResult.errors[]` and `console.error` per-dir failures. Cheap mitigation; toast surface is future work.
- **M9 — rapid-toggle produces silent partial state.** §5.3 + §5.4 both gate the relevant button (Save / Enable) and the rtk checkbox via an `rtkSweepInFlight` / `busy` signal during in-flight sweeps.

If your frontend implementation diverges from any of these, push back through the team — these are not optional polish.

### 5.1 `src/shared/types.ts` — extend `AppSettings`

**Anchor:** the `AppSettings` interface ends at line 157 with `coordSortByActivity: boolean;`.

Add the two fields **immediately after** line 156:

```ts
  coordSortByActivity: boolean;
  injectRtkHook: boolean;
  rtkPromptDismissed: boolean;
}
```

### 5.2 `src/shared/ipc.ts` — extend `SettingsAPI`

**Anchor:** the `SettingsAPI` const at lines 111–119.

**Replace** with:

```ts
export const SettingsAPI = {
  get: () => transport.invoke<AppSettings>("get_settings"),
  update: (settings: AppSettings) =>
    transport.invoke<void>("update_settings", { newSettings: settings }),
  openWebRemote: () => transport.invoke<void>("open_web_remote"),
  startWebServer: () => transport.invoke<boolean>("start_web_server"),
  stopWebServer: () => transport.invoke<boolean>("stop_web_server"),
  getWebServerStatus: () => transport.invoke<boolean>("get_web_server_status"),
  // Narrow setters — hold the SettingsState write lock through save_settings
  // (grinch H3). Use these instead of get+update from the banner.
  setInjectRtkHook: (value: boolean) =>
    transport.invoke<void>("set_inject_rtk_hook", { value }),
  setRtkPromptDismissed: (value: boolean) =>
    transport.invoke<void>("set_rtk_prompt_dismissed", { value }),
  sweepRtkHook: (enabled: boolean) =>
    transport.invoke<RtkSweepResult>("sweep_rtk_hook", { enabled }),
  getRtkStartupStatus: () =>
    transport.invoke<"prompt-enable" | "active" | "auto-disabled" | "silent">(
      "get_rtk_startup_status"
    ),
};
```

Add the result type **alongside** the existing imports at the top of `ipc.ts`:

```ts
export interface RtkSweepResult {
  total: number;
  succeeded: number;
  errors: { path: string; error: string }[];
}
```

### 5.3 `src/sidebar/components/SettingsModal.tsx` — General tab checkbox

**Anchor:** `renderGeneralTab()` at line 248. The "Window" section ends at line 329 with the `raiseTerminalOnClick` checkbox. The "Web Remote Access" section follows at line 331.

**Persistence model (closes §9 Q5 — was open in round 1).** dev-rust verified that `updateField` in `SettingsModal.tsx` (lines 70–76) is **local-only** — it mutates the form draft via `setSettings`. Persistence happens at `handleSave` (lines 215–238) via `await SettingsAPI.update(settings.data)` when the user clicks Save. The checkbox `onChange` therefore must NOT fire the sweep. Sweep is fired from `handleSave` only when `injectRtkHook` changed vs. the snapshot loaded at modal open.

#### 5.3.1 Add the checkbox

Insert a new section **between** line 329 (`</div>` closing "Window") and line 331 (`<div class="settings-section">` opening "Web Remote Access"):

```tsx
      <div class="settings-section">
        <div class="settings-section-title">RTK Token Compression</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.injectRtkHook}
            disabled={rtkSweepInFlight()}
            onChange={(e) => updateField("injectRtkHook", e.currentTarget.checked)}
          />
          <span>Inject RTK hook into agent replicas</span>
        </label>
      </div>
```

Note: `disabled={rtkSweepInFlight()}` is the M9 UI gate — see §5.3.3.

#### 5.3.2 Snapshot the initial value at modal open + sweep from `handleSave`

In the `onMount` (or wherever `SettingsAPI.get()` is called and `setSettings(loaded)` runs), capture a snapshot of `injectRtkHook`:

```tsx
const [initialInjectRtk, setInitialInjectRtk] = createSignal<boolean | null>(null);

onMount(async () => {
  const loaded = await SettingsAPI.get();
  setSettings(loaded);
  setInitialInjectRtk(loaded.injectRtkHook);
});
```

Modify `handleSave` to fire the sweep AFTER `update_settings` succeeds AND `injectRtkHook` changed:

```tsx
const handleSave = async () => {
  try {
    await SettingsAPI.update(settings.data!);
    // RTK sweep — only when the toggle value changed during this modal session.
    const initial = initialInjectRtk();
    const next = settings.data!.injectRtkHook;
    if (initial !== null && initial !== next) {
      setRtkSweepInFlight(true);
      try {
        const result = await SettingsAPI.sweepRtkHook(next);
        // M6: log per-dir errors so partial failures are surfaced in DevTools.
        if (result.errors.length > 0) {
          console.error(
            `[rtk] sweep partial failure: ${result.errors.length}/${result.total} dirs failed`,
            result.errors,
          );
        }
        setInitialInjectRtk(next); // update snapshot — no re-sweep on second Save
      } catch (err) {
        console.error("[rtk] sweep failed:", err);
      } finally {
        setRtkSweepInFlight(false);
      }
    }
    props.onClose();
  } catch (err) {
    console.error("[settings] save failed:", err);
  }
};
```

#### 5.3.3 UI gate while sweep is in flight (closes grinch M9)

Add a `createSignal<boolean>(false)` for `rtkSweepInFlight`:

```tsx
const [rtkSweepInFlight, setRtkSweepInFlight] = createSignal(false);
```

The Save button (already present in the modal, around line 240) gains `disabled={rtkSweepInFlight()}`. The checkbox row (§5.3.1) also disables while in-flight. Together these prevent the rapid-toggle double-Save race that would queue two concurrent sweeps with opposite `enabled` values, leaving replicas in silent partial state.

Implementation pseudocode for the existing Save button:

```tsx
<button class="settings-save-btn" disabled={rtkSweepInFlight()} onClick={handleSave}>
  Save
</button>
```

### 5.4 `src/main/components/RtkBanner.tsx` — new component

A non-blocking banner that mounts at the top of the unified main window. Two modes (`prompt-enable` and `auto-disabled`); also supports `silent` and `active` (renders nothing). Round-2 fixes + §18 amendment:

- **H3** — Uses `setInjectRtkHook` and `setRtkPromptDismissed` (narrow setters, write-lock-held) instead of `get` + `update`. No more IPC-level read-modify-write race against the SettingsModal's `update_settings`.
- **M5** — Subscribes to `rtk_startup_status` BEFORE snapshotting via `getRtkStartupStatus`. If the backend emits between subscribe and snapshot, the listener catches it; if it emits after the snapshot, idempotent `setMode` re-applies.
- **M6** — `sweepRtkHook` result is inspected; per-dir errors logged via `console.error` so partial failures are surfaced in DevTools.
- **M9** — `busy()` signal disables both buttons during the in-flight sweep, preventing the rapid-double-click race.
- **§18 amendment — `auto-disabled` listener/getter consistency.** The banner code below is **unchanged from round 2**. The bug (banner showing then immediately hiding when `mode == "auto-disabled"`) was fixed entirely at the backend layer: `get_rtk_startup_status` now reads a cached boot decision (§4.5.3) instead of recomputing from current state. Subscribe-first M5 ordering still applies; the difference is that snapshot and listener now agree on the mode in ALL four cases (previously they could disagree on `auto-disabled` because the auto-disable side-effect mutated `inject_rtk_hook`, breaking the recompute path).

```tsx
import { Component, createSignal, onMount, Show } from "solid-js";
import { SettingsAPI } from "../../shared/ipc";
import type { UnlistenFn } from "../../shared/transport";
import { listen } from "@tauri-apps/api/event";
import { isTauri } from "../../shared/platform";

type RtkMode = "prompt-enable" | "active" | "auto-disabled" | "silent";

const RtkBanner: Component = () => {
  const [mode, setMode] = createSignal<RtkMode>("silent");
  const [busy, setBusy] = createSignal(false);
  let unlisten: UnlistenFn | null = null;

  onMount(async () => {
    if (!isTauri) return;

    // M5: subscribe FIRST so any emit-during-mount is caught.
    unlisten = await listen<{ mode: RtkMode }>("rtk_startup_status", (e) => {
      setMode(e.payload.mode);
    });

    // Then snapshot the current status. Worst case: the snapshot races with
    // an emit and one of the two redundantly applies the same mode value
    // (idempotent — no double-render concern).
    try {
      const initial = await SettingsAPI.getRtkStartupStatus();
      setMode(initial);
    } catch (err) {
      console.error("[rtk-banner] getRtkStartupStatus failed:", err);
    }
  });

  const onEnable = async () => {
    if (busy()) return;
    setBusy(true);
    try {
      // H3: narrow setter, no IPC-level RMW.
      await SettingsAPI.setInjectRtkHook(true);
      const result = await SettingsAPI.sweepRtkHook(true);
      // M6: surface per-dir failures.
      if (result.errors.length > 0) {
        console.error(
          `[rtk-banner] sweep partial failure: ${result.errors.length}/${result.total} dirs failed`,
          result.errors,
        );
      }
      setMode("active");
    } catch (err) {
      console.error("[rtk-banner] enable failed:", err);
    } finally {
      setBusy(false);
    }
  };

  const onDismissPrompt = async () => {
    if (busy()) return;
    setBusy(true);
    try {
      await SettingsAPI.setRtkPromptDismissed(true);
      setMode("silent");
    } catch (err) {
      console.error("[rtk-banner] dismiss failed:", err);
    } finally {
      setBusy(false);
    }
  };

  const onDismissAutoDisabled = () => setMode("silent");

  return (
    <Show when={mode() === "prompt-enable" || mode() === "auto-disabled"}>
      <Show when={mode() === "prompt-enable"}>
        <div class="rtk-banner rtk-banner-prompt">
          <span>
            RTK is installed. Inject the RTK hook into agent replicas to
            compress Bash output and save tokens?
          </span>
          <button class="rtk-banner-btn" disabled={busy()} onClick={onEnable}>
            Enable
          </button>
          <button
            class="rtk-banner-btn rtk-banner-btn-secondary"
            disabled={busy()}
            onClick={onDismissPrompt}
          >
            Don't ask again
          </button>
        </div>
      </Show>
      <Show when={mode() === "auto-disabled"}>
        <div class="rtk-banner rtk-banner-warning">
          <span>
            RTK was disabled because the binary is no longer in PATH. Hooks
            were removed from all replicas. Re-install RTK and re-enable the
            toggle in Settings to restore.
          </span>
          <button
            class="rtk-banner-btn rtk-banner-btn-secondary"
            onClick={onDismissAutoDisabled}
          >
            Dismiss
          </button>
        </div>
      </Show>
    </Show>
  );
};

export default RtkBanner;
```

### 5.5 `src/main/App.tsx` — mount the banner

**Anchor:** the JSX block at lines 201–240. The `<Titlebar />` is at line 209; `<div class="main-body">` at line 210.

Add **between** line 209 and line 210:

```tsx
      <Titlebar />
      <RtkBanner />
      <div class="main-body">
```

Add the import at the top of the file:

```tsx
import RtkBanner from "./components/RtkBanner";
```

### 5.6 `src/main/styles/main.css` — banner styles

Append (the exact rules can mirror existing `.rtk-` siblings if any — reuse `--accent` / `--bg-secondary` CSS variables. Author should match the project's visual language; the dev-webpage-ui agent will refine in review):

```css
.rtk-banner {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 16px;
  font-size: 13px;
  border-bottom: 1px solid var(--border-color, #2a2a2a);
}

.rtk-banner-prompt {
  background: var(--accent-bg, #1f3a5f);
  color: var(--accent-fg, #cfe1ff);
}

.rtk-banner-warning {
  background: var(--warning-bg, #5f3a1f);
  color: var(--warning-fg, #ffd9a8);
}

.rtk-banner-btn {
  background: var(--btn-bg, #2a2a2a);
  color: var(--btn-fg, #fff);
  border: 1px solid var(--border-color, #444);
  padding: 4px 10px;
  border-radius: 4px;
  cursor: pointer;
}

.rtk-banner-btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.rtk-banner-btn-secondary {
  background: transparent;
}
```

---

## 6. Behavior matrix

| `rtk` in PATH | `injectRtkHook` | `rtkPromptDismissed` | Startup mode | Side effect on boot |
|---|---|---|---|---|
| yes | false | false | `prompt-enable` | none. Banner shown. |
| yes | false | true | `silent` | none. |
| yes | true | * | `active` | re-sweep ON (idempotent recovery). |
| no | false | * | `silent` | none. |
| no | true | * | `auto-disabled` | persist `injectRtkHook=false`; sweep OFF; banner shown. |

User-toggled flow (Settings/General checkbox — fires from `handleSave`):

| Action | Frontend calls | Effect |
|---|---|---|
| Tick checkbox + click Save | `update_settings(full draft)`, then if `injectRtkHook` changed → `sweep_rtk_hook(true)` | Setting persisted via the existing modal save path; ON-sweep runs only when the value actually changed (avoids redundant sweeps on every save). |
| Untick checkbox + click Save | `update_settings(full draft)`, then if changed → `sweep_rtk_hook(false)` | Setting persisted; OFF-sweep removes our marker-bearing entries only. |

Banner button flow (uses narrow setters — closes grinch H3):

| Button | Frontend calls | Effect |
|---|---|---|
| `[Enable]` | `set_inject_rtk_hook(true)`, then `sweep_rtk_hook(true)` | Setting persisted with write-lock-held; ON-sweep runs; banner switches to `silent`. |
| `[Don't ask again]` (prompt-enable) | `set_rtk_prompt_dismissed(true)` | Banner switches to `silent`; suppressed forever until manually re-enabled in Settings. |
| `[Dismiss]` (auto-disabled) | none (UI-only state) | Banner hidden for this session. Reappears next boot if the auto-disable condition still holds — closed in §9 Q3 (informative, self-clearing). |

---

## 7. Idempotency, atomicity, recovery

### 7.1 Per-replica failure
Best-effort + log + collect. The sweep continues across remaining dirs. The `RtkSweepResult` returned to the frontend includes the per-dir errors; the frontend logs them via `console.error` (closes grinch M6). A toast surface is future work, out of scope for #120.

### 7.2 Mid-sweep crash
Idempotency by design. On next boot:
- If `injectRtkHook=true` and `rtk` is present: the startup task re-runs an ON-sweep across all managed dirs. Already-injected dirs are no-ops (marker idempotency); missed dirs are caught up.
- If `injectRtkHook=false`: no startup sweep. A crash mid-OFF-sweep leaves some replicas with our marker-bearing entry and some without. Toggling off and on heals. **Documented limitation — see §9 Q4 for the rationale on NOT adding a `rtk_sweep_dirty` flag**.

### 7.3 Atomic file writes
`std::fs::write` is not atomic on Windows (open-write-close, with possible torn reads). For #120 we accept this — the sweep holds `RtkSweepLockState` (§7.5) so in-process writers cannot race a single file. Cross-process readers (Claude Code) check the file at a much lower frequency than we'd write it. If a stronger guarantee is needed, dev-rust can mirror the `tmp + rename` pattern from `save_settings` in a follow-up; closed in §9 Q6 as deferred.

### 7.4 Concurrent writers
**In-process race (closed in #120 — grinch M8).** `RtkSweepLockState = Arc<tokio::sync::Mutex<()>>` is acquired by every in-process call site that touches `.claude/settings.local.json` via `ensure_claude_md_excludes` or `ensure_rtk_pretool_hook` — `sweep_rtk_hook`, the startup auto-disable + active-recovery sweeps, and the four entity-creation / agent-creator sites (§4.6). The lock blocks interleaved read-modify-writes on the same file across all in-process flows.

**Cross-process race (DOCUMENTED, not addressed in #120).** Two AC instances on the same machine, OR a CLI `create-agent --launch` invocation racing the running app, can still interleave on the same file — the in-process tokio Mutex is not visible across processes. The existing `ensure_claude_md_excludes` flow has the identical race that has not surfaced in production (see issue #84 plan). Closing it requires file-based locking via `fs2::FileExt::lock_exclusive` (or platform-specific equivalents). Out of scope for #120; flagged in §11 as a follow-up candidate.

### 7.5 `RtkSweepLockState` design
- Type: `Arc<tokio::sync::Mutex<()>>`.
- Created in `lib.rs::run` before `tauri::Builder::default()`, registered via `.manage(...)`.
- Acquired by:
  - `commands::config::sweep_rtk_hook` (whole loop).
  - `lib.rs::setup` startup task — auto-disable OFF-sweep AND active-recovery ON-sweep (whole loop each).
  - `commands::agent_creator::write_claude_settings_local` (helper sequence).
  - `commands::entity_creation::create_agent_matrix` (helper sequence).
  - `commands::entity_creation::create_workgroup` (per-replica scope inside the loop).
- Held duration: bounded by the wrapped helper sequence — typically a single read-modify-write per file. Sweep loops hold for N reads/writes, with each write bounded to a few KB.
- Failure mode: if the future holding the guard panics, the guard is dropped — `tokio::sync::Mutex` does NOT poison (unlike `std::sync::Mutex`); subsequent acquirers proceed normally. No deadlock risk. (Round-3 N2 wording fix — earlier drafts incorrectly described tokio's Mutex with std-Mutex poisoning semantics.)

---

## 8. Testing

### 8.1 Rust unit tests

Required, written alongside the helpers:

- `claude_settings::tests` — **22 cases** enumerated in §4.2.5 (10 ON, 6 OFF, 6 enumerator/cross-cutting; round-2 added cases #13–#22 to cover grinch H1, H2, M7, M10, M11 and dev-rust §12.3.1 + §12.3.5).
- `settings::tests` — two new round-trip tests added in §4.1.3.

### 8.2 Rust integration tests (optional, recommended)

A `#[test]` in `claude_settings::tests` that builds a tempdir mimicking a project layout (`proj/.ac-new/_agent_X`, `proj/.ac-new/wg-1-team/__agent_Y`), then invokes the `enumerate + ensure_rtk_pretool_hook` loop directly (bypassing the Tauri command boundary, which is tested implicitly through the unit + manual passes). Asserts: every dir's `.claude/settings.local.json` contains the rtk entry after ON; every entry is removed after OFF.

### 8.3 Manual verification

Dev runs the app with a populated `project_paths`:

1. **Toggle ON.** Tick the checkbox in Settings/General → Save. Confirm: `cat <replica>/.claude/settings.local.json` shows the rtk entry with the marker. Repeat across at least one matrix and one replica.
2. **Toggle OFF.** Untick the checkbox → Save. Confirm: marker-bearing entry gone from the same files; other keys (e.g. `claudeMdExcludes`) preserved.
3. **Idempotency.** Toggle ON → Save → reopen → Save again with no change: no spurious sweep (verified via lack of log line `[rtk-sweep] enabled=...`). Toggle OFF → Save: no marker entries remain.
4. **Existing user manual hook.** Manually add a non-rtk Bash hook to a replica's `.claude/settings.local.json`. Tick the checkbox → Save. Confirm: both hooks present. Untick → Save. Confirm: only the non-rtk hook remains.
5. **Banner — prompt-enable.** Install rtk; ensure `injectRtkHook=false` and `rtkPromptDismissed=false`; reboot AC. Confirm banner appears.
6. **Banner — dismiss.** Click `[Don't ask again]`. Reboot. Confirm banner does NOT appear.
7. **Banner — enable from banner.** Reset `rtkPromptDismissed=false`. Click `[Enable]` from the banner. Confirm: setting flipped, sweep ran, banner gone.
8. **Banner — auto-disabled.** With `injectRtkHook=true`, rename or remove `rtk` from PATH. Reboot. Confirm: setting auto-flipped to false, banner shown explaining auto-disable, replica files no longer contain the marker entry.
9. **Crash recovery.** With `injectRtkHook=true`, kill AC mid-sweep (hard to trigger reliably; alternatively manually corrupt one replica's file). Restart AC. Confirm: ON-sweep at startup heals.

#### Round-2 manual passes

10. **(grinch H1) Malformed JSON preserved.** Edit a replica's `.claude/settings.local.json` to add a trailing comma (`{"hooks":{},}`). Toggle ON → Save. Confirm: file content **unchanged**, AC log shows `[rtk] Skipping ON-sweep ...: file is not a JSON object (preserved as-is)`.
11. **(grinch H2) Wrong-shape preserved.** Hand-write `{"hooks":"broken"}`. Toggle ON → Save. Confirm: file content unchanged, log shows `'hooks' in ... is string (expected object); bailing — preserving user data`.
12. **(grinch M7) Junction skipped.** On Windows, `mklink /J <project>/.ac-new/wg-99-fake/__agent_outside <some other dir>`. Toggle ON → Save. Confirm: `<some other dir>/.claude/settings.local.json` is **not** created.
13. **(grinch M10) Marker-only-different-body.** Hand-write a replica's hook with the marker prefix but a different rewriter body. Toggle ON → Save. Confirm: file unchanged (idempotent by marker). Toggle OFF → Save. Confirm: marker-bearing entry removed.
14. **(grinch M11) BOM file.** Encode a replica's `settings.local.json` as UTF-8 with BOM (use Notepad "Save As" → UTF-8 with BOM). Toggle ON → Save. Confirm: rtk hook added, BOM dropped on write.
15. **(grinch H3) Banner vs SettingsModal save.** Open SettingsModal, tick `coordSortByActivity` (or any other field). DO NOT click Save yet. Click `[Don't ask again]` on the banner. Now click Save in the modal. Confirm: BOTH `coordSortByActivity` AND `rtkPromptDismissed` are persisted (no clobber).
16. **(grinch H4) Auto-disable + concurrent settings save.** With `injectRtkHook=true` and `rtk` removed from PATH, start AC; immediately open SettingsModal and modify any field. Click Save. Confirm on next boot: BOTH the auto-disable AND the user's manual change are persisted (no on-disk divergence vs in-memory).

### 8.4 What is NOT covered by tests

- The frontend banner component (`RtkBanner.tsx`) has no SolidJS unit tests in this plan — the codebase does not currently host frontend unit infrastructure for components. Manual passes #5–#8 + #15 cover banner behavior.
- The CSS rules in `main.css` are visual-only; no regression test.
- The cross-process race (§7.4) is not exercised in tests; closing it is out of scope for #120.

---

## 9. Resolved questions

All previously-open questions are closed below with the final decision and one-line justification. Reopen via §14 changelog if implementation-time evidence contradicts.

**Q1 — Sweep scope: matrices, replicas, or both? → BOTH.**
The four `ensure_claude_md_excludes` callers already cover both `_agent_*` matrices and `__agent_*` replicas. Symmetric wiring of `ensure_rtk_pretool_hook` avoids per-caller asymmetry. Dev-rust §12.2 concurred. The checkbox label keeps the issue-spec wording ("agent replicas") but the code path applies symmetrically; no user surprise expected (the matrix → replica relationship is well-understood by AC users).

**Q2 — Field naming. → KEEP `inject_rtk_hook` and `rtk_prompt_dismissed`.**
Verb form makes the disk side-effect explicit; `_dismissed` parallels `onboarding_dismissed`. Dev-rust §12.2 concurred.

**Q3 — Auto-disabled banner persistence. → KEEP current (UI-only `[Dismiss]`).**
The auto-disable banner is informative, not naggy: it self-clears the moment `rtk` reappears in PATH (which is the only fix the user can apply). Persisting a third bool would add state with unclear lifetime. Dev-rust §12.2 concurred.

**Q4 — `rtk_sweep_dirty` flag. → DO NOT ADD.**
The ON-sweep at startup already covers the high-impact direction (mid-ON-sweep crash heals automatically via active-recovery). The OFF-sweep crash window is tiny and self-correcting on next user toggle. Adding the flag would push `rtk_*` to three settings fields for marginal coverage. Dev-rust §12.2 concurred. Grinch did not insist (M-tier finding M5–M11 are unrelated). Revisit only if a concrete naked-OFF-crash incident surfaces.

**Q5 — `SettingsModal` persistence model. → SWEEP FROM `handleSave`, NOT `onChange`.**
Dev-rust §12.2 verified that `updateField` is local-only and persistence is in `handleSave` (lines 215–238 of `SettingsModal.tsx`). §5.3 was rewritten to snapshot `injectRtkHook` at modal open, fire the sweep from `handleSave` only when the value changed, and gate the Save button while the sweep is in flight (closes grinch M9).

**Q6 — Atomic write of `.claude/settings.local.json`. → DEFERRED.**
Symmetry with `ensure_claude_md_excludes` is preserved (both use plain `std::fs::write`). The new `RtkSweepLockState` (§7.5) eliminates the in-process race that motivated atomicity concerns. Cross-process atomicity would require `tmp+rename` in BOTH helpers — a coordinated change that is out of scope for #120. Flagged in §11 follow-up notes.

**Q7 — `which` crate version. → `which = "7"`.**
Dev-rust §12.2 confirmed: no transitive `which` in current `Cargo.toml`. v7 is current stable with the portable sync API. Verify with `cargo tree | rg which` after editing — if a transitive copy appears, reuse it.

**Q8 — Banner mount: `MainApp` only, or all windows? → `MainApp` only.**
The 0.8.0 unified-window architecture makes `MainApp` the canonical surface; embedded sidebar/terminal panes inside it inherit visibility. If a future release re-splits windows, the banner mount must follow. Documented; no action needed now.

### Round-2 architectural decisions (closed here, see §14 for changelog)

**Q9 — M10 marker form. → ADOPT pre-ship, single fixed marker `RTK_HOOK_MARKER = "@ac-rtk-marker-v1"`.**
Embedded as the leading JS string-expression statement `'@ac-rtk-marker-v1';` inside `node -e "..."` — JS-inert (no-op string in statement position). The cost is one trivial edit to `repo-AgentsCommander/.claude/settings.json` (locked under unit test #14). The benefit is permanent forward-compat: when rtk-ai upstream evolves the rewriter body, AC ships a new constant; OFF-sweep filters by marker substring and cleans up old hooks. ON-sweep idempotency also uses marker substring, so user customizations of the body are preserved across upgrades. The "user removed marker = my own custom hook now" contract gives a clean ownership-transfer signal. Alternative `RTK_LEGACY_COMMANDS` array was rejected as more maintenance burden.

**Q10 — M8 locking: tokio Mutex vs file-based fs2. → `tokio::Mutex<()>` State (in-process only).**
- Pros: zero new dep, simpler, integrates with existing `Arc<...>::manage()` pattern.
- Cons: cross-process race remains. We accept this — closing cross-process requires fs2 and either tightens the helpers' API (sync → async-blocking) or duplicates locking at every call site. The cross-process race is the same one §7.4 already accepts for `ensure_claude_md_excludes`; widening it for #120 alone would be inconsistent. fs2 stays as a follow-up if cross-process incidents surface.

---

## 10. Migration & backwards compatibility

- Old `settings.json` files (without `injectRtkHook` / `rtkPromptDismissed`) deserialize cleanly via `#[serde(default)]`. Both default to `false`.
- Downgrading from a post-#120 build to a pre-#120 build: serde drops unknown fields silently on read, but **the unknown fields persist in the file** — `update_settings` round-trips the full struct, so a downgraded version simply loses these fields on first save. No data loss; the user re-toggles after upgrading.
- Replicas that received the rtk hook from a post-#120 build remain functional under any older AC build (they don't read `.claude/settings.local.json`; only Claude Code does). The hook stays active until the user toggles the post-#120 build OFF.
- **Marker requirement (pre-ship — closes grinch M10).** `RTK_HOOK_MARKER = "@ac-rtk-marker-v1"` MUST ship in the very first build that includes #120. The constant is embedded as a leading JS string-expression statement (`'@ac-rtk-marker-v1';`) inside `RTK_REWRITER_COMMAND`, AND the source `repo-AgentsCommander/.claude/settings.json` is updated in lockstep so unit test #14 holds. **Adding the marker post-launch is not viable** — pre-launch hooks injected without the marker would never be cleaned up by post-launch OFF-sweeps (byte-mismatch on the body). The pre-ship constraint is the cheap fix; the post-ship alternative would be a `RTK_LEGACY_COMMANDS: &[&str]` array that grows over time. Locked here as a pre-ship requirement; flagged in §14 changelog as the highest-risk decision.
- **Marker bump procedure** (informational, far-future). If `@ac-rtk-marker-v1` ever needs retirement (e.g. namespace collision), bump to `@ac-rtk-marker-v2` AND introduce `pub const RTK_LEGACY_MARKERS: &[&str] = &["@ac-rtk-marker-v1"]`. OFF-sweep filters by `command.contains(...)` against any of `[RTK_HOOK_MARKER, RTK_LEGACY_MARKERS[...]]`. ON-sweep idempotency check uses only the current `RTK_HOOK_MARKER`. This pattern keeps the migration burden bounded.

---

## 11. Final notes & implementation notes for dev-rust

- This plan does not propose any merges, commits, or pushes. Sequencing of those decisions belongs to the dev / tech-lead.
- All comments and identifiers in the implementation are in English per the role requirements.
- Implementation risk after round-2 changes: low-medium. The four creation flows now acquire `RtkSweepLockState` around the helper sequence (small structural change; pattern is uniform). The hook merge helper `ensure_rtk_pretool_hook` grew defensive shape-checks; the test matrix in §4.2.5 covers them.

### Round-2 implementation notes (grinch L12–L15 + follow-ups)

- **L12 — Test #7 byte-equality.** Closed in §4.2.5: tests assert structural `serde_json::Value` equality, not byte-equality. Test #11 still pins byte-equality on the canonical command payload (one-line check that `RTK_REWRITER_COMMAND` survives round-trip).
- **L13 — `which::which` is structural detection.** `which` confirms the `rtk` binary exists in PATH; it does NOT smoke-test that the binary works. A corrupt `rtk` (e.g. broken upgrade, missing dependency) passes detection. Document in the user-facing release note: "RTK toggle requires both the `rtk` binary in PATH and `node` available; AC's startup detection only verifies presence". A `rtk --version` smoke test is a follow-up if false positives surface.
- **L14 — Active-mode startup recovery is unconditional.** Every boot with `injectRtkHook=true` runs an ON-sweep. For 100 replicas this is ~100 read+parse+serialize+write roundtrips. Acceptable for a typical AC fleet. Two follow-ups: (a) wrap the loop in `tokio::task::spawn_blocking` if the startup task budget grows; (b) skip files whose mtime is older than the last successful sweep. Neither is needed for v1.
- **L15 — `merge_rtk_hook` borrow chain on older rustc.** The `entry().or_insert_with().as_object_mut().expect(...)` chain compiles cleanly on rustc ≥ 1.71 (NLL). Verify project MSRV before patching. If older, split into `if !exists { insert }; let x = get_mut().unwrap()` (sketch in §4.2.3).

### Cross-process race (§7.4) — follow-up candidate

The CLI `create-agent --launch` flow and any peer AC instance still race on `.claude/settings.local.json` cross-process. Closing requires file-based locking (`fs2` crate, `FileExt::lock_exclusive`). Wrap calls in both `ensure_claude_md_excludes` and `ensure_rtk_pretool_hook`. Out of scope for #120; flagged for grinch's awareness.

### Existing pre-#120 race in `update_settings`

`commands/config.rs::update_settings` reads `root_token` outside the SettingsState write lock (line 38). Pre-existing; not introduced or worsened by #120. Flagged for tech-lead awareness as a separate follow-up.

— Architect (round 2 — see §14 changelog for delta)

---

## 12. Dev-rust review (round 1)

### 12.1 Audit verdict

**All file paths, line numbers, symbol references, and the `RTK_REWRITER_COMMAND` raw-string verified accurate against branch `feature/120-rtk-hook-injection-toggle` tip (`4e85a32`).** Spot-checks:

- `Cargo.toml`: line 31 = `tauri-plugin-dialog = "2.6.0"`, line 33 = `[target.'cfg(windows)'.dependencies]`. ✓
- `config/settings.rs`: `AppSettings` 47–152, `log_level` field at 151, `Default` impl 186–230, `log_level: None` at 227, `coord_sort_by_activity_round_trips_through_serde` at 554, `coord_sort_by_activity_defaults_when_missing_from_json` at 731. ✓
- `config/claude_settings.rs`: doc-block 1–6 (line 7 is blank — replacement subsumes it), `use std::path::Path;` at 8, `ensure_claude_md_excludes` 16–69. ✓
- `commands/config.rs`: file ends at 142, `get_instance_label` at 138–141. ✓
- `lib.rs`: `setup` opens line 276, `AppHandle` → `OnceLock` at 281, web-server boot 309–331, `invoke_handler!` 701–760, `get_settings`/`update_settings` at 713–714. ✓
- `commands/agent_creator.rs::write_claude_settings_local` lines 59–62 (current sig is single-arg `agent_path: String`). ✓
- `cli/create_agent.rs`: `let settings = config::settings::load_settings();` at 123, exclude block 133–140 (the architect's "134–140" elides the opening `match` arm — the actual block is 133–140; either anchor lands the same insertion point). ✓
- `commands/entity_creation.rs`: matrix `#84` block 244–261 (244–248 comment, 249–252 snapshot, 253–261 if-block); workgroup snapshot 569–576 (569–572 comment, 573–576 snapshot); workgroup per-replica `#84` block 600–609. ✓
- `shared/types.ts`: `AppSettings` ends 157, `coordSortByActivity` at 156. ✓
- `shared/ipc.ts`: `SettingsAPI` 111–119; existing `writeClaudeSettingsLocal` consumers in `NewAgentModal.tsx:77` and `SessionItem.tsx:236` invoke the command without explicit `State<>` args, confirming Tauri auto-injection — the §4.6.1 signature change is transparent to those call sites. ✓
- `sidebar/components/SettingsModal.tsx`: `renderGeneralTab` at 248, Window section closes 329, Web Remote opens 331. ✓
- `main/App.tsx`: JSX 201–240, `<Titlebar />` at 209, `<div class="main-body">` at 210. ✓

The `RTK_REWRITER_COMMAND` raw-string in §4.2.2 is byte-identical to the JSON-decoded `command` field of `repo-AgentsCommander/.claude/settings.json:9`. The `r#"…"#` delimiter is safe (no `"#` sequence appears in the JS source). ✓

### 12.2 Position on §11 open questions

**Q1 — Sweep scope: matrices + replicas, or replicas only?**
**Both.** Matrices (`_agent_*`) can be launched as Claude sessions just like replicas; restricting to replicas would create per-caller asymmetry against `ensure_claude_md_excludes` (which already covers both) and surprise readers who expect the two helpers to be wired in lockstep. The cost of "both" is one extra walk per matrix per sweep — negligible.

**Q2 — Field naming.**
**Keep `inject_rtk_hook` and `rtk_prompt_dismissed`.** Verb form makes the disk side-effect explicit; `_dismissed` parallels existing `onboarding_dismissed`. `rtk_hook_enabled` would weaken the contract (we WRITE the hook, not just toggle behavior).

**Q3 — Auto-disabled banner persistence.**
**Keep current (UI-only `[Dismiss]`).** Persisting a third bool adds state with unclear lifetime (when does it reset?). The auto-disable banner is informative, not naggy: it self-clears when rtk reappears in PATH, which is the only fix the user can apply.

**Q4 — `rtk_sweep_dirty` flag.**
**Do not add.** ON-sweep crash heals via the active-mode startup recovery (§4.5.2). OFF-sweep crash window is small AND idempotently repairable by re-toggle. The flag adds yet another piece of state to persist + validate + write atomically; the marginal coverage does not justify the cost. Revisit only if grinch finds a concrete naked-OFF-crash scenario.

**Q5 — `SettingsModal` persistence model.**
**§5.3 pseudocode is wrong as written.** `updateField` (`SettingsModal.tsx:70-76`) is local-only — it mutates the form draft via `setSettings` and does **not** call `SettingsAPI.update`. Persistence happens at `handleSave` (lines 215–238) via `await SettingsAPI.update(settings.data)` when the user clicks "Save". Calling `sweepRtkHook` on every checkbox `onChange` would fire against a backend `SettingsState` that still holds the OLD value, and would also queue redundant sweeps if the user toggles rapidly before saving.

**Required fix (dev-webpage-ui owns the JSX/state details, but the contract is fixed here):**

1. At `onMount` after `SettingsAPI.get()` returns, snapshot the initial value: `const initialInjectRtk = loaded.injectRtkHook;`
2. In `handleSave`, after `await SettingsAPI.update(settings.data)` succeeds, if `settings.data.injectRtkHook !== initialInjectRtk`, call `await SettingsAPI.sweepRtkHook(settings.data.injectRtkHook)`. Errors logged via `console.error`, not blocking.
3. Update the snapshot to the persisted value before the modal closes (or just close — the modal re-mounts on next open).

The §5.3 markup should reflect this: the checkbox `onChange` only updates the local form via `updateField("injectRtkHook", ...)`. The sweep fires from save.

**Q6 — Atomic `.claude/settings.local.json` writes.**
**Accept current `std::fs::write` for #120.** Symmetry with `ensure_claude_md_excludes` matters more here than ad-hoc atomicity in one helper; a follow-up that atomicizes both (mirror `save_settings`'s `tmp + rename` from `config/settings.rs:442-461`) is the right shape if torn-write incidents surface. Out of scope for #120 unless grinch flags concretely.

**Q7 — `which = "7"`.**
**Acceptable.** `which v7` is current stable and offers a portable sync API (`which::which("rtk").is_ok()`). Verification step for dev-rust before adding: run `cargo tree | rg which` after editing `Cargo.toml` to confirm no transitive duplicate. The existing `Cargo.toml` (audited) does not pull `which` directly; transitive risk is low.

### 12.3 Enrichments

#### 12.3.1 OFF path must not destroy malformed user data

In `ensure_rtk_pretool_hook(dir, enabled=false)` as written in §4.2.3, if `settings.local.json` exists but `serde_json::from_str` fails, the `_ => serde_json::json!({})` arm causes the file to be overwritten with an empty object after `remove_rtk_hook` runs — destroying potentially salvageable user data we cannot semantically interpret.

For the OFF path we have nothing to remove from a malformed document. Bail with a warning instead.

**Patch the OFF-path block in `ensure_rtk_pretool_hook` (replaces the parse arm for the `!enabled` case):**

```rust
// OFF-path early exits: nothing to remove if file missing OR malformed.
if !enabled && !settings_path.exists() {
    return Ok(());
}
if !enabled {
    let existing = std::fs::read_to_string(&settings_path)
        .map_err(|e| format!("Failed to read existing settings.local.json: {}", e))?;
    let mut obj = match serde_json::from_str::<serde_json::Value>(&existing) {
        Ok(v) if v.is_object() => v,
        _ => {
            log::warn!(
                "[rtk] Skipping OFF-sweep for {}: file is not a JSON object (preserved as-is)",
                settings_path.display()
            );
            return Ok(());
        }
    };
    remove_rtk_hook(&mut obj);
    let content = serde_json::to_string_pretty(&obj)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&settings_path, format!("{}\n", content))
        .map_err(|e| format!("Failed to write settings.local.json: {}", e))?;
    return Ok(());
}
// ON path falls through to the existing logic (create_dir_all + parse-or-empty + merge).
```

`ensure_claude_md_excludes` has the same destructive behavior on malformed input, but it is additive-only — overwriting a malformed file with an additive set is at least directionally salvageable. For OFF there is no semantic recovery; we just trash data.

**New unit test (extend §4.2.5):**

```
13. enabled=false, file with malformed JSON (`{ invalid`) → file content unchanged on disk; function returns Ok(()); a log::warn was emitted.
```

#### 12.3.2 Both new Tauri commands must register

§4.5.1 shows registering only `sweep_rtk_hook`; §4.5.3 says register `get_rtk_startup_status` "alongside" but the explicit invoke-handler line is missing. Concrete edit:

```rust
            commands::config::sweep_rtk_hook,
            commands::config::get_rtk_startup_status,
```

inserted after line 714 (before `commands::repos::search_repos,`).

#### 12.3.3 In-process startup-recovery vs user-toggle race

The setup task spawned in §4.5.2 runs detached. If the user clicks the checkbox in Settings within the first few seconds of boot, both the startup recovery sweep AND the `sweep_rtk_hook` command can be iterating the same dirs concurrently. On Windows, two `std::fs::write` calls to the same path produce a torn-write window.

**Acceptance for v1:** the window is bounded (boot + ~few seconds) and the next idempotent re-apply heals. Document and move on. **Follow-up candidate:** a `tokio::Mutex<()>` registered as a new state type `RtkSweepLock`, acquired by both the setup task and the `sweep_rtk_hook` command for the duration of their loops. Trivial wiring; defer until contention is observed.

Symmetric note on §7.4 (cross-instance race): two AC instances writing simultaneously remains accepted, same as `ensure_claude_md_excludes`.

#### 12.3.4 Sweep latency on large fleets

`sweep_rtk_hook` runs the per-dir loop directly in the async fn body using sync `std::fs::*` calls. With many replicas the runtime thread stalls until the loop completes. Acceptable for typical sizes (<50 replicas; sub-second on local SSD).

**Flag as a known scaling consideration:** wrap the loop in `tokio::task::spawn_blocking` if latencies emerge. No code change required for #120.

#### 12.3.5 Source-of-truth drift for `RTK_REWRITER_COMMAND`

The doc-comment in §4.2.2 explicitly states the constant must stay byte-identical to `repo-AgentsCommander/.claude/settings.json`. There is no compile-time check; a manual edit to one file without the other silently breaks identity-equality removal for users on the new build cleaning hooks injected by a yet-newer build.

**Add a unit test (#14)** that reads the source `.claude/settings.json` at test time (relative to `CARGO_MANIFEST_DIR`) and asserts its decoded `command` field equals `RTK_REWRITER_COMMAND`:

```rust
#[test]
fn rtk_rewriter_command_matches_source_of_truth() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    // From src-tauri/, the project-level .claude/settings.json sits one level up.
    let source = std::path::Path::new(&manifest)
        .parent()
        .expect("repo root")
        .join(".claude/settings.json");
    let contents = std::fs::read_to_string(&source).expect("read .claude/settings.json");
    let v: serde_json::Value = serde_json::from_str(&contents).expect("parse");
    let cmd = v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .expect("command path");
    assert_eq!(cmd, RTK_REWRITER_COMMAND);
}
```

Cheap, runs in CI, catches silent drift.

#### 12.3.6 No new `Cargo.toml` deps beyond `which`

After auditing the existing `[dependencies]` block, no other crate is needed. The startup detection uses only `which`, `tokio`, `tauri`, `serde_json`, and `log` — all already present.

### 12.4 Phase split impact

Phase A is independently landable as written (modulo the §12.3.1 OFF-path fix and §12.3.2 dual registration). After Phase A:

- `cargo check` and `cargo test` compile clean.
- `sweep_rtk_hook` and `get_rtk_startup_status` are exposed but have no UI consumer; `update_settings` round-trips the new fields silently.
- The startup task emits `rtk_startup_status` to no listener; harmless — Tauri events with no subscriber are dropped.
- Phase B trips no hidden coupling. The frontend can wire the checkbox + banner + sweep round-trip in any order.

### 12.5 Summary of changes requested

| § | Change | Reason |
|---|---|---|
| 4.2.3 | Patch OFF-path to bail on malformed JSON | Protect user data |
| 4.2.5 | Add unit test #13 (OFF + malformed → preserved) | Lock the contract |
| 4.2.5 | Add unit test #14 (constant matches source-of-truth) | Catch drift |
| 4.5.1 | Register BOTH commands (`sweep_rtk_hook`, `get_rtk_startup_status`) | §4.5.3 was implicit |
| 5.3 | Move sweep trigger from checkbox `onChange` to `handleSave` | Match the modal's save-button persistence model |

---

## 13. Grinch review (round 1)

### 13.1 Verdict

The plan is structurally sound and Phase A independence holds. Dev-rust round 1 caught the most critical OFF-path destruction. However, the round-1 fix is **asymmetric**: the same destructive pattern lives unfixed on the ON-path, plus four read-modify-write races (two at the IPC boundary, two in-process) that the plan currently treats as acceptable but allow concrete user-data loss in narrow but reachable windows.

Severity legend:
- **HIGH** — should fix in #120; data-loss or correctness risk under reachable scenarios.
- **MEDIUM** — defer-able but document, OR fix if tech-lead deems #120 the right vehicle.
- **LOW** — follow-up candidate; flagged for awareness.

### 13.2 HIGH findings

#### H1 — ON-path destroys user data on malformed JSON (asymmetric to §12.3.1 OFF-fix)

**What.** §4.2.3 ON-path:

```rust
match serde_json::from_str::<serde_json::Value>(&existing) {
    Ok(v) if v.is_object() => v,
    _ => serde_json::json!({}),
}
```

A malformed-or-non-object document is silently treated as `{}`, then written back fully replaced. Test #10 explicitly asserts this destructive behavior.

**Why.** Trailing commas (the most common JSON typo), JSON-with-comments (a VS Code editor feature some users enable for `.json` files), or any other parse failure causes the user's full `.claude/settings.local.json` — including `claudeMdExcludes` and any unrelated keys — to be wiped on the very first ON-sweep. Dev-rust's §12.3.1 fix handles this only on OFF; ON inherits the original bug. `ensure_claude_md_excludes` has the same flaw, but its damage is bounded — it preserves "additive" semantics, so a re-run restores the missing exclude. Our ON-path silently destroys arbitrary keys with no path back.

**Fix.** Apply the same bail-with-warn pattern as the §12.3.1 OFF-path on the ON-path. Concrete edit to `ensure_rtk_pretool_hook`: when `enabled=true` AND `settings_path.exists()` AND (parse fails OR result is non-object), emit `log::warn!` and `return Ok(())`. Update §4.2.5 test #10 to assert NON-overwrite (the malformed source remains on disk) instead of overwrite. Same flaw exists on `ensure_claude_md_excludes`; addressing it is scope creep for #120 and can be a follow-up.

#### H2 — ON-path silently overwrites non-object `hooks` and non-array `PreToolUse`

**What.** `merge_rtk_hook` lines 260–266 and 272–278: if `hooks` exists but is not an object, OR `PreToolUse` exists but is not an array, the plan REPLACES the value (`*hooks_root = Value::Object(...)`, `*pretool = Value::Array(Vec::new())`). The user's hand-crafted (or experimentally crafted) data is wiped without warning. The inline comment "Top-level 'hooks' exists but is not an object — replace it" acknowledges the destructive choice but provides no audit trail at runtime.

A third instance of the same pattern lives in the inner-hooks branch:

```rust
let inner = match pretool_arr[idx].get_mut("hooks").and_then(|h| h.as_array_mut()) {
    Some(arr) => arr,
    None => {
        pretool_arr[idx]["hooks"] = Value::Array(Vec::new());
        pretool_arr[idx]["hooks"].as_array_mut().unwrap()
    }
};
```

— if the Bash matcher's inner `hooks` is missing OR non-array, we replace it.

**Why.** Even with H1 patched at the TOP level, the user can have `{"hooks": "broken"}` or `{"hooks": {"PreToolUse": "broken"}}` — top-level parse passes (top is an object), but the inner shape is wrong. ON-sweep silently destroys it. No warning is emitted, no audit trail is left, and the value the user spent time crafting (or broke during a hand-edit) is gone.

**Fix.** Two options (preferred first):
1. **Bail with warn** symmetric to H1: on parse OK but `hooks` non-object, OR `PreToolUse` exists and is non-array, OR matcher entry's inner `hooks` exists and is non-array → log + `return Ok(())`. Under this rule, the entire `ensure_rtk_pretool_hook` call becomes a no-op when ANY shape is unexpected.
2. **Keep replace, log displaced shape**: `log::warn!("hooks was {discriminant}, replacing with empty object")` BEFORE the replace. Less safe but at least audit-trailable.

Add a unit test for each shape: `{"hooks": null}`, `{"hooks": "string"}`, `{"hooks": {"PreToolUse": "string"}}`, `{"hooks": {"PreToolUse": {...}}}`, `{"hooks": {"PreToolUse": [{"matcher":"Bash", "hooks": "string"}]}}` — under option 1 all five become no-ops with warn; under option 2 each emits the displaced shape.

#### H3 — Banner buttons IPC-level read-modify-write race with SettingsModal save

**What.** `RtkBanner.tsx` `onEnable` and `onDismissPrompt`:

```ts
const settings = await SettingsAPI.get();           // (1) fetch full struct
await SettingsAPI.update({ ...settings, X: true }); // (2) write full struct back
```

Concurrently, `SettingsModal.tsx::handleSave` does the same get-mutate-update with the modal's draft (per §12.2 Q5 fix). The Tauri-side `update_settings` serializes via the `SettingsState` write lock — but the IPC boundary is read-modify-write at the JS layer. Between (1) and (2), the modal's update can land, and the banner overwrites the modal's just-persisted changes.

**Why.** Concrete scenario:
1. User opens SettingsModal, ticks `coordSortByActivity`. Modal draft holds the change locally.
2. User notices the rtk banner, clicks `[Don't ask again]`.
3. Banner: `SettingsAPI.get()` → IPC dispatches.
4. User clicks Save in modal. Modal: `SettingsAPI.update({...new_settings_with_coordSort})` → persists to disk + memory.
5. Banner's get() resolves with whichever version `get_settings` happened to read (could be PRE- or POST-step-4 depending on tokio scheduling).
6. Banner sends `update({...banner_settings, rtkPromptDismissed: true})`. If banner's get returned PRE-step-4 state, `coordSortByActivity` is reverted to false. The user sees the modal close successfully, but their setting is gone.

The race is structural, not theoretical: SettingsModal lives in the sidebar pane, the banner in main App. They mount independently. The user can interact with both during a single multi-second window.

The same race exists for `[Enable]` (overwrites with `injectRtkHook: true`).

**Fix.** Add narrow Tauri commands to `commands/config.rs`:

```rust
#[tauri::command]
pub async fn set_inject_rtk_hook(settings: State<'_, SettingsState>, value: bool) -> Result<(), String> {
    let snapshot = {
        let mut s = settings.write().await;
        s.inject_rtk_hook = value;
        s.clone()
    };
    save_settings(&snapshot)
}

#[tauri::command]
pub async fn set_rtk_prompt_dismissed(settings: State<'_, SettingsState>, value: bool) -> Result<(), String> {
    let snapshot = {
        let mut s = settings.write().await;
        s.rtk_prompt_dismissed = value;
        s.clone()
    };
    save_settings(&snapshot)
}
```

Each acquires the write lock, mutates only the target field, persists with the lock held. No IPC-level read-modify-write.

Update §5.4 RtkBanner.tsx to call these instead:

```ts
await SettingsAPI.setInjectRtkHook(true);
await SettingsAPI.sweepRtkHook(true);
```

Register both commands alongside `sweep_rtk_hook` and `get_rtk_startup_status` in `lib.rs::invoke_handler!`. Extend §5.2 SettingsAPI accordingly.

This fix also closes H4 (same family).

#### H4 — Startup auto-disable read-modify-write race with concurrent `update_settings`

**What.** §4.5.2:

```rust
let mut new_settings = {
    let s = settings_state.read().await;     // (1) acquire+release READ lock, clone
    s.clone()
};
new_settings.inject_rtk_hook = false;
save_settings(&new_settings)?;                // (2) sync disk write — outside any lock
{
    let mut s = settings_state.write().await; // (3) acquire write lock
    s.inject_rtk_hook = false;                // (4) flip in-memory only
}
```

Between (1) and (3), a concurrent `update_settings` Tauri call from the user can:
- Read state (still has inject=true).
- Save its own version to disk.
- Acquire the write lock and assign.

Now the startup task at (2) writes its OLD-but-flipped snapshot, overwriting the user's just-saved changes on disk. At (4) it patches only `inject_rtk_hook=false` in-memory — leaving in-memory roughly aligned with the user's update but disk reflecting the startup task's stale view. **Disk and memory disagree silently.**

**Why.** The startup task can run for seconds (the auto-disable OFF-sweep over many replicas, plus the initial `which::which` resolve). The user can open SettingsModal during that window, change anything, and click Save. Their save lands on disk briefly, then the startup task overwrites it.

**Fix.** Hold the write lock through the disk save, mirroring the pattern that `update_settings` already establishes:

```rust
let snapshot = {
    let mut s = settings_state.write().await;
    s.inject_rtk_hook = false;
    s.clone()
};
if let Err(e) = save_settings(&snapshot) {
    log::warn!("[rtk-startup] Failed to persist auto-disable: {}", e);
}
```

(Note: `update_settings` itself reads `root_token` outside the write lock — pre-existing issue, not new in #120, flagged separately for tech-lead's awareness but out of scope here.)

### 13.3 MEDIUM findings

#### M5 — Banner mount: snapshot-then-listen ordering loses emit-during-mount

**What.** §5.4 `RtkBanner.tsx::onMount`:

```ts
const initial = await SettingsAPI.getRtkStartupStatus(); // (1) snapshot
setMode(initial);
unlisten = await listen<...>(...);                       // (2) subscribe
```

If the setup task in §4.5.2 emits `rtk_startup_status` between (1) and (2), the event is dropped (no listener) and the banner stays at the snapshot value forever for this session.

**Why.** Setup task runs detached. `which::which` is fast (cached PATH) on warm boots. The setup task can emit BEFORE the banner's `onMount` is fully past `await listen()`. The snapshot might still read `silent` (if `getRtkStartupStatus` ran before setup-task's auto-disable side-effect), then the emit immediately fires `auto-disabled`, but no listener catches it.

**Fix.** Standard subscribe-then-snapshot:

```ts
unlisten = await listen<{ mode: RtkMode }>("rtk_startup_status", (e) => setMode(e.payload.mode));
const initial = await SettingsAPI.getRtkStartupStatus();
setMode(initial);
```

Worst case is the snapshot overwrites a freshly-arrived emit; on the next emit it self-corrects. Combined with idempotent `setMode`, no double-render concern.

#### M6 — Per-replica sweep errors are silently dropped at the UI layer

**What.** §5.3 SettingsModal and §5.4 RtkBanner both call `await SettingsAPI.sweepRtkHook(...)` and only catch the IPC-level reject:

```ts
try { await SettingsAPI.sweepRtkHook(next); }
catch (err) { console.error("[rtk] sweep failed:", err); }
```

The returned `RtkSweepResult.errors[]` array (per-dir failures) is never inspected. §7.1 documents this as "Future work could surface a toast — out of scope for #120" but in practice the plan does not even `console.error` per-dir failures.

**Why.** A 50-replica sweep where 5 fail (permission denied, network drive offline) reports `total=50, succeeded=45, errors=[...]`. The UI tells the user nothing. They have hooks partially applied with no signal that something went wrong. This is the "user knows the system is in partial state" gap the tech-lead flagged in question 6.

**Fix.** Cheap mitigation: log per-dir errors at every call site:

```ts
const result = await SettingsAPI.sweepRtkHook(next);
if (result.errors.length > 0) {
  console.error(
    `[rtk] sweep partial failure: ${result.errors.length}/${result.total} dirs failed`,
    result.errors,
  );
}
```

Apply at SettingsModal save AND banner Enable. A toast surface remains future work.

#### M7 — Symlinks/junctions in `enumerate_managed_agent_dirs` cause writes outside `project_paths`

**What.** §4.2.4 enumeration uses `p.is_dir()`, which follows symlinks/junctions. On Windows specifically, NTFS junctions (`mklink /J`) are common and indistinguishable from real directories at the `is_dir()` API level. `std::fs::FileType::is_symlink` does NOT detect Windows junctions on stable Rust; that is a documented limitation.

**Why.** Concrete scenarios:
- `_agent_foo` is a junction to `_agent_bar` in the same project: sweep writes the same `.claude/settings.local.json` twice (idempotent in disk content, but `total` is inflated, and concurrent writes increase risk of torn reads on Windows where `std::fs::write` is open-write-close).
- A user has `.ac-new/wg-X` as a junction to ANOTHER project's `wg-Y` — sweep writes into a project that is not in `project_paths`. From the user's mental model: "I only enabled rtk in project A; why is project B affected?"
- Worst plausible case: junction points outside the AC ecosystem (e.g., a reorganized directory layout). Our sweep writes there silently.

**Fix.** Two parts:
1. **Prefer `symlink_metadata` for the dir-check** so a symlink-to-dir is NOT followed:

   ```rust
   let md = match entry.path().symlink_metadata() {
       Ok(m) => m,
       Err(_) => continue,
   };
   if !md.is_dir() { continue; }
   ```

   This skips Unix symlinks-to-dir. Note: still does NOT detect Windows junctions on stable Rust — for that, use `std::os::windows::fs::MetadataExt::file_attributes()` and check `FILE_ATTRIBUTE_REPARSE_POINT`.

2. **Canonicalize and dedupe** (Windows-correct):

   ```rust
   let canonical = std::fs::canonicalize(&rp).unwrap_or_else(|_| rp.clone());
   if seen.insert(canonical.clone()) { out.push(rp); }
   ```

Add unit test #15: tempdir with a junction (Windows) or symlink (Unix) from `wg-1-team/__agent_X` to `wg-2-other/__agent_X` → enumeration returns each canonical path exactly once.

#### M8 — Sweep + `entity_creation` / `agent_creator` race on `.claude/settings.local.json` (NEW vs §12.3.3)

**What.** §12.3.3 covered ONLY the sweep-vs-sweep race. A separate, more-common race exists between any sweep iteration and a concurrent `ensure_claude_md_excludes` + `ensure_rtk_pretool_hook` pair from `entity_creation::create_workgroup` / `agent_creator::write_claude_settings_local`. Both sides perform read-modify-write on the same file with no locking.

**Why.** Concrete sequence (boot scenario with auto-disable + user creates workgroup):
1. Startup task auto-disable sweep iterating dirs. Currently in the middle of writing `__agent_X/.claude/settings.local.json` — read `{excludes:[...], hooks:[rtk]}`, mutated to `{excludes:[...]}`, ABOUT to write.
2. User clicks "Create Workgroup". `create_workgroup` runs `ensure_claude_md_excludes(__agent_X)` which reads `{excludes:[...], hooks:[rtk]}` (the pre-step-1 disk state — reads beat the in-flight write), writes `{excludes:[..., new_path], hooks:[rtk]}`.
3. Step 1's write lands. End state: `{excludes:[...]}`. The new exclude AND the user's other keys silently lost.

The window is narrow but real, and `create_workgroup` operates on N replicas in a tight loop, multiplying the race count by N per workgroup creation. This is a normal first-day flow (user installs AC, has rtk, kicks off a workgroup).

The same race exists between `sweep_rtk_hook` (user toggle path) and any concurrent `write_claude_settings_local` Tauri call, though the operator-typical concurrency is lower there.

**Fix.** Promote §12.3.3's `RtkSweepLock` (a `tokio::Mutex<()>`) to v1 AND extend its scope to wrap every call to `ensure_rtk_pretool_hook` AND `ensure_claude_md_excludes` from any caller. Aggressive but eliminates the entire family of races on these files.

Lighter alternative: file-based locking via `fs2 = "0.4"` (`FileExt::lock_exclusive`) on the `.claude/settings.local.json` itself — this also gives cross-instance safety (§7.4). Adds one dep.

Either way: "accept and document" is too lenient given the scenario is reachable on ordinary user flow, not a contrived race.

#### M9 — Rapid-toggle injectRtkHook produces silent partial state

**What.** Per §12.2 Q5 fix, the sweep fires from `handleSave`. User ticks → Save → `update_settings(true) + sweepRtkHook(true)`. User immediately unticks → Save → `update_settings(false) + sweepRtkHook(false)`. The two sweeps run concurrently against the same dir set; per-replica final state depends on which sweep wrote each dir last.

**Why.** End state: `settings.json` says inject=false, but ~50% of replicas still have the rtk hook on disk (or none, depending on interleaving). The UI claims "rtk is OFF" but Claude Code in those replicas still runs the rewriter. The user has no way to detect this.

**Fix.** Disable the SettingsModal Save button (or specifically the rtk checkbox row) while a `sweepRtkHook` call is in flight. Cheap UI gate, eliminates the race for the common path. Banner Enable button needs the same gate. Backend fix is the same `RtkSweepLock` from M8 — but the UI gate is independently effective and lighter.

Add to §5.3 + §5.4: a `busy()` signal that disables both controls until the in-flight sweep promise resolves.

#### M10 — Auto-disable cleanup uses byte-exact match; misses hooks injected by older AC builds

**What.** `remove_rtk_hook` filters by `command == RTK_REWRITER_COMMAND` byte-for-byte. If AC is upgraded between an injection and the auto-disable trigger, AND the upstream rtk rewriter command changed (an older AC bundled an older command string), the OFF-sweep walks past OLD hooks unchanged. Replicas keep stale broken hooks forever.

**Why.** rtk-ai is an external project; if its rewriter command evolves (e.g., the `skip` regex grows), AC's `RTK_REWRITER_COMMAND` constant updates in lockstep — but pre-upgrade replicas still hold the OLD command on disk. After upgrade:
- User uninstalls rtk → AC auto-disables → AC reports "I cleaned up, you're safe".
- Old hooks remain on disk.
- Claude Code in those replicas keeps trying to run rtk on every Bash → every Bash blocks/fails.
- The user trusts the "auto-disabled" banner. They don't dig.

**Fix.** Embed a marker substring in the rewriter command that is NEVER changed across AC upgrades, e.g. (in JS comment form, harmless to node):

```js
// @ac-rtk-marker-v1
node -e "..."
```

OFF-sweep filters by `command.contains("@ac-rtk-marker-v1")` instead of byte-exact. ON-sweep keeps byte-exact for idempotency. Old hooks (no marker) stay unmatched — accept this for the migration; document.

Alternative (heavier): maintain a `pub const RTK_LEGACY_COMMANDS: &[&str] = &[/* prior versions */]` and filter against the union. More maintenance burden.

For #120, the marker approach is preferred: pre-bake it into `RTK_REWRITER_COMMAND` BEFORE first ship so all future legacy concerns are covered from day one. Update §4.2.2 and the unit test #14 source-of-truth check accordingly.

#### M11 — UTF-8 BOM in `.claude/settings.local.json` is treated as malformed

**What.** `serde_json::from_str` does not strip a leading UTF-8 BOM (`\u{feff}`). On Windows, Notepad and various scripts add a BOM when saving JSON. Pre-H1 fix: BOM file → ON wipes. Post-H1 fix: BOM file → ON bails (silent no-op, the user expected rtk to apply but it didn't, no hint why).

**Why.** A BOM-prefixed `.claude/settings.local.json` is technically still a valid Claude Code config (node parsers handle BOM). AC fails to parse it, classifies as malformed, and either destroys it (pre-H1) or silently skips it (post-H1).

**Fix.** Strip BOM before parsing on BOTH ON and OFF read paths:

```rust
let cleaned = existing.strip_prefix('\u{feff}').unwrap_or(existing.as_str());
match serde_json::from_str::<serde_json::Value>(cleaned) { ... }
```

Add unit test #16: BOM-prefixed `{"claudeMdExcludes":[]}` → ON-sweep adds rtk hook successfully, BOM is dropped on write (acceptable — file remains valid UTF-8, no semantic change for downstream readers).

### 13.4 LOW findings

#### L12 — Test #7 byte-equality assertion is too strong

§4.2.5 test #7 claims "no spurious whitespace mismatch" after re-serialization. `serde_json::to_string_pretty` normalizes to its canonical 2-space pretty-print. If the user's file used 4-space indent, single-line JSON, or any non-canonical formatting, the output differs byte-for-byte even when the parsed structure is identical. Reword #7 to assert `serde_json::Value` structural equality, not byte equality.

#### L13 — `which::which` does not validate the binary; corrupted rtk passes detection

Acknowledged by tech-lead in question 4-2; recommend documenting in §11 (and in the user-facing release note) that "rtk in PATH" detection is structural, not functional. A `rtk --version` smoke test can be added if false-positive reports surface. No code change in #120.

#### L14 — Active-mode startup recovery sweeps unconditionally on every boot

§4.5.2 active-mode runs `ensure_rtk_pretool_hook(true)` against every dir on every boot. With 100 replicas this is 100 read+parse+serialize+write roundtrips of presumably identical content on a healthy system. `tokio::task::spawn_blocking` (already a §12.3.4 follow-up candidate) plus an mtime-skip can shave this; not blocking for v1.

#### L15 — `merge_rtk_hook` borrow chain may not compile cleanly on older rustc

`*hooks_root = Value::Object(...); hooks_root.as_object_mut().unwrap()` inside the same `match` arm has historically tripped pre-NLL/Polonius borrow checks. Modern rustc (1.71+) handles it. If the project's MSRV is older, dev-rust may need:

```rust
let needs_replace = !hooks_root.is_object();
if needs_replace { *hooks_root = Value::Object(serde_json::Map::new()); }
let hooks_obj = hooks_root.as_object_mut().unwrap();
```

Verify against project MSRV before writing the patch. Same caveat applies to the parallel `pretool` and inner-`hooks` blocks.

### 13.5 Phase A vs Phase B independence

The independence claim survives all findings. Per-finding phase mapping:

| Finding | Phase | Notes |
|---|---|---|
| H1 | A | `claude_settings.rs` only |
| H2 | A | `claude_settings.rs` only |
| H3 | A + B | New `set_inject_rtk_hook` / `set_rtk_prompt_dismissed` Tauri commands (Phase A); banner consumes them (Phase B) |
| H4 | A | `lib.rs::setup` only |
| M5 | B | banner JSX only |
| M6 | B | banner + SettingsModal JSX only |
| M7 | A | `claude_settings.rs` only |
| M8 | A | `commands/config.rs` + `entity_creation.rs` + `agent_creator.rs` |
| M9 | A + B | UI gate is Phase B; backend lock fallback is Phase A |
| M10 | A | `claude_settings.rs` constant + `remove_rtk_hook` filter |
| M11 | A | `claude_settings.rs` only |
| L12 | A | test only |
| L13 | A | doc only |
| L14 | A | follow-up |
| L15 | A | implementation note |

No finding requires Phase B to land before Phase A or vice versa. Phase A still ships independently after applying H1+H2+H4+M7+M8+M10+M11; Phase B applies H3-frontend+M5+M6+M9 against the new Phase A surface.

### 13.6 Summary

| Severity | ID | One-liner | Phase |
|---|---|---|---|
| HIGH | H1 | ON-path destroys malformed JSON (asymmetric to §12.3.1) | A |
| HIGH | H2 | ON-path silently overwrites non-object hooks / non-array PreToolUse | A |
| HIGH | H3 | Banner buttons IPC read-modify-write race vs SettingsModal | A + B |
| HIGH | H4 | Startup auto-disable RMW race vs `update_settings` | A |
| MEDIUM | M5 | Banner snapshot-then-listen ordering | B |
| MEDIUM | M6 | Per-replica sweep errors silently dropped at UI | B |
| MEDIUM | M7 | Symlinks / junctions leak writes outside `project_paths` | A |
| MEDIUM | M8 | Sweep vs entity_creation race on `.claude/settings.local.json` | A |
| MEDIUM | M9 | Rapid-toggle produces silent partial state across replicas | A + B |
| MEDIUM | M10 | Byte-exact cleanup misses hooks from older AC builds | A |
| MEDIUM | M11 | UTF-8 BOM treated as malformed | A |
| LOW | L12 | Test #7 byte-equality assertion is too strong | A |
| LOW | L13 | `which` does not smoke-test rtk | A |
| LOW | L14 | Active-mode recovery sweep is unconditional | A |
| LOW | L15 | `merge_rtk_hook` borrow chain may need MSRV-tweak | A |

— Grinch (round 1)

— dev-rust (review round 1)

---

## 14. Architect round 2 response (changelog)

This section is a per-finding changelog of round 2. The body of the plan (§1–§11) has been **rewritten in-place** to reflect every Accept; this section is the audit trail of why and where.

### HIGH findings — all accepted

#### H1 — ON-path destroys malformed JSON
- **Decision:** ACCEPT.
- **Patch:** §4.2.3 rewrote `ensure_rtk_pretool_hook`. The shared read+parse block now bails with `log::warn!` and returns `Ok(())` on any parse failure or non-object root, on both ON and OFF paths.
- **Test impact:** §4.2.5 test #10 inverted (was destructive-overwrite, now asserts file unchanged + warn logged). New test #13 added for OFF + malformed (was already in dev-rust §12.3.1 as a request).
- **§ where final code lives:** §4.2.3.

#### H2 — ON-path silently overwrites non-object hooks / non-array PreToolUse / non-array inner-hooks
- **Decision:** ACCEPT (option 1: bail with warn).
- **Patch:** §4.2.3 rewrote `merge_rtk_hook`. Three pre-checks added: `hooks` shape, `PreToolUse` shape, inner `hooks` shape. Each bails with a `discriminant_label`-formatted warn and returns `false` (caller skips the write). `remove_rtk_hook` mirrors the spirit (logs warn, skips that branch, never destroys).
- **Test impact:** §4.2.5 added tests #15 (wrong-shape `hooks`), #16 (wrong-shape `PreToolUse`), #17 (wrong-shape inner `hooks`), #18 (OFF mirror).
- **§ where final code lives:** §4.2.3.

#### H3 — Banner buttons IPC RMW race vs SettingsModal
- **Decision:** ACCEPT.
- **Patch:** §4.4.2 introduced two narrow Tauri commands `set_inject_rtk_hook(value)` and `set_rtk_prompt_dismissed(value)` that hold the SettingsState write lock through `save_settings`. §5.2 added the corresponding `SettingsAPI` wrappers. §5.4 RtkBanner replaced `get` + `update` with the narrow setters.
- **Test impact:** Manual pass #15 in §8.3 covers the regression scenario.
- **§ where final code lives:** §4.4.2 (backend), §5.2 (IPC), §5.4 (banner).

#### H4 — Startup auto-disable RMW race
- **Decision:** ACCEPT.
- **Patch:** §4.5.2 startup task rewrote the auto-disable block. The write lock is acquired ONCE around the field flip + clone, and `save_settings` runs from the cloned snapshot — disk and memory cannot diverge.
- **Test impact:** Manual pass #16 in §8.3.
- **§ where final code lives:** §4.5.2.

### MEDIUM findings (round-2 in-scope)

#### M7 — Symlinks / NTFS junctions escape `project_paths`
- **Decision:** ACCEPT.
- **Patch:** §4.2.4 rewrote `enumerate_managed_agent_dirs` with: (a) `symlink_metadata` instead of `is_dir` for the dir-check, (b) Windows `FILE_ATTRIBUTE_REPARSE_POINT` check via `MetadataExt`, (c) canonicalize+dedupe via `HashSet<PathBuf>`. The wg-* parent dirs are also re-checked against the same filters.
- **Test impact:** §4.2.5 added tests #19 (junction skipped) and #20 (canonical dedupe).
- **§ where final code lives:** §4.2.4.

#### M8 — Sweep vs `entity_creation` / `agent_creator` race
- **Decision:** ACCEPT (in-process only, via `tokio::Mutex<()>` State; cross-process deferred).
- **Justification (vs. fs2):** §9 Q10. tokio Mutex is zero-new-dep, integrates with existing `Arc<...>::manage()` pattern, and matches the in-process-only scope already implicit elsewhere in AC (cross-process race is documented in §7.4 since #84 and not in scope here).
- **Patch:** §4.5.1b registers `RtkSweepLockState`. §4.4.3 sweep, §4.5.2 startup auto-disable + active-recovery, §4.6.1/4.6.3/4.6.4 callers all acquire the lock. CLI (§4.6.2) does NOT — out-of-process, can't share the in-process Mutex; cross-process race documented as a follow-up.
- **Test impact:** No new automated test (the race is hard to reproduce deterministically). Manual pass #16 covers the auto-disable + concurrent save scenario.
- **§ where final code lives:** §4.4.3, §4.5.1b, §4.5.2, §4.6.

#### M10 — Byte-exact OFF-cleanup misses older AC builds
- **Decision:** ACCEPT (pre-ship — adopt marker).
- **Justification (vs. legacy-array alternative):** §9 Q9. A single fixed marker is trivial maintenance vs. a growing `RTK_LEGACY_COMMANDS: &[&str]`. The cost is one one-time edit to `repo-AgentsCommander/.claude/settings.json`. **This is the highest-risk decision** because it cannot be added post-launch without leaving older hooks unreachable forever — locking it in pre-ship.
- **Patch:** §4.2.2 added `RTK_HOOK_MARKER` const and re-baked `RTK_REWRITER_COMMAND` with the marker prefix. §4.2.3 `merge_rtk_hook` and `remove_rtk_hook` filter by `command.contains(RTK_HOOK_MARKER)` instead of byte-equality. §10 documents the pre-ship requirement and the future bump procedure (`v2` + `RTK_LEGACY_MARKERS`). §2 added `repo-AgentsCommander/.claude/settings.json` as a file to touch.
- **Test impact:** §4.2.5 added test #14 (source-of-truth byte-check) and #21 (marker-only-different-body — idempotency + removal).
- **§ where final code lives:** §4.2.2, §4.2.3, §10.

#### M11 — UTF-8 BOM treated as malformed
- **Decision:** ACCEPT.
- **Patch:** §4.2.3 the read block now `strip_prefix('\u{feff}')` before `serde_json::from_str`. Applies on both ON and OFF.
- **Test impact:** §4.2.5 added test #22.
- **§ where final code lives:** §4.2.3.

### MEDIUM findings deferred to Phase B (dev-webpage-ui)

These are deferred to Phase B execution but the plan body now references them so dev-webpage-ui sees them when implementing.

- **M5 — Banner subscribe-then-snapshot order.** §5.4 was rewritten: subscribe FIRST, snapshot second. Idempotent `setMode` handles the worst-case redundant apply.
- **M6 — Per-replica sweep errors silently dropped.** §5.3 (`handleSave`) and §5.4 (banner) both inspect `RtkSweepResult.errors[]` and log via `console.error` if non-empty.
- **M9 — Rapid-toggle silent partial state.** §5.3 added a `rtkSweepInFlight` signal that disables both the Save button and the rtk checkbox during the in-flight sweep. §5.4 banner has the equivalent `busy()` gate (already present in round 1; round 2 confirmed it's necessary, not just defensive).

### LOW findings — implementation notes

All four LOW findings (L12–L15) are accepted and recorded in §11 as implementation notes. No structural code changes; dev-rust is asked to apply at write time:

- **L12** — tests use structural `Value` equality, not byte equality. Done in §4.2.5.
- **L13** — `which` is structural detection only. Documented in §11 + acceptance criteria for the release note.
- **L14** — active-mode recovery sweeps every boot. Acceptable for v1; `spawn_blocking`+mtime-skip is a follow-up.
- **L15** — `merge_rtk_hook` borrow chain may need MSRV-tweak. dev-rust verifies before patching; sketch given in §4.2.3.

### Decisions specifically requested by tech-lead

**M10 (marker):** ADOPTED, pre-ship-only — see §9 Q9 and §10. **This is locked here as the highest-risk decision** because it requires a one-time, coordinated edit to both the Rust constant and `repo-AgentsCommander/.claude/settings.json`, and cannot be retrofitted post-launch without losing the ability to clean up older hooks.

**M8 (locking):** `tokio::Mutex<()>` State — see §9 Q10. fs2 file-based locking remains a follow-up if cross-process incidents surface.

### Sections rewritten in-place

- §1 Overview — architectural-decision bullets re-derived from H1–H4, M7, M8, M10, M11.
- §2 Files to touch — added `repo-AgentsCommander/.claude/settings.json` (M10 source-of-truth), expanded `claude_settings.rs` line, expanded `commands/config.rs` line (4 new commands), expanded `lib.rs` line, added M5/M6/M9 notes against the frontend lines.
- §4.2.2 — RTK_HOOK_MARKER const + marker-bearing RTK_REWRITER_COMMAND.
- §4.2.3 — full rewrite of `ensure_rtk_pretool_hook` + `merge_rtk_hook` + `remove_rtk_hook` with bail-on-malformed (H1), bail-on-wrong-shape (H2), BOM strip (M11), marker filter (M10).
- §4.2.4 — full rewrite of `enumerate_managed_agent_dirs` with `symlink_metadata` + Windows reparse-point check + canonical dedupe (M7).
- §4.2.5 — test list expanded from 12 to 22 cases.
- §4.4 — added narrow setters subsection (4.4.2) for H3+H4; renumbered the sweep to 4.4.3 with lock acquisition for M8.
- §4.5.1 — registers all 4 new commands.
- §4.5.1b — NEW: `RtkSweepLockState` definition + Tauri State registration.
- §4.5.2 — startup task rewrite for H4 + M8.
- §4.6.1 / §4.6.3 / §4.6.4 — each acquires `RtkSweepLockState` around the helper sequence.
- §4.6.2 — explicit note that CLI does NOT acquire the lock (cross-process scope, §7.4).
- §5.2 — added `setInjectRtkHook` and `setRtkPromptDismissed` to `SettingsAPI`.
- §5.3 — full rewrite: snapshot at modal open, sweep from `handleSave` only when changed, log per-error (M6), UI gate (M9).
- §5.4 — full rewrite of `RtkBanner.tsx`: subscribe-first (M5), narrow setters (H3), error logging (M6), busy gate (M9).
- §6 — behavior matrix updated: checkbox uses handleSave path; banner uses narrow setters.
- §7 — atomicity rewritten: §7.4 split into in-process (closed) and cross-process (deferred); §7.5 NEW: RtkSweepLockState design.
- §8 — test count updated to 22; round-2 manual passes #10–#16 added.
- §9 — open questions all closed; round-2 questions Q9 + Q10 added with final decisions.
- §10 — marker pre-ship requirement documented; bump procedure for v2.
- §11 — L12–L15 implementation notes added.

### What did NOT change

- Phase split (§3) is unchanged. Phase A is still independently landable.
- Settings-field naming (Q2). Same fields, same defaults.
- The `which = "7"` dependency choice (Q7).
- The banner mount point (Q8) — `MainApp` only.
- The high-level frontend file inventory (one new component, one CSS section).

— Architect (round 2)

---

## 15. Dev-rust review (round 2)

### 15.1 Verdict

**APPROVED-WITH-NITS.** The plan correctly absorbs every grinch HIGH/MEDIUM/LOW finding and dev-rust round-1 enrichment into §1–§14. The structural rewrite is sound and Phase A independence holds. However, **one critical nit (N1) must be applied at implementation time**: the narrow setters (`set_inject_rtk_hook`, `set_rtk_prompt_dismissed`) and the startup auto-disable block claim "lock held through `save_settings`" in their docstrings/§14 changelog, but the code as written drops the write lock **before** `save_settings` runs. This leaves H3+H4 partially fixed — a small RMW window remains. The patch is a 3-line restructure documented below; no plan-body edit needed beyond the docstring fix once the code is corrected.

### 15.2 Verification of the four points tech-lead requested

| # | Point | Verdict | Notes |
|---|---|---|---|
| 1 | Grinch fixes applied as architect declared | ✓ All 4 H, 4 M (in-scope), 3 M (Phase B), 4 L addressed | See 15.3 cross-table |
| 2 | Narrow Tauri commands contract | ✗ See N1 (critical nit) | Code drops lock before save; docstring says otherwise |
| 3 | M8 lock acquisition sites | ✓ 5 acquired + 1 documented exclusion (CLI) | Granularity correct: per-replica in `create_workgroup`, whole-loop in sweep |
| 4 | M10 marker form + filtering | ✓ Marker placement valid JS; substring match on both ON and OFF | Source-of-truth edit pre-ship is locked in §2 + §10 |

### 15.3 Cross-table — every finding to where it landed

| Finding | Status | § final code | § final tests | Notes |
|---|---|---|---|---|
| H1 (ON destroys malformed) | Applied | §4.2.3 shared parse-bail | §4.2.5 #10 inverted, #13 added | Symmetric with OFF |
| H2 (wrong-shape silent overwrite) | Applied | §4.2.3 `merge_rtk_hook` 3 pre-checks | §4.2.5 #15, #16, #17, #18 | OFF mirror is per-entry skip (correct) |
| H3 (banner IPC RMW) | **Partially applied** | §4.4.2, §5.4 | §8.3 manual #15 | **N1: lock-not-held-through-save** |
| H4 (auto-disable RMW) | **Partially applied** | §4.5.2 | §8.3 manual #16 | **N1: lock-not-held-through-save** |
| M5 (banner subscribe-then-snapshot) | Applied | §5.4 onMount | n/a (manual) | listen first, snapshot second |
| M6 (silent per-dir errors) | Applied | §5.3 + §5.4 console.error | n/a (manual) | toast surface = future |
| M7 (symlinks/junctions) | Applied | §4.2.4 closure | §4.2.5 #19, #20 | `symlink_metadata` + reparse-point + canonicalize |
| M8 (sweep vs entity_creation race) | Applied | §4.5.1b + 5 sites | n/a (manual #16) | Per-replica granularity in `create_workgroup` |
| M9 (rapid-toggle partial state) | Applied | §5.3 + §5.4 busy gate | n/a (manual) | UI gate on Save + checkbox + Enable |
| M10 (byte-exact misses old hooks) | Applied | §4.2.2 marker pre-ship | §4.2.5 #14, #21 | Source `.claude/settings.json` edit required pre-ship |
| M11 (BOM treated as malformed) | Applied | §4.2.3 strip BOM both paths | §4.2.5 #22 | BOM dropped on write — acceptable per plan |
| L12 (test #7 byte-equality) | Applied | §4.2.5 #7 reworded | n/a | Structural `Value` eq, not byte |
| L13 (`which` is structural detection) | Documented | §11 | n/a | Release-note copy |
| L14 (active recovery sweeps every boot) | Documented | §11 | n/a | `spawn_blocking` follow-up |
| L15 (MSRV borrow chain) | Documented | §4.2.3 + §11 | n/a | dev-rust verifies before patching |

### 15.4 N1 — Critical nit: lock not held through `save_settings`

**Where.** §4.4.2 (`set_inject_rtk_hook`, `set_rtk_prompt_dismissed`) and §4.5.2 (startup auto-disable block).

**Symptom.** The current code in §4.4.2 reads:

```rust
let snapshot = {
    let mut s = settings.write().await;
    s.inject_rtk_hook = value;
    s.clone()
};                              // <-- write guard `s` dropped here; lock RELEASED
save_settings(&snapshot)        // <-- runs WITHOUT the lock
```

The `let snapshot = { ... };` block returns the clone and drops `s` (the write guard) at the closing `};`. `save_settings` then runs outside the lock. The docstring at §4.4.2:759 ("Holds the SettingsState write lock through `save_settings`") and the §1 Overview bullet ("narrow setters that hold the `SettingsState` write lock through `save_settings`") and the §14 changelog claim for H3 ("hold the SettingsState write lock through `save_settings`") all promise lock-held-through-save. The code does not deliver.

**Why it matters (concrete divergence).** Concurrent scenario `set_rtk_prompt_dismissed(true)` racing `update_settings({modal_draft})`:

1. T0 — User opens SettingsModal, ticks `coordSortByActivity`. Modal draft: `{coordSortByActivity:true, rtkPromptDismissed:false (original)}`.
2. T1 — User clicks `[Don't ask again]` on banner.
3. T2 — `set_rtk_prompt_dismissed` acquires write, sets `dismissed=true`, clones `snap` (snap has `dismissed=true, coord=false`), **releases lock at the `};`**.
4. T3 — User clicks Save in modal.
5. T4 — `update_settings` reads root_token (separate pre-existing race), validates, `save_settings(draft)`. **Disk: `coord=true, dismissed=false`**.
6. T5 — `update_settings` acquires write lock, `*s = draft`. **Memory: `coord=true, dismissed=false`**.
7. T6 — `set_rtk_prompt_dismissed` (still pending from step 3) calls `save_settings(snap)`. **Disk: `coord=false, dismissed=true`**.
8. **End state: memory `coord=true, dismissed=false`; disk `coord=false, dismissed=true`. DIVERGENCE.**

Window is small (~1ms) but real. Resolves on next boot — but for one session the user perceives "Save" silently lost their `coordSortByActivity` change.

The same race applies to §4.5.2 startup auto-disable: setup task takes snapshot under write lock, releases, calls `save_settings`. A concurrent `update_settings` between release and save can land first; setup's save then overwrites disk while memory holds the user's update. Manual pass #16 in §8.3 was supposed to verify this, but with the current code it would intermittently fail.

**Fix — concrete patches.**

`set_inject_rtk_hook` and `set_rtk_prompt_dismissed` (§4.4.2): hold the guard through the save by keeping `s` in scope until after `save_settings`:

```rust
#[tauri::command]
pub async fn set_inject_rtk_hook(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.inject_rtk_hook = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s); // explicit; lock released after disk write succeeded
    Ok(())
}

#[tauri::command]
pub async fn set_rtk_prompt_dismissed(
    settings: State<'_, SettingsState>,
    value: bool,
) -> Result<(), String> {
    let mut s = settings.write().await;
    s.rtk_prompt_dismissed = value;
    let snapshot = s.clone();
    save_settings(&snapshot)?;
    drop(s);
    Ok(())
}
```

The explicit `drop(s)` is documentation; it's also the natural drop point at the end of the function. The crux is that `save_settings(&snapshot)?` runs **before** `s` falls out of scope, so the guard outlives the save.

§4.5.2 startup auto-disable: same shape, keeping `s` alive across `save_settings`:

```rust
if mode == "auto-disabled" {
    // H4 fix (corrected): hold the SettingsState write lock through
    // save_settings so a concurrent update_settings cannot interleave.
    let project_paths = {
        let mut s = settings_state.write().await;
        s.inject_rtk_hook = false;
        let snapshot = s.clone();
        if let Err(e) = crate::config::settings::save_settings(&snapshot) {
            log::warn!("[rtk-startup] Failed to persist auto-disable: {}", e);
        }
        snapshot.project_paths.clone()
        // `s` (write guard) lives until end of this block.
    };

    // M8 fix: hold RtkSweepLock through the OFF-sweep loop.
    let _guard = sweep_lock.lock().await;
    for dir in enumerate_managed_agent_dirs(&project_paths) {
        if let Err(e) = ensure_rtk_pretool_hook(&dir, false) {
            log::warn!(
                "[rtk-startup] auto-disable sweep failed for {}: {}",
                dir.display(),
                e
            );
        }
    }
}
```

The block returns `project_paths` (`Vec<String>`); `s` is dropped at the block's `}`. `save_settings` ran while `s` was alive. After the block, `s` is gone and we can acquire the sweep_lock without lock-order conflicts (no `s` in scope).

**No plan-body edit required** — when dev-rust applies these patches, the docstring claims of §4.4.2 and §1 become true. If the plan is not updated, also tighten the existing docstring on `set_inject_rtk_hook` (line 748–752) to reflect the actual `drop(s)` placement so future readers can match docstring to code.

**Severity rationale.** This is a critical nit, not NEEDS-CHANGES, because (a) the plan structure is correct, (b) the patch is mechanical, (c) §14's H3+H4 changelog accurately describes the intent — only the implementation diverges. Leaving it would silently re-introduce the very races round 2 is supposed to close.

### 15.5 Minor nits (not blocking)

**N2 — `tokio::sync::Mutex` poisoning wording in §7.5.** §7.5 says "if the future holding the guard panics, tokio's `Mutex` poisons by dropping the guard." `tokio::sync::Mutex` does **not** poison (unlike `std::sync::Mutex`); the guard simply drops. The next sentence ("Subsequent acquirers proceed normally") is correct. Reword to "no poisoning; the guard drops on panic, subsequent acquirers proceed normally" so the docs don't mislead future readers debugging Mutex behavior.

**N3 — `let mut push_if_new` in §4.2.4.** The closure `push_if_new` takes its mutable state as parameters (`out`, `seen`), not via capture. It does not need `mut` on the binding. Compiler will emit `unused_mut` warning. Either remove `mut` or refactor to a private free function (the architect mentions this option in the §4.2.4 trailing note). dev-rust's call at write-time.

**N4 — Test #14 and the `repo-AgentsCommander/.claude/settings.json` source-of-truth edit ordering.** The test (§4.2.5 #14) asserts `RTK_REWRITER_COMMAND` byte-equal to `repo-AgentsCommander/.claude/settings.json`'s `command` field. If dev-rust commits `RTK_REWRITER_COMMAND` (with `'@ac-rtk-marker-v1';` prefix) **before** editing the source `.claude/settings.json` (still missing the marker), `cargo test` fails on test #14 between the two commits.

Confirmed via `grep -c "@ac-rtk-marker-v1" repo-AgentsCommander/.claude/settings.json` → `0` at the audit timestamp; the source file has not yet been updated. **Implementation note:** edit `repo-AgentsCommander/.claude/settings.json` as part of the same atomic commit that adds `RTK_HOOK_MARKER` + `RTK_REWRITER_COMMAND`. Otherwise stage the source edit first.

### 15.6 New cabos sueltos — none material

I scanned for issues introduced by the round-2 in-place rewrite that were not present in round 1. None found beyond N1–N4 above. Specifically:

- **No new lock-order risk.** All sites acquire `SettingsState` write/read first, `RtkSweepLockState` second. `update_settings` does not touch `RtkSweepLockState`. No cycle.
- **No new race on `project_paths`.** Read inside or outside the write lock as appropriate; mid-loop toggle behavior is documented (§4.6.4 trailing note).
- **No new test gap.** §4.2.5's 22 cases cover the H1/H2/M7/M10/M11 surface. The H3/H4 race is intentionally tested via manual passes #15/#16 (hard to deterministically trigger in a unit test).
- **No new file-scope creep.** §2 lists `repo-AgentsCommander/.claude/settings.json` as the only Phase-A addition vs. round 1, justified by M10. Marker edit is JS-inert.

### 15.7 Phase A vs Phase B independence — reaffirmed

- After Phase A (with N1 patched at write-time), `cargo check` and `cargo test` are green. The 4 new Tauri commands plus `RtkSweepLockState` plus the marker-bearing constant all compile clean. Test #14 passes if the source `.claude/settings.json` edit is included in the same Phase A commit (see N4).
- Frontend ignores the new fields silently. The startup task emits to no-listener — harmless.
- Phase B applies cleanly: `setInjectRtkHook` and `setRtkPromptDismissed` IPC wrappers, the `handleSave`-driven sweep, the banner with subscribe-first ordering and busy gate, all consume Phase A's surface without coupling-back.

### 15.8 Recommendation

Merge to consensus pending N1 patch at implementation time. N2–N4 are document-when-touched. dev-rust commits the four-line N1 fix as part of the §4.4.2 + §4.5.2 implementation diff; no plan-body restructure required.

— dev-rust (round 2)

---

## 16. Grinch round 2 verdict

### 16.1 Verdict

**APPROVED-WITH-NITS.** Architect absorbed all 4 HIGH and all 4 in-scope MEDIUM findings cleanly, plus the 3 Phase-B MEDIUMs and the 4 LOWs. The structural rewrite is sound and Phase A independence still holds. I converge with dev-rust's verdict.

**Single critical concern**: dev-rust's N1 is correct — the plan-body code in §4.4.2 (`set_inject_rtk_hook` / `set_rtk_prompt_dismissed`) and §4.5.2 (auto-disable block) **does NOT hold the lock through `save_settings`**, despite the surrounding docstrings, the §1 Overview, and the §14 H3/H4 changelog claiming otherwise. H3 and H4 are therefore only **partially** closed in the plan as currently written. Dev-rust's §15.4 patch is exactly correct and closes them; my one extension is below in §16.3.

### 16.2 Per-finding verification

#### HIGH (4) — applied with N1 dependency

| ID | Architect intent | Plan-body code | After dev-rust N1 patch |
|---|---|---|---|
| H1 (ON malformed-bail) | ✓ Correct | ✓ Correct (§4.2.3 shared parse-bail at line 247) | ✓ Closed |
| H2 (ON wrong-shape bail) | ✓ Correct | ✓ Correct (3 pre-checks in `merge_rtk_hook`, mirrored skips in `remove_rtk_hook`) | ✓ Closed |
| H3 (banner IPC RMW) | ✓ Narrow setters via `set_inject_rtk_hook` / `set_rtk_prompt_dismissed`, banner uses them | ✗ N1 — block-scoped guard releases lock before `save_settings` | ✓ Closed only after N1 patch |
| H4 (auto-disable RMW) | ✓ Lock-held-through-save | ✗ Same N1 in §4.5.2 auto-disable block | ✓ Closed only after N1 patch |

H1 + H2 verification details:
- §4.2.3 shared `read + BOM-strip + parse + non-object check` with `log::warn!` + `return Ok(())`. Applies to BOTH ON and OFF. ✓
- `merge_rtk_hook` returns `bool` for "did mutate"; pre-checks `hooks` (line 309–319), `PreToolUse` (327–335), inner `hooks` (376–386); each bails with `discriminant_label`-formatted warn → `return false` → outer caller writes nothing. ✓
- `remove_rtk_hook` mirrors the spirit per-entry: a wrong-shape inner hooks at one index is skipped (`continue`) without affecting siblings — correct. ✓
- Tests #15, #16, #17 cover the three ON wrong-shape levels. Test #18 covers OFF mirror. ✓

H3 + H4 verification details:
- §4.4.2 narrow setters declared. Banner consumes them (§5.4 onEnable line 1465, onDismissPrompt line 1486). IPC RMW window between banner's `get` and `update` is gone — replaced by single-shot setter call. ✓ at the structural level.
- BUT: as dev-rust correctly identified, the implementation pattern `let snapshot = { let mut s = ...; ...; s.clone() };` releases the guard at `};` BEFORE `save_settings(&snapshot)` runs. A concurrent `update_settings` can land between the guard release and the save, producing the disk/memory divergence dev-rust traces in §15.4. **This is a real race**, not a stylistic issue.
- Dev-rust's patch in §15.4 (keep `s` in function scope; `save_settings(&snapshot)?` runs while guard is alive; explicit `drop(s)` at end of function) is mechanically correct and closes the window.

#### MEDIUM in-scope (4) — all applied

| ID | Plan section | Status |
|---|---|---|
| M7 (symlinks/junctions) | §4.2.4 | ✓ `symlink_metadata` + Windows `FILE_ATTRIBUTE_REPARSE_POINT` (0x0400) + canonicalize/`HashSet<PathBuf>` dedupe. Tests #19 (junction skip) and #20 (canonical dedupe) added. |
| M8 (locking) | §4.5.1b + §4.4.3 + §4.5.2 + §4.6.1/3/4 | ✓ `RtkSweepLockState = Arc<tokio::sync::Mutex<()>>` registered, acquired at all 5 in-process sites, CLI explicitly excluded with rationale (§4.6.2). Per-replica granularity in `create_workgroup` (§4.6.4) is correct — keeps critical section short while still serializing per-file work. |
| M10 (marker pre-ship) | §4.2.2 + §4.2.3 + §10 | ✓ `RTK_HOOK_MARKER = "@ac-rtk-marker-v1"` placed as leading JS string-expression statement (`'@ac-rtk-marker-v1';`) — JS-inert (verified: directive in statement position, valid in cmd.exe and bash quoting); ON-sweep idempotency uses `cmd.contains(MARKER)` (line 350); OFF-sweep removal uses `!s.contains(MARKER)` (line 466). Source-of-truth edit to `repo-AgentsCommander/.claude/settings.json` is locked under §2 + test #14, with explicit pre-ship requirement in §10. |
| M11 (BOM strip) | §4.2.3 | ✓ `raw.strip_prefix('\u{feff}').unwrap_or(raw.as_str())` runs before `from_str` on both ON and OFF read paths (line 244). Test #22 added. |

#### MEDIUM Phase B (3) — all applied

| ID | Plan section | Status |
|---|---|---|
| M5 (subscribe-then-snapshot) | §5.4 onMount lines 1444–1457 | ✓ `listen` registered FIRST (line 1445), then `getRtkStartupStatus` snapshot (1453). Idempotent `setMode` handles both orderings. |
| M6 (per-replica errors silent) | §5.3.2 line 1380 + §5.4 line 1468 | ✓ Both call sites inspect `result.errors` and `console.error` non-empty arrays with the `M/N dirs failed` template. |
| M9 (rapid-toggle gate) | §5.3.3 + §5.4 busy() | ✓ `rtkSweepInFlight` signal disables Save button + checkbox row in modal during sweep (5.3.1 line 1342, 5.3.3 line 1413); banner has equivalent `busy()` gate on both buttons (line 1505, 1510). |

#### LOW (4) — all addressed as documented

| ID | Status |
|---|---|
| L12 (test #7 byte-equality) | ✓ §4.2.5 #7 reworded to structural `Value` equality. |
| L13 (`which` smoke-test) | ✓ Documented in §11. |
| L14 (recovery sweep cost) | ✓ Documented in §11; spawn_blocking + mtime-skip remains a follow-up. |
| L15 (MSRV borrow chain) | ✓ §4.2.3 trailing note + alternative pattern shown at lines 511–521. |

### 16.3 N1 — concur with dev-rust, with one additional recommendation

Dev-rust's N1 analysis at §15.4 is **technically correct** and the patch is the right shape. I confirm independently:
- The block `let snapshot = { let mut s = settings.write().await; s.inject_rtk_hook = value; s.clone() };` drops `s` at the closing `};` per Rust's standard scope rules. `save_settings(&snapshot)` then runs without any lock held.
- The race is concrete: a concurrent `update_settings` between the guard release and `save_settings` produces disk/memory divergence (dev-rust's T0–T8 trace in §15.4 is accurate).
- The patch (`s` in function scope, `save_settings` runs while guard is alive, explicit `drop(s)` at end) is the mechanical fix.

**My one extension to dev-rust's recommendation:** **the plan body should be updated** so §4.4.2 and §4.5.2 show the corrected pattern. Dev-rust said "no plan-body edit required" because they will apply the patch in the implementation diff. This is operationally true, but it leaves a permanent inconsistency between the spec (plan) and the code:

- The plan IS the spec. Future code reviews, audits, "does the diff match what was approved?" gates will compare the implementation against the plan body.
- A plan body that says X and code that does Y is a **trap for the next person who touches this file**. They will read the plan, see the broken pattern, assume it's correct (because it was "approved"), and propagate or reintroduce it.
- Dev-rust correctly notes the docstring at §4.4.2 lines 748–752 ("Holds the SettingsState write lock through `save_settings`") would be FALSE under the current plan-body code. That docstring is shipped with the implementation, not just an internal note.

This is a 6-line edit to two `pub async fn` bodies. Cost is trivial; the consistency win is permanent.

**Concrete request:** before Step 6 (implementation), architect updates §4.4.2 (both setters) and §4.5.2 (auto-disable block) to match dev-rust's §15.4 patches. The §1 Overview claim and §14 changelog already say what the corrected code does — only the code blocks lag.

Severity: I would not block on this — it is **APPROVED-WITH-NITS**, not NEEDS-CHANGES — but I rate the plan-body update as **MUST DO** before implementation, not "optional polish".

### 16.4 New findings (round-2-introduced) — none material

I attacked the round-2 rewrite for new races, lock-order issues, and correctness gaps. Findings:

#### N1' — Lock ordering is safe (not a finding, just confirming dev-rust §15.6)

I traced lock acquisition order at every site:

| Site | Order |
|---|---|
| `set_inject_rtk_hook`, `set_rtk_prompt_dismissed` | settings.write only (no sweep_lock) |
| `update_settings` (existing) | settings.write only |
| `sweep_rtk_hook` | sweep_lock → settings.read (briefly co-held) → release settings.read → loop |
| Startup auto-disable (§4.5.2) | settings.write (released) → save_settings → sweep_lock |
| Startup active recovery | settings.read (released) → sweep_lock |
| §4.6.1 / §4.6.3 / §4.6.4 | settings.read (released) → sweep_lock |

The only site that briefly co-holds both locks is `sweep_rtk_hook` (sweep_lock first, then settings.read). No site holds settings.write while waiting for sweep_lock. **No deadlock cycle exists.** Under the dev-rust N1 patch, `set_inject_rtk_hook` adds another "settings.write only" site — still safe.

Concur with dev-rust §15.6.

#### N2' — Marker collision risk (LOW, awareness only)

`RTK_HOOK_MARKER = "@ac-rtk-marker-v1"` is matched via substring. A user who happens to write a manual hook command containing the literal substring `@ac-rtk-marker-v1` (e.g., as a JS comment or string literal in their own custom rewriter) would have their hook removed by AC's OFF-sweep. This is essentially impossible by accident — the marker is namespaced (`@ac-` prefix, `-v1` suffix) and unique enough — but not impossible by adversarial intent.

**Acceptance:** the marker is sufficiently unique. **Document, do not change.** §10 already documents the bump procedure if a collision is ever observed in the wild.

#### N3' — `sweep_rtk_hook` blocks all entity creation during long sweeps (LOW, already covered)

This is an instance of grinch L14 (active-mode recovery sweeps unconditionally). With 100+ replicas and a slow disk, the boot sweep can hold `RtkSweepLockState` for several seconds, blocking any concurrent `create_agent_matrix` / `create_workgroup` / `write_claude_settings_local` Tauri call.

User impact: the very first action a user takes after AC boot may hang for the duration of the boot sweep. Mitigations are documented (`spawn_blocking` + mtime-skip). For v1, accept.

No new action needed; already in §11 follow-ups.

#### N4' — Confirm dev-rust's N2 (poisoning wording) (LOW)

`tokio::sync::Mutex` does NOT poison on panic, unlike `std::sync::Mutex`. §7.5 line 1657 says "tokio's `Mutex` poisons by dropping the guard. Subsequent acquirers proceed normally." This conflates two things: tokio's Mutex DOES drop the guard on panic, but it does NOT enter a poisoned state — the next `lock()` call succeeds normally.

Dev-rust caught this in N2. Confirming and lifting from "minor nit" to "fix at implementation time" — the docs become a debugging foot-gun for whoever next maintains the lock-related code.

**Concrete reword:** "if the future holding the guard panics, the guard is dropped — `tokio::sync::Mutex` does NOT poison (unlike `std::sync::Mutex`); subsequent acquirers proceed normally."

### 16.5 Phase A vs Phase B independence — reaffirmed

Phase A still ships independently after applying:
- N1 patch at §4.4.2 + §4.5.2 (4 + ~6 lines respectively).
- Optional: §7.5 wording fix for N2.
- All other Phase A changes from §14 changelog.

Phase B applies cleanly against Phase A's surface. No coupling-back. M5 + M6 + M9 are all Phase B-only. dev-webpage-ui has full responsibility.

### 16.6 Recommendation

**APPROVED-WITH-NITS** with the following pre-Step-6 actions, in priority order:

1. **MUST** — apply dev-rust's N1 patch as plan-body edits to §4.4.2 (both setters) and §4.5.2 (auto-disable block). The current code blocks contradict the surrounding documentation; leaving them is a quiet bug-magnet.
2. **SHOULD** — fix §7.5 line 1657 wording per N2'/N2 (tokio `Mutex` does NOT poison).
3. **NICE** — N3 / N4 (dev-rust): unused `mut` on closure binding, source-of-truth edit ordering. Implementation-time cleanups.

If architect or dev-rust pushes back on item 1 ("we'll fix it at implementation time, plan body stays as-is"), I'd re-ratify as APPROVED-WITH-NITS but flag the doc-vs-code drift as a permanent risk in the issue tracker.

After items 1 + 2 are applied, this is ready for Step 6 (dev-rust implements Phase A; dev-webpage-ui implements Phase B). No round 3 escalation needed unless N1 pushback.

— grinch (round 2)

---

## 17. Architect round 3 response

Round 3 is mechanical convergence — both reviewers (dev-rust §15, grinch §16) APPROVED-WITH-NITS on the same items. No pushback; the patches go in as proposed.

### Patches applied to plan body

#### N1 (MUST) — Lock-held-through-save in setters + startup auto-disable

**Where:** §4.4.2 (`set_inject_rtk_hook` and `set_rtk_prompt_dismissed`) AND §4.5.2 (startup auto-disable block).

**Pattern adopted:**

```rust
let mut s = settings.write().await;
s.field = value;
let snapshot = s.clone();
save_settings(&snapshot)?;
drop(s); // explicit; lock released AFTER the disk write
Ok(())
```

The earlier `let snapshot = { let mut s = ...; s.clone() }; save_settings(&snapshot)` form dropped the guard at the closing `};` BEFORE `save_settings` ran — directly contradicting the §1 Overview claim "lock held through save_settings" and §14 changelog claim same. Plan-body now matches the documentation. dev-rust's exact patch in §15.4 was applied verbatim (modulo the obvious adaptation for the startup block, where `project_paths` must be cloned out of `s` before `drop(s)`).

**Code locations updated:**
- §4.4.2 `set_inject_rtk_hook`
- §4.4.2 `set_rtk_prompt_dismissed`
- §4.5.2 startup auto-disable block (the `if mode == "auto-disabled"` arm)

#### N2 (SHOULD) — §7.5 wording fix on `tokio::sync::Mutex` poisoning

**Where:** §7.5 last bullet (failure mode).

**Old:** "tokio's `Mutex` poisons by dropping the guard"

**New:** "the guard is dropped — `tokio::sync::Mutex` does NOT poison (unlike `std::sync::Mutex`); subsequent acquirers proceed normally"

Plus a parenthetical noting the round-3 wording fix.

`tokio::sync::Mutex` does not implement poisoning; the previous wording was a debugging foot-gun for any reader assuming `std::sync::Mutex` semantics. Locked.

### Optional NICE patches — both applied

#### N3 (NICE) — Drop unused `mut` on closure binding in §4.2.4

**Where:** `let mut push_if_new = ...` in `enumerate_managed_agent_dirs`.

**Change:** `let mut push_if_new` → `let push_if_new`.

The closure receives `out` and `seen` as `&mut` parameters; it does NOT mutate captured environment, so the binding does not need `mut`. Without this fix, rustc would emit `unused_mut` lint at compile time. Trivial pre-fix.

#### N4 (NICE) — Test #14 commit ordering note

**Where:** §4.2.5 test #14 description.

**Change:** appended a "Round-3 N4 implementation note" stating that the `RTK_REWRITER_COMMAND` constant edit AND the `repo-AgentsCommander/.claude/settings.json` marker prefix edit MUST land in the **same commit**.

Splitting them across commits breaks `cargo test` on the intermediate commit and harms bisectability. Cheap to document; saves dev-rust from a self-inflicted CI failure during landing.

### What was NOT changed

- §12, §13, §14, §15, §16 are preserved verbatim as historical record per tech-lead's rules.
- §1 Overview, §14 changelog, §10 migration: no edits — they already correctly described the lock-held-through-save semantics that §4.4.2/§4.5.2 only now match.
- All other code blocks (sweep, callers, banner, settings struct, tests #1–#13, #15–#22) are untouched.

### Ready for Step 6

The plan body is now self-consistent: every claim about lock-held-through-save in prose has matching code. Every `tokio::sync::Mutex` reference has correct semantics documented. The closure binding compiles without lints. The unit-test contract for source-of-truth alignment has explicit commit-ordering guidance.

**Verdict: ready for Step 6.**
- dev-rust executes Phase A (settings fields, helpers, sweep + narrow setters + lock state, startup task, four call sites, source-of-truth `.claude/settings.json` marker prefix, 22 unit tests + 2 round-trip tests).
- dev-webpage-ui executes Phase B (types, IPC, SettingsModal handleSave-time sweep + UI gate, RtkBanner subscribe-first + narrow setters + busy gate, mount in MainApp, CSS).

Phase A and Phase B can land in parallel; the plan §3 phase split is unchanged.

— Architect (round 3)


