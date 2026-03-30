use std::path::PathBuf;

/// Returns the path to the global AgentsCommanderContext.md file.
/// Creates it with default content if it doesn't exist yet.
/// This file is static — written once, never modified at runtime.
pub fn ensure_global_context() -> Result<String, String> {
    let config_dir = super::config_dir()
        .ok_or_else(|| "Could not resolve app config directory".to_string())?;
    let file_path = config_dir.join("AgentsCommanderContext.md");

    if !file_path.exists() {
        std::fs::create_dir_all(&config_dir)
            .map_err(|e| format!("Failed to create config dir: {}", e))?;
        std::fs::write(&file_path, DEFAULT_CONTEXT)
            .map_err(|e| format!("Failed to write AgentsCommanderContext.md: {}", e))?;
        log::info!("Created global AgentsCommanderContext.md at {:?}", file_path);
    }

    Ok(file_path.to_string_lossy().to_string())
}

/// Returns the expected path without creating the file.
pub fn global_context_path() -> Option<PathBuf> {
    super::config_dir().map(|d| d.join("AgentsCommanderContext.md"))
}

const DEFAULT_CONTEXT: &str = r#"# AgentsCommander Context

You are running inside an AgentsCommander session — a terminal session manager that coordinates multiple AI agents.

## Session credentials

Your session token and agent root are provided on demand. To request them, output the marker:

```
%%ACRC%%
```

The system will inject a `# === Session Credentials ===` block into your console containing your current token and root. This also happens automatically whenever a `send` command fails due to a stale or missing token.

Your agent root is your current working directory.

## Inter-Agent Messaging

### Send a message to another agent

**MANDATORY**: Before sending any message, resolve the exact agent name via `list-peers`. Never guess agent names — they follow the format `parent_folder/folder` based on where the agent is triggered.

Fire-and-forget (do NOT use --get-output):

```
agentscommander.exe send --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --to "<agent_name>" --message "..." --mode wake
```

The other agent will reply back via your console as a new message.
Do NOT use `--get-output` — it blocks and is only for non-interactive sessions.
After sending, you can stay idle and wait for the reply to arrive.

### List available peers

```
agentscommander.exe list-peers --token <YOUR_TOKEN> --root "<YOUR_ROOT>"
```
"#;
