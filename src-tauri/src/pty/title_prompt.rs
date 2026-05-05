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
//! No I/O. Pure string format. The agent substitutes `<YOUR_TOKEN>`,
//! `<YOUR_ROOT>`, and `<YOUR_BINARY_PATH>` from the `# === Session
//! Credentials ===` block delivered in the same PTY paste (Round 4 §R4.2
//! combined-write design — preserved in Round 5).

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
            "  \"<YOUR_BINARY_PATH>\" brief-set-title --token <YOUR_TOKEN> --root \"<YOUR_ROOT>\" --title \"<your title>\"\n\n",
            "`<YOUR_BINARY_PATH>`, `<YOUR_TOKEN>`, and `<YOUR_ROOT>` are in the ",
            "`# === Session Credentials ===` block immediately above (fields ",
            "`BinaryPath`, `Token`, `Root`). The CLI writes BRIEF.md atomically and ",
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
        assert!(p.contains("<YOUR_BINARY_PATH>"));
        assert!(p.contains("<YOUR_TOKEN>"));
        assert!(p.contains("<YOUR_ROOT>"));
        assert!(p.contains("--title \"<your title>\""));
    }

    #[test]
    fn prompt_starts_with_marker() {
        let p = build_title_prompt("/tmp/BRIEF.md");
        assert!(p.starts_with("[AgentsCommander auto-title]"));
    }

    #[test]
    fn prompt_references_credentials_block_for_substitution() {
        let p = build_title_prompt("/tmp/BRIEF.md");
        assert!(p.contains("`# === Session Credentials ===`"));
        assert!(p.contains("immediately above"));
        assert!(p.contains("`BinaryPath`, `Token`, `Root`"));
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
