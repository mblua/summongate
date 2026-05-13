use clap::Args;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::agent_config::{AgentLocalConfig, CodingAgentEntry};
use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};

#[derive(Args)]
#[command(after_help = "\
OUTPUT: JSON array of team peers. Each entry contains:\n  \
  name              Agent name to use with `send --to` (e.g., \"repos/my-project\")\n  \
  path              Full filesystem path to the agent's root directory\n  \
  status            Legacy: \"active\" iff working==true, else \"unknown\"\n  \
  working           true iff peer has a Running or Active session not\n                    \
                  waiting for input. For WG peers this matches the\n                    \
                  Sidebar running-peer badge exactly.\n  \
  sessionStatus     One of: \"active\", \"running\", \"idle\", \"waiting\",\n                    \
                  \"exited\", \"none\"\n  \
  sessionId         UUID of the matched session (omitted if no match)\n  \
  waitingForInput   true if the matching session is waiting for user input\n  \
  exitCode          Exit code (only present when sessionStatus == \"exited\")\n  \
  role              Summary extracted from the agent's CLAUDE.md\n  \
  teams             List of shared team names\n  \
  reachable         true if you can directly message this agent, false otherwise\n  \
  lastCodingAgent   Last coding CLI used (e.g., \"claude\", \"codex\"), if known\n\n\
NOTES:\n  \
  - Working-state visibility is bound to the binary instance writing\n    \
    sessions.json. Peers running under a different AgentsCommander binary\n    \
    (e.g. agentscommander_mb_wg-20.exe vs agentscommander_mb.exe) will\n    \
    always report sessionStatus=\"none\".\n  \
  - `pendingReview` is a frontend-only state, invisible to the CLI. A\n    \
    peer whose agent has finished but the user has not yet acknowledged in\n    \
    the sidebar will be reported as working/running by this command.\n  \
  - WG peers match by session name (`<wg>/<agent>`); non-WG peers match\n    \
    by working-directory only.\n  \
  See issue #206 for the full rationale.\n\n\
All agents that belong to your team(s) are listed. Agents you cannot directly\n\
message are included with reachable=false. If you have no teams, the result is an empty array.")]
pub struct ListPeersArgs {
    /// Session token for authentication (from '# === Session Credentials ===' block)
    #[arg(long)]
    pub token: Option<String>,

