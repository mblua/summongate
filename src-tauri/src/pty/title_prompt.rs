//! Title-generation prompt builder.
//!
//! Produces the one-shot prompt injected into a Coordinator agent's PTY at
//! spawn (gated by the `auto_generate_brief_title` setting). The agent reads
//! `BRIEF.md` at the absolute path embedded in the prompt and invokes the
//! `brief-set-title` CLI verb to update the title field. The agent NEVER
//! edits `BRIEF.md` directly — the CLI binary writes the file on its
//! behalf, atomically, with a timestamped backup. See plan
//! `_plans/107-auto-brief-title.md` Round 5 §R5.4.2.
//!
//! No I/O. Pure string format. The agent substitutes
//! `<AGENTSCOMMANDER_TOKEN>`, `<AGENTSCOMMANDER_ROOT>`, and
//! `<AGENTSCOMMANDER_BINARY_PATH>` from environment variables first, falling
//! back to the visible `# === Session Credentials ===` block only if env vars
//! are unavailable (Round 4 §R4.2 still provides the compatibility paste).

/// Build the title-generation prompt for an agent whose workgroup's BRIEF.md
/// lives at `brief_absolute_path`.
///
/// The path is interpolated verbatim — caller is responsible for passing an
/// absolute path the agent can resolve, with `\\?\` UNC prefix already
/// stripped (F4 fold, applied at the call-site in `commands/session.rs`).
pub fn build_title_prompt(brief_absolute_path: &str) -> String {
    format!(
        concat!(
            "[AgentsCommander auto-title] The workgroup brief lives at `{path}` ",
            "and has no `title:` field. Read the brief and pick a short summary title ",
            "(8 words or fewer, single line, no trailing period), then set it by running:\n\n",
            "  \"<AGENTSCOMMANDER_BINARY_PATH>\" brief-set-title --token <AGENTSCOMMANDER_TOKEN> --root \"<AGENTSCOMMANDER_ROOT>\" --title \"<your title>\"\n\n",
            "`<AGENTSCOMMANDER_BINARY_PATH>`, `<AGENTSCOMMANDER_TOKEN>`, and ",
            "`<AGENTSCOMMANDER_ROOT>` mean the environment variables of the same names. ",
            "If env vars are unavailable, use `BinaryPath`, `Token`, and `Root` from ",
            "the latest visible `# === Session Credentials ===` fallback block. ",
            "The CLI writes BRIEF.md atomically and ",
            "creates a timestamped `BRIEF.<UTC-ts>.bak.md` backup — do NOT edit ",
            "BRIEF.md directly.\n\n",
            "Skip silently (run nothing) if: the brief is empty, or already has a ",
            "`title:` field. Titles with embedded newlines, NUL, or other control ",
            "characters (except tab) are rejected by the CLI.\n",
        ),
        path = brief_absolute_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_path_and_cli_verb_invocation() {
        let p = build_title_prompt(r"C:\repo\.ac-new\wg-1-foo\BRIEF.md");
        assert!(p.contains(r"C:\repo\.ac-new\wg-1-foo\BRIEF.md"));
        assert!(p.contains("brief-set-title"));
        assert!(p.contains("<AGENTSCOMMANDER_BINARY_PATH>"));
        assert!(p.contains("<AGENTSCOMMANDER_TOKEN>"));
        assert!(p.contains("<AGENTSCOMMANDER_ROOT>"));
        assert!(p.contains("--title \"<your title>\""));
    }

    #[test]
    fn prompt_starts_with_marker() {
        let p = build_title_prompt("/tmp/BRIEF.md");
        assert!(p.starts_with("[AgentsCommander auto-title]"));
    }

    #[test]
    fn prompt_documents_env_first_credentials_with_visible_fallback() {
        let p = build_title_prompt("/tmp/BRIEF.md");
        assert!(p.contains("environment variables"));
        assert!(p.contains("visible"));
        assert!(p.contains("`# === Session Credentials ===`"));
    }

    #[test]
    fn prompt_forbids_direct_brief_edit() {
        let p = build_title_prompt("/tmp/BRIEF.md");
        assert!(p.contains("do NOT edit BRIEF.md directly"));
    }

    #[test]
    fn prompt_documents_skip_conditions() {
        let p = build_title_prompt("/tmp/BRIEF.md");
        assert!(p.contains("Skip silently"));
        assert!(p.contains("brief is empty"));
        assert!(p.contains("`title:` field"));
    }

    #[test]
    fn prompt_handles_path_with_spaces() {
        let p = build_title_prompt(r"C:\Program Files\Stuff\.ac-new\wg-1-x\BRIEF.md");
        assert!(p.contains(r"C:\Program Files\Stuff\.ac-new\wg-1-x\BRIEF.md"));
    }
}
