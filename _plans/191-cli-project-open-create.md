# Plan: CLI verbs to open/create AC projects (#191)

**Branch:** `feature/191-cli-project-open-create`
**Issue:** https://github.com/mblua/AgentsCommander/issues/191

---

## 1. Requirement

### Goal
Let a user register/open an existing AC project — and create a new one — from the CLI:

```
<bin> open-project <path>     # validate .ac-new exists, register the path in settings
<bin> new-project  <path>     # ensure .ac-new exists (create if missing), register the path
```

Both verbs are **idempotent** (re-running with the same path is a no-op for `project_paths`) and must **share the same Rust backend logic** with the GUI's "New Project" / "Open Project" buttons (`src/sidebar/components/ActionBar.tsx:58-94`, `src/sidebar/components/Toolbar.tsx:11-25`). Today the dedup-and-persist step lives only in the frontend (`src/sidebar/stores/project.ts:163-171`); this plan moves it to a shared backend module so CLI and UI behave identically.

### Non-goals
- No discovery work in the CLI (the agents/teams/workgroups walk in `discover_project` / `discover_ac_agents` is GUI-only — CLI just registers the path; the next GUI launch discovers).
- No replacement for `check_project_path` / `create_ac_project` / `discover_project`. They keep their public Tauri surface (web/browser code can still hit them).
- No watcher that reloads `settings.json` in a running GUI when the CLI mutates it. (See §6 race note.)
- No new authentication. The CLI verbs are user-local (any process with shell access can already mutate `settings.json` directly); requiring `--token` would add zero security and divergent UX from `git init` / `npm init`.

---

## 2. Final verb signatures

### `<bin> open-project`
```
<bin> open-project <PATH>
```
| Arg     | Required | Description                                           |
|---------|----------|-------------------------------------------------------|
| `PATH`  | yes      | Folder that already contains a `.ac-new/` subdirectory|

**Stdout on success (exit 0):**
- New registration: `Registered project: <abs-path>`
- Already registered: `Project already registered: <abs-path>`

**Stderr on error (exit 1):** `Error: <message>` (see error matrix in §4).

### `<bin> new-project`
```
<bin> new-project <PATH>
```
| Arg     | Required | Description                                           |
|---------|----------|-------------------------------------------------------|
| `PATH`  | yes      | Folder to make into an AC project (created if missing)|

**Stdout on success (exit 0):**
- Newly created: `Created AC project at <abs-path>` followed by `Registered project: <abs-path>` (two lines)
- `.ac-new` already existed and registered: `AC project already exists at <abs-path>` + `Registered project: <abs-path>`
- Both already in place: `AC project already exists at <abs-path>` + `Project already registered: <abs-path>`

**Stderr on error (exit 1):** as above.

### Why positional `<PATH>` instead of `--path`
Mirrors the conventional shell verbs the user already invokes (`cd <path>`, `code <path>`, `git init <path>`). The flag-based pattern in this codebase (`--root`, `--target`) exists because `send` / `close-session` / `brief-set-title` need *several* required strings; `open-project` / `new-project` need exactly one and would be noisier with `--path`.

### Exit codes
`0` on success (including idempotent re-runs). `1` on every error. Matches every other CLI verb (`cli/send.rs`, `cli/close_session.rs`, `cli/brief_set_title.rs`).

---

## 3. Architecture — the shared helper

### Why a new module
The dedup + register-in-settings logic must run from two non-IPC-related sites:
1. The Tauri commands `open_project` / `new_project` (which hold the live `SettingsState` write lock).
2. The CLI verbs `open_project` / `new_project` (which have no Tauri runtime — they call `load_settings_for_cli()` / `save_settings()` directly; see §4.13 for why the CLI gets a dedicated loader).

Putting the logic in `commands/ac_discovery.rs` would force the CLI to depend on Tauri's `State` machinery; putting it in the CLI module would force the Tauri command to import from `cli/`. A new `config/projects.rs` keeps it Tauri-free and CLI-free, mirroring how `cli/brief_ops.rs` factored out `BriefOp::*` for the brief verbs.

### Module layout
```
src-tauri/src/
  config/
    mod.rs           ← add `pub mod projects;`
    projects.rs      ← NEW — pure logic
  cli/
    mod.rs           ← add OpenProject + NewProject Commands variants
    open_project.rs  ← NEW
    new_project.rs   ← NEW
  commands/
    ac_discovery.rs  ← add 2 new Tauri commands at bottom (existing 5 untouched)
  lib.rs             ← add 2 new commands to invoke_handler!
```

### Path normalisation contract (mirror frontend exactly)
The frontend dedup-key is `path.replace(/\\/g, "/").toLowerCase()` (`src/sidebar/stores/project.ts:17-19`). The Rust helper MUST use the byte-equivalent form so a CLI-registered `C:\foo` and a GUI-registered `c:/FOO` collapse to one entry. The contract also strips any trailing `/` so that shell tab-completion (which appends `\` on directories) does not silently double-register `C:\foo\` and `C:\foo` (Round-1 dev-rust IR.3.2). **Do not** call `std::fs::canonicalize` on Windows — it returns UNC `\\?\` paths that would compare unequal to the GUI's non-UNC entries and silently double-register.

```rust
fn normalize_for_compare(s: &str) -> String {
    s.replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}
```

The same trailing-`/` strip is applied symmetrically to the frontend's `normalizePath` (§4.10) so the FE-side dedup key matches the Rust-side key byte-for-byte.

### Absolute-path resolution (CLI surface only)
The GUI always passes an absolute path (the folder picker returns one). The CLI accepts a relative path (`open-project .`) and must produce an absolute, lexically-normalised path before persisting. Use `std::path::absolute(raw)` — stable since Rust 1.79 (the workspace toolchain is `rustc 1.93.1`, confirmed via `rustc --version`). On Windows (the project's primary target), `std::path::absolute` resolves the path against the process CWD via `GetFullPathNameW` and collapses `.`/`..` segments lexically without performing IO. This closes Round-1 G4 (silent double-registration of `..\projects` from different CWDs) on Windows. On POSIX, the std API preserves `..` segments for symlink-safety reasons; the residual gap is documented as §6.10 (out of scope for this PR). **Do not** call `std::fs::canonicalize` (no symlink resolution) — preserve the user-typed shape so the persisted entry survives a symlink retarget.

---

## 4. Affected files — exact changes

### 4.1 NEW: `src-tauri/src/config/projects.rs`
Create the file with the contents below. This module owns the open/new contract.

```rust
//! Shared open/new-project logic. Used by both the Tauri commands
//! (`commands::ac_discovery::open_project` / `new_project`) and the CLI verbs
//! (`cli::open_project` / `cli::new_project`). The same code path means UI and
//! CLI cannot diverge on dedup, validation, or registration order.
//!
//! This module is intentionally Tauri-free and CLI-free — it operates on a
//! mutable `&mut AppSettings` borrow plus a `&Path`. Callers own the
//! lock-acquire and the `save_settings` call.

use std::path::{Path, PathBuf};

use super::settings::AppSettings;

/// Outcome of a register call. Callers translate this into the verb-specific
/// stdout / IPC payload (CLI prints the lines from §2; Tauri command returns
/// the struct verbatim — `#[serde(rename_all = "camelCase")]`).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRegistration {
    /// Absolute path that was added (or matched) in `project_paths`.
    pub path: String,
    /// `true` when this call appended a new entry; `false` when the path was
    /// already present (case-insensitive, slash-normalised match).
    pub registered: bool,
    /// `true` when this call created `.ac-new/` on disk. Always `false` for
    /// `open_project`. `true` for `new_project` only when the directory did
    /// not already exist.
    pub created: bool,
}

/// Errors returned by the helper. `Display` strings are the exact stderr text
/// the CLI prints (prefixed with `Error: ` by the caller — see §4.4).
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("path '{0}' is empty")]
    EmptyPath(String),
    #[error("path does not exist: {0}")]
    PathMissing(PathBuf),
    #[error("path is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("no AC project at {0} (.ac-new/ not found)")]
    AcNewMissing(PathBuf),
    #[error("failed to resolve absolute path for '{0}': {1}")]
    CwdFailure(String, std::io::Error),
    #[error("failed to create .ac-new directory at {0}: {1}")]
    AcNewCreateFailed(PathBuf, std::io::Error),
    #[error("failed to write .ac-new/.gitignore at {0}: {1}")]
    GitignoreFailed(PathBuf, String),
}

/// Validate an existing AC project and register it in `settings.project_paths`.
/// Errors when the path is missing, not a directory, or has no `.ac-new/`.
///
/// On success, mutates `settings.project_paths` (appends if new) and
/// `settings.project_path` (legacy single-project field — kept in sync with
/// `project_paths[0]` to match the frontend's `persistProjectPaths` contract
/// at `src/sidebar/stores/project.ts:163-171`).
///
/// Caller is responsible for `save_settings(settings)` AFTER this returns Ok.
pub fn register_existing_project(
    settings: &mut AppSettings,
    raw_path: &str,
) -> Result<ProjectRegistration, ProjectError> {
    let abs = absolutise(raw_path)?;
    if !abs.exists() {
        return Err(ProjectError::PathMissing(abs));
    }
    if !abs.is_dir() {
        return Err(ProjectError::NotADirectory(abs));
    }
    let ac_new = abs.join(".ac-new");
    if !ac_new.is_dir() {
        return Err(ProjectError::AcNewMissing(abs));
    }
    let abs_str = abs.to_string_lossy().into_owned();
    let registered = upsert_project_path(settings, &abs_str);
    Ok(ProjectRegistration {
        path: abs_str,
        registered,
        created: false,
    })
}

/// Ensure the AC project structure exists (creating `.ac-new/` and its
/// `.gitignore` when missing) and register it in `settings.project_paths`.
///
/// Errors only when the path is empty, the parent does not exist, or
/// `.ac-new/` cannot be created. A pre-existing `.ac-new/` is fine — the
/// gitignore sweep is opportunistic (matches `discover_project`'s behaviour
/// at `src-tauri/src/commands/ac_discovery.rs:1308-1309`).
pub fn register_new_project(
    settings: &mut AppSettings,
    raw_path: &str,
) -> Result<ProjectRegistration, ProjectError> {
    let abs = absolutise(raw_path)?;
    // Allow PATH to not yet exist as a directory. Reject if PATH exists and
    // is a regular file (caller almost certainly fat-fingered).
    if abs.exists() && !abs.is_dir() {
        return Err(ProjectError::NotADirectory(abs));
    }
    let ac_new = abs.join(".ac-new");
    // Ensure the parent (PATH itself) exists so the non-recursive
    // `create_dir` below can race-detect properly. `create_dir_all` is
    // idempotent on an already-existing dir, so this costs nothing extra
    // when PATH is already there.
    std::fs::create_dir_all(&abs)
        .map_err(|e| ProjectError::AcNewCreateFailed(abs.clone(), e))?;
    // Authoritative `created` flag (Round-1 G9): use non-recursive
    // `create_dir` so we can distinguish "we made the dir" from "another
    // process beat us to it" via `ErrorKind::AlreadyExists`. The previous
    // `is_dir()` check then `create_dir_all` pattern lied under that race.
    let created = match std::fs::create_dir(&ac_new) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(ProjectError::AcNewCreateFailed(ac_new.clone(), e)),
    };
    // Gitignore sweep (Round-1 G15): mandatory when we just created `.ac-new`
    // (a fresh AC project must ship with the protective patterns), best-effort
    // when `.ac-new` pre-existed (a transient FS error on someone else's
    // gitignore should not fail registration of a perfectly valid project).
    match crate::commands::ac_discovery::ensure_ac_new_gitignore(&ac_new) {
        Ok(()) => {}
        Err(e) if !created => {
            log::warn!(
                "[projects] gitignore sweep failed on pre-existing .ac-new at {:?}: {} (best-effort, continuing)",
                ac_new, e
            );
        }
        Err(e) => return Err(ProjectError::GitignoreFailed(ac_new.clone(), e)),
    }

    let abs_str = abs.to_string_lossy().into_owned();
    let registered = upsert_project_path(settings, &abs_str);
    Ok(ProjectRegistration {
        path: abs_str,
        registered,
        created,
    })
}

// ── Private helpers ───────────────────────────────────────────────────────