    /// Agent root directory (required). Your working directory — used to identify you and your teams
    #[arg(long)]
    pub root: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PeerInfo {
    name: String,
    path: String,
    /// Legacy: "active" iff working==true, else "unknown".
    /// Preserved verbatim for callers that string-match the old field.
    /// New callers should read `working` / `sessionStatus`.
    status: String,
    role: String,
    teams: Vec<String>,
    reachable: bool,
    last_coding_agent: Option<String>,

    // ── NEW (issue #206) ────────────────────────────────────────────
    /// True iff the peer has a matching session in Running or Active
    /// state AND `waiting_for_input == false`. Mirrors the sidebar
    /// `running-peer` badge predicate (ProjectPanel.tsx:780-786).
    working: bool,
    /// Fine-grained status. One of:
    ///   "active"   — SessionStatus::Active (focused session)
    ///   "running"  — SessionStatus::Running
    ///   "idle"     — SessionStatus::Idle
    ///   "waiting"  — any matching session has waiting_for_input==true
    ///                (overrides underlying SessionStatus, mirrors
    ///                replicaDotClass() at ProjectPanel.tsx:60)
    ///   "exited"   — SessionStatus::Exited(_)
    ///   "none"     — no session matches this peer (WG peers match by
    ///                name+cwd, non-WG peers match by cwd only)
    session_status: String,
    /// UUID of the matched session, when one was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    /// True if the matched session has waiting_for_input.
    waiting_for_input: bool,
    /// Exit code, present iff session_status == "exited".
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    // ────────────────────────────────────────────────────────────────

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    coding_agents: HashMap<String, CodingAgentEntry>,
}

// Shadow `agent_name_from_path`/`strip_agent_prefix` removed — canonical
// helpers live in `config::teams` (§AR2-order step 7 / §DR2). Origin agents
// use `agent_name_from_path` (project/agent); WG replicas use
// `agent_fqn_from_path` (project:wg-N/agent).

/// Extract a role description from markdown content: finds the `## Role` section
/// and returns up to 3 lines. Falls back to the first `fallback_lines` non-heading lines.
fn extract_role_section(content: &str, fallback_lines: usize, default_msg: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut in_role = false;
    let mut role_lines: Vec<&str> = Vec::new();

    for line in &lines {
        if line.starts_with("## Role Prompt") || line.starts_with("## Role") {
            in_role = true;
            continue;
        }
        if in_role {
            if line.starts_with("## ") || line.starts_with("---") {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                role_lines.push(trimmed);
            }
        }
    }

    if !role_lines.is_empty() {
        return role_lines.into_iter().take(3).collect::<Vec<_>>().join(" ");
    }

    let fallback: Vec<&str> = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(fallback_lines)
        .collect();

    if fallback.is_empty() {
        default_msg.to_string()
    } else {
        fallback.join(" ")
    }
}

/// Read role from CLAUDE.md: extract ## Role section, or first 5 lines.
fn read_role(repo_path: &str) -> String {
    let claude_md = Path::new(repo_path).join("CLAUDE.md");
    match std::fs::read_to_string(&claude_md) {
        Ok(content) => extract_role_section(&content, 5, "No role description available."),
        Err(_) => "No role description available.".to_string(),
    }
}

/// Canonicalize a path, stripping `\\?\` UNC prefix on Windows.
fn canon_str(path: &Path) -> Option<String> {
    let canon = std::fs::canonicalize(path).ok()?;
    let s = canon.to_string_lossy().to_string();
    Some(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

// ── Issue #206: working-state derivation from sessions.json ──────────

struct CandidateSession {
    id: String,
    name: String,
    status: SessionStatus,
    waiting_for_input: bool,
}

struct PeerStatus {
    working: bool,
    session_status: &'static str,
    status_legacy: &'static str,
    session_id: Option<String>,
    waiting_for_input: bool,
    exit_code: Option<i32>,
}

impl PeerStatus {
    fn none() -> Self {
        PeerStatus {
            working: false,
            session_status: "none",
            status_legacy: "unknown",
            session_id: None,
            waiting_for_input: false,
            exit_code: None,
        }
    }
}

fn norm_path(path: &str) -> String {
    let stripped = path.strip_prefix(r"\\?\").unwrap_or(path);
    stripped
        .replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string()
}

fn canon_or_norm(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(canon) => norm_path(&canon.to_string_lossy()),
        Err(_) => norm_path(path),
    }
}

/// Pure inner: build the cwd → candidate index from a slice of persisted rows.
/// Exposed (private) so unit tests can drive it without touching the filesystem.
fn build_session_index_from(rows: &[PersistedSession]) -> HashMap<String, Vec<CandidateSession>> {
    let mut index: HashMap<String, Vec<CandidateSession>> = HashMap::new();
    for ps in rows {
        if ps.name.starts_with(TEMP_SESSION_PREFIX) {
            continue;
        }
        let (Some(id), Some(status)) = (ps.id.clone(), ps.status.clone()) else {
            continue;
        };
        let key = canon_or_norm(&ps.working_directory);
        index.entry(key).or_default().push(CandidateSession {
            id,
            name: ps.name.clone(),
            status,
            waiting_for_input: ps.waiting_for_input.unwrap_or(false),
        });
    }
    index
}

/// Production entry point: read sessions.json and build the index.
fn build_session_index() -> HashMap<String, Vec<CandidateSession>> {
    build_session_index_from(&load_sessions_raw())
}

/// Priority: waiting(4) > active(3) > running(2) > idle(1) > exited(0).
/// Uses `match &c.status` to avoid moving the non-Copy `SessionStatus`
/// (matches the proven pattern in `list_sessions.rs:status_tag`).
fn priority(c: &CandidateSession) -> u8 {
    if c.waiting_for_input {
        return 4;
    }
    match &c.status {
        SessionStatus::Active => 3,
        SessionStatus::Running => 2,
        SessionStatus::Idle => 1,
        SessionStatus::Exited(_) => 0,
    }
}

/// Compute a peer's working state.
///
/// `expected_name`:
///   - `Some("wg/agent")` for WG peers → filters candidates by exact session
///     name to mirror the sidebar's `findSessionByName` predicate.
///   - `None` for non-WG peers → cwd-only match (no sidebar predicate to
///     mirror; see §6.1 and §10.8 of the plan).
fn compute_peer_status(
    peer_path: &str,
    expected_name: Option<&str>,
    index: &HashMap<String, Vec<CandidateSession>>,
) -> PeerStatus {
    let key = canon_or_norm(peer_path);
    let Some(candidates) = index.get(&key) else {
        return PeerStatus::none();
    };

    let filtered: Vec<&CandidateSession> = match expected_name {
        Some(name) => candidates.iter().filter(|c| c.name == name).collect(),
        None => candidates.iter().collect(),
    };

    let Some(chosen) = filtered.iter().copied().max_by_key(|c| priority(c)) else {
        return PeerStatus::none();
    };

    let (session_status, status_legacy, working, exit_code): (&str, &str, bool, Option<i32>) =
        if chosen.waiting_for_input {
            ("waiting", "unknown", false, None)
        } else {
            match &chosen.status {
                SessionStatus::Active => ("active", "active", true, None),
                SessionStatus::Running => ("running", "active", true, None),
                SessionStatus::Idle => ("idle", "unknown", false, None),
                SessionStatus::Exited(n) => ("exited", "unknown", false, Some(*n)),
            }
        };

    PeerStatus {
        working,
        session_status,
        status_legacy,
        session_id: Some(chosen.id.clone()),
        waiting_for_input: chosen.waiting_for_input,
        exit_code,
    }
}

struct WgReplicaInfo {
    my_agent_name: String,
    my_wg_name: String,
    my_wg_dir: PathBuf,
    ac_new_dir: PathBuf,
    /// Project folder name (the dir containing `.ac-new/`). Forms the
    /// LHS of the canonical FQN for WG replicas.
    my_project: String,
}

/// Detect if `root` is a WG replica: path matches `*/.ac-new/wg-*/__agent_*/`.
fn detect_wg_replica(root: &str) -> Option<WgReplicaInfo> {
    let path = PathBuf::from(root);
    let canon = match std::fs::canonicalize(&path) {
        Ok(c) => c,
        Err(_) => return None,
    };

    let my_dir_name = canon.file_name()?.to_str()?;
    if !my_dir_name.starts_with("__agent_") {
        return None;
    }
    let my_agent_name = my_dir_name.strip_prefix("__agent_")?.to_string();

    let wg_dir = canon.parent()?;
    let wg_name = wg_dir.file_name()?.to_str()?;
    if !wg_name.starts_with("wg-") {
        return None;
    }

    let ac_new_dir = wg_dir.parent()?;
    let ac_new_name = ac_new_dir.file_name()?.to_str()?;
    if ac_new_name != ".ac-new" {
        return None;
    }

    let my_project = ac_new_dir.parent()?.file_name()?.to_str()?.to_string();

    Some(WgReplicaInfo {
        my_agent_name,
        my_wg_name: wg_name.to_string(),
        my_wg_dir: wg_dir.to_path_buf(),
        ac_new_dir: ac_new_dir.to_path_buf(),
        my_project,
    })
}

/// Resolve the coordinator agent name for a WG by matching replica identity
/// paths against the team coordinator path in `.ac-new/_team_*/config.json`.
/// Only checks the team whose name matches the WG suffix (e.g. `wg-1-ac-devs` → `_team_ac-devs`).
fn resolve_wg_coordinator(ac_new_dir: &Path, wg_dir: &Path) -> Option<String> {
    // Derive expected team dir from WG name: "wg-1-ac-devs" → "_team_ac-devs"
    let wg_name = wg_dir.file_name()?.to_str()?;
    let team_suffix = wg_name
        .strip_prefix("wg-")
        .and_then(|s| s.split_once('-').map(|(_, rest)| rest))?;
    let expected_team_dir = format!("_team_{}", team_suffix);

    let entries = match std::fs::read_dir(ac_new_dir) {
        Ok(e) => e,
        Err(_) => return None,
    };

    for entry in entries.flatten() {
        let team_dir = entry.path();
        if !team_dir.is_dir() {
            continue;
        }
        match team_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) if n == expected_team_dir => {}
            _ => continue,
        }

        let team_config: serde_json::Value =
            match std::fs::read_to_string(team_dir.join("config.json"))
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
            {
                Some(v) => v,
                None => continue,
            };

        let coordinator_ref = match team_config.get("coordinator").and_then(|c| c.as_str()) {
            Some(c) => c.to_string(),
            None => continue,
        };

        let coordinator_abs = match canon_str(&team_dir.join(&coordinator_ref)) {
            Some(s) => s,
            None => continue,
        };

        // Check each replica in the WG for identity match
        let replica_entries = match std::fs::read_dir(wg_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for replica_entry in replica_entries.flatten() {
            let replica_dir = replica_entry.path();
            if !replica_dir.is_dir() {
                continue;
            }
            let dir_name = match replica_dir.file_name().and_then(|n| n.to_str()) {
                Some(n) if n.starts_with("__agent_") => n,
                _ => continue,
            };

            let config: serde_json::Value =
                match std::fs::read_to_string(replica_dir.join("config.json"))
                    .ok()
                    .and_then(|c| serde_json::from_str(&c).ok())
                {
                    Some(v) => v,
                    None => continue,
                };

            let identity_ref = match config.get("identity").and_then(|i| i.as_str()) {
                Some(i) => i.to_string(),
                None => continue,
            };

            let identity_abs = match canon_str(&replica_dir.join(&identity_ref)) {
                Some(s) => s,
                None => continue,
            };

            if identity_abs == coordinator_abs {
                return Some(
                    dir_name
                        .strip_prefix("__agent_")
                        .unwrap_or(dir_name)
                        .to_string(),
                );
            }
        }
    }

    None
}

/// Read role from a WG replica's identity matrix Role.md, falling back to CLAUDE.md.
fn read_wg_role(replica_dir: &Path) -> String {
    let config: serde_json::Value = match std::fs::read_to_string(replica_dir.join("config.json"))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
    {
        Some(v) => v,
        None => return "WG replica agent.".to_string(),
    };

    let identity_ref = match config.get("identity").and_then(|i| i.as_str()) {
        Some(i) => i,
        None => return "WG replica agent.".to_string(),
    };

    let matrix_dir = replica_dir.join(identity_ref);
    let role_path = matrix_dir.join("Role.md");
    match std::fs::read_to_string(&role_path) {
        Ok(content) => extract_role_section(&content, 3, "WG replica agent."),
        Err(_) => read_role(&matrix_dir.to_string_lossy()),
    }
}

/// Build a PeerInfo for a WG replica directory. Also bootstraps IPC dirs.
/// `project` is the project folder name (dir containing `.ac-new/`) and forms
/// the LHS of the canonical FQN.
fn build_wg_peer(
    project: &str,
    agent_name: &str,
    wg_name: &str,
    agent_path: &Path,
    reachable: bool,
    session_index: &HashMap<String, Vec<CandidateSession>>,
) -> PeerInfo {
    let replica_ac = agent_path.join(crate::config::agent_local_dir_name());
    let _ = std::fs::create_dir_all(replica_ac.join("inbox"));
    let _ = std::fs::create_dir_all(replica_ac.join("outbox"));

    let peer_config: AgentLocalConfig = replica_ac
        .join("config.json")
        .to_str()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();

    let expected_session_name = format!("{}/{}", wg_name, agent_name);
    let ps = compute_peer_status(
        &agent_path.to_string_lossy(),
        Some(&expected_session_name),
        session_index,
    );

    PeerInfo {
        name: format!("{}:{}/{}", project, wg_name, agent_name),
        path: agent_path.to_string_lossy().to_string(),
        status: ps.status_legacy.to_string(),
        role: read_wg_role(agent_path),
        teams: vec![wg_name.to_string()],
        reachable,
        last_coding_agent: peer_config.tooling.last_coding_agent,
        working: ps.working,
        session_status: ps.session_status.to_string(),
        session_id: ps.session_id,
        waiting_for_input: ps.waiting_for_input,
        exit_code: ps.exit_code,
        coding_agents: peer_config.tooling.coding_agents,
    }
}

/// WG-specific peer discovery — self-contained, returns exit code.
fn execute_wg_discovery(wg: WgReplicaInfo) -> i32 {
    let session_index = build_session_index();
    let mut peers: Vec<PeerInfo> = Vec::new();
    let discovered = crate::config::teams::discover_teams();
    // Canonical FQN: `<project>:<wg>/<agent>`. All downstream routing checks
    // (`can_communicate`) compare project-qualified strings.
    let my_full_name = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, wg.my_agent_name);

    let coordinator = resolve_wg_coordinator(&wg.ac_new_dir, &wg.my_wg_dir);
    let i_am_coordinator = coordinator.as_deref() == Some(wg.my_agent_name.as_str());

    if coordinator.is_none() {
        eprintln!(
            "Warning: no coordinator found for WG '{}', showing all replicas",
            wg.my_wg_name
        );
    }

    // Collect all replicas in my WG
    let replicas: Vec<(String, PathBuf)> = std::fs::read_dir(&wg.my_wg_dir)
        .into_iter()
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| {
            let p = e.path();
            if !p.is_dir() {
                return None;
            }
            let name = p.file_name()?.to_str()?;
            let agent = name.strip_prefix("__agent_")?.to_string();
            Some((agent, p))
        })
        .collect();

