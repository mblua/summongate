use std::io::ErrorKind;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedAgentFolder {
    pub agent_dir: PathBuf,
    pub display_name: String,
    pub claude_md: String,
}

/// Creates an agent folder with a CLAUDE.md inside it.
///
/// This is the single backend implementation used by both the UI
/// `create_agent_folder` command and the CLI `create-agent` verb.
pub fn create_agent_folder_on_disk(
    parent_path: &str,
    agent_name: &str,
) -> Result<CreatedAgentFolder, String> {
    let parent = PathBuf::from(parent_path);
    if !parent.exists() {
        return Err(format!("Parent folder does not exist: {}", parent_path));
    }

    let agent_name = agent_name.trim();
    if agent_name.is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }
    if agent_name.contains('/') || agent_name.contains('\\') || agent_name.contains('\0') {
        return Err("Agent name cannot contain path separators".to_string());
    }

    let agent_dir = parent.join(agent_name);
    match std::fs::create_dir(&agent_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            return Err(format!("Folder already exists: {}", agent_dir.display()));
        }
        Err(e) => return Err(format!("Failed to create folder: {}", e)),
    }

    let parent_name = parent
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| parent_path.to_string());
    let display_name = format!("{}/{}", parent_name, agent_name);

    let claude_md = format!("You are the agent {}", display_name);
    let claude_path = agent_dir.join("CLAUDE.md");
    std::fs::write(&claude_path, &claude_md)
        .map_err(|e| format!("Failed to write CLAUDE.md: {}", e))?;

    // TODO: When replica creation is added (for __agent_* dirs inside workgroups),
    // write config.json with: { "context": ["$AGENTSCOMMANDER_CONTEXT"] }
    // so that replicas get the global context by default.

    Ok(CreatedAgentFolder {
        agent_dir,
        display_name,
        claude_md,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_folder_and_claude_md_matching_ui_modal() {
        let tmp = tempdir().expect("tempdir");
        let parent = tmp.path().join("ProjectAlpha");
        std::fs::create_dir_all(&parent).expect("parent");
        let parent_s = parent.to_string_lossy().to_string();

        let created = create_agent_folder_on_disk(&parent_s, "architect").expect("created");

        let expected_dir = parent.join("architect");
        assert_eq!(created.agent_dir, expected_dir);
        assert_eq!(created.display_name, "ProjectAlpha/architect");
        assert_eq!(
            created.claude_md,
            "You are the agent ProjectAlpha/architect"
        );
        assert!(expected_dir.is_dir());
        assert_eq!(
            std::fs::read_to_string(expected_dir.join("CLAUDE.md")).expect("claude"),
            "You are the agent ProjectAlpha/architect"
        );
    }

    #[test]
    fn trims_name_before_creating_folder_and_display_name() {
        let tmp = tempdir().expect("tempdir");
        let parent = tmp.path().join("ProjectAlpha");
        std::fs::create_dir_all(&parent).expect("parent");
        let parent_s = parent.to_string_lossy().to_string();

        let created = create_agent_folder_on_disk(&parent_s, " MyAgent ").expect("created");

        let expected_dir = parent.join("MyAgent");
        assert_eq!(created.agent_dir, expected_dir);
        assert_eq!(created.display_name, "ProjectAlpha/MyAgent");
        assert_eq!(created.claude_md, "You are the agent ProjectAlpha/MyAgent");
        assert!(expected_dir.is_dir());
        assert!(!parent.join(" MyAgent ").exists());
    }

    #[test]
    fn errors_when_parent_folder_is_missing() {
        let tmp = tempdir().expect("tempdir");
        let missing = tmp.path().join("missing");
        let missing_s = missing.to_string_lossy().to_string();

        let err = create_agent_folder_on_disk(&missing_s, "architect").expect_err("missing parent");

        assert_eq!(err, format!("Parent folder does not exist: {}", missing_s));
    }

    #[test]
    fn errors_when_agent_name_is_empty_after_trim() {
        let tmp = tempdir().expect("tempdir");
        let parent_s = tmp.path().to_string_lossy().to_string();

        let err = create_agent_folder_on_disk(&parent_s, "   ").expect_err("empty");

        assert_eq!(err, "Agent name cannot be empty");
    }

    #[test]
    fn errors_when_agent_name_contains_path_separator_or_nul() {
        let tmp = tempdir().expect("tempdir");
        let parent_s = tmp.path().to_string_lossy().to_string();

        for name in ["a/b", "a\\b", "a\0b"] {
            let err = create_agent_folder_on_disk(&parent_s, name).expect_err("separator");
            assert_eq!(err, "Agent name cannot contain path separators");
        }
    }

    #[test]
    fn errors_when_agent_folder_already_exists_without_overwriting() {
        let tmp = tempdir().expect("tempdir");
        let parent = tmp.path().join("ProjectAlpha");
        let agent_dir = parent.join("architect");
        std::fs::create_dir_all(&agent_dir).expect("agent dir");
        std::fs::write(agent_dir.join("CLAUDE.md"), "keep me").expect("seed");
        let parent_s = parent.to_string_lossy().to_string();

        let err = create_agent_folder_on_disk(&parent_s, "architect").expect_err("exists");

        assert_eq!(
            err,
            format!("Folder already exists: {}", agent_dir.display())
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("CLAUDE.md")).expect("claude"),
            "keep me"
        );
    }
}
