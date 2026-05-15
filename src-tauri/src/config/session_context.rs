/// Writes a per-agent copy of AgentsCommanderContext.md with the agent's own
/// root path interpolated into the GOLDEN RULE. For WG replicas, also exposes
/// the canonical Agent Matrix scope derived from config.json "identity". Uses a
/// deterministic filename based on the agent_root to prevent races between
/// concurrent session launches.
pub fn ensure_session_context(agent_root: &str) -> Result<String, String> {
    let config_dir =
        super::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join("context-cache");
    std::fs::create_dir_all(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    // Canonicalize path for consistent display in the GOLDEN RULE text
    let canonical_root = std::fs::canonicalize(agent_root)
        .map(|p| display_path(&p))
        .unwrap_or_else(|_| agent_root.to_string());
    let matrix_root = resolve_replica_matrix_root(agent_root);

    let hash = simple_hash(agent_root);
    let file_path = context_dir.join(format!("ac-context-{}.md", hash));

    std::fs::write(
        &file_path,
        default_context(&canonical_root, matrix_root.as_deref()),
    )
    .map_err(|e| format!("Failed to write per-agent AgentsCommanderContext.md: {}", e))?;
    log::info!(
        "Refreshed per-agent AgentsCommanderContext.md for {} → {:?}",
        agent_root,
        file_path
    );

    Ok(file_path.to_string_lossy().to_string())
}

const MANAGED_CONTEXT_FILENAMES: &[&str] =
    &["last_ac_context.md", "CLAUDE.md", "GEMINI.md", "AGENTS.md"];

#[derive(Debug, Clone, Copy)]
pub enum ManagedContextTarget {
    Claude,
    Gemini,
    Codex,
}

impl ManagedContextTarget {
    fn filename(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE.md",
            Self::Gemini => "GEMINI.md",
            Self::Codex => "AGENTS.md",
        }
    }
}

/// Special token in context[] that resolves to the global AgentsCommanderContext.md.
const CONTEXT_TOKEN_GLOBAL: &str = "$AGENTSCOMMANDER_CONTEXT";

/// Special token in context[] that generates workspace repo info from the "repos" field.
const CONTEXT_TOKEN_REPOS: &str = "$REPOS_WORKSPACE_INFO";

/// Filename for the agent role definition, auto-injected from the identity matrix.
const ROLE_MD_FILENAME: &str = "Role.md";

/// Convert a path to a stable, user-facing display string on Windows.
fn display_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

/// Resolve the canonical Agent Matrix root for a WG replica from config.json "identity".
fn resolve_replica_matrix_root(replica_root: &str) -> Option<String> {
    if !is_replica_agent_dir(replica_root) {
        return None;
    }

    let replica_path = std::path::Path::new(replica_root);
    let config_path = replica_path.join("config.json");
    let config_content = std::fs::read_to_string(&config_path).ok()?;
    let config: serde_json::Value = serde_json::from_str(&config_content).ok()?;
    let identity = config.get("identity")?.as_str()?;
    let matrix_path = replica_path.join(identity);

    std::fs::canonicalize(&matrix_path)
        .map(|p| display_path(&p))
        .ok()
        .or_else(|| Some(display_path(&matrix_path)))
}

fn canonical_or_original(path: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn find_ac_new_root(path: &std::path::Path) -> Option<std::path::PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.eq_ignore_ascii_case(".ac-new"))
                .unwrap_or(false)
        })
        .map(canonical_or_original)
}

fn is_agent_matrix_dir(cwd: &str) -> bool {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("_agent_"))
        .unwrap_or(false)
}

fn is_agent_dir(cwd: &str) -> bool {
    is_replica_agent_dir(cwd) || is_agent_matrix_dir(cwd)
}

