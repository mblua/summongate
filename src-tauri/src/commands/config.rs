use std::sync::Arc;
use tauri::State;

use crate::config::claude_settings::{enumerate_managed_agent_dirs, ensure_rtk_pretool_hook};
use crate::config::settings::{load_settings, save_settings, AppSettings, SettingsState};
use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::web::auth::WebAccessToken;
use crate::web::broadcast::WsBroadcaster;
use crate::{RtkSweepLockState, WebServerHandle};

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

#[tauri::command]
pub async fn save_debug_logs(content: String) -> Result<(), String> {
    let path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join("debug-logs.txt");
    tokio::fs::write(&path, &content)
        .await
        .map_err(|e| format!("Failed to write logs: {}", e))?;
    log::info!("Debug logs saved to {:?} ({} bytes)", path, content.len());
    Ok(())
}

#[tauri::command]
pub async fn get_settings(settings: State<'_, SettingsState>) -> Result<AppSettings, String> {
    let s = settings.read().await;
    let mut result = s.clone();
    result.root_token = None; // never expose root token to frontend
    Ok(result)
}

#[tauri::command]
pub async fn update_settings(
    settings: State<'_, SettingsState>,
    new_settings: AppSettings,
) -> Result<(), String> {
    let mut to_save = new_settings;
    // Preserve existing root token — frontend cannot overwrite it
    to_save.root_token = settings.read().await.root_token.clone();
    crate::config::settings::validate_agent_commands(&to_save)?;
    save_settings(&to_save)?;
    let mut s = settings.write().await;
    *s = to_save;
    Ok(())
}

#[tauri::command]
pub async fn open_web_remote() -> Result<(), String> {
    let settings = load_settings();
    if !settings.web_server_enabled {
        return Err("Web server is not enabled".into());
    }

    let token_path = crate::config::config_dir()
        .ok_or("No config dir")?
        .join("web-token.txt");

    let token = std::fs::read_to_string(&token_path)
        .map_err(|e| format!("Cannot read web token: {}", e))?;

    let url = format!(
        "http://{}:{}/?window=browser&remoteToken={}",
        settings.web_server_bind,
        settings.web_server_port,
        token.trim()
    );

    open::that(&url).map_err(|e| format!("Failed to open browser: {}", e))?;
    Ok(())
}

// Tauri command: State<> injections push us over clippy's 7-arg threshold.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_web_server(
    app_handle: tauri::AppHandle,
    ws_handle: State<'_, WebServerHandle>,
    settings: State<'_, SettingsState>,
    web_token: State<'_, Arc<WebAccessToken>>,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<std::sync::Mutex<PtyManager>>>,
    broadcaster: State<'_, WsBroadcaster>,
    shutdown: State<'_, crate::shutdown::ShutdownSignal>,
) -> Result<bool, String> {
    let s = settings.read().await;
    let bind = s.web_server_bind.clone();
    let port = s.web_server_port;
    drop(s);

    // Check if already listening
    let addr = format!("{}:{}", bind, port);
    if tokio::net::TcpStream::connect(&addr).await.is_ok() {
        return Ok(false); // already running
    }

    let join_handle = crate::web::start_server(
        bind,
        port,
        Arc::clone(&web_token),
        Arc::clone(&session_mgr),
        Arc::clone(&pty_mgr),
        Arc::clone(&settings),
        (*broadcaster).clone(),
        app_handle,
        shutdown.inner().clone(),
    );

    *ws_handle.lock().unwrap() = Some(join_handle);
    log::info!("[web-server] Started via command");
    Ok(true)
}

#[tauri::command]
pub async fn stop_web_server(ws_handle: State<'_, WebServerHandle>) -> Result<bool, String> {
    let mut guard = ws_handle.lock().unwrap();
    if let Some(handle) = guard.take() {
        handle.abort();
        log::info!("[web-server] Stopped via command");
        Ok(true)
    } else {
        Ok(false)
    }
}

#[tauri::command]
pub async fn get_web_server_status(settings: State<'_, SettingsState>) -> Result<bool, String> {
    let s = settings.read().await;
    let addr = format!("{}:{}", s.web_server_bind, s.web_server_port);
    drop(s);
    // Probe the port to check if the server is actually listening
    match tokio::net::TcpStream::connect(&addr).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Returns the runtime instance label for the titlebar badge.
/// E.g. "STAGE", "STANDALONE", or "" for prod (no badge).
#[tauri::command]
pub fn get_instance_label() -> String {
    crate::config::profile::instance_label().to_string()
}

/// Narrow setter — flips ONLY `inject_rtk_hook`. Holds the SettingsState
/// write lock through `save_settings` so a concurrent `update_settings`
/// from the SettingsModal cannot overwrite the change at the IPC boundary
/// (issue #120, grinch H3 + N1). The explicit `drop(s)` after `save_settings`
/// makes the guard scope visually unambiguous: lock-then-write-then-release.
/// Caller is responsible for triggering `sweep_rtk_hook` if disk side-effects
/// on replicas are desired.
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
/// pattern as `set_inject_rtk_hook` (issue #120, grinch H3 + N1).
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

/// Sweep every AC-managed agent directory and apply
/// `ensure_rtk_pretool_hook(dir, enabled)`. Best-effort per directory:
/// per-dir failures are logged + appended to `errors` and the sweep
/// continues. Reads `project_paths` from the live `SettingsState` (avoids a
/// disk-read race against `save_settings`).
///
/// Acquires `RtkSweepLockState` for the entire loop — eliminates the
/// in-process race vs. concurrent `ensure_claude_md_excludes` /
/// `ensure_rtk_pretool_hook` calls from `entity_creation` /
/// `agent_creator` (issue #120, grinch M8). Cross-process races (two AC
/// instances) remain documented in the plan §7.4.
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

/// Snapshot of the RTK startup decision, computed lazily so the frontend can
/// query it after mounting (avoids the event race with `lib.rs::setup`'s
/// emit). Pure read — does NOT auto-disable, does NOT sweep. Auto-disable
/// runs once per boot in the setup task.
#[tauri::command]
pub async fn get_rtk_startup_status(
    settings: State<'_, SettingsState>,
) -> Result<String, String> {
    let rtk_present = which::which("rtk").is_ok();
    let s = settings.read().await;
    let mode = match (rtk_present, s.inject_rtk_hook, s.rtk_prompt_dismissed) {
        (true, false, false) => "prompt-enable",
        (true, true, _) => "active",
        (false, true, _) => "auto-disabled",
        _ => "silent",
    };
    Ok(mode.to_string())
}