fn absolutise(raw: &str) -> Result<PathBuf, ProjectError> {
    if raw.trim().is_empty() {
        return Err(ProjectError::EmptyPath(raw.to_string()));
    }
    // `std::path::absolute` (stable since Rust 1.79; toolchain is 1.93.1)
    // lexically resolves the path against the process CWD. On Windows it
    // also collapses `.`/`..` segments via `GetFullPathNameW` — closing
    // Round-1 G4 (silent double-registration of `..\projects` from
    // different CWDs). On POSIX the std API preserves `..` for
    // symlink-safety reasons; documented as §6.10. No filesystem IO,
    // no symlink resolution.
    std::path::absolute(raw)
        .map_err(|e| ProjectError::CwdFailure(raw.to_string(), e))
}

/// Mirrors the frontend `normalizePath` at
/// `src/sidebar/stores/project.ts:17-19`. Comparison only — the persisted
/// entry retains the original byte sequence.
fn normalize_for_compare(s: &str) -> String {
    // Slashes normalised, lowercased, trailing `/` stripped. The trailing
    // strip closes Round-1 IR.3.2 (shell tab-completion appends `\` on dirs;
    // without trim, `C:\foo\` and `C:\foo` would become DIFFERENT entries).
    s.replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

/// Append `abs_path` to `settings.project_paths` iff no existing entry
/// normalises to the same key. Always re-syncs `settings.project_path` to
/// `project_paths[0]` so the legacy single-project field never drifts.
/// Returns `true` if a new entry was added.
fn upsert_project_path(settings: &mut AppSettings, abs_path: &str) -> bool {
    let key = normalize_for_compare(abs_path);
    let exists = settings
        .project_paths
        .iter()
        .any(|p| normalize_for_compare(p) == key);
    let appended = if exists {
        false
    } else {
        settings.project_paths.push(abs_path.to_string());
        true
    };
    // Keep legacy `projectPath` field in lockstep with the head of the list,
    // matching the frontend's `persistProjectPaths` at
    // `src/sidebar/stores/project.ts:166-170`.
    settings.project_path = settings.project_paths.first().cloned();
    appended
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::AppSettings;

    /// Auto-cleaned temp dir; mirrors `cli::brief_ops::tests::FixtureRoot`.
    struct FixtureRoot(PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl FixtureRoot {
        fn new(prefix: &str) -> Self {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}",
                prefix,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                h.finish()
            ));
            std::fs::create_dir_all(&path).expect("fixture root");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    // ── register_existing_project ────────────────────────────────────────

    #[test]
    fn open_rejects_empty_path() {
        let mut s = AppSettings::default();
        assert!(matches!(
            register_existing_project(&mut s, ""),
            Err(ProjectError::EmptyPath(_))
        ));
    }

    #[test]
    fn open_rejects_missing_path() {
        let fix = FixtureRoot::new("proj-open-missing");
        let p = fix.path().join("does-not-exist");
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, p.to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::PathMissing(_))));
        assert!(s.project_paths.is_empty());
    }

    #[test]
    fn open_rejects_path_without_ac_new() {
        let fix = FixtureRoot::new("proj-open-noacnew");
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::AcNewMissing(_))));
        assert!(s.project_paths.is_empty());
    }

    #[test]
    fn open_registers_path_with_ac_new() {
        let fix = FixtureRoot::new("proj-open-ok");
        std::fs::create_dir_all(fix.path().join(".ac-new")).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(r.registered);
        assert!(!r.created);
        assert_eq!(s.project_paths.len(), 1);
        assert_eq!(s.project_path.as_deref(), Some(r.path.as_str()));
    }

    #[test]
    fn open_is_idempotent_on_repeat_call() {
        let fix = FixtureRoot::new("proj-open-idem");
        std::fs::create_dir_all(fix.path().join(".ac-new")).unwrap();
        let mut s = AppSettings::default();
        let _ = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        let r2 = register_existing_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(!r2.registered);
        assert_eq!(s.project_paths.len(), 1);
    }

    #[test]
    fn open_dedup_is_case_insensitive_and_slash_agnostic() {
        let fix = FixtureRoot::new("proj-open-norm");
        std::fs::create_dir_all(fix.path().join(".ac-new")).unwrap();
        let mut s = AppSettings::default();
        // Seed an entry with the exact path
        let original = fix.path().to_string_lossy().to_string();
        let _ = register_existing_project(&mut s, &original).unwrap();
        // Re-register with mixed slash + case
        let mangled = original.replace('\\', "/").to_uppercase();
        let r2 = register_existing_project(&mut s, &mangled).unwrap();
        assert!(!r2.registered, "case+slash variant should dedup");
        assert_eq!(s.project_paths.len(), 1);
        // Original retained, NOT replaced with the mangled form.
        assert_eq!(s.project_paths[0], original);
    }

    // ── register_new_project ─────────────────────────────────────────────

    #[test]
    fn new_creates_ac_new_when_missing() {
        let fix = FixtureRoot::new("proj-new-mkdir");
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(r.created);
        assert!(r.registered);
        assert!(fix.path().join(".ac-new").is_dir());
        assert!(fix.path().join(".ac-new").join(".gitignore").is_file());
    }

    #[test]
    fn new_skips_creation_when_ac_new_already_exists() {
        let fix = FixtureRoot::new("proj-new-existing");
        std::fs::create_dir_all(fix.path().join(".ac-new")).unwrap();
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(!r.created);
        assert!(r.registered);
        // gitignore swept opportunistically even though .ac-new pre-existed
        assert!(fix.path().join(".ac-new").join(".gitignore").is_file());
    }

    #[test]
    fn new_is_idempotent_for_registration() {
        let fix = FixtureRoot::new("proj-new-idem");
        let mut s = AppSettings::default();
        let _ = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        let r2 = register_new_project(&mut s, fix.path().to_str().unwrap()).unwrap();
        assert!(!r2.created);
        assert!(!r2.registered);
        assert_eq!(s.project_paths.len(), 1);
    }

    #[test]
    fn new_rejects_when_path_is_a_regular_file() {
        let fix = FixtureRoot::new("proj-new-file");
        let f = fix.path().join("file.txt");
        std::fs::write(&f, b"x").unwrap();
        let mut s = AppSettings::default();
        let r = register_new_project(&mut s, f.to_str().unwrap());
        assert!(matches!(r, Err(ProjectError::NotADirectory(_))));
        assert!(s.project_paths.is_empty());
    }

    // ── upsert keeps legacy projectPath in lockstep ───────────────────────

    #[test]
    fn upsert_syncs_legacy_project_path_field() {
        let fix1 = FixtureRoot::new("proj-legacy-1");
        let fix2 = FixtureRoot::new("proj-legacy-2");
        std::fs::create_dir_all(fix1.path().join(".ac-new")).unwrap();
        std::fs::create_dir_all(fix2.path().join(".ac-new")).unwrap();
        let mut s = AppSettings::default();
        let r1 = register_existing_project(&mut s, fix1.path().to_str().unwrap()).unwrap();
        assert_eq!(s.project_path.as_deref(), Some(r1.path.as_str()));
        let r2 = register_existing_project(&mut s, fix2.path().to_str().unwrap()).unwrap();
        // project_path tracks the HEAD of the list (same as FE persistProjectPaths).
        assert_eq!(s.project_path.as_deref(), Some(r1.path.as_str()));
        assert_eq!(s.project_paths, vec![r1.path.clone(), r2.path.clone()]);
    }

    // ── absolutise: relative + dot-dot collapse (Round-1 G4 + G13) ────────

    /// CWD is process-wide; restore on Drop. Any other test that mutates
    /// CWD in this same module would race — keep this confined.
    struct CwdGuard(PathBuf);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.0);
        }
    }

    #[test]
    fn absolutise_resolves_relative_path_against_cwd() {
        let fix = FixtureRoot::new("proj-rel");
        std::fs::create_dir_all(fix.path().join(".ac-new")).unwrap();
        let prev = std::env::current_dir().unwrap();
        let _guard = CwdGuard(prev);
        std::env::set_current_dir(fix.path()).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, ".").unwrap();
        // Persisted path must be absolute and equal to fix.path() lexically
        // (after `std::path::absolute(".")` collapses the trailing `.`).
        assert!(Path::new(&r.path).is_absolute(), "not absolute: {}", r.path);
        let normalized_persisted = r.path.replace('\\', "/").to_lowercase();
        let normalized_fixture =
            fix.path().to_string_lossy().replace('\\', "/").to_lowercase();
        assert_eq!(normalized_persisted, normalized_fixture);
    }

    /// `..` collapse is Windows-only behaviour of `std::path::absolute`
    /// (POSIX preserves `..` for symlink-safety). On Windows the persisted
    /// path must contain no `..` component.
    #[cfg(windows)]
    #[test]
    fn absolutise_collapses_dotdot_segments_on_windows() {
        let fix = FixtureRoot::new("proj-dotdot");
        let project = fix.path().join("project");
        std::fs::create_dir_all(project.join(".ac-new")).unwrap();
        let sibling = fix.path().join("sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        let prev = std::env::current_dir().unwrap();
        let _guard = CwdGuard(prev);
        std::env::set_current_dir(&sibling).unwrap();
        let mut s = AppSettings::default();
        let r = register_existing_project(&mut s, "..\\project").unwrap();
        assert!(
            !r.path.contains(".."),
            "persisted path should not contain `..` on Windows: {}",
            r.path
        );
    }

    // ── trailing-separator dedup (Round-1 IR.3.2) ────────────────────────

    #[test]
    fn upsert_dedup_strips_trailing_separator() {
        let fix = FixtureRoot::new("proj-trailing");
        std::fs::create_dir_all(fix.path().join(".ac-new")).unwrap();
        let mut s = AppSettings::default();
        let original = fix.path().to_string_lossy().to_string();
        let _ = register_existing_project(&mut s, &original).unwrap();
        // Add `\` (or `/` on POSIX) to simulate shell tab-completion.
        let with_trailing = if cfg!(windows) {
            format!("{}\\", original)
        } else {
            format!("{}/", original)
        };
        let r2 = register_existing_project(&mut s, &with_trailing).unwrap();
        assert!(
            !r2.registered,
            "trailing-separator variant should dedup: {} vs {}",
            original, with_trailing
        );
        assert_eq!(s.project_paths.len(), 1);
    }

    // ── serde camelCase shape lock (Round-1 G14) ─────────────────────────

    #[test]
    fn project_registration_serializes_camel_case() {
        // Today's fields are already lowercase single-words, so no rename
        // happens. This test locks the invariant: a future field like
        // `ac_new_dir` must serialize to `acNewDir`. If the
        // `#[serde(rename_all = "camelCase")]` attribute is ever dropped,
        // this test catches it before the FE silently breaks.
        let r = ProjectRegistration {
            path: "X".to_string(),
            registered: true,
            created: false,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"path\""), "missing path field: {}", json);
        assert!(json.contains("\"registered\""), "missing registered field: {}", json);
        assert!(json.contains("\"created\""), "missing created field: {}", json);
        // No snake_case relics from any current field name.
        assert!(!json.contains("ac_new"), "snake_case field name leaked: {}", json);
    }
}
```

### 4.2 MODIFIED: `src-tauri/src/config/mod.rs`
Add the new module declaration. Insert at line 7 (after `pub mod teams;`):

**Before (lines 1–8):**
```rust
pub mod agent_config;
pub mod claude_settings;
pub mod profile;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;
pub mod teams;

use std::path::PathBuf;
```

**After:**
```rust
pub mod agent_config;
pub mod claude_settings;
pub mod profile;
pub mod projects;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;
pub mod teams;

use std::path::PathBuf;
```

### 4.3 MODIFIED: `src-tauri/src/commands/ac_discovery.rs`
Two new Tauri commands. The visibility on `ensure_ac_new_gitignore` is already `pub(crate)` (line 1213), so the helper at `config::projects::register_new_project` can call it across crates.

**Insertion anchor (Round-1 G3):** insert immediately AFTER `set_replica_context_files` (closing `}` at line 1674) and BEFORE the `#[cfg(test)] mod tests` opener at line 1676. Do NOT append at end-of-file — that would land the new commands AFTER the test module and after the `read_brief_fields` orphan helpers (lines 1770–1782), which violates the file's tests-last convention.