/// Build the GIT_CEILING_DIRECTORIES value for agent sessions rooted in `.ac-new`.
/// This blocks Git from traversing upward into the parent project repo when the
/// current directory is an agent matrix, a WG replica, or a descendant of those roots.
pub fn git_ceiling_directories_for_session_root(cwd: &str) -> Option<String> {
    if !is_agent_dir(cwd) {
        return None;
    }

    let cwd_path = std::path::Path::new(cwd);
    let mut ordered: Vec<std::path::PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push_unique = |path: std::path::PathBuf| {
        let canonical = canonical_or_original(&path);
        let key = display_path(&canonical);
        if seen.insert(key) {
            ordered.push(canonical);
        }
    };

    if let Some(ac_new_root) = find_ac_new_root(cwd_path) {
        push_unique(ac_new_root);
    }

    push_unique(cwd_path.to_path_buf());

    if let Some(matrix_root) = resolve_replica_matrix_root(cwd) {
        push_unique(std::path::PathBuf::from(matrix_root));
    }

    if ordered.is_empty() {
        return None;
    }

    std::env::join_paths(ordered.iter())
        .ok()
        .map(|paths| paths.to_string_lossy().to_string())
        .or_else(|| {
            Some(
                ordered
                    .iter()
                    .map(|p| display_path(p))
                    .collect::<Vec<_>>()
                    .join(if cfg!(windows) { ";" } else { ":" }),
            )
        })
}

/// Generate a markdown file with workspace repo information from the replica's config.
/// Reads "repos" from `config`, resolves paths relative to `cwd_path`, detects git branches.
/// Returns the path to the generated temp file.
fn generate_repos_workspace_info(
    cwd_path: &std::path::Path,
    config: &serde_json::Value,
) -> Result<std::path::PathBuf, String> {
    let config_dir =
        super::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join("context-cache");
    std::fs::create_dir_all(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    let hash = simple_hash(&cwd_path.to_string_lossy());
    let file_path = context_dir.join(format!("repos-workspace-{}.md", hash));

    let repos = config
        .get("repos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if repos.is_empty() {
        std::fs::write(
            &file_path,
            "# Workspace Repos\n\nNo repos configured for this replica.\n",
        )
        .map_err(|e| format!("Failed to write repos workspace info: {}", e))?;
        return Ok(file_path);
    }

    let mut md = String::from(
        "# Workspace Repos\n\n\
         You are working inside a workgroup replica. Your working directory is your agent dir, \
         but your code repos are listed below. You MUST change to the appropriate repo directory \
         before doing any code work (git, file edits, builds, etc).\n\n\
         ## Repos\n\n",
    );

    for repo_val in &repos {
        let rel = match repo_val.as_str() {
            Some(s) => s,
            None => continue,
        };

        let resolved = cwd_path.join(rel);
        // Canonicalize to get a clean absolute path (strip \\?\ on Windows)
        let abs_path = std::fs::canonicalize(&resolved)
            .map(|p| display_path(&p))
            .unwrap_or_else(|_| resolved.to_string_lossy().to_string());

        let repo_name = resolved.file_name().and_then(|n| n.to_str()).unwrap_or(rel);

        if !resolved.exists() {
            md.push_str(&format!(
                "- **{}** — Path: `{}` — **(NOT FOUND)**\n",
                repo_name, abs_path
            ));
            continue;
        }

        let branch = detect_git_branch(&abs_path).unwrap_or_else(|| "unknown".to_string());
        md.push_str(&format!(
            "- **{}** — Path: `{}` — Branch: `{}`\n",
            repo_name, abs_path, branch
        ));
    }

    std::fs::write(&file_path, &md)
        .map_err(|e| format!("Failed to write repos workspace info: {}", e))?;

    Ok(file_path)
}

/// Detect git branch for a given directory path.
fn detect_git_branch(dir: &str) -> Option<String> {
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut cmd = std::process::Command::new("git");
    crate::pty::credentials::scrub_credentials_from_std_command(&mut cmd);
    cmd.args(["-C", dir, "branch", "--show-current"]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if branch.is_empty() || branch == "HEAD" {
                None
            } else {
                Some(branch)
            }
        }
        _ => None,
    }
}