    for (agent_name, agent_path) in &replicas {
        if *agent_name == wg.my_agent_name {
            continue;
        }
        let peer_full_name = format!("{}:{}/{}", wg.my_project, wg.my_wg_name, agent_name);
        let reachable =
            crate::config::teams::can_communicate(&my_full_name, &peer_full_name, &discovered);
        peers.push(build_wg_peer(
            &wg.my_project,
            agent_name,
            &wg.my_wg_name,
            agent_path,
            reachable,
            &session_index,
        ));
    }

    // Coordinator also sees coordinators of OTHER WGs in the same .ac-new
    // (same project, different WG — still qualified with `wg.my_project`).
    if i_am_coordinator {
        if let Ok(entries) = std::fs::read_dir(&wg.ac_new_dir) {
            for entry in entries.flatten() {
                let other_wg_dir = entry.path();
                if !other_wg_dir.is_dir() {
                    continue;
                }
                let other_wg_name = match other_wg_dir.file_name().and_then(|n| n.to_str()) {
                    Some(n) if n.starts_with("wg-") && n != wg.my_wg_name => n.to_string(),
                    _ => continue,
                };

                if let Some(other_coord) = resolve_wg_coordinator(&wg.ac_new_dir, &other_wg_dir) {
                    let coord_dir = other_wg_dir.join(format!("__agent_{}", other_coord));
                    if !coord_dir.is_dir() {
                        continue;
                    }
                    let peer_name = format!("{}:{}/{}", wg.my_project, other_wg_name, other_coord);
                    if peers.iter().any(|p| p.name == peer_name) {
                        continue;
                    }
                    let reachable = crate::config::teams::can_communicate(
                        &my_full_name,
                        &peer_name,
                        &discovered,
                    );
                    peers.push(build_wg_peer(
                        &wg.my_project,
                        &other_coord,
                        &other_wg_name,
                        &coord_dir,
                        reachable,
                        &session_index,
                    ));
                }
            }
        }
    }

