# Plan: Fix list-peers to expose live/working peer status (#206)

Branch: `bug/206-list-peers-working-status`

---

## 1. Requirement

`list-peers` currently reports peer status by checking for `<peer_root>/.<binary>/active` — a marker file that NO code writes. As a result every live peer reads `"unknown"`.

The CLI must instead derive each peer's working state from the same source the running app already uses: the persisted `sessions.json` snapshot (already consumed by `list-sessions`).

"Working" is defined as follows, with the scope explicitly split:

- **WG peers**: `working == true` iff there exists a session named `<wg_name>/<agent_name>` (the same name the sidebar's `replicaSessionName` constructs at `ProjectPanel.tsx:50-52`) at the agent's cwd, with `SessionStatus::Running` or `SessionStatus::Active` and `waiting_for_input == false`. This matches the Sidebar's `running-peer` badge (`ProjectPanel.tsx:780-786`) exactly, with one documented residual divergence: `pending_review` is a frontend-only signal invisible to the CLI (see §10.4).
- **Non-WG peers**: `working == true` iff any session at the peer's cwd has `SessionStatus::Running` or `SessionStatus::Active` and `waiting_for_input == false`. The sidebar has no `running-peer` predicate for non-WG team members, so cwd-keyed matching is the best available signal (see §6.1 and §10.8).

Anything else (`Idle`, `Exited`, `waiting`, no matching session) → `working == false`.

The fix must:

- Replace the broken marker-file check.
- Add explicit machine-readable fields so automation can determine working-state without string-matching the legacy `status` field.
- Preserve backward compatibility for callers that consume the existing `status` string.

---

## 2. Affected files

| File | Why |
|---|---|
| `src-tauri/src/cli/list_peers.rs` | All peer-construction sites; new schema; matching logic; tests |
| `src-tauri/tauri.conf.json` | Version bump (per project rule) |
| `src-tauri/src/config/sessions_persistence.rs` | **Read-only except one narrow opt-in derive change** — see §15.4. May add `#[derive(Default)]` to `PersistedSession` for test-fixture ergonomics (no behavior change). Already exposes `load_sessions_raw()` and `PersistedSession.{id, status, name, waiting_for_input, working_directory}` for runtime data. |
| `src-tauri/src/session/session.rs` | **Reference only — no changes.** `SessionStatus`, `TEMP_SESSION_PREFIX`, and `Session` schema unchanged. |

**Frontend: NO CHANGES.** The Sidebar reads `sessionsStore` via Tauri events, not via the CLI. The fix is server-side only.

---

## 3. JSON output fields and compatibility strategy

### 3.1 New schema for `PeerInfo`

Replace `PeerInfo` (current `src-tauri/src/cli/list_peers.rs:30-42`) with:

```rust
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
    ///   "none"     — no session matches this peer (see §6.1 for the
    ///                matching predicate: WG peers match by name+cwd,
    ///                non-WG peers match by cwd only)
    session_status: String,
    /// UUID of the matched session, when one was found.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    /// True if at least one matching session has waiting_for_input.
    waiting_for_input: bool,
    /// Exit code, present iff session_status == "exited".
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    // ────────────────────────────────────────────────────────────────

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    coding_agents: HashMap<String, CodingAgentEntry>,
}
```

### 3.2 Backward compatibility

- Legacy `status` keeps **exactly two** possible values: `"active"` or `"unknown"` (same domain as today).
- Semantics: `status = "active"` iff `working == true`; otherwise `"unknown"`.
- No old caller can break: old "active" still means "peer is live and working"; old "unknown" still means "no signal".
- New fields (`working`, `sessionStatus`, `sessionId`, `waitingForInput`, `exitCode`) are additive — old JSON consumers that ignore unknown keys are unaffected.

### 3.3 `after_help` doc update

Replace `src-tauri/src/cli/list_peers.rs:9-19` with:

```rust
#[command(after_help = "\
OUTPUT: JSON array of team peers. Each entry contains:\n  \
  name              Agent name to use with `send --to` (e.g., \"repos/my-project\")\n  \
  path              Full filesystem path to the agent's root directory\n  \
  status            Legacy: \"active\" iff working==true, else \"unknown\"\n  \
  working           true iff peer has a Running or Active session not\n                      \
                  waiting for input. For WG peers this matches the\n                      \
                  Sidebar running-peer badge exactly.\n  \
  sessionStatus     One of: \"active\", \"running\", \"idle\", \"waiting\",\n                      \
                  \"exited\", \"none\"\n  \
  sessionId         UUID of the matched session (omitted if no match)\n  \
  waitingForInput   true if the matching session is waiting for user input\n  \
  exitCode          Exit code (only present when sessionStatus == \"exited\")\n  \
  role              Summary extracted from the agent's CLAUDE.md\n  \
  teams             List of shared team names\n  \
  reachable         true if you can directly message this agent, false otherwise\n  \
  lastCodingAgent   Last coding CLI used (e.g., \"claude\", \"codex\"), if known\n\n\
NOTES:\n\
  - Working-state visibility is bound to the binary instance writing\n    \
  sessions.json. Peers running under a different AgentsCommander binary\n    \
  (e.g. agentscommander_mb_wg-20.exe vs agentscommander_mb.exe) will\n    \
  always report sessionStatus=\"none\".\n\
  - `pendingReview` is a frontend-only state, invisible to the CLI. A\n    \
  peer whose agent has finished but the user has not yet acknowledged in\n    \
  the sidebar will be reported as working/running by this command.\n\
  - WG peers match by session name (`<wg>/<agent>`); non-WG peers match\n    \
  by working-directory only.\n\
  See issue #206 for the full rationale.\n\n\
All agents that belong to your team(s) are listed. Agents you cannot directly\n\
message are included with reachable=false. If you have no teams, the result is an empty array.")]
```

---

## 4. Status mapping table

For the selected candidate session after the matching filter (see §6.1):

| Selected candidate | `working` | `sessionStatus` | `status` (legacy) | `waitingForInput` (output) | `exitCode` |
|---|---|---|---|---|---|
| No candidate after filter (WG: no name+cwd match; non-WG: no cwd match) | false | `"none"` | `"unknown"` | false | absent |
| `SessionStatus::Active`, `waiting=false` | **true** | `"active"` | `"active"` | false | absent |
| `SessionStatus::Running`, `waiting=false` | **true** | `"running"` | `"active"` | false | absent |
| `SessionStatus::Idle`, `waiting=false` | false | `"idle"` | `"unknown"` | false | absent |
| Any status with `waiting=true` (chosen by priority) | false | `"waiting"` | `"unknown"` | true | absent |
| `SessionStatus::Exited(n)` | false | `"exited"` | `"unknown"` | false | `n` |

**Note on `id == None` rows**: `build_session_index` drops any row missing `id` or `status` (matches the filter `list-sessions` applies at `list_sessions.rs:88`). Such rows therefore can never reach this table — there is no public `"unknown"` `sessionStatus`; a row with no usable id contributes to neither match nor mismatch.

**Note on `waitingForInput` output**: now reflects ONLY the chosen candidate. Previous drafts aggregated across all candidates at a cwd; with the WG name filter (§6.1) at most one candidate is chosen for the predicate, so aggregation is no longer meaningful for WG peers. For non-WG peers a single chosen candidate by priority is also used — drop the aggregate to keep semantics consistent across both paths.

**Why waiting overrides everything**: mirrors `replicaDotClass()` at `src/sidebar/components/ProjectPanel.tsx:60` — `waitingForInput` is checked before `status`. A peer with `waiting=true` is paused, not working.

**`pending_review` is intentionally NOT surfaced**: it is a frontend-only field that `SessionInfo::from(&Session)` hardcodes to `false` (`session/session.rs:202`); it never reaches `PersistedSession`. See §10.4 for the divergence this causes.

---

## 5. Windows path normalization

### 5.1 Normalization pipeline

Two paths refer to the same logical location iff both produce the same string after:

1. **Strip `\\?\` extended-length prefix** (`std::fs::canonicalize` emits this on Windows).
2. **Replace `\` with `/`**.
3. **Lowercase the whole string** (NTFS is case-insensitive). Mirrors what `sessions_persistence::deduplicate` already does at `sessions_persistence.rs:87` and `:99`.
4. **Trim trailing `/`s** (implementation uses `trim_end_matches('/')` which strips all consecutive trailing slashes — semantically equivalent on Windows; extra slashes are no-ops).

### 5.2 Implementation

Add private helpers in `list_peers.rs`. Insert AFTER the existing `canon_str` (currently ends at line 104) and BEFORE `struct WgReplicaInfo` (currently at line 106). The block is shown in full in §8.1.

```rust
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
```

### 5.3 Both sides MUST be normalized

- `peer.path` originates from `read_dir` (`list_peers.rs:299`) — native separator, original case, no `\\?\`.
- `session.working_directory` is the literal cwd passed at session creation (`session/session.rs:61`, set by `Session::from`) — any of: native `\`, forward `/`, with or without `\\?\`, varying case.

Applying `canon_or_norm` to both ensures equality regardless of shape. Falling back to `norm_path` when `canonicalize` fails (deleted agent dir, missing session cwd) still covers the common case-insensitive / separator / trailing-slash variation.

---

## 6. Candidate selection

`load_sessions_raw()` (used by `list-sessions`, `sessions_persistence.rs:145`) does NOT deduplicate. The same `working_directory` may appear in multiple rows (user opened two terminals at the same dir, restart cycles, etc.). Selection must therefore be deterministic.

### 6.1 Matching predicate per peer kind

There are two peer kinds and two different matching rules. This is the design decision for grinch §14.2.2.

**WG peers** (constructed by `build_wg_peer`, called from `execute_wg_discovery()` and from the WG-scan inside `execute()` at lines 530-615): match the sidebar's `findSessionByName` predicate exactly. Sidebar uses `replicaSessionName = "${wg.name}/${replica.name}"` (`ProjectPanel.tsx:50-52`). Filter candidates by:

  1. `canon_or_norm(c.working_directory) == canon_or_norm(peer_path)`, AND
  2. `c.name == format!("{}/{}", wg_name, agent_name)`.

If no candidate passes both filters → `sessionStatus = "none"`. This guarantees the §1 invariant: WG-peer `working` matches the sidebar `running-peer` badge exactly (modulo `pending_review`, see §10.4).

**Non-WG peers** (constructed in `execute()`'s standard team loop at lines 450-518): there is no sidebar `running-peer` badge to mirror — the sidebar is replica-scoped, not team-member-scoped. Filter candidates by cwd only:

  1. `canon_or_norm(c.working_directory) == canon_or_norm(peer_path)`.

This is a deliberate, documented divergence from the WG predicate (§10.8, added below). A non-WG peer reports `working: true` if any session at its cwd is Active/Running and not waiting — strictly looser than the WG predicate but the best available signal without a sidebar reference.

### 6.2 Priority and selection from filtered candidates

After filtering per §6.1, if more than one candidate remains, pick the highest-priority one:

1. Priority 4: `waiting_for_input == true` → `sessionStatus = "waiting"`, `working = false`.
2. Priority 3: `SessionStatus::Active` → `sessionStatus = "active"`, `working = true`.
3. Priority 2: `SessionStatus::Running` → `sessionStatus = "running"`, `working = true`.
4. Priority 1: `SessionStatus::Idle` → `sessionStatus = "idle"`, `working = false`.
5. Priority 0: `SessionStatus::Exited(n)` → `sessionStatus = "exited"`, `working = false`, `exit_code = Some(n)`.

`waiting` (priority 4) outranks `active/running` — intentional, mirrors the sidebar override at `replicaDotClass()` `ProjectPanel.tsx:64`.

Output fields:

- `sessionId` = the chosen candidate's `id`.
- `waitingForInput` = the chosen candidate's `waiting_for_input` (NOT an aggregate — see §4 note).
- `exitCode` = `Some(n)` only when `sessionStatus == "exited"`.

### 6.3 Build the session index once

To avoid repeated `canonicalize` syscalls (O(P*S)), construct the index once per `execute()` / `execute_wg_discovery()` call. The index is keyed by normalized cwd; the WG name filter is then applied at lookup time (cheap string equality).

Split into a pure inner function (takes `&[PersistedSession]`) and a thin loader (calls `load_sessions_raw()`). The split is required for the §9.1 tests to drive the index without touching the filesystem.

```rust
fn build_session_index_from(rows: &[PersistedSession]) -> HashMap<String, Vec<CandidateSession>> {
    use crate::session::session::TEMP_SESSION_PREFIX;

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

fn build_session_index() -> HashMap<String, Vec<CandidateSession>> {
    build_session_index_from(&load_sessions_raw())
}
```

Pass `&session_index` to every peer-construction site. `build_wg_peer` additionally derives `expected_name = format!("{}/{}", wg_name, agent_name)` from its existing args.

---

## 7. Temp session handling

Temp sessions (`name` starts with `TEMP_SESSION_PREFIX` = `"[temp]"`, defined at `session/session.rs:27`) are ephemeral and MUST NOT count as "working":

- Excluded from `build_session_index()` (§6.3).
- Matches the convention of `load_sessions()` (`sessions_persistence.rs:184-194`) and `snapshot_sessions()` (`sessions_persistence.rs:320-330`).
- Rationale: temp sessions exist for transient operations (e.g., the smart-pick prompt flow). Including them would cause `list-peers` to flicker `working: true` for fractions of a second.

---

## 8. Concrete edits

### 8.1 Add top-of-file imports + helper block

**Imports** (add to the existing import block at `list_peers.rs:1-6`):

```rust
use crate::config::sessions_persistence::{load_sessions_raw, PersistedSession};
use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX};
```

Place these after the existing `use crate::config::agent_config::...` line. Per §13.3.2, keep all `use` at the top — do not nest inside helper functions.

**Helper block** (insert after current `canon_str` which ends at line 104, before `struct WgReplicaInfo` at line 106):

```rust
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
///     mirror; see §6.1 and §10.8).
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
```

Changes vs. earlier draft (incorporates §13.2.2, §13.3.2, §14.2.1, §14.2.2, §14.2.7):

- `CandidateSession` gains a `name: String` field for the WG name filter.
- `use crate::session::session::{SessionStatus, TEMP_SESSION_PREFIX}` is hoisted to module top, alongside `load_sessions_raw` / `PersistedSession`.
- `priority` and the inner mapping use `match &c.status` / `match &chosen.status`; `Exited(n)` binds `n: &i32` and emits `Some(*n)` for `exit_code` (fixes the non-Copy `SessionStatus` borrow concern).
- `build_session_index` is split into a pure `build_session_index_from(&[PersistedSession])` (testable) and a thin `build_session_index()` loader.
- `compute_peer_status` gains an `expected_name: Option<&str>` parameter; WG peers pass `Some("wg/agent")`, non-WG peers pass `None`.
- `PeerStatus::none()` keeps `session_status: "none"`, never `"unknown"` — the `"unknown"` enum value is dropped from the public surface per §14.2.1. `status_legacy` (the legacy `status` field) still uses `"unknown"`; that field's domain is unchanged.
- `waitingForInput` in the output reflects the chosen candidate, not an aggregate (consistent with §4 note).

(The existing `use std::collections::HashMap;` at line 3 already covers `HashMap`; the new top-level imports listed above are the only additions.)

### 8.2 Widen `PeerInfo` (lines 30-42)

Replace with the schema in §3.1.

### 8.3 Remove the broken `active`-marker checks (TWO sites)

Per §13.2.1: the original draft wrongly instructed to delete the `peer_ac` binding at lines 488-492. That binding is still used at lines 500-505 to load `peer_config: AgentLocalConfig`. The correct fix removes only the 5-line marker check.

- **`build_wg_peer`, lines 284-288.** Delete the 5-line `let status = if replica_ac.join("active").exists() { ... };` block. `replica_ac` itself stays — it is used by `create_dir_all(replica_ac.join("inbox"))` above (281-282) and by the `peer_config` load below (290-295). `build_wg_peer` now computes status via `compute_peer_status` — see §8.4.
- **`execute()` standard path, lines 494-498.** Delete ONLY the 5-line block:

  ```rust
  // DELETE (current 494-498):
  let status = if peer_ac.join("active").exists() {
      "active"
  } else {
      "unknown"
  };
  ```

  **KEEP the `peer_ac` binding (488-492) intact** — lines 500-505 still read `peer_ac.join("config.json")` to load `peer_config`. No re-pointing is needed. The previous "delete peer_ac" instruction is cancelled.

### 8.4 Update `build_wg_peer` (lines 273-307)

New signature and body. The `expected_session_name = "{wg_name}/{agent_name}"` exactly mirrors `replicaSessionName` (`ProjectPanel.tsx:50-52`).

```rust
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
```

### 8.5 Build index once in `execute()` and `execute_wg_discovery()`

Per §14.2.4: the index must be built only on the path that uses it, never on the WG fast-return path (which builds its own index inside `execute_wg_discovery`).

**`execute()` (line 410):** add immediately AFTER the WG fast-return (after line 428's `return execute_wg_discovery(wg);` closing brace), BEFORE `let my_name = ...` at line 436. Place between the comment-block separator and the `my_name` assignment:

```rust
    // ── Standard discovery-based peer listing ────────────────────────
    // (existing comment block stays)
    let my_name = crate::config::teams::agent_fqn_from_path(&root);
    let discovered = crate::config::teams::discover_teams();
    let session_index = build_session_index();   // ← NEW: after WG fast-return

    let mut peers: Vec<PeerInfo> = Vec::new();
    // ...
```

Rationale: a WG-replica invocation reaches `return execute_wg_discovery(wg);` at line 427 and never executes the rest of `execute()`. Building the index above the fast-return would waste one full `sessions.json` read + parse + canonicalize loop on every WG call (which is the primary call path for this fix).

**`execute_wg_discovery()` (line 310):** add at the very top of the function body, before `let mut peers: Vec<PeerInfo> = Vec::new();` on line 311:

```rust
    let session_index = build_session_index();
```

### 8.6 Update non-WG peer construction in `execute()`

Two edits within the team-member loop (current lines 484-518):

1. **Delete lines 494-498** — the `let status = if peer_ac.join("active").exists() ... else "unknown" };` block (per §8.3 second bullet).
2. **Leave the rest intact**: `peer_ac` binding (488-492) and `peer_config` load (500-505) stay as-is.
3. **Insert before `peers.push`**: a single `compute_peer_status` call (non-WG path → `None` for the name filter).
4. **Rewrite the `peers.push(PeerInfo { ... })` block** (current 507-518) to include the new fields:

```rust
            // peer_ac, peer_config above unchanged (lines 488-505).
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
```

The `None` for `expected_name` is deliberate (non-WG peers, per §6.1). The `peer_config` load is unchanged — no re-pointing needed.

### 8.7 Update `build_wg_peer` call sites

- **`execute_wg_discovery()` line 349-355**: change `build_wg_peer(&wg.my_project, agent_name, &wg.my_wg_name, agent_path, reachable,)` to add `&session_index` as the 6th argument.
- **`execute_wg_discovery()` line 386-392**: same — add `&session_index` as 6th arg.
- **`execute()` line 603-609** (the WG-discovery scan inside the non-WG path): same — add `&session_index` as 6th arg.

### 8.8 Update `after_help` (lines 9-19)

Replace with the new block in §3.3.

---

## 9. Test plan

### 9.1 Unit tests (new `#[cfg(test)] mod tests` block at bottom of `list_peers.rs`)