/// Build a combined context file for a replica session.
/// Reads config.json from `cwd`, looks for `context[]` array.
/// Entries are resolved in order:
/// - `$AGENTSCOMMANDER_CONTEXT` → resolves to the global AgentsCommanderContext.md
/// - `$REPOS_WORKSPACE_INFO` → generates workspace repo info from the "repos" field
/// - Any other string → resolved as a path relative to `cwd`
///
/// After resolving context[], if `identity` is set in config.json and `<identity>/Role.md`
/// exists on disk, it is auto-appended (unless already resolved from context[]).
/// The global context is NOT auto-prepended — it is only included if the token is in the array.
///
/// Returns Ok(Some(path)) with the combined temp file, Ok(None) if no context[] field,
/// or Err with details about missing files.
pub fn build_replica_context(cwd: &str) -> Result<Option<String>, String> {
    let cwd_path = std::path::Path::new(cwd);
    let config_path = cwd_path.join("config.json");

    // No config.json → no replica context, fall back to default behavior
    if !config_path.exists() {
        return Ok(None);
    }

    let config_content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read {}: {}", config_path.display(), e))?;

    let config: serde_json::Value = serde_json::from_str(&config_content)
        .map_err(|e| format!("Failed to parse {}: {}", config_path.display(), e))?;

    // No "context" field → no replica context
    let context_array = match config.get("context").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() => arr,
        _ => return Ok(None),
    };

    // Resolve and validate all paths (supporting special tokens)
    let mut resolved_paths: Vec<(String, std::path::PathBuf)> = Vec::new(); // (label, abs_path)
    let mut missing: Vec<String> = Vec::new();

    for entry in context_array {
        let raw = match entry.as_str() {
            Some(s) => s,
            None => continue,
        };

        if raw == CONTEXT_TOKEN_GLOBAL {
            let global_path = ensure_session_context(cwd)?;
            resolved_paths.push((
                "AgentsCommanderContext.md".to_string(),
                std::path::PathBuf::from(&global_path),
            ));
        } else if raw == CONTEXT_TOKEN_REPOS {
            let repos_path = generate_repos_workspace_info(cwd_path, &config)?;
            resolved_paths.push(("Workspace Repos".to_string(), repos_path));
        } else {
            let abs = cwd_path.join(raw);
            if abs.exists() {
                let label = abs
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(raw)
                    .to_string();
                resolved_paths.push((label, abs));
            } else {
                missing.push(raw.to_string());
            }
        }
    }

    // Auto-inject Role.md from identity matrix if present and not already resolved
    if let Some(identity) = config.get("identity").and_then(|v| v.as_str()) {
        let role_abs = cwd_path.join(format!("{}/{}", identity, ROLE_MD_FILENAME));
        let already_included = resolved_paths.iter().any(|(_, p)| *p == role_abs);
        if !already_included && role_abs.exists() {
            resolved_paths.push((ROLE_MD_FILENAME.to_string(), role_abs));
        }
    }

    if !missing.is_empty() {
        let replica_name = cwd_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        return Err(format!(
            "Replica '{}' has missing context files:\n{}",
            replica_name,
            missing
                .iter()
                .map(|m| format!("  - {}", m))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    // Build combined content in context[] order (no auto-prepend of global context)
    let mut combined = String::new();
    let mut first = true;

    for (label, path) in &resolved_paths {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read context file {}: {}", path.display(), e))?;
        if first {
            combined.push_str(&content);
            first = false;
        } else {
            combined.push_str(&format!("\n\n---\n\n# Context: {}\n\n", label));
            combined.push_str(&content);
        }
    }

    // Write to a temp file in the app config dir
    let config_dir =
        super::config_dir().ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let context_dir = config_dir.join("context-cache");
    std::fs::create_dir_all(&context_dir)
        .map_err(|e| format!("Failed to create context-cache dir: {}", e))?;

    // Use a deterministic filename based on the cwd to avoid temp file accumulation
    let hash = simple_hash(cwd);
    let file_path = context_dir.join(format!("replica-context-{}.md", hash));
    std::fs::write(&file_path, &combined)
        .map_err(|e| format!("Failed to write combined context file: {}", e))?;

    log::info!(
        "Built replica context for {} ({} context files) → {}",
        cwd,
        resolved_paths.len(),
        file_path.display()
    );

    Ok(Some(file_path.to_string_lossy().to_string()))
}

/// Resolve the final session context content for a replica directory.
/// Prefers replica config.json context[] and falls back to the per-agent default context.
fn resolve_session_context_content(cwd: &str) -> Result<Option<String>, String> {
    if !is_replica_agent_dir(cwd) {
        return Ok(None);
    }

    let context_path = match build_replica_context(cwd) {
        Ok(Some(combined_path)) => {
            log::info!(
                "Using replica combined context for agent session: {}",
                combined_path
            );
            combined_path
        }
        Ok(None) => ensure_session_context(cwd)?,
        Err(e) => return Err(e),
    };

    let content = std::fs::read_to_string(&context_path).map_err(|e| {
        format!(
            "Failed to read resolved session context {}: {}",
            context_path, e
        )
    })?;
    Ok(Some(content))
}

/// Delete stale agent-specific context files from a replica cwd and rewrite the
/// current resolved context into the single provider-specific filename required
/// by the coding agent being launched.
pub fn materialize_agent_context_file(
    cwd: &str,
    target: ManagedContextTarget,
) -> Result<Option<String>, String> {
    let content = match resolve_session_context_content(cwd)? {
        Some(content) => content,
        None => return Ok(None),
    };

    let cwd_path = std::path::Path::new(cwd);
    for filename in MANAGED_CONTEXT_FILENAMES {
        let path = cwd_path.join(filename);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| {
                format!(
                    "Failed to remove stale context file {}: {}",
                    path.display(),
                    e
                )
            })?;
        }
    }

    let target_path = cwd_path.join(target.filename());
    std::fs::write(&target_path, &content)
        .map_err(|e| format!("Failed to write {}: {}", target_path.display(), e))?;

    log::info!(
        "Materialized managed agent context file in {}: {}",
        cwd,
        target_path.display()
    );

    Ok(Some(target_path.to_string_lossy().to_string()))
}