```rust
// ── #191 — shared open/new project commands ──────────────────────────────

/// Validate an existing AC project at `path` and register it in
/// `settings.project_paths`. Mirrors the ActionBar "Open Project" flow at
/// `src/sidebar/components/ActionBar.tsx:78-94` but performs the dedup +
/// persist atomically against `SettingsState`.
///
/// Holds the SettingsState write lock through `save_settings` — same pattern
/// as `set_inject_rtk_hook` (`src-tauri/src/commands/config.rs:184-194`) — so
/// concurrent `update_settings` calls cannot race.
#[tauri::command]
pub async fn open_project(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let mut s = settings.write().await;
    let result = crate::config::projects::register_existing_project(&mut s, &path)
        .map_err(|e| e.to_string())?;
    let snapshot = s.clone();
    crate::config::settings::save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(result)
}

/// Ensure an AC project at `path` (creating `.ac-new/` if missing) and
/// register it in `settings.project_paths`. Mirrors the ActionBar "New
/// Project" flow at `src/sidebar/components/ActionBar.tsx:58-71`.
#[tauri::command]
pub async fn new_project(
    settings: State<'_, SettingsState>,
    path: String,
) -> Result<crate::config::projects::ProjectRegistration, String> {
    let mut s = settings.write().await;
    let result = crate::config::projects::register_new_project(&mut s, &path)
        .map_err(|e| e.to_string())?;
    let snapshot = s.clone();
    crate::config::settings::save_settings(&snapshot)?;
    drop(s); // explicit; lock released AFTER the disk write completes
    Ok(result)
}
```

### 4.4 MODIFIED: `src-tauri/src/lib.rs`
Register the two new commands in `invoke_handler!`. Insert between `commands::ac_discovery::create_ac_project,` and `commands::ac_discovery::discover_project,` (the absolute line numbers in the original Round-0 plan were off by 2; use the surrounding-line anchors as authoritative — Round-1 G10):

**Before (lines 832–838):**
```rust
            commands::ac_discovery::discover_ac_agents,
            commands::ac_discovery::check_project_path,
            commands::ac_discovery::create_ac_project,
            commands::ac_discovery::discover_project,
            commands::ac_discovery::get_replica_context_files,
            commands::ac_discovery::set_replica_context_files,
            commands::entity_creation::create_agent_matrix,
```

**After:**
```rust
            commands::ac_discovery::discover_ac_agents,
            commands::ac_discovery::check_project_path,
            commands::ac_discovery::create_ac_project,
            commands::ac_discovery::open_project,
            commands::ac_discovery::new_project,
            commands::ac_discovery::discover_project,
            commands::ac_discovery::get_replica_context_files,
            commands::ac_discovery::set_replica_context_files,
            commands::entity_creation::create_agent_matrix,
```

### 4.5 NEW: `src-tauri/src/cli/open_project.rs`
```rust
//! `open-project <PATH>` CLI verb — validate an existing AC project and
//! register it in `settings.project_paths`. Shares the registration logic
//! with the Tauri command at `commands::ac_discovery::open_project` via the
//! `config::projects` module.
//!
//! No `--token` requirement: project registration mutates the user-local
//! `settings.json`, which any process with shell access can already write to.
//! Adding token gating would not change the security boundary; it would only
//! diverge the CLI UX from `git init` / `npm init`.
//!
//! GUI concurrency caveat: when AC's GUI is running, its in-memory
//! `SettingsState` is the source of truth. A subsequent GUI `update_settings`
//! built from a stale snapshot can clobber a CLI-registered entry. Documented
//! in the plan §6 — a watcher/reload story is a follow-up issue.

use clap::Args;

use crate::config::projects::{register_existing_project, ProjectError};
use crate::config::settings::{load_settings_for_cli, save_settings};

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Register an existing AC project so it appears in the GUI sidebar on \
next launch. The folder must already contain `.ac-new/` (use `new-project` to \
create one).\n\n\
PATH: Absolute or relative — relative paths are resolved against the current \
working directory. The persisted entry is the absolute form.\n\n\
IDEMPOTENCY: Re-registering the same path is a no-op; the verb prints \
\"Project already registered\" and exits 0.")]
pub struct OpenProjectArgs {
    /// Path to an existing AC project folder (must contain `.ac-new/`)
    pub path: String,
}

pub fn execute(args: OpenProjectArgs) -> i32 {
    // Use the CLI-specific loader (Round-1 G5): unlike `load_settings`, this
    // does NOT auto-generate or persist a `root_token`, so error paths and
    // pre-validation reads do not silently rewrite settings.json.
    let mut settings = load_settings_for_cli();
    let result = match register_existing_project(&mut settings, &args.path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            // Append CLI-specific guidance when the user pointed at a folder
            // without `.ac-new/`. The bare error string is GUI-friendly
            // (Round-1 G8); only the CLI knows about `new-project`.
            if matches!(e, ProjectError::AcNewMissing(_)) {
                eprintln!("Hint: use `new-project <PATH>` to create the .ac-new structure.");
            }
            return 1;
        }
    };
    if result.registered {
        if let Err(e) = save_settings(&settings) {
            eprintln!("Error: failed to persist settings: {}", e);
            return 1;
        }
        println!("Registered project: {}", result.path);
    } else {
        println!("Project already registered: {}", result.path);
    }
    log::info!(
        "[cli] open-project: path={} registered={}",
        result.path,
        result.registered
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct FixtureRoot(PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl FixtureRoot {
        fn new(prefix: &str) -> Self {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}",
                prefix,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                h.finish()
            ));
            std::fs::create_dir_all(&path).expect("fixture root");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    // The CLI execute() touches the live settings.json on disk. These two
    // tests exercise the arg parsing + early-error paths only — the
    // persistence-mutating success paths are covered by `config::projects`
    // unit tests, which use an in-memory AppSettings.

    #[test]
    fn open_project_returns_1_when_path_missing() {
        let fix = FixtureRoot::new("cli-open-missing");
        let bogus = fix.path().join("does-not-exist");
        let code = execute(OpenProjectArgs {
            path: bogus.to_string_lossy().into(),
        });
        assert_eq!(code, 1);
    }

    #[test]
    fn open_project_returns_1_when_no_ac_new() {
        let fix = FixtureRoot::new("cli-open-noacnew");
        let code = execute(OpenProjectArgs {
            path: fix.path().to_string_lossy().into(),
        });
        assert_eq!(code, 1);
    }

    #[test]
    fn help_text_documents_open_project() {
        use clap::CommandFactory;
        let help = crate::cli::Cli::command().render_help().to_string();
        assert!(help.contains("open-project"), "help missing verb: {}", help);
    }
}
```

### 4.6 NEW: `src-tauri/src/cli/new_project.rs`
```rust
//! `new-project <PATH>` CLI verb — ensure an AC project structure at PATH
//! (creating `.ac-new/` if missing) and register it in
//! `settings.project_paths`. Shares the registration logic with the Tauri
//! command at `commands::ac_discovery::new_project` via the
//! `config::projects` module.
//!
//! Same GUI concurrency caveat as `open-project` — see that file.

use clap::Args;

use crate::config::projects::register_new_project;
use crate::config::settings::{load_settings_for_cli, save_settings};

#[derive(Args)]
#[command(after_help = "\
PURPOSE: Create an AC project at PATH (mkdir-p `.ac-new/` and write its \
`.gitignore` if missing) and register it in the GUI sidebar's project list.\n\n\
PATH: Absolute or relative — relative paths are resolved against the current \
working directory. The folder is created if it does not yet exist.\n\n\
IDEMPOTENCY: Re-running on a folder that already has `.ac-new/` is safe — the \
gitignore is swept (missing patterns appended), and the registration step \
deduplicates against any prior entry.")]
pub struct NewProjectArgs {
    /// Path to make into an AC project (folder created if missing)
    pub path: String,
}

pub fn execute(args: NewProjectArgs) -> i32 {
    // Round-1 G5: use the CLI-specific loader so we never trigger a spurious
    // root_token write on first-boot or error-path invocations.
    let mut settings = load_settings_for_cli();
    let result = match register_new_project(&mut settings, &args.path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    // Save when we either created `.ac-new` or appended a new path entry.
    // (A pure no-op call still prints the status lines.)
    if result.created || result.registered {
        if let Err(e) = save_settings(&settings) {
            eprintln!("Error: failed to persist settings: {}", e);
            return 1;
        }
    }
    if result.created {
        println!("Created AC project at {}", result.path);
    } else {
        println!("AC project already exists at {}", result.path);
    }
    if result.registered {
        println!("Registered project: {}", result.path);
    } else {
        println!("Project already registered: {}", result.path);
    }
    log::info!(
        "[cli] new-project: path={} created={} registered={}",
        result.path,
        result.created,
        result.registered
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct FixtureRoot(PathBuf);
    impl Drop for FixtureRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl FixtureRoot {
        fn new(prefix: &str) -> Self {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::process::id().hash(&mut h);
            std::thread::current().id().hash(&mut h);
            let path = std::env::temp_dir().join(format!(
                "{}-{}-{}",
                prefix,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
                h.finish()
            ));
            std::fs::create_dir_all(&path).expect("fixture root");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    #[test]
    fn new_project_returns_1_when_path_is_a_file() {
        let fix = FixtureRoot::new("cli-new-isfile");
        let f = fix.path().join("note.txt");
        std::fs::write(&f, b"x").unwrap();
        let code = execute(NewProjectArgs {
            path: f.to_string_lossy().into(),
        });
        assert_eq!(code, 1);
    }

    #[test]
    fn help_text_documents_new_project() {
        use clap::CommandFactory;
        let help = crate::cli::Cli::command().render_help().to_string();
        assert!(help.contains("new-project"), "help missing verb: {}", help);
    }
}
```

### 4.7 MODIFIED: `src-tauri/src/cli/mod.rs`
Wire the two new verbs into the `Commands` enum and the `handle_cli` dispatcher.

**Module declarations (insert after line 5 `pub mod create_agent;`):**
```rust
pub mod new_project;
pub mod open_project;
```
After insertion, the block reads:
```rust
pub mod brief_append_body;
pub mod brief_ops;
pub mod brief_set_title;
pub mod close_session;
pub mod create_agent;
pub mod list_peers;
pub mod list_sessions;
pub mod new_project;
pub mod open_project;
pub mod send;
```

**`Commands` enum (lines 29–45)** — append two variants before the closing brace:
```rust
    /// Set the title field in the workgroup BRIEF.md frontmatter (coordinator-only)
    BriefSetTitle(brief_set_title::BriefSetTitleArgs),
    /// Append text to the body of the workgroup BRIEF.md (coordinator-only)
    BriefAppendBody(brief_append_body::BriefAppendBodyArgs),
    /// Register an existing AC project (.ac-new must already exist) in settings
    OpenProject(open_project::OpenProjectArgs),
    /// Create an AC project (mkdir .ac-new if missing) and register it in settings
    NewProject(new_project::NewProjectArgs),
}
```

**`handle_cli` dispatcher (lines 140–153)** — append two arms:
```rust
        Commands::BriefSetTitle(args) => brief_set_title::execute(args),
        Commands::BriefAppendBody(args) => brief_append_body::execute(args),
        Commands::OpenProject(args) => open_project::execute(args),
        Commands::NewProject(args) => new_project::execute(args),
    };
```

### 4.8 MODIFIED: `src/shared/types.ts`
Add the IPC type after `BriefUpdateResult` (around line 326). Append:
```typescript
// ---------------------------------------------------------------------------
// Project registration result (#191 — shared open/new project flow)
// Mirrors src-tauri/src/config/projects.rs::ProjectRegistration.
// ---------------------------------------------------------------------------

export interface ProjectRegistration {
  /** Absolute path that was added (or matched) in projectPaths. */
  path: string;
  /** True when this call appended a new entry, false when already present. */
  registered: boolean;
  /** True when this call created .ac-new/ on disk (always false for openProject). */
  created: boolean;
}
```

### 4.9 MODIFIED: `src/shared/ipc.ts`
Update the `ProjectAPI` block (lines 410–417) to add the two new typed wrappers. Add `ProjectRegistration` to the import on line 14:

**Before (line 14):**
```typescript
  AcDiscoveryResult,
```
**After:**
```typescript
  AcDiscoveryResult,
  ProjectRegistration,
```

