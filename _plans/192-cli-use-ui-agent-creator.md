# Plan: CLI create-agent reuses UI agent folder creator (#192)

Branch: `feature/192-cli-use-ui-agent-creator`
Repo: `repo-AgentsCommander`
Anchored against: current branch state inspected on 2026-05-10.

## 1. Requirement

The UI modal path is the proven agent folder creation path. The CLI `create-agent`
verb currently duplicates the folder creation and `CLAUDE.md` write logic in
`src-tauri/src/cli/create_agent.rs`, which allows UI and CLI behavior to drift.

Refactor the Rust backend creation logic into one shared helper used by both:

- UI command: `src-tauri/src/commands/agent_creator.rs::create_agent_folder`
- CLI command: `src-tauri/src/cli/create_agent.rs::execute`

Keep CLI `--launch` and session request behavior intact. This issue is only
about the plain folder creation implementation: `<parent>/<name>/`, the
`CLAUDE.md` content, the returned path/name fields, and preserving the existing
launch continuation after creation.

## 2. Affected Files

| File | Lines now | Change |
|---|---:|---|
| `src-tauri/src/config/mod.rs` | 1-7 | Export the new pure filesystem helper module. |
| `src-tauri/src/config/agent_creation.rs` | new | Add the shared creation helper and unit tests. |
| `src-tauri/src/commands/agent_creator.rs` | clean HEAD: 1, 20-55; dirty draft: 3-85 and 114-164 | Remove the inline draft helper/tests if present; keep Tauri command glue and delegate to `config::agent_creation`. |
| `src-tauri/src/cli/create_agent.rs` | clean HEAD: 1-5, 63-117, 135-149, 165-172, 197-203; dirty draft: 63-77, 96-107, 123-131, 154-160 | Remove duplicate create/write logic and consume the shared helper result while preserving launch/session-request flow. |
| `src-tauri/tauri.conf.json` | 4 | Bump version from `0.8.13` to `0.8.14` per saved release-identification feedback. |

No frontend files need to change. Verified:

- `src/shared/ipc.ts:471-479` keeps `AgentCreatorAPI.createFolder(...) -> string`.
- `src/sidebar/components/NewAgentModal.tsx:32-35, 62, 77, 98-101` keeps trimming and launch behavior.
- `src-tauri/src/phone/mailbox.rs:1812-1840` keeps consuming the existing camelCase `SessionRequest` JSON.

## 3. Implementation Instructions

Before coding, clean the working tree so the issue implementation does not
include unrelated drift. The current working tree contains a partial tech-lead
draft in `commands/agent_creator.rs` and `cli/create_agent.rs`, plus unrelated
changes in many other files. Implementation must not commit the unrelated
files. The implementer may stash/revert unrelated local drift, but must not lose
user work.

### 3.1 `src-tauri/src/config/mod.rs`

Current module block is:

```rust
pub mod agent_config;
pub mod claude_settings;
pub mod profile;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;
pub mod teams;
```

Insert the new module immediately after `agent_config`:

```rust
pub mod agent_config;
pub mod agent_creation;
pub mod claude_settings;
pub mod profile;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;
pub mod teams;
```

### 3.2 New file `src-tauri/src/config/agent_creation.rs`

Create this file with the shared, synchronous helper below. It must not depend
on Tauri state, tokio, IPC types, CLI args, or `commands::*`.

Use `std::fs::create_dir`, not `create_dir_all`. Because the helper validates
that `agent_name` is a single path segment and verifies the parent exists,
`create_dir` is sufficient and avoids the race where a second concurrent caller
can pass an `exists()` precheck and still get `Ok(())` from `create_dir_all`.

```rust
use std::io::ErrorKind;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedAgentFolder {
    pub agent_dir: PathBuf,
    pub display_name: String,
    pub claude_md: String,
}

/// Creates an agent folder with a CLAUDE.md inside it.
///
/// This is the single backend implementation used by both the UI
/// `create_agent_folder` command and the CLI `create-agent` verb.
pub fn create_agent_folder_on_disk(
    parent_path: &str,
    agent_name: &str,
) -> Result<CreatedAgentFolder, String> {
    let parent = PathBuf::from(parent_path);
    if !parent.exists() {
        return Err(format!("Parent folder does not exist: {}", parent_path));
    }

    let agent_name = agent_name.trim();
    if agent_name.is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }
    if agent_name.contains('/') || agent_name.contains('\\') || agent_name.contains('\0') {
        return Err("Agent name cannot contain path separators".to_string());
    }

    let agent_dir = parent.join(agent_name);
    match std::fs::create_dir(&agent_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            return Err(format!("Folder already exists: {}", agent_dir.display()));
        }
        Err(e) => return Err(format!("Failed to create folder: {}", e)),
    }

    let parent_name = parent
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| parent_path.to_string());
    let display_name = format!("{}/{}", parent_name, agent_name);

    let claude_md = format!("You are the agent {}", display_name);
    let claude_path = agent_dir.join("CLAUDE.md");
    std::fs::write(&claude_path, &claude_md)
        .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;

    // TODO: When replica creation is added (for __agent_* dirs inside workgroups),
    // write config.json with: { "context": ["$AGENTSCOMMANDER_CONTEXT"] }
    // so that replicas get the global context by default.

    Ok(CreatedAgentFolder {
        agent_dir,
        display_name,
        claude_md,
    })
}
```