/// Simple deterministic hash for a string (for temp file naming).
fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    hash
}

/// Generate the default agent context with a per-agent GOLDEN RULE that embeds
/// the agent's own replica root path and, for WG replicas, the allowed Agent
/// Matrix scope.
fn default_context(agent_root: &str, matrix_root: Option<&str>) -> String {
    let allowed_places = "the entries listed below";
    let replica_usage =
        "   Use this for replica-local scratch, personal notes, inbox/outbox, role drafts, and session artifacts. Do NOT store canonical memory or plans here. Do NOT write into other agents' replica directories.";
    let matrix_section = match matrix_root {
        Some(matrix_root) => format!(
            "3. **Your origin Agent Matrix, but only for the canonical agent state listed below:**\n   ```\n   {matrix_root}\n   ```\n   Allowed there:\n   - `memory/`\n   - `plans/`\n   - `Role.md`\n\n",
            matrix_root = matrix_root,
        ),
        None => String::new(),
    };
    let matrix_allowed = match matrix_root {
        Some(matrix_root) => format!(
            "- **Allowed**: Full read/write inside your origin Agent Matrix's `memory/`, `plans/`, and `Role.md` ({matrix_root})\n",
            matrix_root = matrix_root,
        ),
        None => String::new(),
    };
    let messaging_dir_display =
        crate::phone::messaging::workgroup_root(std::path::Path::new(agent_root))
            .ok()
            .map(|wg| {
                let dir = wg.join(crate::phone::messaging::MESSAGING_DIR_NAME);
                display_path(&dir)
            });
    let messaging_exception = match &messaging_dir_display {
        Some(path) => format!(
            "**Narrow exception — workgroup messaging directory:**\n\n\
             You MAY create message files inside this directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Strictly limited to canonical inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` (the CLI rejects any other shape). Used by the two-step protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete any message file once written. Do NOT write any other kind of file here.\n\n",
            path = path,
        ),
        None => String::new(),
    };
    let messaging_allowed = match &messaging_dir_display {
        Some(path) => format!(
            "- **Allowed (narrow)**: Create canonical inter-agent message files in your workgroup messaging directory ({path}). No other writes there.\n",
            path = path,
        ),
        None => String::new(),
    };
    let workspace_root_phrase = if messaging_dir_display.is_some() {
        "the workspace root (other than the narrow messaging exception above)"
    } else {
        "the workspace root"
    };
    let forbidden_scope = if matrix_root.is_some() {
        format!(
            "the entries listed above — including other agents' replica directories, any other files inside the Agent Matrix, {ws}, parent project dirs, user home files, or arbitrary paths on disk",
            ws = workspace_root_phrase,
        )
    } else {
        format!(
            "the entries listed above — including other agents' replica directories, {ws}, parent project dirs, user home files, or arbitrary paths on disk",
            ws = workspace_root_phrase,
        )
    };
    let git_scope = if matrix_root.is_some() {
        "Your replica directory and origin Agent Matrix are typically inside a parent repository's `.ac-new/` folder, which is `.gitignore`d. Do NOT run `git` commands that alter state (commit, branch, reset, etc.) from inside either location — that would affect the parent repo unintentionally. AgentsCommander blocks Git repository discovery above these `.ac-new` roots for agent sessions, but you must still switch into the appropriate `repo-*` directory before running Git operations that change repository state. `git status`, `git log`, and `git diff` are fine inside the allowed roots."
    } else {
        "Your agent directory is typically inside a parent repository's `.ac-new/` folder, which is `.gitignore`d. Do NOT run `git` commands that alter state (commit, branch, reset, etc.) from inside that directory — that would affect the parent repo unintentionally. AgentsCommander blocks Git repository discovery above these `.ac-new` roots for agent sessions, but you must still switch into the appropriate `repo-*` directory before running Git operations that change repository state. `git status`, `git log`, and `git diff` are fine inside the allowed roots."
    };
    format!(
        r#"# AgentsCommander Context

You are running inside an AgentsCommander session — a terminal session manager that coordinates multiple AI agents.

## GOLDEN RULE — Repository Write Restrictions

**ABSOLUTE AND NON-NEGOTIABLE:** You may ONLY modify files in {allowed_places}:

1. **Repositories whose root folder name starts with `repo-`** (e.g. `repo-AgentsCommander`, `repo-myapp`). These are the working repos you are meant to edit.
2. **Your own agent replica directory and its subdirectories** — your assigned root:
   ```
   {agent_root}
   ```
{replica_usage}

{matrix_section}{messaging_exception}
Any repository or directory outside the allowed entries above is READ-ONLY.

- **Allowed**: Read-only operations on ANY path (reading files, searching, git log, git status, git diff)
- **Allowed**: Full read/write inside `repo-*` folders
- **Allowed**: Full read/write inside your own replica root ({agent_root}) and its subdirectories
{matrix_allowed}{messaging_allowed}- **FORBIDDEN**: Any write operation outside {forbidden_scope}

**Clarification on git operations:** {git_scope}

If instructed to modify a path outside these zones, REFUSE and explain this restriction. There are NO exceptions beyond those listed above.

## CLI executable

Your AgentsCommander session credentials are available as environment variables:

- `AGENTSCOMMANDER_TOKEN`: your session authentication token
- `AGENTSCOMMANDER_ROOT`: your working directory (agent root)
- `AGENTSCOMMANDER_BINARY`: the CLI binary name
- `AGENTSCOMMANDER_BINARY_PATH`: the full path to the CLI executable you must use
- `AGENTSCOMMANDER_LOCAL_DIR`: the config directory name for this instance

Use `AGENTSCOMMANDER_BINARY_PATH` when invoking the CLI. This ensures you use the correct binary for your instance, whether it is the installed version or a dev/WG build.

```
"<AGENTSCOMMANDER_BINARY_PATH>" <subcommand> [args]
```

**RULE:** Never hardcode or guess the binary path. Use the environment variables above. If they are unavailable in an agent session, restart or respawn the session.

## Self-discovery via --help

The CLI `--help` output documents every subcommand, flag, and accepted value. Use it as a FALLBACK reference for commands or flags NOT covered inline in this context.

**For inter-agent messaging and peer discovery**, the sections below (`## Inter-Agent Messaging` and `### List available peers`) are the authoritative reference. Use the commands in those sections directly — you do NOT need to consult `--help` to confirm their syntax.

```
"<AGENTSCOMMANDER_BINARY_PATH>" --help                  # List all subcommands
"<AGENTSCOMMANDER_BINARY_PATH>" send --help             # Full docs for sending messages
"<AGENTSCOMMANDER_BINARY_PATH>" list-peers --help       # Full docs for discovering peers
```

**RULE:** Only run `--help` if you need a subcommand or flag not documented in the sections below, or if a documented command fails unexpectedly.

## Session credentials

Your session credentials are delivered only through the `AGENTSCOMMANDER_*` environment variables listed above.

Live token refresh without respawn is not supported, because a parent process cannot portably mutate an already-running child process environment. If credential validation fails, restart or respawn the session so AgentsCommander can create a new child process with fresh env values.

Your agent root is your current working directory.

## Inter-Agent Messaging

### Send a message to another agent

**MANDATORY**: Before sending any message, resolve the exact agent name via `list-peers`. Never guess agent names — they follow the format `parent_folder/folder` based on where the agent is triggered.

Messaging is **file-based** to avoid PTY truncation. Two steps:

1. Write your message to a new file in the workgroup messaging directory. The
   directory lives at `<workgroup-root>/messaging/` (walk up from your root
   until you find the parent `wg-<N>-*` folder). Filename must follow the
   pattern `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` (UTC
   timestamp, sanitized kebab-case slug ≤50 chars).
2. Fire the send:

```
"<AGENTSCOMMANDER_BINARY_PATH>" send --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>" --to "<agent_name>" --send <filename> --mode wake
```

**IMPORTANT: `--send` takes the filename ONLY — never a path.**

- BAD:  `--send "C:\...\messaging\20260419-143052-wg3-you-to-wg3-peer-hello.md"`
- GOOD: `--send "20260419-143052-wg3-you-to-wg3-peer-hello.md"`

The CLI resolves the filename against `<workgroup-root>/messaging/` automatically. Passing a path triggers `filename '...' contains path separators or traversal`.

The recipient receives a short notification pointing to your file's absolute
path and reads the content via filesystem. Do NOT use `--get-output` — it
blocks and is only for non-interactive sessions. After sending, stay idle and
wait for the reply.

### List available peers

```
"<AGENTSCOMMANDER_BINARY_PATH>" list-peers --token <AGENTSCOMMANDER_TOKEN> --root "<AGENTSCOMMANDER_ROOT>"
```
"#,
        agent_root = agent_root,
        allowed_places = allowed_places,
        replica_usage = replica_usage,
        matrix_section = matrix_section,
        matrix_allowed = matrix_allowed,
        messaging_exception = messaging_exception,
        messaging_allowed = messaging_allowed,
        forbidden_scope = forbidden_scope,
        git_scope = git_scope,
    )
}