The dev must also adjust the test imports to pull in `PersistedSession` for the `build_session_index_from` tests. The pattern below uses helpers `cand()` (for `CandidateSession`) and `ps_row()` (for `PersistedSession`) to keep the fixtures terse.

```rust
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
    /// `id_present` and `status_present` control whether the filtered fields
    /// are populated; `name`/`cwd` are explicit so each test can show intent.
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
            // Other PersistedSession fields default-populated (Default derive
            // assumed; if not derived, copy the minimal struct literal from
            // sessions_persistence.rs and zero out non-relevant fields).
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
        // Session row uses \\?\ form
        idx.insert(
            "c:/x".to_string(),
            vec![cand("any", SessionStatus::Running, false)],
        );
        // Peer path comes in plain form
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
                cand("[temp]-foo", SessionStatus::Active, false), // filtered upstream
                cand("other-name", SessionStatus::Running, true), // waiting, but wrong name
            ],
        );
        let ps = compute_peer_status("C:/X", Some("wg-20/dev"), &idx);
        // Only the wg-20/dev candidate matches: Active, not waiting.
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

    // ── build_session_index_from filter tests (§14.2.7) ──────────────

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
            false, // id absent
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
        // Key must be the normalized form, not the raw \\?\ form.
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
```

**Note on `PersistedSession` construction**: if `PersistedSession` does not currently derive `Default`, the dev must either (a) add `#[derive(Default)]` to it in `sessions_persistence.rs` — acceptable, no behavior change; the §2 "reference only" restriction is lifted for this one-line derive — or (b) replace the `..Default::default()` in `ps_row` with an explicit field-by-field literal. Option (a) is preferred for test ergonomics.