Add unit tests in the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_folder_and_claude_md_matching_ui_modal() {
        let tmp = tempdir().expect("tempdir");
        let parent = tmp.path().join("ProjectAlpha");
        std::fs::create_dir_all(&parent).expect("parent");
        let parent_s = parent.to_string_lossy().to_string();

        let created = create_agent_folder_on_disk(&parent_s, "architect").expect("created");

        let expected_dir = parent.join("architect");
        assert_eq!(created.agent_dir, expected_dir);
        assert_eq!(created.display_name, "ProjectAlpha/architect");
        assert_eq!(created.claude_md, "You are the agent ProjectAlpha/architect");
        assert!(expected_dir.is_dir());
        assert_eq!(
            std::fs::read_to_string(expected_dir.join("CLAUDE.md")).expect("claude"),
            "You are the agent ProjectAlpha/architect"
        );
    }

    #[test]
    fn trims_name_before_creating_folder_and_display_name() {
        let tmp = tempdir().expect("tempdir");
        let parent = tmp.path().join("ProjectAlpha");
        std::fs::create_dir_all(&parent).expect("parent");
        let parent_s = parent.to_string_lossy().to_string();

        let created = create_agent_folder_on_disk(&parent_s, " MyAgent ").expect("created");

        let expected_dir = parent.join("MyAgent");
        assert_eq!(created.agent_dir, expected_dir);
        assert_eq!(created.display_name, "ProjectAlpha/MyAgent");
        assert_eq!(created.claude_md, "You are the agent ProjectAlpha/MyAgent");
        assert!(expected_dir.is_dir());
        assert!(!parent.join(" MyAgent ").exists());
    }

    #[test]
    fn errors_when_parent_folder_is_missing() {
        let tmp = tempdir().expect("tempdir");
        let missing = tmp.path().join("missing");
        let missing_s = missing.to_string_lossy().to_string();

        let err = create_agent_folder_on_disk(&missing_s, "architect").expect_err("missing parent");

        assert_eq!(err, format!("Parent folder does not exist: {}", missing_s));
    }

    #[test]
    fn errors_when_agent_name_is_empty_after_trim() {
        let tmp = tempdir().expect("tempdir");
        let parent_s = tmp.path().to_string_lossy().to_string();

        let err = create_agent_folder_on_disk(&parent_s, "   ").expect_err("empty");

        assert_eq!(err, "Agent name cannot be empty");
    }

    #[test]
    fn errors_when_agent_name_contains_path_separator_or_nul() {
        let tmp = tempdir().expect("tempdir");
        let parent_s = tmp.path().to_string_lossy().to_string();

        for name in ["a/b", "a\\b", "a\0b"] {
            let err = create_agent_folder_on_disk(&parent_s, name).expect_err("separator");
            assert_eq!(err, "Agent name cannot contain path separators");
        }
    }

    #[test]
    fn errors_when_agent_folder_already_exists_without_overwriting() {
        let tmp = tempdir().expect("tempdir");
        let parent = tmp.path().join("ProjectAlpha");
        let agent_dir = parent.join("architect");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        std::fs::write(agent_dir.join("CLAUDE.md"), "keep me").expect("seed");
        let parent_s = parent.to_string_lossy().to_string();

        let err = create_agent_folder_on_disk(&parent_s, "architect").expect_err("exists");

        assert_eq!(err, format!("Folder already exists: {}", agent_dir.display()));
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("CLAUDE.md")).expect("claude"),
            "keep me"
        );
    }
}
```

### 3.3 `src-tauri/src/commands/agent_creator.rs`

If the current dirty draft is still present, remove these draft-only pieces:

- `CreatedAgentFolder` at current dirty lines 3-9.
- `create_agent_folder_on_disk` at current dirty lines 38-85.
- the helper tests at current dirty lines 114-164.

Keep `PathBuf`; it is still needed by `pick_folder` and
`write_claude_settings_local`.

Add the helper import below the existing `PathBuf` import:

```rust
use std::path::PathBuf;

use crate::config::agent_creation;
```

Replace the full body of `create_agent_folder` from clean HEAD lines 27-54 with:

```rust
    let created = agent_creation::create_agent_folder_on_disk(&parent_path, &agent_name)?;
    Ok(created.agent_dir.to_string_lossy().to_string())