fn is_replica_agent_dir(cwd: &str) -> bool {
    std::path::Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.starts_with("__agent_"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_embeds_filename_only_warning() {
        let out = default_context("C:/tmp/fake-agent", None);
        assert!(out.contains("filename ONLY"));
        assert!(out.contains("BAD:"));
        assert!(out.contains("GOOD:"));
    }

    #[test]
    fn default_context_replica_under_wg_includes_messaging_exception() {
        let out = default_context("C:/fake/wg-7-dev-team/__agent_architect", None);
        assert!(
            out.contains("Narrow exception — workgroup messaging directory"),
            "expected messaging exception header, got:\n{}",
            out
        );
        assert!(
            out.contains("wg-7-dev-team"),
            "expected workgroup name in messaging path, got:\n{}",
            out
        );
        assert!(
            out.contains("- **Allowed (narrow)**: Create canonical inter-agent message files"),
            "expected narrow-allowed bullet, got:\n{}",
            out
        );
    }

    #[test]
    fn default_context_non_workgroup_omits_messaging_exception() {
        let out = default_context("C:/fake/plain/agent", None);
        assert!(
            !out.contains("Narrow exception — workgroup messaging directory"),
            "expected no messaging exception header for non-WG agent, got:\n{}",
            out
        );
        assert!(
            !out.contains("- **Allowed (narrow)**:"),
            "expected no narrow-allowed bullet for non-WG agent, got:\n{}",
            out
        );
    }

    #[test]
    fn default_context_replica_with_matrix_and_messaging_renders_both_sections() {
        let out = default_context(
            "C:/fake/wg-7-dev-team/__agent_architect",
            Some("C:/fake/_agent_architect"),
        );
        assert!(
            out.contains("3. **Your origin Agent Matrix"),
            "matrix section header missing, got:\n{}",
            out
        );
        assert!(
            out.contains("Narrow exception — workgroup messaging directory"),
            "messaging exception header missing, got:\n{}",
            out
        );
        // Composition: matrix bullets immediately followed by exception header
        // (single blank line between, matrix_section ends with \n\n).
        assert!(
            out.contains("- `Role.md`\n\n**Narrow exception"),
            "expected matrix → exception boundary, got:\n{}",
            out
        );
        // Composition: ordering of the three structural markers.
        let exception_pos = out
            .find("Narrow exception")
            .expect("messaging exception must be present");
        let summary_pos = out
            .find("Any repository or directory outside the allowed entries above is READ-ONLY.")
            .expect("summary line must be present");
        let forbidden_pos = out
            .find("- **FORBIDDEN**")
            .expect("forbidden bullet must be present");
        assert!(
            exception_pos < summary_pos,
            "exception must precede summary; exception_pos={exception_pos}, summary_pos={summary_pos}"
        );
        assert!(
            summary_pos < forbidden_pos,
            "summary must precede forbidden bullet; summary_pos={summary_pos}, forbidden_pos={forbidden_pos}"
        );
        // The FORBIDDEN bullet acknowledges the messaging exception by name.
        assert!(
            out.contains("the workspace root (other than the narrow messaging exception above)"),
            "FORBIDDEN bullet missing the messaging-exception qualifier, got:\n{}",
            out
        );
        // Regression guard: the FORBIDDEN bullet must reference "the entries listed above"
        // (R-1.2 / R-1.3 fix). A regression that reverts forbidden_scope to "two zones"
        // would slip past every other assertion in this test.
        assert!(
            out.contains("- **FORBIDDEN**: Any write operation outside the entries listed above"),
            "FORBIDDEN bullet missing 'the entries listed above' prefix, got:\n{}",
            out
        );
    }

    #[test]
    fn default_context_documents_env_only_credentials() {
        let out = default_context("C:/fake/wg-7-dev-team/__agent_architect", None);
        let legacy_header = ["# === Session", "Credentials ==="].join(" ");
        let legacy_compat = ["compatibility", "fallback"].join(" ");
        let legacy_refresh_notice = ["token refresh", "notice"].join(" ");
        let legacy_visible_refresh = ["visible", "refresh"].join(" ");

        assert!(out.contains("AGENTSCOMMANDER_TOKEN"));
        assert!(out.contains("delivered only through"));
        assert!(out.contains("restart or respawn"));
        assert!(!out.contains(&legacy_header));
        let lower = out.to_ascii_lowercase();
        assert!(!lower.contains(&legacy_compat));
        assert!(!lower.contains(&legacy_refresh_notice));
        assert!(!lower.contains(&legacy_visible_refresh));
    }
}