    match serde_json::to_string_pretty(&peers) {
        Ok(json) => {
            println!("{}", json);
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize peers: {}", e);
            1
        }
    }
}

pub fn execute(args: ListPeersArgs) -> i32 {
    // Validate token before any discovery
    if let Err(msg) = crate::cli::validate_cli_token(&args.token) {
        eprintln!("{}", msg);
        return 1;
    }

    let root = match args.root {
        Some(ref r) => r.clone(),
        None => {
            eprintln!("Error: --root is required. Specify your agent's root directory.");
            return 1;
        }
    };
    // ── WG replica fast path ──────────────────────────────────────────
    // If we're a WG replica, use dedicated discovery and return early.
    if let Some(wg) = detect_wg_replica(&root) {
        return execute_wg_discovery(wg);
    }

    // ── Standard discovery-based peer listing ────────────────────────
    //
    // `execute` is the non-WG-replica path (WG replicas return early above).
    // `root` is an origin matrix agent CWD → `agent_fqn_from_path` gives the
    // origin form `project/agent` (identical to the legacy behavior for
    // non-WG paths). Using the canonical helper eliminates the shadow.
    let my_name = crate::config::teams::agent_fqn_from_path(&root);
    let discovered = crate::config::teams::discover_teams();
    let session_index = build_session_index();

    let mut peers: Vec<PeerInfo> = Vec::new();

    // Find teams where I'm a member, then list their other members.
    // Also: if I'm a coordinator, show other coordinators (cross-team).
    let i_am_coordinator = discovered.iter().any(|t| {
        crate::config::teams::is_in_team(&my_name, t)
            && t.coordinator_name
                .as_deref()
                .is_some_and(|cn| cn == my_name)
    });

    for team in &discovered {
        let i_am_in_team = crate::config::teams::is_in_team(&my_name, team);

        if !i_am_in_team && !i_am_coordinator {
            continue;
        }

        for (i, display_name) in team.agent_names.iter().enumerate() {
            let member_path = team.agent_paths.get(i).and_then(|p| p.as_ref());
            let peer_name = member_path
                .map(|p| crate::config::teams::agent_fqn_from_path(&p.to_string_lossy()))
                .unwrap_or_else(|| display_name.clone());

            // Skip ourselves
            if peer_name == my_name || display_name == &my_name {
                continue;
            }

            // Determine reachability using the canonical routing rules
            let reachable =
                crate::config::teams::can_communicate(&my_name, &peer_name, &discovered);

            // Skip duplicates — add team to existing peer, upgrade reachable if needed
            if let Some(existing) = peers.iter_mut().find(|p| p.name == peer_name) {
                if !existing.teams.contains(&team.name) {
                    existing.teams.push(team.name.clone());
                }
                // If reachable via any team, mark as reachable
                if reachable {
                    existing.reachable = true;
                }
                continue;
            }

            let path_str = member_path
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let peer_ac = member_path
                .map(|p| p.join(crate::config::agent_local_dir_name()))
                .unwrap_or_else(|| {
                    PathBuf::from(&path_str).join(crate::config::agent_local_dir_name())
                });

            let peer_config: AgentLocalConfig = peer_ac
                .join("config.json")
                .to_str()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|c| serde_json::from_str(&c).ok())
                .unwrap_or_default();

            let ps = compute_peer_status(&path_str, None, &session_index);
            peers.push(PeerInfo {
                name: peer_name,
                path: path_str,
                status: ps.status_legacy.to_string(),
                role: member_path
                    .map(|p| read_role(&p.to_string_lossy()))
                    .unwrap_or_else(|| "No role description available.".to_string()),
                teams: vec![team.name.clone()],
                reachable,
                last_coding_agent: peer_config.tooling.last_coding_agent,
                working: ps.working,
                session_status: ps.session_status.to_string(),
                session_id: ps.session_id,
                waiting_for_input: ps.waiting_for_input,
                exit_code: ps.exit_code,
                coding_agents: peer_config.tooling.coding_agents,
            });
        }
    }

    // ── WG replica discovery ──────────────────────────────────────────────
    // Scan project_paths for .ac-new/wg-*/__agent_* replicas
    let settings = crate::config::settings::load_settings();
    for base_path in &settings.project_paths {
        let base = Path::new(base_path);
        if !base.is_dir() {
            continue;
        }
        // Check base and its immediate children (same pattern as ac_discovery)
        let mut dirs_to_check = vec![base.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        dirs_to_check.push(p);
                    }
                }
            }
        }
        for repo_dir in dirs_to_check {
            let ac_new_dir = repo_dir.join(".ac-new");
            if !ac_new_dir.is_dir() {
                continue;
            }
            // Project folder name (parent of .ac-new) — LHS of the canonical FQN.
            let project_folder = match repo_dir.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let wg_entries = match std::fs::read_dir(&ac_new_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for wg_entry in wg_entries.flatten() {
                let wg_path = wg_entry.path();
                if !wg_path.is_dir() {
                    continue;
                }
                let wg_name = match wg_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) if n.starts_with("wg-") => n.to_string(),
                    _ => continue,
                };
                // Derive team name from WG name: "wg-1-ac-devs" → "ac-devs"
                let wg_team = wg_name
                    .strip_prefix("wg-")
                    .and_then(|s| s.split_once('-').map(|(_, rest)| rest))
                    .unwrap_or(&wg_name)
                    .to_string();

                let agent_entries = match std::fs::read_dir(&wg_path) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for agent_entry in agent_entries.flatten() {
                    let agent_path = agent_entry.path();
                    if !agent_path.is_dir() {
                        continue;
                    }
                    let agent_dir = match agent_path.file_name().and_then(|n| n.to_str()) {
                        Some(n) if n.starts_with("__agent_") => n.to_string(),
                        _ => continue,
                    };
                    let agent_short = agent_dir
                        .strip_prefix("__agent_")
                        .unwrap_or(&agent_dir)
                        .to_string();
                    let peer_name = format!("{}:{}/{}", project_folder, wg_name, agent_short);

                    // Skip self
                    if peer_name == my_name {
                        continue;
                    }
                    // Skip duplicates
                    if peers.iter().any(|p| p.name == peer_name) {
                        continue;
                    }

                    let reachable =
                        crate::config::teams::can_communicate(&my_name, &peer_name, &discovered);
                    let mut peer = build_wg_peer(
                        &project_folder,
                        &agent_short,
                        &wg_name,
                        &agent_path,
                        reachable,
                        &session_index,
                    );
                    peer.teams = vec![wg_team.clone()];
                    peers.push(peer);
                }
            }
        }
    }

    // Output as JSON
    match serde_json::to_string_pretty(&peers) {
        Ok(json) => {
            println!("{}", json);
            0
        }
        Err(e) => {
            eprintln!("Error: failed to serialize peers: {}", e);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::sessions_persistence::PersistedSession;
    use crate::session::session::SessionStatus;

    fn cand(name: &str, status: SessionStatus, waiting: bool) -> CandidateSession {
        CandidateSession {
            id: "11111111-1111-1111-1111-111111111111".to_string(),
            name: name.to_string(),
            status,
            waiting_for_input: waiting,
        }
    }

    /// Build a minimal PersistedSession for build_session_index_from tests.
    fn ps_row(
        name: &str,
        cwd: &str,
        status: Option<SessionStatus>,
        id_present: bool,
    ) -> PersistedSession {
        PersistedSession {
            name: name.to_string(),
            working_directory: cwd.to_string(),
            id: if id_present {
                Some("11111111-1111-1111-1111-111111111111".to_string())
            } else {
                None
            },
            status,
            waiting_for_input: Some(false),
            ..Default::default()
        }
    }

    // ── norm_path / canon_or_norm ────────────────────────────────────

    #[test]
    fn norm_path_lowercases_and_normalizes_slashes() {
        assert_eq!(norm_path(r"C:\Users\Foo\Bar"), "c:/users/foo/bar");
        assert_eq!(norm_path("C:/Users/Foo/Bar/"), "c:/users/foo/bar");
        assert_eq!(norm_path(r"\\?\C:\Users\Foo"), "c:/users/foo");
        assert_eq!(norm_path("c:/x"), "c:/x");
    }

    // ── compute_peer_status (non-WG, expected_name=None) ─────────────

    #[test]
    fn no_session_yields_none() {
        let idx: HashMap<String, Vec<CandidateSession>> = HashMap::new();
        let ps = compute_peer_status(r"C:\does\not\exist", None, &idx);
        assert!(!ps.working);
        assert_eq!(ps.session_status, "none");
        assert_eq!(ps.status_legacy, "unknown");
        assert!(ps.session_id.is_none());
        assert!(!ps.waiting_for_input);
        assert!(ps.exit_code.is_none());
    }

    #[test]
    fn running_session_is_working() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Running, false)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert!(ps.working);
        assert_eq!(ps.session_status, "running");
        assert_eq!(ps.status_legacy, "active");
        assert!(ps.session_id.is_some());
    }

    #[test]
    fn active_session_is_working() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Active, false)],
        );
        let ps = compute_peer_status(r"C:\X", None, &idx);
        assert!(ps.working);
        assert_eq!(ps.session_status, "active");
        assert_eq!(ps.status_legacy, "active");
    }

    #[test]
    fn idle_session_is_not_working() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Idle, false)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert!(!ps.working);
        assert_eq!(ps.session_status, "idle");
        assert_eq!(ps.status_legacy, "unknown");
    }

    #[test]
    fn waiting_overrides_running() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Running, true)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert!(!ps.working);
        assert_eq!(ps.session_status, "waiting");
        assert!(ps.waiting_for_input);
    }

    #[test]
    fn exited_session_carries_exit_code() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Exited(42), false)],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert_eq!(ps.session_status, "exited");
        assert_eq!(ps.exit_code, Some(42));
        assert!(!ps.working);
    }

    #[test]
    fn priority_picks_active_over_idle_at_same_cwd() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![
                cand("any", SessionStatus::Idle, false),
                cand("any", SessionStatus::Active, false),
            ],
        );
        let ps = compute_peer_status("C:/X", None, &idx);
        assert_eq!(ps.session_status, "active");
        assert!(ps.working);
    }

    #[test]
    fn extended_length_prefix_normalizes() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Running, false)],
        );
        let ps = compute_peer_status(r"\\?\C:\X", None, &idx);
        assert_eq!(ps.session_status, "running");
    }

    // ── compute_peer_status with WG name filter (expected_name=Some) ─

    #[test]
    fn wg_name_filter_matches_only_named_session() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![
                cand("wg-20/dev", SessionStatus::Active, false),
                cand("[temp]-foo", SessionStatus::Active, false),
                cand("other-name", SessionStatus::Running, true),
            ],
        );
        let ps = compute_peer_status("C:/X", Some("wg-20/dev"), &idx);
        assert_eq!(ps.session_status, "active");
        assert!(ps.working);
        assert!(!ps.waiting_for_input);
    }

    #[test]
    fn wg_name_filter_returns_none_when_no_name_match() {
        let mut idx = HashMap::new();
        idx.insert(
            "c:/x".to_string(),
            vec![cand("other-name", SessionStatus::Active, false)],
        );
        let ps = compute_peer_status("C:/X", Some("wg-20/dev"), &idx);
        assert_eq!(ps.session_status, "none");
        assert!(!ps.working);
    }

    // ── build_session_index_from filter tests ────────────────────────

    #[test]
    fn build_index_skips_temp_sessions() {
        let rows = vec![ps_row(
            "[temp]-dispatch",
            r"C:\X",
            Some(SessionStatus::Active),
            true,
        )];
        let idx = build_session_index_from(&rows);
        assert!(idx.is_empty(), "temp-prefixed sessions must be skipped");
    }

    #[test]
    fn build_index_skips_rows_without_id() {
        let rows = vec![ps_row(
            "wg-20/dev",
            r"C:\X",
            Some(SessionStatus::Active),
            false,
        )];
        let idx = build_session_index_from(&rows);
        assert!(idx.is_empty(), "rows without id must be skipped");
    }

    #[test]
    fn build_index_skips_rows_without_status() {
        let rows = vec![ps_row("wg-20/dev", r"C:\X", None, true)];
        let idx = build_session_index_from(&rows);
        assert!(idx.is_empty(), "rows without status must be skipped");
    }

    #[test]
    fn build_index_normalizes_cwd_with_extended_prefix() {
        let rows = vec![ps_row(
            "wg-20/dev",
            r"\\?\C:\X",
            Some(SessionStatus::Active),
            true,
        )];
        let idx = build_session_index_from(&rows);
        assert!(idx.contains_key("c:/x"));
    }

    #[test]
    fn build_index_groups_multiple_rows_at_same_cwd() {
        let rows = vec![
            ps_row("wg-20/dev", r"C:\X", Some(SessionStatus::Active), true),
            ps_row("other", r"C:/X", Some(SessionStatus::Idle), true),
        ];
        let idx = build_session_index_from(&rows);
        let bucket = idx.get("c:/x").expect("entries grouped under c:/x");
        assert_eq!(bucket.len(), 2);
    }
}
