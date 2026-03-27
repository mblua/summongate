use std::sync::{Arc, Mutex};
use tauri::Manager;
use uuid::Uuid;

use crate::pty::manager::PtyManager;
use crate::session::manager::SessionManager;

/// Returns true when the given shell command requires a separate Enter keystroke
/// to submit pasted input. Coding agents (Claude, Codex, etc.) all need explicit
/// Enter after a text block paste. Plain shells (bash, powershell) don't go through
/// this path — they're filtered out before reaching inject_text_into_session.
fn needs_explicit_enter(shell: &str) -> bool {
    let s = shell.trim_start();
    s.starts_with("codex") || s.starts_with("claude")
}

/// Inject a text block into a session's PTY stdin.
///
/// - `submit = false` → passive injection (init prompt, token refresh).
///   Text is written as-is. No Enter is ever sent, regardless of agent type.
/// - `submit = true` → active injection (message delivery, Telegram input).
///   For agents that require explicit Enter (Codex), a `\r` is sent as a
///   separate write after a 500 ms delay — mirrors the voice auto-execute pattern.
///
/// This is the ONLY function that should be used for text-block injection.
/// Direct keystrokes from xterm.js (single chars, Ctrl sequences) bypass this
/// and call PtyManager::write() directly via the pty_write Tauri command.
pub async fn inject_text_into_session(
    app: &tauri::AppHandle,
    session_id: Uuid,
    text: &str,
    submit: bool,
) -> Result<(), String> {
    // Resolve shell without holding any lock across an await point
    let shell = {
        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let result = mgr.get_shell(session_id).await;
        drop(mgr);
        result
    };

    let send_enter = submit && shell.as_deref().map(needs_explicit_enter).unwrap_or(false);

    // Write the text block
    {
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?
            .write(session_id, text.as_bytes())
            .map_err(|e| format!("PTY write failed: {}", e))?;
    }

    // Codex: send Enter as a separate write after 500 ms
    if send_enter {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let pty_mgr = app.state::<Arc<Mutex<PtyManager>>>();
        pty_mgr
            .lock()
            .map_err(|_| "PtyManager lock poisoned".to_string())?
            .write(session_id, b"\r")
            .map_err(|e| format!("PTY Enter write failed: {}", e))?;
    }

    Ok(())
}