### 9.2 Manual verification (Windows, after build)

1. **Bump `tauri.conf.json` version** (per `feedback_bump_version_on_builds`) so the running build is visually distinguishable.
2. Build the WG-specific binary (per `feedback_shipper_wg_only_deploy` — deploy only to `_wg-<N>.exe`, never the bare standalone) and launch AgentsCommander.
3. **Open a session at a peer's agent dir** via the sidebar (click a replica in the workgroup).
4. From a *different* agent in the same team, run:
   ```
   <binary> list-peers --token <token> --root <my_root>
   ```
   Expect the launched peer's entry to show: `working: true`, `sessionStatus: "running"` or `"active"`, non-null `sessionId`, `status: "active"`.
5. **Trigger `waiting_for_input`** in the peer's session by issuing a prompt that produces a Claude tool-use confirmation (e.g., have the agent run a shell command requiring user approval). **Watch your own sidebar** for that peer's status dot — wait until it turns yellow (`waiting` class via `replicaDotClass` at `ProjectPanel.tsx:64`). The dot turning yellow confirms `mark_idle`/`persist_current_state` have fired; without that, the CLI may read a stale snapshot. Do NOT skip the visual confirmation — this is the only branch that exercises priority-4.
6. Re-run `list-peers`. Expect: `working: false`, `sessionStatus: "waiting"`, `waitingForInput: true`, `status: "unknown"`.
7. Make the peer **Idle** (let the agent finish responding). Re-run. Expect `sessionStatus: "idle"`, `working: false`, `status: "unknown"`.
8. **Close the peer's session** from the sidebar.
9. Re-run `list-peers`. Expect either `sessionStatus: "exited"` (with a numeric `exitCode`) if the row survived, or `sessionStatus: "none"` if the row was purged.
10. **Backward-compat spot-check**: confirm `status` is `"active"` ONLY when `working == true`, never for `"idle"` / `"waiting"` / `"exited"`.
11. **Cross-binary sanity**: launch a peer using a *different* binary (e.g., `agentscommander_standalone.exe`). `list-peers` from the WG binary should still report `sessionStatus: "none"` for it — expected (§10.1).
12. **Path-shape sanity**: temporarily edit `sessions.json` to store the session's `workingDirectory` with `\\?\` prefix and mixed case (`\\?\C:\Users\MARIA\...`). Re-run `list-peers`; the peer should still resolve to `working: true`. (Optional — covered by unit test §9.1 `extended_length_prefix_normalizes`.)
13. **`after_help` rendering**: run `<binary> list-peers --help` and visually inspect the new NOTES block — the multi-line bullets should render without stray gaps or broken indentation. Paste the rendered output into the PR description so reviewers can confirm. (Per §14.2.10.)

### 9.3 Existing integration tests

`src-tauri/tests/cli_powershell_capture.rs` has two `#[ignore]`-marked tests (`list_peers_outputs_valid_json_under_*_noninteractive`) that only assert stdout is valid JSON. They will continue to pass without modification — the new fields are additive.

### 9.4 Build / test commands

```
cargo test --package agentscommander-new --lib -- list_peers::tests
cargo build --release
```

