use std::sync::Arc;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;
use crate::session::session::SessionInfo;

#[tauri::command]
pub async fn create_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    profile_name: Option<String>,
) -> Result<SessionInfo, String> {
    let _ = profile_name; // TODO: use profiles in Phase 2

    let shell = "powershell.exe".to_string();
    let shell_args = vec!["-NoLogo".to_string()];
    let cwd = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "C:\\".to_string());

    let mgr = session_mgr.read().await;
    let session = mgr
        .create_session(shell.clone(), shell_args.clone(), cwd.clone())
        .await
        .map_err(|e| e.to_string())?;

    let id = session.id;

    // Spawn PTY
    pty_mgr
        .lock()
        .unwrap()
        .spawn(id, &shell, &shell_args, &cwd, 120, 30, app.clone())
        .map_err(|e| e.to_string())?;

    let info = SessionInfo::from(&session);

    // Emit session_created event
    let _ = app.emit("session_created", info.clone());

    Ok(info)
}

#[tauri::command]
pub async fn destroy_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    // Kill the PTY first
    pty_mgr
        .lock()
        .unwrap()
        .kill(uuid)
        .map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    let new_active = mgr
        .destroy_session(uuid)
        .await
        .map_err(|e| e.to_string())?;

    let _ = app.emit("session_destroyed", serde_json::json!({ "id": id }));

    // If a new session was auto-activated, emit switch event
    if let Some(new_id) = new_active {
        let _ = app.emit(
            "session_switched",
            serde_json::json!({ "id": new_id.to_string() }),
        );
    }

    Ok(())
}

#[tauri::command]
pub async fn switch_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    mgr.switch_session(uuid)
        .await
        .map_err(|e| e.to_string())?;

    let _ = app.emit("session_switched", serde_json::json!({ "id": id }));

    Ok(())
}

#[tauri::command]
pub async fn rename_session(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    id: String,
    name: String,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;

    let mgr = session_mgr.read().await;
    mgr.rename_session(uuid, name.clone())
        .await
        .map_err(|e| e.to_string())?;

    let _ = app.emit(
        "session_renamed",
        serde_json::json!({ "id": id, "name": name }),
    );

    Ok(())
}

#[tauri::command]
pub async fn list_sessions(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
) -> Result<Vec<SessionInfo>, String> {
    let mgr = session_mgr.read().await;
    Ok(mgr.list_sessions().await)
}

#[tauri::command]
pub async fn get_active_session(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
) -> Result<Option<String>, String> {
    let mgr = session_mgr.read().await;
    Ok(mgr.get_active().await.map(|id| id.to_string()))
}
