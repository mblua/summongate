pub mod brief_append_body;
pub mod brief_ops;
pub mod brief_set_title;
pub mod close_session;
pub mod create_agent;
pub mod list_peers;
pub mod list_sessions;
pub mod new_project;
pub mod open_project;
pub mod send;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Agent terminal session manager with inter-agent messaging")]
#[command(after_help = "\
TOKEN: In agent sessions, pass AGENTSCOMMANDER_TOKEN from the environment. \
If the env var is unavailable, use the latest visible '# === Session Credentials ===' fallback block. \
If a token expires, any failed `send` triggers an automatic token refresh.\n\n\
EXIT CODES: All subcommands return 0 on success, 1 on error.\n\n\
AGENT NAMES: Agents are identified by their path-based name (e.g., \"repos/my-project\"). \
Use `list-peers` to discover valid agent names before sending messages.")]
pub struct Cli {
    /// Launch the GUI application
    #[arg(long)]
    pub app: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Send a message to another agent
    Send(send::SendArgs),
    /// List reachable peers (returns JSON array with name, status, role, teams)
    ListPeers(list_peers::ListPeersArgs),
    /// List all sessions in the running app instance (returns JSON)
    ListSessions(list_sessions::ListSessionsArgs),
    /// Create a new agent: folder + CLAUDE.md, optionally launch it
    CreateAgent(create_agent::CreateAgentArgs),
    /// Close all sessions for a target agent (coordinator authorization required)
    CloseSession(close_session::CloseSessionArgs),
    /// Set the title field in the workgroup BRIEF.md frontmatter (coordinator-only)
    BriefSetTitle(brief_set_title::BriefSetTitleArgs),
    /// Append text to the body of the workgroup BRIEF.md (coordinator-only)
    BriefAppendBody(brief_append_body::BriefAppendBodyArgs),
    /// Register an existing AC project (.ac-new must already exist) in settings
    OpenProject(open_project::OpenProjectArgs),
    /// Create an AC project (mkdir .ac-new if missing) and register it in settings
    NewProject(new_project::NewProjectArgs),
}

/// Attach to parent console (or allocate a new one) ONLY if both stdout and stderr
/// have invalid/missing handles. When stdio is already valid (inherited pipes,
/// inherited console handles, or file redirects from the parent), `AttachConsole`
/// would REBIND the std handles to a fresh console buffer — breaking those
/// inherited channels. That rebinding is the root cause of issue #129: PS
/// -NonInteractive `&` direct calls inherit pipe handles to the GUI-subsystem
/// child, and AttachConsole's rebind sends subsequent writes to a console buffer
/// that PS does not surface, dropping all output.
///
/// The condition uses `GetFileType` on the std handles. `FILE_TYPE_UNKNOWN`
/// (returned for null/invalid handles) is the only case where attaching is
/// useful (the user double-clicked the GUI exe in explorer.exe, etc.). For PIPE,
/// CHAR, DISK, REMOTE — the inherited handle is already routable, leave it alone.
#[cfg(target_os = "windows")]
#[allow(clippy::collapsible_if)]
pub fn attach_parent_console() {
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_UNKNOWN};
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        let err = GetStdHandle(STD_ERROR_HANDLE);

        // GetFileType returns FILE_TYPE_UNKNOWN for null/invalid handles. Short-
        // circuit the null check first so GetFileType is never called on null.
        let out_invalid = out.is_null() || GetFileType(out) == FILE_TYPE_UNKNOWN;
        let err_invalid = err.is_null() || GetFileType(err) == FILE_TYPE_UNKNOWN;

        if out_invalid && err_invalid {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                AllocConsole();
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn attach_parent_console() {
    // No-op on non-Windows
}

/// Validate CLI token: must be provided and must be either the root_token or a valid UUID.
/// Returns `Ok((token_string, is_root))` on success, or an error message on failure.
/// `is_root` is true when the token matches the persisted root_token in settings.
pub fn validate_cli_token(token: &Option<String>) -> Result<(String, bool), String> {
    let token = match token {
        Some(t) if !t.is_empty() => t.clone(),
        _ => {
            return Err(
                "Error: --token is required. In agent sessions, pass AGENTSCOMMANDER_TOKEN \
                 from the environment, or use the latest '# === Session Credentials ===' \
                 fallback block if the env var is unavailable."
                    .to_string(),
            );
        }
    };

    // Accept root_token from settings
    let settings = crate::config::settings::load_settings();
    if settings.root_token.as_deref() == Some(&token) {
        return Ok((token, true));
    }

    // Accept master token from persisted file
    if let Some(master_path) = crate::config::config_dir().map(|d| d.join("master-token.txt")) {
        if let Ok(master) = std::fs::read_to_string(&master_path) {
            if master.trim() == token {
                return Ok((token, true));
            }
        }
    }

    // Otherwise must be a valid UUID (all session tokens are UUIDs)
    if uuid::Uuid::parse_str(&token).is_err() {
        return Err(
            "Error: invalid token supplied. Expected a valid session token (UUID) or root token. \
             In agent sessions, use AGENTSCOMMANDER_TOKEN from the environment, or the latest \
             visible credentials fallback block if the env var is unavailable."
                .to_string(),
        );
    }

    Ok((token, false))
}

/// Dispatch CLI subcommands. Returns exit code.
///
/// Caller contract: `attach_parent_console()` MUST be called before this — see
/// `main.rs`. Done there (not here) so the eprintln!s inside `init_logger()`
/// reach the user's terminal on Windows release builds.
pub fn handle_cli(cmd: Commands) -> i32 {
    let code = match cmd {
        Commands::Send(args) => send::execute(args),
        Commands::ListPeers(args) => list_peers::execute(args),
        Commands::ListSessions(args) => list_sessions::execute(args),
        Commands::CreateAgent(args) => create_agent::execute(args),
        Commands::CloseSession(args) => close_session::execute(args),
        Commands::BriefSetTitle(args) => brief_set_title::execute(args),
        Commands::BriefAppendBody(args) => brief_append_body::execute(args),
        Commands::OpenProject(args) => open_project::execute(args),
        Commands::NewProject(args) => new_project::execute(args),
    };

    flush_outputs();
    code
}

/// Flush stdout and stderr. Called before any `std::process::exit` to ensure
/// that pending writes are committed before the process is torn down.
///
/// `std::process::exit` skips destructors, so the default flush-on-drop
/// behavior of `Stdout`/`Stderr` does not run. This helper forces an
/// explicit flush. Errors are silenced — there is nothing meaningful to do
/// with a flush failure at process exit.
pub fn flush_outputs() {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_cli_token_does_not_echo_invalid_input() {
        let supplied = "super-secret-token-with-hidden-garbage";
        let err = validate_cli_token(&Some(supplied.to_string())).unwrap_err();

        assert!(err.contains("invalid token supplied"));
        assert!(!err.contains(supplied));
        assert!(!err.contains(&supplied[..8]));
    }
}