**Before (lines 410–417):**
```typescript
// Project API
export const ProjectAPI = {
  checkPath: (path: string) =>
    transport.invoke<boolean>("check_project_path", { path }),
  createAcProject: (path: string) =>
    transport.invoke<void>("create_ac_project", { path }),
  discover: (path: string) =>
    transport.invoke<AcDiscoveryResult>("discover_project", { path }),
};
```
**After:**
```typescript
// Project API
export const ProjectAPI = {
  checkPath: (path: string) =>
    transport.invoke<boolean>("check_project_path", { path }),
  createAcProject: (path: string) =>
    transport.invoke<void>("create_ac_project", { path }),
  discover: (path: string) =>
    transport.invoke<AcDiscoveryResult>("discover_project", { path }),
  /**
   * Validate an existing AC project at `path` and register it in
   * settings.projectPaths. Wraps the `open_project` Tauri command added in
   * #191 — same backend logic as the CLI `open-project` verb.
   */
  open: (path: string) =>
    transport.invoke<ProjectRegistration>("open_project", { path }),
  /**
   * Ensure an AC project at `path` (mkdir `.ac-new/` if missing) and register
   * it in settings.projectPaths. Wraps the `new_project` Tauri command added
   * in #191 — same backend logic as the CLI `new-project` verb.
   */
  new: (path: string) =>
    transport.invoke<ProjectRegistration>("new_project", { path }),
};
```

### 4.10 MODIFIED: `src/sidebar/stores/project.ts`
Replace the discover→persist round-trip in `loadProject` and `createAndLoad` with the new typed wrappers. The CLI and UI then traverse the SAME backend code path. **`persistProjectPaths` stays** (Round-1 G1 resolution): `removeProject` still calls it, and inlining the SettingsAPI.update logic into `removeProject` is out of scope for this PR — a future `remove_project` CLI verb (deferred per §7) will revisit.

**Update `normalizePath` (lines 17–19)** — apply the same trailing-`/` strip as the Rust `normalize_for_compare` (Round-1 IR.3.2):
```typescript
function normalizePath(p: string): string {
  return p.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
}
```

**Replacement of lines 36–82** (`loadProject` + `initFromSettings` + `createAndLoad`):
```typescript
  /** Register a project path in settings (via shared backend) and load its discovery data. */
  async loadProject(path: string) {
    const normalized = normalizePath(path);
    if (projects().some((p) => normalizePath(p.path) === normalized)) return;

    loadingCount++;
    setLoading(true);
    try {
      // #191 — backend owns the validation + dedup + persist atomically.
      // Throws if `.ac-new/` is missing; caller (createAndLoad / pickAndCheck)
      // is responsible for creating it first via projectStore.createAndLoad
      // when that case is expected.
      const reg = await ProjectAPI.open(path);
      const result = await ProjectAPI.discover(reg.path);
      const folderName =
        reg.path.replace(/\\/g, "/").split("/").pop() ?? "unknown";
      // Round-1 G2: re-check against the BACKEND-absolutised reg.path
      // (which may differ from the input `path` in case/slashes/`..`),
      // mirroring the inner dedup pattern in createAndLoad. Closes the
      // double-render race when two concurrent calls pass differently-
      // shaped strings that resolve to the same registered entry.
      const normalizedReg = normalizePath(reg.path);
      setProjects((prev) => {
        if (prev.some((p) => normalizePath(p.path) === normalizedReg)) return prev;
        return [
          ...prev,
          {
            path: reg.path,
            folderName,
            workgroups: result.workgroups,
            agents: result.agents,
            teams: result.teams,
          },
        ];
      });
    } catch (e) {
      // Round-1 G11 deferred: surface this to the user via toast/sidebar
      // chip in a follow-up. For now, preserve the existing swallow-and-log
      // so behaviour is no worse than today (initFromSettings silently drops
      // a project whose .ac-new was deleted between sessions — see §6.11).
      console.error("Failed to load project:", e);
    } finally {
      loadingCount--;
      if (loadingCount === 0) setLoading(false);
    }
  },

  /** Initialize from saved settings (call on mount) */
  async initFromSettings(projectPaths: string[], legacyPath: string | null) {
    // Merge legacy single path into the array (deduplicated)
    const paths = [...projectPaths];
    if (legacyPath && !paths.some((p) => normalizePath(p) === normalizePath(legacyPath))) {
      paths.push(legacyPath);
    }
    for (const path of paths) {
      await projectStore.loadProject(path);
    }
  },

  /** Create .ac-new in path (if missing) and register/load it. */
  async createAndLoad(path: string) {
    const reg = await ProjectAPI.new(path);
    // After ensuring .ac-new exists + persistence is set, run discovery for UI.
    const result = await ProjectAPI.discover(reg.path);
    const folderName =
      reg.path.replace(/\\/g, "/").split("/").pop() ?? "unknown";
    const normalized = normalizePath(reg.path);
    setProjects((prev) => {
      if (prev.some((p) => normalizePath(p.path) === normalized)) return prev;
      return [
        ...prev,
        {
          path: reg.path,
          folderName,
          workgroups: result.workgroups,
          agents: result.agents,
          teams: result.teams,
        },
      ];
    });
  },
```

**`removeProject` (lines 151–155)** — **NO CHANGE.** Round-1 G1 resolution: keeping the existing `await persistProjectPaths()` call avoids a parallel SettingsAPI.update inline that would have to be re-unified once a backend `remove_project` command lands.

**`persistProjectPaths` (lines 162–171)** — **KEEP.** Still called by `removeProject:154`. The Round-0 plan's "delete this helper" instruction was incorrect (would have broken the build per Round-1 G1). The helper becomes dead code only once `removeProject` is migrated to a backend `remove_project` Tauri command — deferred per §7.

**Imports (line 2)** — unchanged. `AgentCreatorAPI` is still needed by `pickAndCheck`; `SettingsAPI` is still needed by `removeProject` via `persistProjectPaths`. The `ProjectAPI` import already exists; it gains the new `open` and `new` members from §4.9.

### 4.11 NO CHANGE: `src/sidebar/components/ActionBar.tsx`, `src/sidebar/components/Toolbar.tsx`
The existing handlers (`handleNewProject`, `handleOpenProject`, `handleConfirmCreate`) call `projectStore.createAndLoad`, `projectStore.loadProject`, and `projectStore.pickAndCheck`. Those store methods now route through the new backend commands, so the UI components need no edit. This is the minimum-blast-radius shape of "share Rust backend logic for create/open/register".

### 4.12 MODIFIED: version bump across all three manifests
Per the standing rule that every feature build bumps the version so the user can visually confirm the new build is loaded. Edit ALL THREE files together so `cargo`, `npm`, and Tauri agree (Round-1 G7 added the third file):

- `src-tauri/tauri.conf.json` — bump `version` `0.8.15` → `0.8.16`
- `package.json` — bump `version` `0.8.9` → `0.8.16` (currently out of sync — fix the gap with this PR)
- `src-tauri/Cargo.toml` line 3 — bump `version = "0.8.9"` → `version = "0.8.16"` (also out of sync)

`tauri.prod.conf.json` and `tauri.stage.conf.json` do **not** carry a `version` field — they only override `productName`/`identifier`/`mainBinaryName`. Verified by reading both files; the Round-0 plan's "(if present)" hedge is moot (Round-1 IR.1).

### 4.13 NEW: `load_settings_for_cli()` in `src-tauri/src/config/settings.rs`
Round-1 G5 resolution. The current `load_settings()` (`config/settings.rs:376-444`) is NOT pure-read: when `root_token.is_none()`, it auto-generates a UUID and synchronously calls `save_settings(&settings)` from inside `load_settings`. The CLI verbs need a loader that does NOT have this side effect, so error-path invocations and pre-validation reads do not silently rewrite `settings.json`. This also makes the §4.5/§4.6 CLI tests safe by construction (Round-1 G6).

**Insertion point:** in `src-tauri/src/config/settings.rs`, immediately AFTER the `load_settings()` function (which ends at line 444) and BEFORE `read_log_level_from_path` (which starts at line 452).

```rust
/// CLI-only variant of `load_settings`. Reads disk and applies the same
/// in-memory migrations as `load_settings`, but does NOT auto-generate or
/// persist a `root_token`. Used by CLI verbs that mutate settings
/// (`open-project`, `new-project`) so error paths and pre-validation reads
/// do NOT silently rewrite `settings.json` (Round-1 G5 in #191's plan).
///
/// The CLI verbs do not consume the root_token; if a future verb needs it,
/// `settings.root_token == None` on a brand-new install is fine — the CLI
/// is read-only with respect to it. The GUI still owns root_token
/// generation via the next `load_settings()` call when it boots.
///
/// **Migration duplication is intentional for this PR.** Extracting an
/// `apply_in_memory_migrations(&mut AppSettings)` helper that both loaders
/// share is a clean follow-up, but pulls in scope outside #191 (touches
/// `load_settings`'s control flow). Keep both copies in lockstep until
/// then; if you add a new in-memory migration to `load_settings`, mirror
/// it here too.
pub fn load_settings_for_cli() -> AppSettings {
    let path = match settings_path() {
        Some(p) => p,
        None => {
            log::warn!("[cli] Could not determine home directory, using defaults");
            return AppSettings::default();
        }
    };

    let mut settings = if !path.exists() {
        log::info!("[cli] No settings file found at {:?}, using defaults", path);
        AppSettings::default()
    } else {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<AppSettings>(&contents) {
                Ok(s) => {
                    log::info!("[cli] Loaded settings from {:?}", path);
                    s
                }
                Err(e) => {
                    log::error!("[cli] Failed to parse settings file: {}", e);
                    AppSettings::default()
                }
            },
            Err(e) => {
                log::error!("[cli] Failed to read settings file: {}", e);
                AppSettings::default()
            }
        }
    };

    // 0.8.0 unified-window migration — must mirror `load_settings` exactly,
    // EXCEPT for the root_token auto-gen + save_settings call.
    if settings.main_geometry.is_none() {
        if let Some(ref g) = settings.terminal_geometry {
            settings.main_geometry = Some(g.clone());
        }
    }
    if (settings.main_zoom - default_zoom()).abs() < f64::EPSILON
        && (settings.sidebar_zoom - default_zoom()).abs() > f64::EPSILON
    {
        settings.main_zoom = settings.sidebar_zoom;
    }
    if !settings.main_always_on_top && settings.sidebar_always_on_top {
        settings.main_always_on_top = true;
    }

    // NO root_token auto-gen, NO save_settings call.
    settings
}
```

**Test for the helper** — add to the existing `#[cfg(test)] mod tests` in `config/settings.rs` (or create one if absent — confirm by reading the bottom of the file):

```rust
#[test]
fn load_settings_for_cli_does_not_persist_root_token() {
    // Use a sandboxed config dir via env override if available, else skip.
    // If there is no env-var override for `config_dir()`, this test still
    // exercises the no-write contract via in-memory state: call the loader,
    // confirm the returned `AppSettings.root_token` is None on a missing
    // settings file (we do not generate one), and that no .tmp/save artifact
    // appears in the dir between calls.
    //
    // The cheap version: assert that on a default AppSettings path (no file
    // present), the returned settings have root_token == None. Pair with a
    // manual smoke step that runs `<bin> open-project /nonexistent` against a
    // clean home dir and grep settings.json for root_token absence.
    //
    // Implementation note: dev should pick whichever path their test
    // infrastructure supports. If `config_dir()` cannot be sandboxed, leave
    // the manual smoke step (§5.1 step 9) as the verifier and skip the unit
    // test — flagged in the plan rather than hidden.
}
```

If `config_dir()` cannot be cheaply sandboxed for tests (read `config/mod.rs::config_dir()` to confirm), drop the unit-test scaffolding and rely on §5.1 step 9 (added below) for verification.

| Command                                                                   | Purpose                                                       |
|---------------------------------------------------------------------------|---------------------------------------------------------------|
| `cd src-tauri && cargo test config::projects`                             | Pure-logic unit tests added in §4.1                           |
| `cd src-tauri && cargo test cli::open_project cli::new_project`           | CLI arg/error tests added in §4.5–4.6                         |
| `cd src-tauri && cargo test --lib`                                        | Full library suite — confirm nothing else regressed           |
| `cd src-tauri && cargo build`                                             | Compile-check the Tauri command + invoke_handler! wiring      |
| `npm test`                                                                | Vitest run for `src/` (no FE-only tests added; sanity check)  |
| `npm run build`                                                           | Type-check the IPC wrapper + types changes                    |
| `npm run tauri dev`                                                       | Manual smoke test — see §5.1                                  |

