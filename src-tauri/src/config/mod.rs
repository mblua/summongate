pub mod claude_settings;
pub mod dark_factory;
pub mod profile;
pub mod session_context;
pub mod sessions_persistence;
pub mod settings;

use std::path::PathBuf;

/// Returns the app config directory based on build profile.
/// DEV: `~/.agentscommander-new-dev`
/// PROD/STAGE: `~/.agentscommander` (shared)
pub fn config_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(profile::config_dir_name()))
}
