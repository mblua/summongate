//! Tauri commands wrapping `cli::brief_ops::perform` for the BRIEF panel
//! action buttons (issue #162).
//!
//! Trust model: these commands run inside the GUI process under the
//! user's authority — same model as `rename_session`, `destroy_session`,
//! etc. No coordinator gate (the CLI verbs in `cli/brief_*.rs` retain
//! their gate; this file does not call into them).

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::cli::brief_ops::{self, BriefOp, EditOutcome};
use crate::session::manager::SessionManager;
use crate::session::session::{find_workgroup_brief_path_for_cwd, read_workgroup_brief_for_cwd};

/// Payload returned to the frontend after a successful brief mutation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefUpdateResult {
    /// Absolute path of the workgroup root the BRIEF.md belongs to.
    /// Stripped of the Windows `\\?\` extended-length prefix when present.
    pub workgroup_root: String,
    /// Trimmed BRIEF.md content as displayed by the panel. `None` when the
    /// file is empty or missing post-edit (defensive — should not happen
    /// after a successful Wrote, but possible on race-deletion).
    pub brief: Option<String>,
}

/// Resolve the workgroup root for a session id, returning a user-facing
/// error string suitable for direct propagation through the Tauri command
/// `Result<_, String>` boundary.
async fn resolve_wg_root(
    session_mgr: &Arc<tokio::sync::RwLock<SessionManager>>,
    session_id: &str,
) -> Result<std::path::PathBuf, String> {
    let uuid = Uuid::parse_str(session_id).map_err(|e| format!("invalid session id: {}", e))?;
    let mgr = session_mgr.read().await;
    let cwd = mgr
        .get_session(uuid)
        .await
        .map(|s| s.working_directory.clone())
        .ok_or_else(|| format!("session {} not found", session_id))?;
    drop(mgr);

    let brief_path = find_workgroup_brief_path_for_cwd(&cwd)
        .ok_or_else(|| format!("session {} is not under a wg-* ancestor", session_id))?;
    let wg_root = brief_path
        .parent()
        .ok_or_else(|| "workgroup BRIEF.md path has no parent".to_string())?
        .to_path_buf();
    Ok(wg_root)
}

fn strip_unc(p: &Path) -> String {
    let raw = p.to_string_lossy().into_owned();
    raw.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(raw)
}

fn emit_brief_updated(app: &AppHandle, wg_root: &Path, brief: &Option<String>) {
    let _ = app.emit(
        "workgroup_brief_updated",
        serde_json::json!({
            "workgroupRoot": strip_unc(wg_root),
            "brief": brief.clone(),
        }),
    );
}

/// Read the current YAML-frontmatter `title:` value of the workgroup
/// BRIEF.md for the given session. Returns `None` when there is no
/// frontmatter or no `title:` line.
#[tauri::command]
pub async fn brief_get_title(
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<Option<String>, String> {
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    let brief_path = wg_root.join("BRIEF.md");
    let content = match std::fs::read_to_string(&brief_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("read BRIEF.md: {}", e)),
    };
    let parsed = brief_ops::parse_brief(&content);
    Ok(brief_ops::title_value_of(&parsed))
}

/// Set the YAML-frontmatter `title:` field of the workgroup BRIEF.md for
/// the given session. Returns the new (post-edit) trimmed BRIEF.md
/// content for direct local refresh, AND emits `workgroup_brief_updated`
/// for sibling sessions/windows.
#[tauri::command]
pub async fn brief_set_title(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
    title: String,
) -> Result<BriefUpdateResult, String> {
    if title.trim().is_empty() {
        return Err("title cannot be empty".to_string());
    }
    if title.chars().any(|c| c.is_control() && c != '\t') {
        return Err(
            "title must be a single line of printable characters \
             (control characters other than tab are not allowed)"
                .to_string(),
        );
    }
    // Round 2 (Grinch LOW-1): cap at 256 chars (typical YAML scalar
    // convention). Prevents a 1 MB pasted blob from becoming the title
    // and breaking panel layout / file ergonomics. Counts Unicode
    // scalars, not bytes — a 256-emoji title is allowed and renders
    // sensibly.
    if title.chars().count() > 256 {
        return Err("title is too long (max 256 characters)".to_string());
    }
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    match brief_ops::perform(&wg_root, BriefOp::SetTitle(title)) {
        Ok(EditOutcome::Wrote { .. }) | Ok(EditOutcome::NoOp) => {
            let brief = read_workgroup_brief_for_cwd(&wg_root.to_string_lossy());
            let result = BriefUpdateResult {
                workgroup_root: strip_unc(&wg_root),
                brief: brief.clone(),
            };
            emit_brief_updated(&app, &wg_root, &brief);
            Ok(result)
        }
        Err(e) => Err(format!("{}", e)),
    }
}

/// Replace the workgroup BRIEF.md with the canonical Limpio form
/// (`title: 'Limpio'` + body `Limpio`). Returns the new content and
/// emits `workgroup_brief_updated`.
#[tauri::command]
pub async fn brief_clean(
    app: AppHandle,
    session_mgr: State<'_, Arc<tokio::sync::RwLock<SessionManager>>>,
    session_id: String,
) -> Result<BriefUpdateResult, String> {
    let wg_root = resolve_wg_root(&session_mgr, &session_id).await?;
    match brief_ops::perform(&wg_root, BriefOp::Clean) {
        Ok(EditOutcome::Wrote { .. }) | Ok(EditOutcome::NoOp) => {
            let brief = read_workgroup_brief_for_cwd(&wg_root.to_string_lossy());
            let result = BriefUpdateResult {
                workgroup_root: strip_unc(&wg_root),
                brief: brief.clone(),
            };
            emit_brief_updated(&app, &wg_root, &brief);
            Ok(result)
        }
        Err(e) => Err(format!("{}", e)),
    }
}