### 5.1 Manual smoke test (Windows)
1. Build the dev binary: `npm run tauri dev`
2. From a separate PowerShell prompt: `& "<dev-binary-path>" new-project C:\tmp\acnew-smoke` — expect exit 0, `.ac-new` created, two stdout lines.
3. Repeat the same command — expect exit 0, "AC project already exists" + "Project already registered".
4. `Remove-Item C:\tmp\acnew-smoke -Recurse -Force` then `& "<dev-binary-path>" open-project C:\tmp\acnew-smoke` — expect exit 1, `Error: path does not exist`.
5. `mkdir C:\tmp\open-only` then `& "<dev-binary-path>" open-project C:\tmp\open-only` — expect exit 1, `Error: no AC project at ... (.ac-new/ not found)`.
6. Restart the GUI; confirm the projects registered via the CLI now appear in the sidebar with their workgroups/agents.
7. From the GUI: New Project → pick a fresh folder. Confirm: `.ac-new` created, sidebar shows the new project, `settings.json` has the path appended exactly once.
8. From the GUI: Open Project on a folder without `.ac-new`. Confirm the existing toast/confirm-modal path still works (no regression — handlers untouched).
9. **Round-1 G5 verification.** Stop AC. Backup and delete `~/.agentscommander*/settings.json` (whatever path your `LocalDir` points at). Run `& "<dev-binary-path>" open-project C:\nonexistent` — expect exit 1 (path missing). Inspect the config dir: **no `settings.json` should have been created** by the CLI. If `settings.json` was created with a freshly-generated `root_token`, `load_settings_for_cli()` is misbehaving (Round-1 G5 regression). Restore the backup when done.
10. **Round-1 G4 verification (Windows-only).** From `C:\Users\<you>`, run `& "<dev-binary-path>" open-project ..\<some-existing-project>`. Inspect `settings.json`: the persisted `project_paths[]` entry must contain NO `..` segment — `std::path::absolute` collapsed it via `GetFullPathNameW`. Run the same verb a second time with an absolute path to the same project — expect "Project already registered" (dedup works across `..`-shape and absolute-shape inputs).

---

## 6. Notes / constraints — things the dev MUST NOT do

### 6.1 Do not canonicalise paths
- `std::fs::canonicalize` returns `\\?\C:\foo` on Windows. The frontend's persisted paths are non-UNC. Comparing UNC vs. non-UNC after lowercasing+slashifying still mismatches (`\\?\` vs nothing). Use only the simple absolute-join + `replace('\\', "/").to_lowercase()` form.
- Symlink resolution would also break the user's mental model: `D:\projects\current` (symlink) → `D:\projects\v3` would persist as `v3`, not `current`. Keep the user-typed shape.

### 6.2 Gitignore sweep is best-effort on pre-existing `.ac-new/`
`register_new_project` calls `ensure_ac_new_gitignore` even when `created == false`, matching `discover_project`'s opportunistic sweep (`src-tauri/src/commands/ac_discovery.rs:1308-1309`). However, when `created == false`, the sweep MUST be best-effort (Round-1 G15): a transient FS error (read-only `.gitignore`, disk full, unusual permissions) logs a warning and continues with registration. When `created == true`, the sweep is mandatory — a freshly-created `.ac-new/` without its protective patterns is unsafe to ship. The `match` arms in §4.1's `register_new_project` enforce this asymmetry.

### 6.3 GUI-running concurrency — known limitation, NOT a fix in this PR
When AC's GUI is running, its in-memory `SettingsState` is the source of truth. The CLI verbs use `load_settings_for_cli()` / `save_settings()` directly (no IPC to the running app). A subsequent GUI `update_settings` whose draft was built from a stale snapshot will overwrite the CLI's addition.

This is the same documented race that already exists between `update_settings` and the narrow setters (`set_inject_rtk_hook`, `set_sounds_enabled` — see `src-tauri/src/commands/config.rs:171-180` doc comment).

**Round-1 G5 partial mitigation:** the new `load_settings_for_cli()` (§4.13) closes the *first-boot root_token race* — the CLI no longer auto-generates a token on read, so two parallel boots (GUI + CLI) cannot each generate a different UUID and clobber each other. The bigger GUI-`update_settings`-clobbers-CLI race remains open. For #191's primary use case ("register projects before launching the GUI"), the race window does not exist. **Document the residual limitation** in the verb's `--help` `after_help` block already drafted in §4.5/4.6, and **open a follow-up issue** for either (a) GUI watches `settings.json` mtime and reloads on change, or (b) `update_settings` re-reads disk before merging — closing the race for all narrow setters at once.