```

The final command should be:

```rust
#[tauri::command]
pub async fn create_agent_folder(
    parent_path: String,
    agent_name: String,
) -> Result<String, String> {
    let created = agent_creation::create_agent_folder_on_disk(&parent_path, &agent_name)?;
    Ok(created.agent_dir.to_string_lossy().to_string())
}
```

Do not change `pick_folder` or `write_claude_settings_local`.

### 3.4 `src-tauri/src/cli/create_agent.rs`

Update imports at clean HEAD lines 1-5:

```rust
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::config::{self, agent_creation};
```

Remove `use std::path::PathBuf;`. Validation is now centralized in the helper.

Replace clean HEAD lines 63-117 with:

```rust
pub fn execute(args: CreateAgentArgs) -> i32 {
    let created = match agent_creation::create_agent_folder_on_disk(&args.parent, &args.name) {
        Ok(created) => created,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };

    let agent_path_str = created.agent_dir.to_string_lossy().to_string();
    let mut launched = false;
    let mut launch_agent_id: Option<String> = None;
```

If applying over the current dirty draft, also replace the dirty call to
`crate::commands::agent_creator::create_agent_folder_on_disk` at current dirty
lines 63-71 with the `agent_creation` call above, and remove the dirty
temporaries at current dirty lines 73-77:

```rust
let agent_dir = created.agent_dir.clone();
let agent_path_str = created.agent_path.clone();
let full_agent_name = created.agent_name.clone();
let claude_content = created.claude_md.clone();
```

In the `--launch` block, replace the `agent_dir` borrows at clean HEAD lines
137 and 146 with `created.agent_dir`:

```rust
if agent.exclude_global_claude_md {
    if let Err(e) = config::claude_settings::ensure_claude_md_excludes(&created.agent_dir) {
        eprintln!("Warning: failed to write claude settings: {}", e);
    }
}
if let Err(e) = config::claude_settings::ensure_rtk_pretool_hook(
    &created.agent_dir,
    settings.inject_rtk_hook,
) {
    eprintln!("Warning: failed to apply rtk hook: {}", e);
}
```

Do not move these calls into the new helper. They are launch-agent-specific
behavior, not plain folder creation.

In `SessionRequest` construction at clean HEAD lines 165-172, use the helper's
display name:

```rust
let request = SessionRequest {
    id: uuid::Uuid::new_v4().to_string(),
    cwd: agent_path_str.clone(),
    session_name: created.display_name.clone(),
    agent_id: agent.id.clone(),
    shell,
    shell_args,
    timestamp: chrono::Utc::now().to_rfc3339(),
};
```

In the JSON result at clean HEAD lines 197-203, use the helper fields:

```rust
let result = CreateAgentResult {
    agent_path: agent_path_str,
    agent_name: created.display_name,
    claude_md: created.claude_md,
    launched,
    launch_agent: launch_agent_id,
};
```

Do not otherwise change the `--launch` block. Preserve:

- agent lookup by id, label, label substring, or command prefix at clean HEAD lines 125-131.
- `git_pull_before` wrapping into `cmd.exe /K git pull && ...` at clean HEAD lines 153-157.
- normal shell split behavior at clean HEAD lines 152 and 159-162.
- `write_session_request(&request)` at clean HEAD lines 175-183.
- warning-only behavior when launch agent is not found at clean HEAD lines 185-192.

### 3.5 `src-tauri/tauri.conf.json`

At line 4, change:

```json
"version": "0.8.13",
```

to:

```json
"version": "0.8.14",
```

## 4. Behavior Parity

After implementation, these must remain true:

- UI `AgentCreatorAPI.createFolder(parentPath, agentName)` still invokes `create_agent_folder` and receives only the created folder path string.
- UI folder creation still writes `CLAUDE.md` as `You are the agent <parentFolder>/<agentName>` with no trailing newline.
- CLI `create-agent --parent P --name N` returns the same JSON fields as before: `agentPath`, `agentName`, `claudeMd`, `launched`, `launchAgent`.
- CLI names are trimmed before creation. `--name " MyAgent "` creates `<parent>/MyAgent`, not `<parent>/ MyAgent `.
- CLI invalid names now surface helper wording through stderr, e.g. `Error: Agent name cannot be empty` and `Error: Agent name cannot contain path separators`. Exit code remains `1`.
- CLI `create-agent --parent P --name N --launch A` still creates the folder first, then resolves `A`, writes launch-specific Claude settings when applicable, writes a session request JSON file, and reports `launched`/`launchAgent`.
- CLI `--launch` with an unknown agent still leaves the created folder in place and prints the existing warning without writing a session request.
- No new frontend IPC method, Tauri command, or session manager path is introduced.

## 5. Dependencies

No new runtime dependencies.

No new dev dependency is required because `src-tauri/Cargo.toml:46-47` already includes:

```toml
[dev-dependencies]
tempfile = "3"
```

## 6. Verification

Run after cleaning the working tree and applying only this issue's changes:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml agent_creation
cargo check --manifest-path src-tauri/Cargo.toml
```

For CLI smoke, do not run a stale `target/debug` binary after only
`cargo check`. Either run through Cargo:

```powershell
New-Item -ItemType Directory -Force $env:TEMP\ac-192-parent | Out-Null
cargo run --manifest-path src-tauri/Cargo.toml -- create-agent --parent $env:TEMP\ac-192-parent --name cliSmoke
```

or build first and then run the freshly built binary:

```powershell
cargo build --manifest-path src-tauri/Cargo.toml
.\src-tauri\target\debug\agentscommander-new.exe create-agent --parent $env:TEMP\ac-192-parent --name cliSmoke
```

Expected stdout still includes these fields:

```json
{
  "agentPath": "...",
  "agentName": "ac-192-parent/cliSmoke",
  "claudeMd": "You are the agent ac-192-parent/cliSmoke",
  "launched": false,
  "launchAgent": null
}
```

For `--launch`, use an existing configured agent id and verify that a new JSON
file appears in `<config_dir>/session-requests/` with:

- `cwd` equal to the created folder path.
- `sessionName` equal to `<parentFolder>/<name>`.
- `agentId` equal to the resolved configured agent id.
- `shell` and `shellArgs` unchanged from current behavior.

UI smoke:

- Open the New Agent modal.
- Create a folder.
- Confirm the returned path is the new folder.
- Confirm `CLAUDE.md` content matches `You are the agent <parentFolder>/<agentName>`.
- If launching from the modal with an agent that excludes global Claude MD, confirm `write_claude_settings_local` still writes launch-specific settings.

## 7. Notes and Edge Cases

- Validation belongs in the helper, not only in CLI or client-side UI. The helper rejects empty-after-trim names and names containing `/`, `\`, or NUL.
- Do not call the async Tauri command from the CLI. The shared helper is sync and filesystem-only so both surfaces can use it safely.
- Do not move `ensure_claude_md_excludes` or `ensure_rtk_pretool_hook` into the helper. Those are launch/config behaviors, not folder creation. Moving them would change UI behavior for "create then close without launching".
- Do not add rollback on partial failure. Today, if folder creation succeeds but `CLAUDE.md` write fails, the command returns an error and may leave the directory behind. Changing cleanup behavior is out of scope.
- Do not fix command splitting for quoted commands in this issue. The current `split_whitespace` behavior in `create_agent.rs` is preserved to keep launch/session-request behavior intact.
- Do not change `SessionRequest` serialization. `phone/mailbox.rs:1812-1840` depends on the existing camelCase fields and processing semantics.
- The helper returns a `PathBuf` rather than a pre-stringified path so future call sites can pass paths to filesystem helpers without lossy string round-trips. String conversion remains at the IPC/JSON boundaries.
- Keep helper location as `src-tauri/src/config/agent_creation.rs`. `config/` already contains pure filesystem/config helpers, while `commands/` should remain Tauri command glue.
- Keep returned struct shape as `CreatedAgentFolder { agent_dir, display_name, claude_md }`. Do not add a duplicate `agent_path` string or call the display label `agent_name`.
- Operational review gap: the tech lead reported two Grinch review sessions hung and had to be closed. A Grinch review block was present in the existing draft plan, and its actionable findings have been folded into this resolved plan: `create_dir` instead of `create_dir_all`, trim-semantics tests, and non-stale CLI smoke.

## 8. Implementation Order for Dev-Rust

1. Clean/stash unrelated working-tree drift. Keep only issue #192 changes in the final diff.
2. Remove the partial inline helper draft from `src-tauri/src/commands/agent_creator.rs` if it is still present.
3. Add `src-tauri/src/config/agent_creation.rs` exactly as specified, including tests.
4. Export `pub mod agent_creation;` in `src-tauri/src/config/mod.rs`.
5. Update `src-tauri/src/commands/agent_creator.rs` to delegate the Tauri command to `config::agent_creation`.
6. Update `src-tauri/src/cli/create_agent.rs` to call `config::agent_creation`, use `created.agent_dir` for launch settings, use `created.display_name` for session/request JSON names, and remove `PathBuf`.
7. Bump `src-tauri/tauri.conf.json` from `0.8.13` to `0.8.14`.
8. Run the verification commands in section 6 and fix any failures before reporting completion.
9. Confirm `git diff --stat` contains only the planned files, then commit to `feature/192-cli-use-ui-agent-creator` only.
