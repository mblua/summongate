use serde::Serialize;
use std::path::Path;
use tauri::State;

use crate::config::settings::SettingsState;

/// Known agent/tool directory markers and their display labels
const AGENT_MARKERS: &[(&str, &str)] = &[
    (".claude", "Claude"),
    (".codex", "Codex"),
    (".cursor", "Cursor"),
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoMatch {
    pub name: String,
    pub path: String,
    /// Which agent tools are detected in this repo (e.g. ["Claude", "Codex"])
    pub agents: Vec<String>,
}

/// Detect which agent tools are configured in a repo directory
fn detect_agents(repo_path: &Path) -> Vec<String> {
    AGENT_MARKERS
        .iter()
        .filter(|(dir, _)| repo_path.join(dir).is_dir())
        .map(|(_, label)| label.to_string())
        .collect()
}

/// Add a repo to results if it matches the query and hasn't been seen yet
fn try_add_repo(
    path: &Path,
    query_lower: &str,
    seen_paths: &mut std::collections::HashSet<String>,
    results: &mut Vec<RepoMatch>,
) {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n.to_string(),
        None => return,
    };

    // Skip DEPRECATED
    if name.to_uppercase().starts_with("DEPRECATED") {
        return;
    }

    if !query_lower.is_empty() && !name.to_lowercase().contains(query_lower) {
        return;
    }

    let path_str = path.to_string_lossy().to_string();
    if seen_paths.insert(path_str.clone()) {
        let agents = detect_agents(path);
        results.push(RepoMatch {
            name,
            path: path_str,
            agents,
        });
    }
}

/// Scan configured repo_paths for directories matching the query.
/// Each repo_path is treated as either:
/// - A repo itself (if it contains .git), or
/// - A parent directory whose children are repos
///
/// For each repo found, detects agent tooling (.claude, .codex, .cursor).
#[tauri::command]
pub async fn search_repos(
    settings: State<'_, SettingsState>,
    query: String,
) -> Result<Vec<RepoMatch>, String> {
    let cfg = settings.read().await;
    let query_lower = query.to_lowercase();
    let mut results: Vec<RepoMatch> = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    for base_path in &cfg.repo_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }

        // If this base_path is itself a repo (has .git), add it directly
        if base.join(".git").is_dir() {
            try_add_repo(base, &query_lower, &mut seen_paths, &mut results);
            continue;
        }

        // Otherwise scan children as repos
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            // Skip hidden directories
            if name.starts_with('.') {
                continue;
            }

            try_add_repo(&path, &query_lower, &mut seen_paths, &mut results);
        }
    }

    // Sort alphabetically
    results.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(results)
}
