//! Title-generation prompt builder.
//!
//! Produces the one-shot prompt injected into a Coordinator agent's PTY at
//! spawn (gated by the `auto_generate_brief_title` setting). The agent reads
//! `BRIEF.md` at the absolute path embedded in the prompt and writes a YAML
//! `title:` frontmatter line.
//!
//! No I/O. Pure string format. See plan `_plans/107-auto-brief-title.md`.

/// Build the title-generation prompt for an agent whose workgroup's BRIEF.md
/// lives at `brief_absolute_path`.
///
/// The path is interpolated verbatim — caller is responsible for passing an
/// absolute path the agent can resolve. The prompt instructs the agent to:
///   - read the brief at the given path,
///   - add ONLY a YAML frontmatter `title:` line at the very top,
///   - cap the title at ~8 words,
///   - leave the body untouched.
pub fn build_title_prompt(brief_absolute_path: &str) -> String {
    format!(
        concat!(
            "[AgentsCommander auto-title] Read the workgroup brief at `{path}` ",
            "and add a YAML frontmatter `title:` line at the very top of that file. ",
            "Use a short summary of the brief (ideally 8 words or fewer, no trailing period). ",
            "Format exactly:\n\n",
            "---\n",
            "title: <your short summary>\n",
            "---\n\n",
            "<existing brief body, unchanged>\n\n",
            "Rules: only add the frontmatter — do not modify or reflow any other line. ",
            "If the file is empty, do nothing. ",
            "If the file already starts with `---` and contains a `title:` field, do nothing.\n",
        ),
        path = brief_absolute_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_path_and_format_template() {
        let p = build_title_prompt(r"C:\repo\.ac-new\wg-1-foo\BRIEF.md");
        assert!(p.contains(r"C:\repo\.ac-new\wg-1-foo\BRIEF.md"));
        assert!(p.contains("---\ntitle: <your short summary>\n---"));
        assert!(p.contains("8 words or fewer"));
    }

    #[test]
    fn prompt_starts_with_marker() {
        let p = build_title_prompt("/tmp/BRIEF.md");
        assert!(p.starts_with("[AgentsCommander auto-title]"));
    }

    // R2 fold F5 / G13 — additional path edge cases.

    #[test]
    fn build_title_prompt_handles_path_with_spaces() {
        let p = build_title_prompt(r"C:\Program Files\Stuff\.ac-new\wg-1-x\BRIEF.md");
        assert!(p.contains(r"C:\Program Files\Stuff\.ac-new\wg-1-x\BRIEF.md"));
        assert!(p.contains("---\ntitle: <your short summary>\n---"));
    }

    #[test]
    fn build_title_prompt_handles_path_with_trailing_whitespace() {
        // Path is embedded verbatim — caller's job to normalise. Test just
        // confirms format string doesn't choke on whitespace inside the {path}
        // interpolation.
        let p = build_title_prompt("/tmp/x   /BRIEF.md");
        assert!(p.contains("/tmp/x   /BRIEF.md"));
    }
}