### 6.4 Do not require `--token` on these verbs
The other CLI verbs gate on `--token` because their actions are *cross-agent* (sending a message, closing someone else's session, editing a workgroup BRIEF on behalf of a coordinator role). `open-project` / `new-project` are *user-local*: they mutate the same `settings.json` the user already has read+write access to via `notepad`. A token requirement adds zero security and divergent UX. If a future requirement emerges (e.g. multi-tenant AC instance), revisit then.

### 6.5 Preserve the legacy `project_path` field
The `upsert_project_path` helper rewrites `settings.project_path = settings.project_paths.first().cloned()` on every call. **Do not** drop this. The frontend `persistProjectPaths` does the same (`src/sidebar/stores/project.ts:166-170`); other consumers (e.g. older settings.json importers, and the `initFromSettings` legacy-merge path at `project.ts:66-75`) still read it.

### 6.6 Do not introduce new crates
`thiserror`, `serde`, `clap`, `uuid`, `tokio` are all already in `src-tauri/Cargo.toml`. No new dependency is needed.

### 6.7 Do not refactor `check_project_path` / `create_ac_project` / `discover_project`
These three Tauri commands keep their public surface — the web/browser code, future tooling, and existing FE call sites still hit them. The new `open_project` / `new_project` are additive; they internally call `register_*` which ALSO calls `ensure_ac_new_gitignore`, but the legacy path is untouched.

### 6.8 Do not remove `OpenProjectArgs` / `NewProjectArgs` from `cli::mod.rs` exports
clap's `derive(Subcommand)` requires the `Args` struct paths to be reachable from the enum variant declarations. Keep the `pub mod open_project;` / `pub mod new_project;` lines (§4.7). The internal `execute` functions are reachable via `<verb>::execute(args)` from the `handle_cli` dispatcher.

### 6.9 CLI-vs-CLI lost-update race — accepted (Round-1 IR.3.1)
Two parallel `<bin> open-project` (or `new-project`) processes each `load_settings_for_cli()` from disk, mutate independently, then race on the atomic-rename in `save_settings()`. Last-write wins; one registration silently lost. Window = duration of the load → mutate → write sequence (milliseconds). **No advisory locking is added in this PR** — matches the rest of `settings.json`'s mutation model. An advisory lockfile (e.g. `<config_dir>/.cli-write.lock` acquired non-blocking via `OpenOptions::new().create_new(true)`) could close it but pulls in scope outside #191. Worth opening a follow-up issue if real users hit it.

### 6.10 Case-insensitive dedup on case-sensitive filesystems — accepted (Round-1 IR.3.3)
The dedup key lowercases the path to match the FE's `normalizePath` contract (`src/sidebar/stores/project.ts:17-19`). On Linux/macOS where `/foo/Bar` and `/foo/bar` are distinct directories, only the FIRST registered case-variant survives in `project_paths`. This is **inherited from the FE's existing behaviour** — not a regression introduced by #191. Same caveat applies to `..`/`.` collapse: `std::path::absolute` collapses on Windows but not on POSIX (per Rust stdlib docs); the §3 contract is therefore weaker on POSIX. Cross-platform-correct dedup (canonicalize via `dunce::canonicalize` + careful symlink handling, plus case-preserving comparison) is deferred — the project's primary target is Windows where both gaps are closed.

### 6.11 `initFromSettings` silently drops projects whose `.ac-new/` was deleted — PR-note (Round-1 IR.3.5)
**Behavioural change worth surfacing in the PR description, not blocking.** Today, `initFromSettings → loadProject → ProjectAPI.discover()` silently returns empty results for paths missing `.ac-new/` and the project still appears in the sidebar (with empty workgroups/agents/teams). After §4.10, `loadProject → ProjectAPI.open()` throws `AcNewMissing` on missing `.ac-new/` and the project is silently dropped from the sidebar (only a `console.error`). This is **arguably the right new behaviour** — paths without `.ac-new/` shouldn't be in the project list — but it IS a behavioural change. A user-visible toast / sidebar warning chip ("Project at X is no longer available — remove from list?") is a follow-up (Round-1 G11 deferred); for this PR the swallow-and-log behaviour is preserved.

---

## 7. Phase order

This plan delivers MVP + Full Features in one PR — the change set is small, atomic, and there are no follow-ups to defer that would meaningfully change the design.

- **MVP**: §4.1 + §4.5 + §4.6 + §4.7 + §4.13 — CLI verbs work (with the new `load_settings_for_cli` helper), FE untouched. Ship-ready as a CLI-only feature.
- **Full Features**: §4.3 + §4.4 + §4.8 + §4.9 + §4.10 — Tauri commands wired, FE store routes through them (with `loadProject` post-`reg.path` dedup and `normalizePath` trailing-slash strip), IPC contract finalised.
- **Polish**: §4.12 (version bump across all three manifests), `--help` text refinement, stdout copy-edits.
- **Extras (deferred to follow-up issues)**: GUI watches `settings.json` mtime and reloads (§6.3); CLI `remove-project` verb for symmetry (§4.10 note); typed Rust→TS error variants instead of stringly errors; advisory lockfile for CLI-vs-CLI race (§6.9); user-visible toast for stale-`.ac-new` projects on sidebar load (§6.11); cross-platform-correct path dedup that handles POSIX `..` and case-sensitivity (§6.10); shared `apply_in_memory_migrations` helper to de-duplicate the §4.13 / `load_settings` migration block.

---

## Grinch Review

Plan was reviewed against the actual code on `feature/191-cli-project-open-create`. **Status: NOT APPROVED — must fix G1, G2, G3 before implementation. Several other findings need clarifications or test additions.**

### G1 — SHOWSTOPPER: Plan §4.10 contradicts itself; following it as written breaks the build.
- **What.** The plan says (a) "Update `removeProject` (lines 151–155)... **No change required to this method**" — i.e. keep it calling `persistProjectPaths()`, AND (b) "**Delete `persistProjectPaths` (lines 162–171)** — no longer called from anywhere." Both cannot be true.
- **Why.** `src/sidebar/stores/project.ts:154` literally `await persistProjectPaths()` inside `removeProject`. Confirmed by grep: the helper is referenced from line 56 (replaced by the plan) AND line 154 (NOT replaced). If the dev deletes `persistProjectPaths`, `removeProject` fails to compile (TypeScript) or throws at runtime (no module). If the dev keeps it, the "no longer called from anywhere" justification is false but the code works.
- **Fix.** Pick one and say so explicitly: either (i) keep `persistProjectPaths` because `removeProject` still uses it (and remove the "delete" instruction), or (ii) inline the SettingsAPI.update call inside `removeProject` and then delete the helper. Option (i) is the lower-risk, smaller-blast-radius change for this PR; the §4.10 note already plans `remove-project` CLI as a follow-up that will revisit.

### G2 — HIGH: Frontend `loadProject` push lacks dedup against `reg.path`; double-render race remains.
- **What.** New `loadProject` (plan §4.10):
  ```typescript
  const reg = await ProjectAPI.open(path);
  const result = await ProjectAPI.discover(reg.path);
  setProjects((prev) => [...prev, { path: reg.path, ... }]);   // no dedup
  ```
  The early dedup uses the *input* `path`, but the absolutized backend `reg.path` may differ in case/slashes. If two callers pass `C:\foo` and `c:/FOO/` (backend normalizes both to one settings entry), both pass the early dedup, both succeed against the backend (registered=true once, registered=false the other time), both push, and the FE shows TWO project cards for one persisted entry.
- **Why.** Concretely: `initFromSettings` iterates the persisted array, `pickAndCheck`/`loadProject` may run concurrently from the user clicking, and Solid's signals are not awaited — the `setProjects` updates are queued and the second push sees the first's state only after its own `await`s complete. The new `createAndLoad` already handles this correctly with an inner re-check (plan §4.10), but `loadProject` does not. Inconsistency = bug.
- **Fix.** In `loadProject`, mirror `createAndLoad`'s inner check:
  ```typescript
  const normalizedReg = normalizePath(reg.path);
  setProjects((prev) => {
    if (prev.some((p) => normalizePath(p.path) === normalizedReg)) return prev;
    return [...prev, { path: reg.path, folderName, ... }];
  });
  ```

### G3 — HIGH: Plan's "append at end of file (around line 1646)" instruction is wrong; will be inserted into the wrong place.
- **What.** §4.3 says: "**Append** at the end of the file (after `set_replica_context_files`, around line 1646)". Actual file: `set_replica_context_files` ends at line 1674; lines 1676-1768 hold `#[cfg(test)] mod tests { ... }`; lines 1770-1782 hold `type BriefFields` + `fn read_brief_fields`. End of file is line 1782, not 1646.
- **Why.** A literal "append at the end" places the new `#[tauri::command]` functions AFTER the test module and after `read_brief_fields`. That compiles, but it is messy and out of order with the file's convention (tests last). A literal "insert after `set_replica_context_files`" puts them at line 1675 — between the function and the test module — which is the right place but conflicts with the "around line 1646" hint.
- **Fix.** Specify: "insert immediately AFTER `set_replica_context_files` (closing `}` at line 1674) and BEFORE the `#[cfg(test)] mod tests` opener at line 1676." Drop the "around line 1646" line number — it's misleading.

### G4 — HIGH: `absolutise` does not normalize `.` / `..`, breaking the dedup contract.
- **What.** `cwd.join(p)` in `absolutise()` (plan §4.1) does no lexical normalization. If the user runs `<bin> open-project ..\projects` from `C:\Users\maria`, the persisted entry is literally `C:\Users\maria\..\projects`. The FE's `normalizePath` only does `.replace('\\','/').toLowerCase()`, so this becomes `c:/users/maria/../projects` — which never collides with a separately-typed `C:\Users\projects` (`c:/users/projects`).
- **Why.** Result: silent double-registration when a user opens the same project from two different CWDs. The plan's own §3 contract claims "CLI-registered `C:\foo` and a GUI-registered `c:/FOO` collapse to one entry" — but only true for case/slash, not for path traversal.
- **Fix.** Either: (a) use `std::path::absolute` (stable since Rust 1.79; plan's claim that it's "Rust-edition-dependent" is incorrect — it's a stdlib function gated on rustc version, and the workspace already uses edition 2021 with a modern toolchain), which normalizes `..`/`.` lexically; or (b) explicitly walk the path components and collapse `..` segments before persisting. Document the choice and add a unit test: `register_existing_project(s, "../projects")` from a known CWD must produce a path with no `..` components.

### G5 — HIGH: `load_settings()` is NOT pure-read; the CLI verbs interact with it dangerously.
- **What.** `cli::open_project::execute` and `cli::new_project::execute` both call `load_settings()` (plan §4.5/§4.6). But `src-tauri/src/config/settings.rs:376-444` shows `load_settings`:
  - Migrates `terminal_geometry → main_geometry`, `sidebar_zoom → main_zoom`, `sidebar_always_on_top → main_always_on_top`.
  - **Auto-generates and persists a new `root_token`** if `root_token.is_none()` — a `save_settings()` call inside `load_settings`!
- **Why.** Two concrete failures:
  1. Concurrency: GUI is running, holds in-memory `SettingsState` with root_token=X. User runs CLI `open-project`. CLI's `load_settings` reads disk, finds root_token=X (already set, fine). Then CLI mutates `project_paths`, calls `save_settings`. **No interaction with the GUI's in-memory state.** GUI's next `update_settings` writes a stale snapshot back, **wiping the CLI's addition.** This is the documented race (§6.3) but its severity is understated: the plan markets the verbs as safe-while-GUI-running and just adds a one-line `--help` caveat. Real users will lose registrations and not understand why.
  2. First-ever CLI invocation race: if a brand-new install has no settings.json, `load_settings` generates a root_token AND saves. Then the CLI's `save_settings` saves again. Two writes, both atomic, but if the GUI is also booting and its `load_settings` runs concurrently, both processes generate a UUID and one wins — the loser's UUID was already echoed back to the user via Session Credentials and is now invalid.
- **Fix.** (a) For §6.3, add an explicit "GUI-running detection" — at minimum a non-blocking advisory lockfile in `config_dir/.cli-write.lock` that the CLI tries to acquire before mutating, and document the expected failure mode if the GUI is running. Better: have `update_settings` re-read disk before writing (re-acquire the project_paths from disk, merge, then write). The plan should at least narrate which mitigation the dev should pursue rather than punting wholesale to a follow-up. (b) For the first-boot race, defer load_settings's `root_token` generation when called from a CLI verb (introduce `load_settings_for_cli()` or similar that does NOT auto-generate).

### G6 — MEDIUM: Tests in §4.5 and §4.6 mutate the user's REAL `settings.json` on disk.
- **What.** `cli::open_project::execute(args)` calls `load_settings()` and `save_settings()` against `config_dir()` (i.e., the live `settings.json` next to the running test binary). Plan §4.5 has tests `open_project_returns_1_when_path_missing` and `open_project_returns_1_when_no_ac_new` that call `execute()`. Both fail before reaching `save_settings`, BUT both still call `load_settings` first — which can persist a freshly-generated `root_token`.
- **Why.** Running `cargo test` on a user's machine could rotate their root_token (any test that triggers `load_settings()` against an absent settings.json). And the success-path test (e.g., a future `open_project_writes_to_settings`) would clobber the user's project_paths. Test parallelism + shared `config_dir` = flaky.
- **Fix.** Either: (a) sandbox the tests via an env-var-overridable config dir (introduce `CONFIG_DIR_OVERRIDE` env var, honored by `config_dir()`), (b) refactor `execute()` to take a `settings_path: &Path` arg so tests pass a tempdir, or (c) explicitly note in the plan that these tests must NOT touch `execute()` and only exercise `register_existing_project` / `register_new_project` directly via the in-memory `AppSettings::default()` fixture (which the §4.1 tests already do). Pick one and write it down.

### G7 — MEDIUM: Plan version-bump scope misses `Cargo.toml`.
- **What.** §4.12 says: bump `tauri.conf.json` 0.8.15 → 0.8.16, bump `package.json` 0.8.9 → 0.8.16, "grep `0.8.15` and `0.8.9` to be sure". But `src-tauri/Cargo.toml` line 3 is `version = "0.8.9"` — the same out-of-sync version as package.json. The plan does not mention Cargo.toml explicitly.
- **Why.** A grep would find it, but "the dev should grep" is not as good as "edit these N files". Skipping Cargo.toml means Cargo's emitted binary still reports 0.8.9 internally; mismatches with the Tauri-shown version will reappear next time someone investigates.
- **Fix.** Add explicit list: bump `src-tauri/tauri.conf.json` (0.8.15→0.8.16), `package.json` (0.8.9→0.8.16), `src-tauri/Cargo.toml` (0.8.9→0.8.16). Drop the "if present" tauri.prod/stage clause — those files don't carry a `version` field (verified).

### G8 — MEDIUM: `register_existing_project`'s error string mentions a CLI verb but is shown to GUI users too.
- **What.** `ProjectError::AcNewMissing` formats as `"no AC project at {0} (.ac-new/ not found). Use `new-project` to create one."` (plan §4.1). The Tauri command (§4.3) does `.map_err(|e| e.to_string())`, which propagates this string verbatim to the GUI.
- **Why.** A GUI user encountering this in a toast/console sees `Use \`new-project\` to create one.` — which is meaningless to them. They are not running a CLI; they don't know what `new-project` is.
- **Fix.** Either: (a) drop the "Use `new-project`..." sentence from the error and have callers append context-appropriate guidance (CLI prepends/appends its own hint, GUI uses its own toast text), or (b) split into two error variants (`AcNewMissingForCli`, `AcNewMissingForGui`) — over-engineered. Option (a) is cleaner.

### G9 — MEDIUM: TOCTOU on `created` field in `register_new_project`.
- **What.**
  ```rust
  let created = !ac_new.is_dir();
  if created { std::fs::create_dir_all(&ac_new).map_err(...)?; }
  ```
  Between `is_dir()` and `create_dir_all`, another process can create the directory. `create_dir_all` succeeds either way (idempotent), but `created=true` is now a lie — the message will print "Created AC project" when the directory was actually created by someone else.
- **Why.** Cosmetic-only failure mode (the success outcome is the same: directory exists). But the §4.6 stdout "Created AC project at X" misleads the user when the directory pre-existed by milliseconds. Also affects the IPC `ProjectRegistration.created` field — frontend code that branches on `created === true` to show a "creation success" banner would show it for a no-op.
- **Fix.** Detect creation more authoritatively: try `std::fs::create_dir(&ac_new)` (non-recursive, fails if exists), then on `AlreadyExists` set created=false; otherwise propagate the error. Or do the existence check AFTER `create_dir_all` based on returned metadata. Also add a unit test that asserts `created=false` when `.ac-new/` already exists at call time (the §4.1 test `new_skips_creation_when_ac_new_already_exists` does this) — verify the parallel race is impossible by design, not by hope.

### G10 — MEDIUM: Plan instruction §4.4 line numbers are off-by-2.
- **What.** Plan says "Insert at line 836 (between `commands::ac_discovery::create_ac_project,` and `commands::ac_discovery::discover_project,`)". Actual: `create_ac_project,` is at line 834, `discover_project,` is at line 835. The two-line block to insert lands at line 835, not 836.
- **Why.** Not a correctness issue (the instruction's "between X and Y" is unambiguous), but it costs the dev a context-switch when the line number doesn't match. Plan §4.2 has the same problem (claims "insert at line 7" when the new module name `projects` should sort alphabetically right BEFORE `session_context` at line 4 → line 4, after `profile` at line 3).
- **Fix.** Either drop the line numbers entirely (the "between X and Y" anchor suffices) or recompute them. If retained, the dev will keep re-confirming them and the plan reads as imprecise.

### G11 — MEDIUM: `loadProject` swallows backend errors silently.
- **What.** `loadProject` wraps the new `ProjectAPI.open(path)` + `ProjectAPI.discover(reg.path)` calls in `try { ... } catch (e) { console.error("Failed to load project:", e); }` (plan §4.10). The user sees nothing.
- **Why.** Pre-change, `discover` errors were silently logged — same behavior. But now `open()` returns *validation* errors (path missing, not a directory, no `.ac-new`) that the user OUGHT to see. The Open Project flow in `ActionBar.handleOpenProject` does its own `checkPath` + toast guard before calling `loadProject`, so most users won't hit this. But `initFromSettings` calls `loadProject` blind on every persisted path — if a project folder was deleted between sessions, the user gets no indication; the project just silently disappears from the sidebar with a `console.error` that no one reads.
- **Fix.** At minimum, distinguish ESTABLISHED-project path errors (user-facing toast / sidebar warning chip "Project at X is no longer available — remove from list?") from per-session validation errors. Or: surface a typed result from `loadProject` so callers can decide. Don't ship a feature where settings.json silently desyncs from the visible sidebar.

### G12 — LOW: TOCTOU on `register_existing_project`'s 3-syscall validation.
- **What.** `abs.exists()` → `abs.is_dir()` → `ac_new.is_dir()` are three separate syscalls. Between them, the directory can be deleted/replaced.
- **Why.** Practical impact: minimal. A user actively deleting their project folder while running `open-project` on it deserves whatever error they get. Worth noting only because the plan doesn't acknowledge it.
- **Fix.** Replace with a single `std::fs::metadata(&ac_new)?` call that returns ENOENT for the missing-folder, missing-`.ac-new`, and not-a-directory cases, then disambiguate by re-checking `abs.metadata()`. Two syscalls, narrower window. Or just leave it and note "best-effort validation".

### G13 — LOW: No test exercises the relative-path branch of `absolutise()`.
- **What.** All §4.1 tests use `fix.path()` (an absolute path). The `else` branch of `absolutise` (`cwd.join(p)`) is never hit by the proposed tests.
- **Why.** Unexercised code path. If a future refactor breaks the relative-path branch, no test catches it. Also coupled with G4 above — if you fix `..` normalization, you need a test to lock in the behavior.
- **Fix.** Add a unit test that constructs a relative path, calls `register_existing_project(s, "subdir")` after `std::env::set_current_dir(fix.path())`, and asserts the persisted path is `fix.path().join("subdir")` (and NOT `subdir` alone). Note: `set_current_dir` is process-wide, so use a `serial_test` style or accept some test contamination — or refactor `absolutise` to take `cwd` as a parameter so the test can pass a fake.

### G14 — LOW: No test confirms the IPC `ProjectRegistration` shape matches the TypeScript `interface`.
- **What.** `#[serde(rename_all = "camelCase")]` on `ProjectRegistration` (plan §4.1) maps `path/registered/created` to themselves (already lowercase single-word fields, so no actual rename). TS interface (§4.8) declares the same three fields. No test asserts the JSON shape.
- **Why.** If a future field is added (say, `ac_new_dir: PathBuf`), the camelCase rename matters (`acNewDir`), and a missing `#[serde(rename_all)]` regression would silently break the FE without a Rust test catching it.
- **Fix.** Add `#[test] fn project_registration_serializes_camel_case()` in `config::projects`: serialize a `ProjectRegistration { path: "x".into(), registered: true, created: true }` and assert the JSON contains exactly `"path"`, `"registered"`, `"created"` (and no snake_case variants). Cheap insurance.

### G15 — LOW: Verb-stdout copy lies on partial-success paths.
- **What.** §2 says `new-project` on a folder where `.ac-new/` pre-exists prints `AC project already exists at <abs-path>` + `Project already registered: <abs-path>`. But what if `.ac-new` exists AND the gitignore sweep fails (e.g., disk full, .gitignore is read-only)? `register_new_project` returns `ProjectError::GitignoreFailed`, which the CLI prints as `Error: failed to write .ac-new/.gitignore at ...` and exits 1 — so the registration never happens, even though `.ac-new` was already there.
- **Why.** Surprising failure mode: user runs `new-project` on a perfectly fine existing AC project, hits a transient FS error on the gitignore sweep, and the verb fails. The user expects "register what's there" semantics; the plan gives them "register only if I can also rewrite your gitignore."
- **Fix.** Make the gitignore sweep best-effort when `created == false` (log a warning, continue with registration). Mandatory only when `created == true`. Update §6.2 accordingly: the rationale ("opportunistic") supports lenient handling, but the implementation in `register_new_project` is currently strict.

### G16 — LOW: Plan §6.6 claims "no new dependency", but tests would benefit from `tempfile`.
- **What.** §6.6 says "do not introduce new crates". `tempfile` is ALREADY a dev-dependency (`Cargo.toml:47`). The plan's hand-rolled `FixtureRoot` (4× duplicated across 3 modules) reimplements `tempfile::TempDir` with a less-robust cleanup story (panics in `Drop` swallowed; nanos-based suffix not collision-proof).
- **Why.** Not a bug, but the duplicate code adds noise across §4.1, §4.5, §4.6. Could be one shared `tests/common/fixture_root.rs` or just `tempfile::TempDir`.
- **Fix.** Either accept the duplication (since `cli::brief_set_title` and `cli::brief_ops` already do this — consistency wins), or have the dev use `tempfile::TempDir` and drop the four `FixtureRoot` blocks. Note in the plan which choice you want — currently the dev will copy-paste the duplication.

### G17 — INFO: `pickAndCheck` flow does double-validation after the change.
- **What.** `pickAndCheck` calls `ProjectAPI.checkPath(picked)` and then, if true, `loadProject(picked)` which now calls `ProjectAPI.open(picked)` — `open_project` validates `.ac-new` exists again.
- **Why.** Two filesystem round-trips for the same check. Performance: negligible. Correctness: fine. But the redundancy is worth noting in case a future refactor wants to eliminate `checkPath` for opened projects (the new `open_project` is a strict superset).
- **Fix.** Not required for this PR. Note in the plan: "Future cleanup — `ProjectAPI.checkPath` is now redundant for the open-existing-project flow; can be removed in a follow-up once all call sites are audited."

### Summary
- **Showstopper (must fix before coding)**: G1.
- **High (must fix before merge)**: G2, G3, G4, G5.
- **Medium (should fix before merge)**: G6, G7, G8, G9, G10, G11.
- **Low (nice to have)**: G12, G13, G14, G15, G16.
- **Info-only**: G17.

The architectural shape (shared helper in `config/projects.rs`, narrow Tauri commands, CLI verbs that delegate) is sound. The user-requirement-to-protect — "CLI and UI must share the same Rust backend" — is correctly met by the design. But the plan as written has one self-contradiction (G1) that will break the build, and several real correctness gaps (G2, G4, G5) that will produce silent data loss in plausible user scenarios. Recommend the architect revise §4.10 (G1), §4.1's `absolutise` (G4), and the §6.3 race story (G5), then re-submit.

---

## Implementer review (dev-rust, 2026-05-09)

### IR.1 Verification scope

I verified every code reference in §4 against the current branch HEAD (`2c1980e`):

- `src-tauri/src/cli/mod.rs` (1–167): `Commands` enum at 29–45, `handle_cli` at 140–153 — confirmed.
- `src-tauri/src/config/mod.rs` (1–50): module list at 1–7 — confirmed.
- `src-tauri/src/config/settings.rs`: `AppSettings` has both `project_path` (`Option<String>`) and `project_paths` (`Vec<String>`); `SettingsState = Arc<RwLock<AppSettings>>` (`tokio::sync::RwLock`); `load_settings()` at 376–444 (confirms G5 — auto-generates root_token + saves on first load); `save_settings()` at 467–486 does atomic tmp+rename.
- `src-tauri/src/commands/ac_discovery.rs`: `ensure_ac_new_gitignore` at 1213, `pub(crate)`, returns `Result<(), String>` — visibility OK for cross-module use; signature compatible with the plan's `GitignoreFailed(PathBuf, String)` variant. Imports `Path`, `PathBuf`, `State`, `SettingsState` already present at top of file.
- `src-tauri/src/lib.rs:828–842`: invoke_handler! confirmed — `create_ac_project` is at line 834, `discover_project` at 835 (G10 confirmed; "between X and Y" anchor remains correct).
- `src/sidebar/stores/project.ts:1–172`: `normalizePath` at 17–19, `loadProject` at 36–63, `initFromSettings` at 65–75, `createAndLoad` at 77–81, `removeProject` at 150–155 (calls `persistProjectPaths()` on line 154 — confirms G1), `persistProjectPaths` at 162–171.
- `src/sidebar/components/ActionBar.tsx:58–94`: `handleNewProject` 58–71, `handleOpenProject` 78–94 — confirmed.
- `src/sidebar/components/Toolbar.tsx:11–25`: `handleOpenProject` 11–17, `handleConfirmCreate` 19–25 — confirmed.
- `src/shared/ipc.ts:410–417` ProjectAPI block — confirmed.
- `src/shared/types.ts:316–333`: `BriefUpdateResult` at 322–325, `WorkgroupBriefUpdatedEvent` at 327–333 — insertion point should be line 334 (after both), not 326 (inside the Brief block).
- `src-tauri/Cargo.toml` line 3: `version = "0.8.9"` — confirms G7.
- `src-tauri/tauri.prod.conf.json` and `tauri.stage.conf.json`: NO `version` field — only `productName`/`identifier`/`mainBinaryName` overrides. The "(if present)" hedge in §4.12 is moot.
- `cli::brief_ops::tests::FixtureRoot` at brief_ops.rs:613–640 — matches the plan's proposed FixtureRoot byte-for-byte.

### IR.2 Position on the Grinch Review

I read the Grinch Review and concur with **G1, G3, G4, G5, G6, G7, G8, G9, G10, G11, G15** as written. **G2, G12, G13, G14, G16, G17** are accurate but I rank them lower — see below.

**Showstoppers I cannot route around as the implementer (need architect/tech-lead decision):**
- **G1 (§4.10 contradiction).** Cannot resolve without picking one of the two mutually-exclusive instructions. My recommendation: keep `persistProjectPaths` AND `removeProject` unchanged for this PR (lower blast radius; the deferred `unregister_project` follow-up will revisit). But the architect must explicitly authorize this deviation from the written plan.
- **G4 (`absolutise` does not normalize `..`/`.`).** This is a real silent-double-registration bug. My preferred fix: use `std::path::absolute` (stable since Rust 1.79; the plan's "Rust-edition-dependent" remark is mistaken). If the workspace's MSRV doesn't reach 1.79, fall back to manual component-walk + `..` collapse. Need architect sign-off on the approach because it changes the §3 contract.
- **G5 (`load_settings()` is not pure-read; root_token gen race).** The cleanest scope-controlled fix is a new `load_settings_for_cli()` that does NOT auto-generate the root_token (the CLI verbs don't need it). I recommend implementing that helper for this PR. The bigger GUI-vs-CLI race fix (advisory lockfile or `update_settings` disk re-read) should be a separate issue — too much surface for #191.

**Mediums I will resolve myself during implementation (no architect decision needed):**
- **G6 (tests touch live settings.json).** Will refactor §4.5/§4.6 tests to ONLY exercise error paths that fail BEFORE `load_settings()` triggers a save. The success-path coverage stays in `config::projects` unit tests (§4.1) which use in-memory `AppSettings::default()`. If G5's `load_settings_for_cli()` is adopted, this is even cleaner — the CLI tests become safe by construction.
- **G7 (Cargo.toml missed).** Will bump `src-tauri/Cargo.toml` 0.8.9→0.8.16 alongside the other two manifests. Drop the "(if present)" tauri.prod/stage clause — verified absent.
- **G8 (error string mentions CLI verb in GUI context).** Will drop the "Use `new-project` to create one." sentence from `ProjectError::AcNewMissing` and have the CLI verb append its own hint after the formatted error. The Tauri command propagates the bare error.
- **G9 (TOCTOU on `created`).** Will use `match std::fs::create_dir(&ac_new) { Ok(_) => created=true, Err(e) if e.kind() == ErrorKind::AlreadyExists => created=false, Err(e) => return Err(...) }`. To preserve the `mkdir -p`-the-parent semantic, run `create_dir_all(parent)` first when the parent does not exist. Adds one extra syscall in the create case; eliminates the lying-`created`-flag race.
- **G10 (line numbers).** Will treat the "between X and Y" anchors as authoritative and ignore the absolute line numbers (already off-by-2 in §4.4 and other places).
- **G11 (`loadProject` swallows errors).** Out of scope for §4.10's minimum-blast-radius rewrite, but I will preserve the existing `console.error` so behavior is at least not WORSE than today. A user-visible toast for "project at X is gone" is a follow-up.
- **G15 (gitignore sweep is strict on pre-existing `.ac-new`).** Will make the gitignore sweep best-effort when `created == false` (log a warning, continue). Mandatory only when `created == true`. Updates §6.2's intent to match the implementation.

**Lows I will resolve only if the change is one-line:**
- **G14 (no serde camelCase test).** Adding the test is one line — will add. Cheap insurance against future field additions.
- **G13 (no relative-path test).** Will add a test using `set_current_dir` inside a fixture; accept the process-wide CWD contamination caveat.

**Lows / Info I will skip:**
- **G2 (loadProject dedup against `reg.path`).** Valid concern, but the `setProjects` re-render-with-dedup pattern from `createAndLoad` is a one-line addition to `loadProject`. I'll fold it in — the plan §4.10 already does this for `createAndLoad`, so consistency calls for it.
- **G12 (TOCTOU on existence checks).** Accepting; the practical impact is negligible (active filesystem racing while running validation deserves whatever error). Leaving the 3-syscall validation as drafted.
- **G16 (use `tempfile::TempDir` instead of FixtureRoot).** Accepting the FixtureRoot duplication for consistency with `cli::brief_set_title` and `cli::brief_ops`. Follow-up issue could de-dup all four sites at once.
- **G17 (pickAndCheck double-validation).** Future cleanup, not needed here.

### IR.3 Items NOT covered by the Grinch Review

#### IR.3.1 CLI-vs-CLI lost-update race
G5 documents the GUI-vs-CLI race. There is also a CLI-vs-CLI race: two parallel `<bin> open-project` processes each `load_settings()` from disk, mutate independently, then race on the atomic-rename in `save_settings()`. Last-write wins; one registration silently lost. Window = duration of file IO. **Mitigation: out of scope** (matches the rest of `settings.json`'s mutation model; an advisory lockfile could close it but is bigger surface). **Add §6.9 to the plan** documenting this:
> **6.9 CLI-vs-CLI lost-update race.** Two parallel CLI invocations both `load_settings()` → mutate → `save_settings()`. The second `rename` clobbers the first. No advisory locking is added in this PR.

#### IR.3.2 Trailing-separator dedup gap (separate from G4)
`normalize_for_compare` does `replace('\\', "/").to_lowercase()` but does NOT strip a trailing `/`. So `C:\foo\` (key `c:/foo/`) and `C:\foo` (key `c:/foo`) become DIFFERENT entries. This is a separate bug from G4 (which is about `..`/`.`). The CLI surface aggravates this because shell tab-completion typically appends a trailing `\` on directories. **Recommended fix:**
```rust
fn normalize_for_compare(s: &str) -> String {
    s.replace('\\', "/").to_lowercase().trim_end_matches('/').to_string()
}
```
Mirror the same `trim_end_matches('/')` in `src/sidebar/stores/project.ts::normalizePath`. Both are one-line additions in the same PR.

#### IR.3.3 Case-insensitive dedup on case-sensitive filesystems
§3 explicitly mirrors the FE's `to_lowercase()`. On Linux/macOS case-sensitive filesystems, `/foo/Bar` and `/foo/bar` are distinct directories; the helper would dedup them into ONE registration. FE-inherited bug, not a regression. **Add §6.10 to the plan:**
> **6.10 Case-insensitive dedup on case-sensitive filesystems.** The dedup key lowercases the path to match the FE's contract. On Linux/macOS where `/foo/Bar` and `/foo/bar` are distinct, only the FIRST registered case-variant survives in `project_paths`. Same behaviour as the FE today. Cross-platform-correct dedup is deferred.

#### IR.3.4 DerefMut coercion in §4.3 Tauri command (confirmation, not a problem)
The plan's `let mut s = settings.write().await; register_existing_project(&mut s, ...)` relies on Rust's automatic `DerefMut` coercion `&mut RwLockWriteGuard<AppSettings> → &mut AppSettings`. Confirmed equivalent to the field-mutation pattern in `set_inject_rtk_hook` (`commands/config.rs:188-189`). The snippet compiles as-is — no `&mut *s` reborrow needed. Flagging only because the snippet looks ambiguous to a casual reader.

#### IR.3.5 Behavioral change: `initFromSettings` silently drops projects whose `.ac-new/` was deleted
Today, `initFromSettings → loadProject → ProjectAPI.discover()` silently returns empty results for paths missing `.ac-new/` and the project still appears in the sidebar (empty). After §4.10, `loadProject → ProjectAPI.open()` throws on missing `.ac-new/` and the project is silently dropped from the sidebar (only a `console.error`). This is **arguably the right new behaviour** — paths without `.ac-new/` shouldn't be in the project list — but it IS a behavioural change. **Acceptable for this PR; mention in PR description.** A toast would be a nice follow-up but requires plumbing changes outside §4.10's scope.

### IR.4 Final position

**I cannot start coding until the architect explicitly resolves G1, G4, and G5.** The other items (including all the "I will resolve myself" mediums and lows) are within my implementer mandate per Role.md ("If the plan is missing something … add it to the plan file with your reasoning. If the plan is wrong, say so."). G1, G4, and G5 cross the line into design changes — they need architect sign-off.

**Suggested architect actions (in priority order):**
1. **G1.** Choose: (a) keep `persistProjectPaths` and `removeProject` unchanged (recommended), or (b) inline the SettingsAPI.update into `removeProject` and delete the helper. Update §4.10 to match the choice.
2. **G4.** Choose: (a) `std::path::absolute` (Rust 1.79+, recommended), or (b) hand-rolled component-walk that collapses `..`/`.`. Update §4.1's `absolutise` and §3's contract paragraph. Add a unit test asserting `register_existing_project(s, "../projects")` from a known CWD persists a path with no `..` components.
3. **G5.** Approve a `load_settings_for_cli()` helper that does NOT auto-generate the root_token (recommended), OR explicitly accept the current race for this PR with a sentence in §6.3. Either is fine; both close the issue.

Once G1/G4/G5 have a written resolution in the plan, I am ready to implement everything in §4 with the IR.2 self-resolutions and IR.3 additions folded in.

---

## Round-1 Resolution (architect, 2026-05-09)

The plan body above (§3, §4.1, §4.3, §4.4, §4.5, §4.6, §4.10, §4.12, §4.13, §5.1, §6.2, §6.3, §6.9, §6.10, §6.11, §7) has been edited in-place to resolve every Round-1 finding. This section is the index — for each finding, what was decided, where the decision lands in the plan, and (where it matters) why this option over the alternative.

### Showstoppers

- **G1 — §4.10 contradiction. RESOLVED — Option (i): keep `persistProjectPaths` and `removeProject` unchanged.**
  - **Why this option over (ii):** lower blast radius. `removeProject` keeps working with zero touch, and `persistProjectPaths` stays the single source of truth for FE-driven persistence until the deferred backend `remove_project` Tauri command lands. Inlining `SettingsAPI.update` inside `removeProject` (Option ii) would create a parallel persistence path that has to be re-unified later — extra surface for no immediate gain.
  - **Where:** §4.10 — narrative paragraph at the top updated; the "Delete `persistProjectPaths`" instruction removed; new explicit "NO CHANGE" note on `removeProject`; new "KEEP" note on `persistProjectPaths`. §7 lists the eventual `remove_project` follow-up.

### High

- **G2 — `loadProject` post-`reg.path` dedup. RESOLVED.**
  - **Where:** §4.10's `loadProject` block now has the inner `setProjects` callback re-checking against `normalizePath(reg.path)` — same pattern `createAndLoad` already uses. Closes the double-render race when two concurrent calls pass differently-shaped strings that resolve to the same registered entry.

- **G3 — §4.3 insertion anchor. RESOLVED.**
  - **Where:** §4.3 anchor rewritten as "insert immediately AFTER `set_replica_context_files` (closing `}` at line 1674) and BEFORE the `#[cfg(test)] mod tests` opener at line 1676." The misleading "around line 1646" hint is dropped.

- **G4 — `absolutise` does not collapse `..`/`.`. RESOLVED — Option (a): `std::path::absolute(raw)`.**
  - **Why this option over (b) hand-rolled component-walk:** the workspace toolchain is `rustc 1.93.1` (verified via `rustc --version`), well above `std::path::absolute`'s 1.79 stabilization. It uses `GetFullPathNameW` on Windows (the project's primary target), which collapses `.`/`..` lexically without IO. Hand-rolling adds 12+ lines of code that the stdlib already gives us for free on the platform we ship to.
  - **Residual gap:** on POSIX, `std::path::absolute` preserves `..` for symlink-safety. Documented as §6.10 (out of scope, deferred). The Round-0 plan's "Rust-edition-dependent" remark was wrong and has been corrected.
  - **Where:** §3 contract paragraph rewritten; §4.1 `absolutise` body simplified to a single `std::path::absolute(raw).map_err(...)` call; §4.1 tests gain `absolutise_resolves_relative_path_against_cwd` (cross-platform) and `absolutise_collapses_dotdot_segments_on_windows` (`#[cfg(windows)]`); §5.1 step 10 added as a Windows manual-smoke verification.

- **G5 — `load_settings()` is not pure-read. RESOLVED — Option (a): new `load_settings_for_cli()` helper.**
  - **Why this option over (b) accepting the race:** the CLI does not consume `root_token`, so generating one from a CLI invocation is pure side-effect. The cost of a parallel loader is small (one duplicated migration block, flagged for a future de-dup follow-up); the benefit is that every CLI error path becomes safe-by-construction (Round-1 G6 also closes as a side effect).
  - **Where:** new §4.13 specifies the helper in `src-tauri/src/config/settings.rs`. §4.5 / §4.6 switch their imports from `load_settings` to `load_settings_for_cli`. §6.3 narrative narrowed: first-boot root_token race is closed; GUI-`update_settings`-clobbers-CLI race remains documented as a follow-up. §5.1 step 9 added as a manual-smoke verification.

### Medium

- **G6 — tests touch real settings.json. RESOLVED.** Now safe by construction: `load_settings_for_cli()` does NOT write, so the §4.5/§4.6 CLI tests (which exercise pre-load failure paths) cannot mutate disk. Success-path coverage stays in §4.1's in-memory tests.
- **G7 — Cargo.toml version bump missed. RESOLVED.** §4.12 now lists three files explicitly (tauri.conf.json, package.json, Cargo.toml) and drops the moot tauri.prod/stage hedge.
- **G8 — CLI hint in shared error string. RESOLVED.** `ProjectError::AcNewMissing` is now `"no AC project at {0} (.ac-new/ not found)"`. The CLI's `open_project::execute` appends a CLI-specific hint after the bare error when it sees `AcNewMissing`. Tauri command propagates the bare error (GUI-friendly).
- **G9 — TOCTOU on `created` flag. RESOLVED.** §4.1's `register_new_project` now uses `create_dir_all(&abs)` (mkdir-p the parent, idempotent) followed by non-recursive `create_dir(&ac_new)` with `Ok` / `AlreadyExists` matching to set `created` authoritatively.
- **G10 — line numbers off-by-2. RESOLVED.** §4.4 narrative now treats "between X and Y" anchors as authoritative and warns against the old absolute numbers.
- **G11 — `loadProject` swallows backend errors. ACCEPTED — out of scope for this PR.** Existing `console.error` swallow preserved. Documented as §6.11 PR-note + §7 follow-up.

### Low

- **G12 — TOCTOU on existence checks. ACCEPTED — no change.** Active filesystem racing during validation is a self-inflicted user error.
- **G13 — no relative-path test. RESOLVED.** §4.1 tests now include `absolutise_resolves_relative_path_against_cwd` (cross-platform) using a CWD-restoring guard. The `..`-collapse counterpart is `#[cfg(windows)]`-gated per G4's POSIX caveat.
- **G14 — no serde camelCase test. RESOLVED.** §4.1 tests now include `project_registration_serializes_camel_case` — locks the invariant for future field additions.
- **G15 — gitignore strict on pre-existing `.ac-new/`. RESOLVED.** §4.1 `register_new_project` `match`-arms make the sweep best-effort when `created == false`. §6.2 narrative updated to match.
- **G16 — use `tempfile::TempDir`. ACCEPTED — keep duplication.** Consistency with `cli::brief_set_title` and `cli::brief_ops` wins. A future de-dup pass can touch all four sites.

### Info-only

- **G17 — `pickAndCheck` double-validation. ACCEPTED — no change.** Future cleanup; not blocking.

### Dev-rust IR.3 additions

- **IR.3.1 — CLI-vs-CLI race. ACCEPTED.** Added as §6.9. No advisory locking in this PR.
- **IR.3.2 — trailing-separator dedup. RESOLVED.** §4.1's `normalize_for_compare` now ends with `.trim_end_matches('/').to_string()`. Symmetric one-line change in §4.10 to `src/sidebar/stores/project.ts::normalizePath`. New test `upsert_dedup_strips_trailing_separator` in §4.1.
- **IR.3.3 — case-sensitive POSIX dedup. ACCEPTED.** Added as §6.10 (alongside the POSIX `..` non-collapse note from G4).
- **IR.3.4 — DerefMut coercion confirmation. NOOP.** No plan change needed.
- **IR.3.5 — `initFromSettings` silently drops invalid projects. ACCEPTED.** Added as §6.11 — PR-note only, behavioural change to surface in the PR description.

### Verdict

All Round-1 findings have an explicit resolution. The architectural shape — shared helper in `config/projects.rs`, narrow Tauri commands `open_project`/`new_project`, CLI verbs delegating through the same helper — is unchanged from Round-0. The user requirement ("CLI and UI share the same Rust backend") is preserved.

**READY_FOR_IMPLEMENTATION**
