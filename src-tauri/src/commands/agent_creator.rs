use std::path::PathBuf;

/// Opens a native folder picker dialog and returns the selected path.
#[tauri::command]
pub async fn pick_folder(default_path: Option<String>) -> Result<Option<String>, String> {
    let mut dialog =
        rfd::AsyncFileDialog::new().set_title("Select parent folder for the new agent");

    if let Some(ref p) = default_path {
        let path = PathBuf::from(p);
        if path.exists() {
            dialog = dialog.set_directory(&path);
        }
    }

    let result = dialog.pick_folder().await;
    Ok(result.map(|h| h.path().to_string_lossy().to_string()))
}

/// Creates an agent folder with a CLAUDE.md inside it.
/// Returns the full path of the created folder.
#[tauri::command]
pub async fn create_agent_folder(
    parent_path: String,
    agent_name: String,
) -> Result<String, String> {
    let parent = PathBuf::from(&parent_path);
    if !parent.exists() {
        return Err(format!("Parent folder does not exist: {}", parent_path));
    }

    let agent_dir = parent.join(&agent_name);
    if agent_dir.exists() {
        return Err(format!("Folder already exists: {}", agent_dir.display()));
    }

    std::fs::create_dir_all(&agent_dir).map_err(|e| format!("Failed to create folder: {}", e))?;

    // Derive the display name: last component of parent / agent_name
    let parent_name = parent
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| parent_path.clone());

    let claude_md = format!("You are the agent {}/{}", parent_name, agent_name);
    let claude_path = agent_dir.join("CLAUDE.md");
    std::fs::write(&claude_path, claude_md)
        .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;

    // TODO: When replica creation is added (for __agent_* dirs inside workgroups),
    // write config.json with: { "context": ["$AGENTSCOMMANDER_CONTEXT"] }
    // so that replicas get the global context by default.

    Ok(agent_dir.to_string_lossy().to_string())
}

/// Creates or updates .claude/settings.local.json with claudeMdExcludes in the given directory.
/// Works on any directory — both new agent folders and existing repos.
///
/// Issue #120 — also applies the rtk PreToolUse hook based on the global toggle.
/// Acquires `RtkSweepLockState` around the helper sequence (M8) so concurrent
/// sweeps cannot interleave a read-modify-write on the same file.
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