The release build is mandatory to catch the `windows_subsystem = "windows"` interaction with the CLI flow (per issue #129 history).

---

## 10. Risks and limitations

### 10.1 Same-binary `sessions.json` visibility (PRIMARY LIMITATION)

`config_dir()` is **portable** (`config/mod.rs:29-50`) — it returns `<binary_parent>/.<binary_stem>/`. Every binary instance owns its own `sessions.json`. Therefore:

- A peer running under `agentscommander_standalone.exe` is invisible to `list-peers` running under `agentscommander_mb.exe`, even on the same machine.
- WG-specific binaries (one per workgroup) likewise own separate `sessions.json` files.
- Cross-binary peers will ALWAYS report `sessionStatus: "none"`, `working: false`, regardless of their actual state.

**Worked example (WG-shipper deploy pattern)**: per the team rule `feedback_shipper_wg_only_deploy`, WG builds are shipped to `agentscommander_mb_wg-<N>.exe`, never the bare `agentscommander_mb.exe`. A typical machine therefore has BOTH binaries side-by-side. If the user launched the app via `agentscommander_mb_wg-20.exe` and runs CLI commands via the bare `agentscommander_mb.exe`, the CLI reads `<binary_parent>/.agentscommander_mb/sessions.json` while the running app writes `<binary_parent>/.agentscommander_mb_wg-20/sessions.json`. Every peer will report `sessionStatus: "none"` despite being live. Always invoke the CLI via the **same binary** that launched the app — i.e., the binary in the user's `# === Session Credentials ===` `BinaryPath` field.

**Mitigation**: document in the `after_help` block (§3.3) AND in the PR description. Manual verification step 11 (§9.2) exercises this mismatch as a positive control. A future enhancement could scan known binary `LocalDir`s, but that is out of scope for #206.

### 10.2 Path-equality false negatives

If a session was created with a cwd whose `canonicalize` succeeds on disk but produces a path that does NOT match the peer dir's canonicalization (rare: symlinks, junctions, mapped drives), the lookup may miss. `canon_or_norm` falls back to literal `norm_path` when canonicalization fails, but if BOTH sides succeed and produce different canonical forms, we report `"none"`.

**Severity**: low — not a regression. Old behavior was always `"unknown"`.

### 10.3 Subdirectory sessions

If a session was opened at `<agent_root>/scratch`, `list-peers` will NOT report the peer as working. This matches the sidebar (`replicaSession()` matches by name, not path — `ProjectPanel.tsx:55-57`), so behavior is consistent. Mention in PR description.

### 10.4 `pending_review` invisibility — REAL DIVERGENCE from the sidebar

`pending_review` is a **frontend-only field**. Concretely:

- It exists on `Session` (`session/session.rs:65-66`, `#[serde(default)]`) and on the IPC payload `SessionInfo`.
- But `impl From<&Session> for SessionInfo` **hardcodes `pending_review: false`** at `session/session.rs:202` — the field is set by the frontend after IPC, never by the backend.
- It is NOT a field of `PersistedSession` and therefore never reaches `sessions.json`.

Consequence: the CLI cannot see `pending_review` even theoretically. The sidebar's `replicaDotClass` returns `"pending"` BEFORE checking `waitingForInput` or `status` (`ProjectPanel.tsx:63`), and the `running-peer` badge predicate at `:784` rejects `"pending"`. So a peer the sidebar shows as "agent done, needs your attention":

- Sidebar: `dot === "pending"` → NOT a running peer.
- CLI: `pending_review` invisible → reads underlying `SessionStatus` (typically `Running`) → reports `working: true, sessionStatus: "running"`.

This is a **concrete user-observable inconsistency**, not a UI cosmetic. Automation gating on `working: true` will treat a paused-for-review peer as busy. The §1 invariant ("exactly the sidebar `running-peer` badge") is therefore exact modulo `pending_review` for WG peers — it is not absolute.

**Fix scope**: out of scope for #206. Surfacing `pending_review` from the CLI requires extending `PersistedSession` AND updating `snapshot_sessions` to populate it from a backend source of truth (currently there is none — the field is a frontend signal). Track as a follow-up.

**Mitigation**: documented in the `after_help` NOTES block (§3.3) and the PR description.

### 10.5 Stale snapshot between writes

`sessions.json` is atomically rewritten via `.tmp` → rename (`sessions_persistence.rs:302-306`). Partial reads cannot occur, but the snapshot is only as fresh as the last `save_sessions()` call (`persist_current_state` is called on session lifecycle events). `list-peers` may observe data lagging real state by a few seconds.

**Severity**: low — acceptable for automation. Sub-second freshness would require an HTTP IPC to the running app (out of scope for #206).

### 10.6 No frontend changes

Intentional. The sidebar already derives state from `sessionsStore` (Tauri events). Touching the frontend would expand blast radius without improving the CLI fix.

### 10.7 `Session::working_directory` mutability

`Session::working_directory` is set at creation and not updated when the underlying shell `cd`s elsewhere (PTY cwd doesn't propagate back to the parent). This is correct for our match — the AC-managed cwd is what we care about.

### 10.8 Non-WG peer predicate divergence (cwd-only matching)

For non-WG peers (the team-member loop in `execute()`), `compute_peer_status` is called with `expected_name = None` — it picks the highest-priority session at the peer's cwd regardless of session name. This is **not** a port of the sidebar's `running-peer` predicate: the sidebar is replica-scoped (only WG replicas have a `findSessionByName(wg/agent)` lookup); team members listed in `execute()` have no equivalent sidebar widget.

Concrete consequence: at a non-WG peer's cwd, multiple sessions with unrelated names can satisfy `working: true`. For example, a session named "manual-debug" at the peer's cwd would count, even though no sidebar UI would treat it as the peer's status.

This divergence is acceptable because there is no sidebar reference predicate to compare against for non-WG peers. The §1 invariant ("exactly the sidebar `running-peer` badge") applies **only to WG peers**; non-WG peers get the best-effort cwd-keyed predicate.

A future improvement would require defining what a non-WG peer's "session" means in UI terms — out of scope for #206.

### 10.9 Silent failures in `load_sessions_raw`

`load_sessions_raw` (`sessions_persistence.rs:153-156`) collapses both file-read errors and JSON parse failures to `vec![]` with no logging. After this fix, the consumer-facing `working` field is the headline signal; its silent failure mode is "every peer reports `sessionStatus: none`".

Sources of `vec![]`:

- File missing — expected on a fresh install before any session has been persisted.
- File unreadable (permissions, Windows file-rename race during `save_sessions`'s atomic `tmp → rename` at `sessions_persistence.rs:302-306`).
- File corrupt JSON.

The CLI cannot distinguish these from each other or from "no sessions" — the user sees identical output in all cases.

**Decision for #206**: do NOT modify `load_sessions_raw` in this PR. The §2 reference-only restriction on `sessions_persistence.rs` is preserved. Documentation alone for now; logging is a separate concern.

**Recommended follow-up** (NOT in this PR): add `log::warn!` to both error arms in `load_sessions_raw`. If the user wants quick diagnostics in the meantime, point them to `<config_dir>/sessions.json` (path resolvable via the `BinaryPath` from session credentials → `<binary_parent>/.<binary_stem>/sessions.json`).

---

## 11. Build sequence (for the implementing dev)

Apply in this order. Each step is independently compile-checkable.

1. **Add helper block** (§8.1) at top of `list_peers.rs`. Compile-only (no callers yet) — should build.
2. **Widen `PeerInfo`** (§8.2). Build will FAIL with errors at all PeerInfo construction sites — that's your edit checklist.
3. **Update `build_wg_peer` signature + body** (§8.4) and delete the dead status check inside it (§8.3 first bullet).
4. **Update `execute_wg_discovery`** (§8.5 second bullet, §8.7 first two bullets): build the index, thread it through both call sites. Verify it compiles.
5. **Update `execute()` non-WG section** (§8.5 first bullet, §8.6, §8.3 second bullet, §8.7 third bullet). Verify it compiles.
6. **Update `after_help`** (§8.8 / §3.3).
7. **Add unit tests** (§9.1).
8. **Bump `tauri.conf.json` version** per the project rule.
9. `cargo test --package agentscommander-new --lib -- list_peers::tests` — all new tests pass.
10. `cargo build --release` — release build succeeds.
11. Manual verification per §9.2.
12. Commit + PR.

---

## 12. Notes / DO-NOTs for the dev

- **Do NOT** change `SessionStatus`, `Session`, `PersistedSession`, or `sessions_persistence.rs`. All needed data is already exposed.
- **Do NOT** touch any file under `src/` (frontend). The sidebar already works correctly; the CLI is the only consumer that needs the fix.
- **Do NOT** add a new "active marker" writer. The marker concept is being retired; the source of truth is `sessions.json`.
- **Do NOT** remove the legacy `status` field. Old automation may still parse it; we only widen-and-redefine.
- **Do NOT** add or strengthen status reporting for `reachable: false` peers — the new logic ALREADY treats reachability and working-state as orthogonal (a peer can be live in our sessions.json regardless of team membership), which matches the old behavior.
- **Use `&'static str`** for `session_status` / `status_legacy` inside `PeerStatus` — they are always literals; `.to_string()` only at the JSON-build boundary.
- **Apply `canon_or_norm` to BOTH sides** of the lookup (peer.path AND session.working_directory). Skipping either side reintroduces shape-sensitive bugs.
- **Do NOT delete `peer_ac`** at lines 488-492 — it is still used at lines 500-505 to load `peer_config`. Only delete the 5-line marker check at lines 494-498. (Per §13.2.1 — earlier drafts of the plan got this wrong; the §15 verdict cancels that instruction.)
- **Status filter parity with `list-sessions`**: `list-sessions` filters `s.id.is_some()` (`list_sessions.rs:88`). Our `build_session_index` does the same via the `let (Some(id), Some(status)) = ... else continue;` pattern. Do not relax that filter.
- **Test on Windows**. Path normalization is the riskiest part; trust `cargo test` for the normalization unit tests, but also run the manual verification end-to-end before merging.

---

## 13. Dev-Rust review notes (added 2026-05-13)

Reviewer: `wg-20-dev-team/dev-rust`. All file paths, line numbers, and referenced functions/structs verified against the current branch (`bug/206-list-peers-working-status`).

### 13.1 ✅ Verified correct

- `PersistedSession.{id, status, waiting_for_input, working_directory}` (`sessions_persistence.rs:60-66, 21`) — runtime fields populated by `snapshot_sessions` (`:331-354`).
- `load_sessions_raw()` (`sessions_persistence.rs:145-157`) — read-only, no dedupe, no temp filter. Correct dependency.
- `TEMP_SESSION_PREFIX = "[temp]"` at `session/session.rs:27`. Correct.
- `SessionStatus` is `Active | Running | Idle | Exited(i32)` and derives `PartialEq` (`session/session.rs:113-120`). All match arms compile.
- `list_sessions.rs:88` already applies `s.id.is_some()` — parity preserved.
- `ProjectPanel.tsx:60-67` (`replicaDotClass`) and `:780-786` (running-peer predicate) — verified verbatim.
- `config_dir()` is portable / per-binary (`config/mod.rs:29-50`) — §10.1 limitation is real.
- `canonicalize` emits `\\?\` UNC prefix on Windows (already handled by existing `canon_str` at `list_peers.rs:100-104`).
- `sessions_persistence.rs:87, 99` already do `replace('\\', "/").to_lowercase()` for dedupe keys — `norm_path` is consistent with that.
- Rust edition `2021` (`Cargo.toml:4`); `let-else` syntax in §6.3 / §8.1 is supported.

### 13.2 ❌ Errors in the plan — MUST be fixed before implementation

#### 13.2.1 BLOCKER — §8.3 wrongly instructs to delete `peer_ac` binding

§8.3 states for the `execute()` standard path:
> "The `let peer_ac = ...` binding ABOVE it (lines 488-492) is also dead once removed — DELETE it as well (no other reads of `peer_ac` exist below)."

**This is wrong.** Lines 500-505 in the current code still read `peer_ac`:

```rust
// list_peers.rs:500-505 (current)
let peer_config: AgentLocalConfig = peer_ac
    .join("config.json")
    .to_str()
    .and_then(|p| std::fs::read_to_string(p).ok())
    .and_then(|c| serde_json::from_str(&c).ok())
    .unwrap_or_default();
```

Deleting `peer_ac` as §8.3 says will break the build at line 500.

§8.6's contradictory "Important" note acknowledges the conflict ("If `peer_config` in the original code was loaded via `peer_ac.join("config.json")`, it must be re-pointed..."), but the simplest correct path is:

**KEEP `peer_ac` (lines 488-492) intact. Delete ONLY the 5-line broken status check (lines 494-498).** No re-pointing of `peer_config` needed. Concretely, the diff in `execute()` is:

```rust
// DELETE these 5 lines (current 494-498):
let status = if peer_ac.join("active").exists() {
    "active"
} else {
    "unknown"
};

// KEEP the peer_ac binding (488-492) and peer_config load (500-505) as-is.
```

Then in the `peers.push(PeerInfo { ... })` block (§8.6), insert `let ps = compute_peer_status(&path_str, &session_index);` immediately before the push and use `ps.*` for the new fields. The legacy `status:` field becomes `ps.status_legacy.to_string()`.

§8.3 second bullet and the §8.6 "Important" note should be rewritten to match this. Treat the original "delete peer_ac" instruction as cancelled.

#### 13.2.2 BORROW CHECKER — `match c.status` may not compile

In §8.1, both `priority(c: &CandidateSession)` and the inner match in `compute_peer_status` write:

```rust
match c.status { SessionStatus::Active => ..., SessionStatus::Exited(_) => ..., }
//    ^^^^^^^^ place expression through `&CandidateSession`
```

`SessionStatus` does NOT implement `Copy` (it only derives `Clone`). Whether `match c.status` is accepted depends on match ergonomics applied to a place behind a shared ref — borderline and easy to break with future variant additions (e.g. an `Exited(String)`).

**Fix — use the same pattern as `list_sessions.rs:status_tag` (which is the proven precedent in this codebase):**

```rust
fn priority(c: &CandidateSession) -> u8 {
    if c.waiting_for_input {
        return 4;
    }
    match &c.status {                       // borrow
        SessionStatus::Active => 3,
        SessionStatus::Running => 2,
        SessionStatus::Idle => 1,
        SessionStatus::Exited(_) => 0,
    }
}
```

And inside `compute_peer_status`, change `match chosen.status` to `match &chosen.status`. The `Exited(n)` arm binds `n: &i32` — adjust to `Some(*n)` for the `exit_code`:

```rust
match &chosen.status {
    SessionStatus::Active => ("active", "active", true, None),
    SessionStatus::Running => ("running", "active", true, None),
    SessionStatus::Idle => ("idle", "unknown", false, None),
    SessionStatus::Exited(n) => ("exited", "unknown", false, Some(*n)),
}
```

### 13.3 ⚠️ Smaller items to address during implementation

#### 13.3.1 `pending_review` divergence is louder than §10.4 admits

`replicaDotClass()` returns `"pending"` BEFORE checking `waitingForInput` or `status` (`ProjectPanel.tsx:63`). So a sidebar peer in `pending` state is **not** a sidebar running-peer (`:784` predicate rejects it), yet the CLI will report `working: true, sessionStatus: "running"` for that peer (since `pending_review` is invisible to `PersistedSession`).

This is a **real divergence** — not just "a UI affordance". Recommendations:
- Add one line to the §3.3 `after_help` NOTE block: "`pendingReview` is not surfaced by the CLI; a peer in sidebar `pending` state will appear as `working: true` here."
- Mention it in the PR description.

Acceptable as a known limitation (out of scope: requires extending `PersistedSession.pending_review`). Just document it visibly.

#### 13.3.2 Place `use SessionStatus` at module top, not inside the helper block

Stylistic. §8.1 puts `use crate::session::session::SessionStatus;` inside the helper block. The rest of `list_peers.rs` keeps imports at the top (lines 1-6). Move the `use` next to the existing imports — `use` is hoisted to the enclosing scope anyway, and grouping imports keeps the file consistent.

Same for `use crate::config::sessions_persistence::load_sessions_raw;` and `use crate::session::session::TEMP_SESSION_PREFIX;` currently nested inside `build_session_index` — these could live at module top too. Minor; either way compiles.

#### 13.3.3 `agent_path` in `execute_wg_discovery` is already canonical

`agent_path` originates from `read_dir(&wg.my_wg_dir)` where `wg.my_wg_dir` was canonicalized in `detect_wg_replica` (`list_peers.rs:119, 147`). So `canon_or_norm(&agent_path.to_string_lossy())` re-canonicalizes a canonical path. Idempotent — no correctness issue, just a wasted syscall per WG replica. Acceptable.

In `execute()`'s WG-scan branch (lines 530-615), `agent_path` from `read_dir(&wg_path)` is NOT canonicalized (no prior canonicalization). So `canon_or_norm` does real work there. Both paths still produce the same normalized key.

#### 13.3.4 `compute_peer_status` works correctly for empty `path_str`

`member_path` may be `None` (lines 484-486 set `path_str = ""`). `canon_or_norm("")` → canonicalize fails → `norm_path("") == ""` → `index.get("")` returns `None` → `PeerStatus::none()`. Safe.

#### 13.3.5 `trim_end_matches('/')` trims ALL trailing slashes, not "a single trailing /"

§5.1 step 4 says "Trim a single trailing `/`". The implementation in §8.1 uses `trim_end_matches('/')` which strips **all** trailing slashes (`"c:/foo///"` → `"c:/foo"`). This is more aggressive than the description but still semantically correct on Windows (extra slashes are equivalent). Just update the §5.1 prose to match the code, or change the code to `trim_end_matches(|c| c == '/').to_string()` with a take-one variant. Not blocking.

### 13.4 ✅ Ready for implementation (with the §13.2 fixes)

After applying:
- §13.2.1 (keep `peer_ac` binding; only delete the 5-line marker check),
- §13.2.2 (`match &c.status` + `match &chosen.status`),
- §13.3.1 docstring update (one extra line in `after_help`),

the plan is **ready to implement**. The test plan (§9.1) covers the priority logic, normalization, and edge cases adequately. Manual verification (§9.2) covers the live PTY/sessions.json interaction that unit tests cannot reach.

### 13.5 Remaining concerns / things to watch during implementation

1. **`sessions.json` only contains rows the running app has persisted.** If `list-peers` is run while the app is starting up (before the first `persist_current_state` call), the file is the previous snapshot — peers may briefly report `sessionStatus: "exited"` or `"none"`. Not a regression vs. the old marker-file check.
2. **Cross-binary invisibility (§10.1) is the headline limitation.** Make sure the PR description leads with it so callers don't think `working: false` always means "not running".
3. **Clippy on the new code**: anticipate suggestions like `clippy::redundant_closure_for_method_calls` and `clippy::needless_lifetimes`. Apply as suggested.
4. **`cargo test` on Windows**: the new tests use Windows-style paths in the assertions (`C:\X` etc.). They will run on non-Windows hosts too because `norm_path` is pure string manipulation — but the `extended_length_prefix_normalizes` test exercises a Windows-specific path shape and asserts a Windows-style key. That's fine for our use case (this binary is Windows-only deployed).

---

## 14. Grinch Review (added 2026-05-13)

Reviewer: `wg-20-dev-team/dev-rust-grinch`. Independent adversarial pass after `dev-rust`'s §13 — verified against the same branch. Findings here are **additive** to §13.2/13.3; the two §13.2 blockers stand and must still be fixed.

### 14.1 Verification of §13's blocker findings

Both §13.2.1 (`peer_ac` is not dead) and §13.2.2 (`match &c.status` borrow) replicate independently — confirmed against `list_peers.rs:488-505` and the Rust 2021 borrow rules. Do not reopen these; ship the §13.2 fixes verbatim.

### 14.2 Additional findings — DISAGREE WITH §13.4 "ready for implementation"

#### 14.2.1 BLOCKER — `sessionStatus = "unknown"` is documented but unreachable

- **What.** §3.1 (PeerInfo doc), §4 row 2 ("Match exists but `id == None`"), and §3.3 `after_help` advertise `"unknown"` as a possible `sessionStatus` value meaning *"matching row in `sessions.json` had no runtime id"*. But `build_session_index` in §8.1 explicitly drops those rows: `let (Some(id), Some(status)) = (ps.id, ps.status) else { continue; };`. No candidate ever reaches `compute_peer_status` with `id == None`, so `compute_peer_status` only ever returns `"none"` for missing entries — never `"unknown"`.
- **Why.** Automation consumers will branch on a value that can never occur. Test authors will look for the case and either skip it (silent gap) or write fake data that doesn't reflect runtime behavior. The plan promises a 7-value enum but ships a 6-value one.
- **Fix.** Pick one:
  - (a) **Remove `"unknown"`** from §3.1 doc, §3.3 after_help, §4 row 2, and `PeerStatus::none()` (which currently sets `status_legacy: "unknown"` — that's the **legacy** field, not `sessionStatus`, but the doc conflates them). Final `sessionStatus` enum: `"active" | "running" | "idle" | "waiting" | "exited" | "none"`. Recommended.
  - (b) Keep `"unknown"` and emit it from `compute_peer_status` for some real edge case (e.g., a row that survived the `id`/`status` filter but failed downstream — currently no such case). Not recommended; manufactures a state for the sake of the docs.

#### 14.2.2 BLOCKER — `working` predicate is not the sidebar predicate when multiple sessions share a cwd

- **What.** §1 promises: *"`working` must mean **exactly what the Sidebar's `running-peer` badge means**"*. The sidebar binds **one** session per replica via `findSessionByName(wg/agent)` (`ProjectPanel.tsx:50-57`) and `replicaDotClass` evaluates only that named session (`:60-67`). The plan's `compute_peer_status` collects ALL candidates at the peer's normalized cwd and applies a 5-level priority. Concretely:
  - Sidebar: peer X has session "wg-20/dev" (Active, not waiting) → badge = `running`. The CLI's plan: ALSO has session "[temp]-foo" filtered out (good) AND a stray "Session 7" at the same cwd (Idle, waiting=true) → priority 4 wins → `working: false, sessionStatus: "waiting"`. **Contradicts the sidebar.**
  - This is realistic: rapid restart cycles or a user opening an extra terminal at the same dir leave multiple non-temp rows in `sessions.json`. The dedupe in `load_sessions()` does NOT run via `load_sessions_raw()` (the path the CLI uses).
- **Why.** The §1 invariant fails on a common case. Tech-lead spec ambiguity becomes user-visible bug reports ("the sidebar says I'm running, the CLI says I'm waiting").
- **Fix.** Pick one:
  - (a) **Be honest in §1 and §10**: the predicate is *path-keyed across all candidates at the cwd*; document the divergence as §10.X. Acceptable.
  - (b) **Match the sidebar exactly**: scope `compute_peer_status` by **session name** (`wg/agent`) instead of cwd. This requires `Session.name` in the lookup — already on `PersistedSession.name`. The sidebar uses name; the CLI should too. More work but eliminates the divergence.
  - Recommend (b) — it's not significantly more code (filter `candidates` by name before priority pick), and it actually delivers the §1 promise. If the tech-lead insists on cwd-keyed (e.g., for compat with non-replica peers), then (a) — but rewrite §1 to drop the "exactly" claim.

#### 14.2.3 MEDIUM — `pending_review` divergence is wider than §13.3.1 admits

§13.3.1 already noted the docstring fix. Adding teeth: the divergence is **two-way**, not one-way:
- **Sidebar `pending` → CLI `running`**: a peer whose agent finished and is awaiting user attention shows `pending` dot, is excluded from `running-peer` badges (`replicaDotClass` returns `"pending"` which fails the `dot === "running" || dot === "active"` check at `:784`). The CLI will report `working: true, sessionStatus: "running"` — the user looking at sidebar sees "agent done, needs attention" while the CLI says "currently working". Wrong direction for automation that wants to gate on "is anyone busy".
- **Caveat**: `Session::pending_review` is `#[serde(default)]` and `SessionInfo::from(&Session)` hardcodes `pending_review: false` (`session.rs:202`). So `pending_review` is set by the **frontend** state, not the backend. The CLI cannot see it even theoretically — it's a UI-only field.
- **Fix.** §10.4 should be honest: "pending_review is invisible to the CLI and to `sessions.json` because it's a frontend-only field. A peer whose agent has finished but the user hasn't acknowledged will be reported as `working: true` (because the underlying `SessionStatus` is still `Running`), even though the sidebar shows it as needing attention." Don't soft-pedal as "acceptable: pending-review is a UI affordance" — it's a real predicate divergence with concrete user-observable inconsistency.

#### 14.2.4 MEDIUM — `session_index` is built before the WG fast-return → wasted I/O

- **What.** §8.5 first bullet places `let session_index = build_session_index();` **after** root validation but **before** the WG-replica branch (`if let Some(wg) = detect_wg_replica(&root) { return execute_wg_discovery(wg); }` at `list_peers.rs:426-428`). `execute_wg_discovery` then re-builds its own index at the top. WG-path invocations do the work twice and discard the first.
- **Why.** Wasted full sessions.json read + parse + canonicalize loop on every WG `list-peers` call (which is the *primary* call path for this fix). On a large `sessions.json` with N rows, that's 2N canonicalize syscalls.
- **Fix.** Move §8.5 first bullet to **after** the WG fast-return — immediately before the `let my_name = ...` line at `:436`. The standard path uses it; the WG path doesn't need it (it builds its own).

#### 14.2.5 MEDIUM — `load_sessions_raw` silently swallows parse/read errors → "all peers offline" is undiagnosable

- **What.** `sessions_persistence.rs:153-156`:
  ```rust
  match std::fs::read_to_string(&path) {
      Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
      Err(_) => vec![],
  }
  ```
  Both branches collapse to `vec![]` with no logging. After this fix, the user sees `working: false, sessionStatus: "none"` for every peer identically whether: file is missing (expected), file is unreadable (permissions), file is corrupt JSON (real bug), or file genuinely has zero sessions. The new `working` field becomes the headline signal — and its silent failure mode is "all your peers look dead".
- **Why.** Pre-existing issue, but exponentially more painful now that `working` is the consumer-facing predicate. Future debugging of "why is everyone offline?" is a guessing game.
- **Fix.** Two options:
  - (a) Add `log::warn!` to both error arms in `load_sessions_raw` (one-line change to a file the plan otherwise leaves alone — but justifiable given the new exposure). The plan says `sessions_persistence.rs` is reference-only; either lift that restriction for this one log line, or
  - (b) Add §10.X documenting the silent-failure mode and recommending users check `<config_dir>/sessions.json` directly when `list-peers` reports unexpected `none`s.
  - Recommend (a). The CLAUDE-md guidance "silenced errors should at least be logged" applies.

#### 14.2.6 MEDIUM — `sessions.json` race with concurrent rewrites on Windows

- **What.** `save_sessions` does atomic `tmp → rename` (`sessions_persistence.rs:302-306`). On Windows, `std::fs::rename` is `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` — atomic on the same volume, but the destination file briefly enters a state where opens from other processes can hit `ERROR_SHARING_VIOLATION`. If `list-peers` reads while the app is rewriting, `read_to_string` may fail with a transient I/O error → `vec![]` → all peers `none`.
- **Why.** Low-frequency but real. User runs `list-peers` repeatedly during a switch_session storm; some invocations return all-offline. Combined with §14.2.5 (silent failure), the user has zero signal that this happened.
- **Fix.** Either (a) retry once with a 50ms backoff in `load_sessions_raw` on transient read errors, or (b) at minimum log the read error so the user can correlate with timing. Combine with §14.2.5 fix.

#### 14.2.7 MEDIUM — Missing test coverage for `build_session_index` filters

- **What.** §9.1 has 9 tests for `compute_peer_status` and `norm_path`, but **zero** tests for `build_session_index`'s filters: (a) temp-session prefix skip, (b) `id == None` skip, (c) `status == None` skip, (d) cwd canonicalization on the index side (the `extended_length_prefix_normalizes` test only canonicalizes the lookup side).
- **Why.** These filters determine which rows are visible. A regression (e.g., dropping the temp-session check during a refactor) would silently flicker `working: true` for `[temp]` sessions — the exact bug §7 says we're avoiding. Untested filters rot.
- **Fix.** Add tests for `build_session_index` directly. Suggested:
  ```rust
  #[test]
  fn build_index_skips_temp_sessions() { /* ... */ }
  #[test]
  fn build_index_skips_rows_without_id() { /* ... */ }
  #[test]
  fn build_index_skips_rows_without_status() { /* ... */ }
  #[test]
  fn build_index_normalizes_cwd_with_extended_prefix() { /* uses \\?\C:\... in the row */ }
  ```
  These need `build_session_index` to be testable — either expose it `pub(crate)` or accept a `&[PersistedSession]` parameter (preferred — pure function, easier test).

#### 14.2.8 MINOR — §10.1 cross-binary doc misses the WG-shipper deploy pattern

- **What.** §10.1 mentions cross-binary visibility generically. Per the team's working rule (`feedback_shipper_wg_only_deploy`), the WG build is shipped to `agentscommander_mb_wg-<N>.exe` (NOT the bare `agentscommander_mb.exe`). Users will routinely have both side-by-side. Running `list-peers` from the wrong one returns "all peers `none`" with no clue why.
- **Why.** The §10.1 prose is correct but abstract. A user staring at `working: false` for a peer they just opened will not connect "different binary" → "different sessions.json" without a worked example.
- **Fix.** Add to §10.1: *"Worked example: if the app was launched via `agentscommander_mb_wg-20.exe` (per the WG shipper convention), the CLI must also be invoked as `agentscommander_mb_wg-20.exe list-peers`. Running the bare `agentscommander_mb.exe list-peers` reads `.agentscommander_mb/sessions.json` instead of `.agentscommander_mb_wg-20/sessions.json` and reports `sessionStatus: 'none'` for every peer."* Also add a manual-verification step in §9.2 that exercises this mismatch deliberately (positive control: confirm "none" is reported when the binaries don't match).

#### 14.2.9 MINOR — §9.2 step 5 ("Trigger `waiting_for_input`") is under-specified

- **What.** Step 5 says *"send a prompt awaiting user input"*. For a Claude session, the bridge from agent prompting → `mark_idle()` firing → `persist_current_state()` writing → CLI reading is non-trivial. Without an exact recipe, the manual verification will be skipped or done inconsistently. This is the test that exercises priority-4 (waiting) → `working: false` → `sessionStatus: "waiting"` — the most novel branch.
- **Fix.** Tighten to: *"From the peer's session, send any prompt that produces a Claude permission/approval prompt (e.g., a tool call requiring confirmation). Wait until the sidebar dot for that peer turns yellow (waiting). Then run `list-peers`."* Or simpler: *"Wait until `replicaDotClass` returns `'waiting'` for the peer in your own sidebar — that confirms `mark_idle` has fired and persisted."*

#### 14.2.10 NIT — §3.3 `after_help` lineation needs a build-time eyeball

The escaped newlines in the rust string literal (`\n                      \  `) in §3.3 have uneven leading whitespace that may render with stray gaps in `--help` output. Run `agentscommander_mb_wg-20.exe list-peers --help` after the change and paste actual rendered output into the PR description to confirm it doesn't look broken.

### 14.3 Verdict

**FAIL.** Beyond §13.2 blockers, the plan has two additional blocking issues:
- §14.2.1 — `"unknown"` `sessionStatus` is documented but unreachable. Ship inconsistent docs and you ship a broken contract.
- §14.2.2 — `working` predicate diverges from sidebar in the multi-session-per-cwd case. The §1 invariant ("**exactly what the Sidebar's `running-peer` badge means**") is false as currently scoped.

**Required architect edits before implementation begins:**

1. **§3.1 / §4 row 2 / §3.3 / `PeerStatus::none()`** — drop `"unknown"` from `sessionStatus` (per §14.2.1 fix (a)), OR change `build_session_index` to admit `Some(id) + None status` rows as "unknown" candidates and add a test. Pick one and reconcile every doc surface.
2. **§1 + §6 + §10** — choose between matching the sidebar by name (preferred, §14.2.2 fix (b)) or being honest about the path-keyed divergence (acceptable, fix (a)). If (b), update §6.1, §6.3, and add a name field to `CandidateSession`. If (a), rewrite §1 to drop "exactly" and add §10.X.
3. **§10.4** — rewrite per §14.2.3 to admit the `pending_review` divergence is real and bidirectional (not just "a UI affordance"). Add the `after_help` line per §13.3.1.
4. **§8.5 first bullet** — move `let session_index = build_session_index();` to **after** the WG fast-return (after `:428`), before `:436`.
5. **§9.1** — add the four missing `build_session_index` tests per §14.2.7. Make `build_session_index` testable (suggest: take `&[PersistedSession]` parameter).
6. **§10.1** — add the WG-shipper worked example per §14.2.8.
7. **§9.2 step 5** — tighten the waiting-trigger recipe per §14.2.9.

**Optional (recommended):**
- §14.2.5 + §14.2.6 — add `log::warn!` in `load_sessions_raw`'s error arms (lift the "reference only" restriction in §2 for one log line). Or document the silent-failure mode in §10.X.
- §3.3 — eyeball the `after_help` rendering and paste actual output into the PR.

After §13.2 + §14.2.1–4 are addressed, the plan is implementable. §14.2.5–10 can land in the same PR or as follow-up.

---

## 15. Architect verdict and decisions (added 2026-05-13)

This section records the architect's resolutions of every reviewer finding and the final state of the plan. The body of the plan (§1–§12) has been edited to reflect these decisions; §13 and §14 remain as historical record of the review.

### 15.1 Decisions on blocking findings

| Finding | Resolution | Where applied |
|---|---|---|
| **§13.2.1** — `peer_ac` is not dead; only the marker block must be deleted | **Applied verbatim.** §8.3 rewritten: delete ONLY lines 494-498 (and lines 284-288 in `build_wg_peer`'s `replica_ac.join("active").exists()` block). Keep `peer_ac` (488-492) and `peer_config` (500-505) intact. The contradictory "delete peer_ac" instruction is cancelled. | §8.3, §8.6 |
| **§13.2.2** — `match c.status` on non-Copy `SessionStatus` | **Applied verbatim.** §8.1 helper block now uses `match &c.status` and `match &chosen.status`; `Exited(n)` arms produce `Some(*n)`. | §8.1 |
| **§14.2.1** — `sessionStatus = "unknown"` is unreachable | **Drop `"unknown"`** (grinch's recommended option a). Final `sessionStatus` enum is 6 values: `"active" \| "running" \| "idle" \| "waiting" \| "exited" \| "none"`. The legacy `status` field is unchanged (still `"active" \| "unknown"`). Doc and code updated. | §3.1, §3.3, §4, §8.1 |
| **§14.2.2** — `working` predicate diverges from sidebar with multi-session-per-cwd | **Match by name for WG peers** (grinch's recommended option b). `CandidateSession` gains a `name: String` field; `compute_peer_status` accepts `expected_name: Option<&str>`. WG peers pass `Some("<wg_name>/<agent_name>")` (mirrors `replicaSessionName`); non-WG peers pass `None` (cwd-only — there is no sidebar `running-peer` predicate for team members, divergence documented in §10.8). The §1 invariant is now scoped per peer kind. | §1, §6.1, §8.1, §8.4, §8.6 |

### 15.2 Decisions on recommended (non-blocking) findings

| Finding | Resolution | Where applied |
|---|---|---|
| §13.3.1 / §14.2.3 — `pending_review` divergence honesty | **Rewritten.** §10.4 now states explicitly: `pending_review` is hardcoded `false` in `SessionInfo::from(&Session)` (`session.rs:202`), never reaches `PersistedSession`, and produces a real bidirectional inconsistency (sidebar `pending` → CLI `running`). §3.3 `after_help` NOTES block also documents this. | §3.3, §10.4 |
| §13.3.2 — `use` placement | **Applied.** All new `use` statements (`SessionStatus`, `TEMP_SESSION_PREFIX`, `load_sessions_raw`, `PersistedSession`) hoisted to the existing import block at `list_peers.rs:1-6`. | §8.1 |
| §13.3.5 / §5.1 — `trim_end_matches` prose | **Applied.** §5.1 step 4 reworded to "Trim trailing `/`s" matching code behavior. | §5.1 |
| §14.2.4 — `session_index` placement | **Applied.** §8.5 moved the `let session_index = build_session_index();` insertion in `execute()` to AFTER the WG fast-return (after line 428), before `let my_name = ...` at line 436. The WG path uses its own index inside `execute_wg_discovery`. | §8.5 |
| §14.2.7 — Tests for `build_session_index` filters | **Applied.** `build_session_index` is split into `build_session_index_from(&[PersistedSession])` (pure, testable) and `build_session_index()` (thin loader). Five new index tests added in §9.1 covering temp-prefix skip, missing-id skip, missing-status skip, `\\?\`-prefix normalization on the index side, and multi-row cwd grouping. Two new `compute_peer_status` tests added for the WG name filter. The previous `waiting_at_any_candidate_sets_waiting_aggregate` test is dropped — aggregate semantics replaced with chosen-candidate semantics per §4. | §9.1 |
| §14.2.8 — WG-shipper worked example | **Applied.** §10.1 now includes a concrete `agentscommander_mb_wg-20.exe` vs `agentscommander_mb.exe` walk-through showing the `<binary_parent>/.<binary_stem>/sessions.json` divergence. Manual step 11 in §9.2 is retained as a positive control. | §10.1 |
| §14.2.9 — Waiting-trigger recipe | **Applied.** §9.2 step 5 now specifies a Claude tool-use confirmation prompt and requires waiting until the sidebar dot turns yellow before re-running `list-peers`. | §9.2 |
| §14.2.10 — `after_help` rendering eyeball | **Applied.** New manual verification step 13 added to §9.2: run `<binary> list-peers --help` and paste the rendered output into the PR description. | §9.2 |

### 15.3 Decisions on optional findings (deferred)

| Finding | Resolution | Rationale |
|---|---|---|
| §14.2.5 — `load_sessions_raw` silent failure | **Documented, not fixed in this PR.** §10.9 (new) records the silent-failure mode and points users to `<config_dir>/sessions.json` for diagnostics. The §2 reference-only restriction on `sessions_persistence.rs` is preserved. Future PR can add `log::warn!`. | Keeps blast radius minimal for #206. The diagnostic gap exists today; adding it as a follow-up is acceptable. |
| §14.2.6 — Windows file-rename race | **Documented in §10.9.** Same rationale as §14.2.5 — defer the retry/log fix to a follow-up PR. | Same. |

### 15.4 Architectural change permitted in this PR

One narrow exception to the §2 "reference only" rule is **required** by the §9.1 test fixtures:

- **`PersistedSession`**: add `#[derive(Default)]` (along with the existing `Debug, Clone, Serialize, Deserialize` derives) in `src-tauri/src/config/sessions_persistence.rs:15`. Pure derive addition — no behavior change, no field changes. Required because the new `build_session_index_from` tests construct `PersistedSession` fixtures with `..Default::default()` (`SessionStatus` is `Option<_>` in `PersistedSession`, so no Default needed there).

If the dev prefers NOT to touch `sessions_persistence.rs` at all, the `ps_row` helper in §9.1 can be rewritten with an explicit field-by-field literal. Either path is acceptable; the Default derive is the cleaner choice.

### 15.5 Implementation instructions dev-rust must not miss

1. **Order**: follow §11 step-by-step. Each step is independently compile-checkable.
2. **Imports**: add the two new `use` lines at the top of `list_peers.rs` (per §8.1) BEFORE adding the helper block, so `cargo check` doesn't emit a flood of unresolved-name errors.
3. **`peer_ac` survival**: re-read `list_peers.rs:488-505` in your editor BEFORE editing. Confirm visually that `peer_ac` is referenced at line 500 by the `peer_config` load. Delete ONLY lines 494-498. (The same applies to `replica_ac` in `build_wg_peer` lines 284-288 — `replica_ac` is reused below.)
4. **Borrow form**: write `match &c.status` and `match &chosen.status`, with `Exited(n) => ... Some(*n)`. Do NOT write `match c.status` — `SessionStatus` is not `Copy`.
5. **WG peers pass a name; non-WG peers pass `None`**: `build_wg_peer` constructs `expected_session_name = format!("{}/{}", wg_name, agent_name)` and threads it into `compute_peer_status`. The non-WG branch in `execute()` passes `None`. This is the §14.2.2 fix — do not unify them.
6. **session_index placement**: inside `execute()`, build the index AFTER the WG fast-return at line 428, not before. WG discovery builds its own.
7. **`PersistedSession` Default**: add `#[derive(Default)]` per §15.4, unless you prefer explicit literals in `ps_row`.
8. **Clippy**: anticipate `clippy::redundant_closure_for_method_calls` and similar; apply suggestions inline. Do NOT blanket `#[allow]`.
9. **Manual verification on Windows**: §9.2 steps must be run end-to-end. The unit tests cannot cover the live PTY → `mark_idle` → `persist_current_state` → CLI read pipeline.
10. **PR description**: lead with the cross-binary visibility limitation (§10.1 worked example), the `pending_review` divergence (§10.4), and the WG-vs-non-WG predicate split (§1). Paste the rendered `--help` output (§9.2 step 13).

### 15.6 Verdict

`READY_FOR_IMPLEMENTATION`

