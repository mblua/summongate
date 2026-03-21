use std::sync::{Arc, Mutex};
use tauri::State;
use uuid::Uuid;

use crate::pty::manager::PtyManager;

#[tauri::command]
pub fn pty_write(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    pty_mgr
        .lock()
        .unwrap()
        .write(uuid, &data)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_resize(
    pty_mgr: State<'_, Arc<Mutex<PtyManager>>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let uuid = Uuid::parse_str(&session_id).map_err(|e| e.to_string())?;
    pty_mgr
        .lock()
        .unwrap()
        .resize(uuid, cols, rows)
        .map_err(|e| e.to_string())
}
