# Plan: Issue #186 — Claude Code auto-resume regression with custom `CLAUDE_CONFIG_DIR` wrappers

Branch: `bug/186-claude-config-dir-resume`
Issue: https://github.com/mblua/AgentsCommander/issues/186

---

## 1. Requirement

Restore Claude Code auto-resume for sessions whose agent command is a wrapper script (e.g. `claude-mb.cmd`) that exports a custom `CLAUDE_CONFIG_DIR` before invoking real `claude`. After app restart these sessions must spawn with `claude --continue` exactly as plain `claude` does — provided a prior conversation actually exists for the session's CWD under the wrapper-configured store.

The user-visible symptom: Claude sessions launched via wrappers like `claude-mb` come back as fresh conversations after AC restarts. Codex and Gemini are unaffected because their auto-resume injection has no filesystem-existence gate.

---

## 2. Root cause

`create_session_inner` in `src-tauri/src/commands/session.rs` decides whether to inject `--continue` via `should_inject_continue(...)`. One of its inputs, `claude_project_exists`, is computed at lines **443–453** by probing exactly:

```
<dirs::home_dir()>/.claude/projects/<mangle_cwd_for_claude(cwd)>
```

This path is hard-coded to the default `~/.claude` config root. When the agent command is a wrapper (`claude-mb`, `claude-phi`, …) that sets `CLAUDE_CONFIG_DIR=...` to a different base (e.g. `C:\Users\maria\.claude-mb`), the real Claude project store lives under `<custom-base>/projects/<mangled-cwd>`, which the probe never inspects.

Result:
- For `claude-mb` sessions, `claude_project_exists` is always `false`, so `should_inject_continue` returns `false` and `--continue` is never injected — even when a prior JSONL transcript clearly exists under `C:\Users\maria\.claude-mb\projects\<mangled>`.
- The bug is inert for `claude` (default install) because the probed path matches the actual store.
- It does not affect Codex/Gemini because `inject_codex_resume` / `inject_gemini_resume` are gated solely on `!skip_auto_resume` and a "resume token already present" check — no filesystem precondition.

This regression was introduced (or surfaced) by the issue #82 fix that added the filesystem-existence gate to `should_inject_continue`. Before #82 the bug was masked by the looser injection rule.

A second, parallel bug surfaces once we restore injection: `strip_auto_injected_args` in `src-tauri/src/config/sessions_persistence.rs` (lines **451–478**) detects Claude with strict `eq_ignore_ascii_case("claude")` against `file_stem`. For `claude-mb`, `is_claude` evaluates to `false`, the stripper short-circuits at line **480–482** and returns args unchanged. Auto-injected `--continue` therefore bakes into the saved recipe and self-perpetuates across restarts (issue #82 §rationale), defeating `restart_session(skip_auto_resume=true)`. We must align the persistence detector with the injection detector.

---

## 3. Implementation scope

Two surgical changes, both inside the backend, no IPC/frontend impact:

**Change A — Resolve the real Claude projects dir for wrappers** (auto-inject side).
**Change B — Align the persistence stripper detector with the injection detector** (recipe side).

No new crates. `which = "7"` and `tempfile = "3"` are already declared in `src-tauri/Cargo.toml`.

The frontend, IPC types, Tauri commands, and event signatures are unchanged. `should_inject_continue` keeps its current pure-boolean signature; only the resolver feeding `claude_project_exists` changes.

---

## 4. Affected files and exact changes

### 4.1 `src-tauri/src/commands/session.rs`

#### 4.1.1 Add a new private helper: `resolve_claude_projects_dir`

**Location:** insert immediately above `should_inject_continue` (i.e. between the closing `}` of `inject_codex_resume` at current line **252** and the `///` doc-comment of `should_inject_continue` at current line **254**).

**Body:**

```rust
/// Resolve the directory where Claude Code stores its project transcripts for
/// `cwd`, taking `CLAUDE_CONFIG_DIR` overrides set by `.cmd`/`.bat`/`.ps1`
/// wrapper scripts into account.
///
/// Background: a user can put a wrapper like `claude-mb.cmd` on `%PATH%`:
///
/// ```bat
/// @echo off
/// set CLAUDE_CONFIG_DIR=C:\Users\maria\.claude-mb
/// claude %*
/// ```
///
/// Real Claude then writes project transcripts under
/// `C:\Users\maria\.claude-mb\projects\<mangled-cwd>`, NOT
/// `~/.claude/projects/<mangled-cwd>`. This helper finds the right base.
///
/// Returns `Some(<base>/projects/<mangled-cwd>)` when a Claude-family token
/// exists in the launch command, else `None`. Falls back to `~/.claude/...`
/// whenever the wrapper cannot be resolved or parsed; this preserves the
/// pre-#186 default-install behavior exactly.
fn resolve_claude_projects_dir(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    fn default_base() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude"))
    }

    fn parse_config_dir_from_wrapper(path: &Path) -> Option<PathBuf> {
        // Cap read at 64 KiB; real wrappers are < 1 KiB. Refusing larger
        // files protects against accidentally treating an exe-renamed-as-cmd
        // as a wrapper.
        const MAX: u64 = 64 * 1024;
        let metadata = std::fs::metadata(path).ok()?;
        if metadata.len() > MAX {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        // Strip UTF-8 BOM if present; tolerate non-UTF-8 by lossy decode.
        let text_bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let text = String::from_utf8_lossy(text_bytes);

        for raw_line in text.lines() {
            let line = raw_line.trim_start();
            // `cmd`/`.bat`: `set CLAUDE_CONFIG_DIR=...`
            // `.ps1`:       `$env:CLAUDE_CONFIG_DIR = ...`
            // Bare:         `CLAUDE_CONFIG_DIR=...`
            let after_prefix = if let Some(rest) =
                strip_ascii_prefix_ci(line, "set ")
            {
                rest.trim_start()
            } else if let Some(rest) =
                strip_ascii_prefix_ci(line, "$env:")
            {
                rest.trim_start()
            } else {
                line
            };
            let Some(rest) =
                strip_ascii_prefix_ci(after_prefix, "CLAUDE_CONFIG_DIR")
            else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let value = rest.trim();
            // Strip a single pair of surrounding quotes (single or double).
            let unquoted = if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };
            if unquoted.is_empty() {
                return None;
            }
            return Some(PathBuf::from(unquoted));
        }
        None
    }

    fn strip_ascii_prefix_ci<'a>(haystack: &'a str, needle: &str) -> Option<&'a str> {
        if haystack.len() < needle.len() {
            return None;
        }
        if haystack.as_bytes()[..needle.len()].eq_ignore_ascii_case(needle.as_bytes()) {
            Some(&haystack[needle.len()..])
        } else {
            None
        }
    }

    fn looks_like_wrapper_extension(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let lc = e.to_ascii_lowercase();
                matches!(lc.as_str(), "cmd" | "bat" | "ps1" | "sh")
            })
            .unwrap_or(false)
    }

    fn resolve_token_to_file(token: &str) -> Option<PathBuf> {
        let p = Path::new(token);
        // Direct path (absolute, or relative with separator) — use as-is if
        // it exists. Avoids consulting %PATH% when the user already gave us
        // a full location.
        let has_separator = token.contains('/') || token.contains('\\');
        if has_separator || p.is_absolute() {
            return if p.is_file() { Some(p.to_path_buf()) } else { None };
        }
        // Bare basename — defer to %PATH% + PATHEXT (Windows) via `which`.
        which::which(token).ok()
    }

    // Find the first token whose basename starts with "claude" across
    // shell + shell_args, splitting each arg on whitespace so cmd-wrapped
    // strings ("git pull && claude-mb -x") are also covered.
    let claude_token: Option<String> = {
        let direct = std::iter::once(shell.to_string()).chain(
            shell_args
                .iter()
                .flat_map(|a| a.split_whitespace().map(str::to_string)),
        );
        direct.find(|t| executable_basename(t).starts_with("claude"))
    };
    let claude_token = claude_token?;

    let mangled = crate::session::session::mangle_cwd_for_claude(cwd);

    // Stem == "claude" → no wrapper, default base. This covers `claude`,
    // `claude.exe`, `C:\Tools\claude.cmd` (where the .cmd is the official
    // installer's launcher and writes nothing of its own), etc.
    if executable_basename(&claude_token) == "claude" {
        return default_base().map(|base| base.join("projects").join(&mangled));
    }

    // Non-default name (e.g. `claude-mb`). Try to resolve to an actual file
    // and parse it for a CLAUDE_CONFIG_DIR override.
    if let Some(file) = resolve_token_to_file(&claude_token) {
        if looks_like_wrapper_extension(&file) {
            if let Some(custom_base) = parse_config_dir_from_wrapper(&file) {
                return Some(custom_base.join("projects").join(&mangled));
            }
        }
    }

    // Fall back to default base. Preserves pre-fix behavior whenever the
    // wrapper is missing, unreadable, has no `CLAUDE_CONFIG_DIR` line, or
    // points at a non-text extension.
    default_base().map(|base| base.join("projects").join(&mangled))
}
```

Style notes for the dev:
- Helpers are nested inside the function so we don't pollute the module namespace; mirror the layout used by `build_title_prompt_appendage` (lines 308–336).
- No `unwrap()` outside test code. All errors → `None` → fallback to default base.
- Do not import `regex`. The `strip_ascii_prefix_ci` helper is intentionally tiny.

#### 4.1.2 Replace the `claude_project_exists` block in `create_session_inner`

**Location:** current lines **443–453** (inside `create_session_inner`, just after the `is_claude` / `is_codex` / `is_gemini` detection block).

**Before:**

```rust
    let claude_project_exists = {
        if let Some(home) = dirs::home_dir() {
            let mangled = crate::session::session::mangle_cwd_for_claude(&cwd);
            home.join(".claude")
                .join("projects")
                .join(&mangled)
                .is_dir()
        } else {
            false
        }
    };
```

**After:**

```rust
    let claude_project_exists = resolve_claude_projects_dir(&shell, &shell_args, &cwd)
        .map(|p| p.is_dir())
        .unwrap_or(false);
```

`shell_args` here refers to the (mutable) `shell_args` rebound at line **413** (`let mut shell_args = shell_args;`), which at this point still holds the *configured* args — `--continue` has not been pushed yet. That is what we want: the resolver inspects the user's command, not our injection.

No other changes inside `create_session_inner`. The downstream `if should_inject_continue(...)` block at lines **454–478** stays as-is, including the `executable_basename(&shell) == "cmd"` / "last arg contains claude" branching.

#### 4.1.3 Tests — add to the existing `mod tests` (`#[cfg(test)]`)

Add at the end of the test module (after the `build_title_prompt_appendage_*` tests, current line ~**1925**), before the closing `}` of `mod tests`. Keep the existing `should_inject_continue_*` tests untouched — they remain valid because the public boolean predicate signature did not change.

```rust
    // ── Issue #186 — resolve_claude_projects_dir ──
    use std::path::PathBuf;

    fn write_wrapper(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn resolve_claude_projects_dir_uses_home_for_bare_claude() {
        // Default install → fall back to <home>/.claude/projects/<mangled>.
        let cwd = "C:\\Users\\Test\\repo";
        let resolved = super::resolve_claude_projects_dir("claude", &[], cwd);
        // Skip if the test host has no home dir (CI sandboxes sometimes don't).
        let Some(home) = dirs::home_dir() else { return; };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_uses_home_for_direct_claude_exe_path() {
        // Direct executable path with file_stem == "claude" → still default base.
        let cwd = "C:\\Users\\Test\\repo";
        let resolved =
            super::resolve_claude_projects_dir("C:\\Tools\\claude.exe", &[], cwd);
        let Some(home) = dirs::home_dir() else { return; };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_returns_none_when_no_claude_token() {
        let resolved =
            super::resolve_claude_projects_dir("powershell.exe", &["-NoLogo".to_string()], "C:\\x");
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_claude_projects_dir_parses_wrapper_with_set_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let cwd = "C:\\Users\\Test\\repo";
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            cwd,
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude(cwd));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_strips_quotes_around_value() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR=\"{}\"\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "C:\\x",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_falls_back_when_wrapper_lacks_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            "@echo off\r\nclaude %*\r\n",
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "C:\\x",
        );
        let Some(home) = dirs::home_dir() else { return; };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_falls_back_when_wrapper_missing() {
        let resolved = super::resolve_claude_projects_dir(
            "C:\\definitely\\not\\there\\claude-mb.cmd",
            &[],
            "C:\\x",
        );
        let Some(home) = dirs::home_dir() else { return; };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_finds_claude_token_in_cmd_wrapper_args() {
        // shell=cmd.exe, args=["/K", "<abs path to claude-mb.cmd>", "--effort", "max"]
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            "cmd.exe",
            &[
                "/K".to_string(),
                wrapper.to_str().unwrap().to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ],
            "C:\\x",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_finds_claude_token_in_embedded_cmd_string() {
        // shell=cmd.exe, args=["/K", "git pull && <abs path>\\claude-mb.cmd --effort max"]
        // Embedded form — per-arg whitespace split must surface claude-mb.cmd.
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let combined = format!("git pull && {} --effort max", wrapper.display());
        let resolved = super::resolve_claude_projects_dir(
            "cmd.exe",
            &["/K".to_string(), combined],
            "C:\\x",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_ignores_oversized_wrapper() {
        // Large file (> 64 KiB cap) → treated as not-a-wrapper, fall back.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("claude-mb.cmd");
        let mut body = String::with_capacity(80 * 1024);
        body.push_str("set CLAUDE_CONFIG_DIR=C:\\should-not-be-read\r\n");
        body.push_str(&"x".repeat(80 * 1024));
        std::fs::write(&path, body).unwrap();
        let resolved =
            super::resolve_claude_projects_dir(path.to_str().unwrap(), &[], "C:\\x");
        let Some(home) = dirs::home_dir() else { return; };
        let expected = home
            .join(".claude")
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }
```

The `tempfile` crate is already a `[dev-dependencies]` declarant — verify by grepping `Cargo.toml` if uncertain (search yielded `tempfile = "3"`).

`which::which` lookup of a bare name (e.g. `claude-mb`) is intentionally NOT covered by a unit test: it depends on the host PATH and is environment-sensitive. The two `cmd_wrapper_args` tests above use absolute paths, which exercise the same parsing branch deterministically.

---

### 4.2 `src-tauri/src/config/sessions_persistence.rs` — align Claude detection

#### 4.2.1 Loosen the Claude detector to accept wrapper basenames

**Location:** `strip_auto_injected_args`, current lines **451–459** (the `is_claude` definition).

**Before:**

```rust
    let is_claude = std::iter::once(shell)
        .chain(args.iter().flat_map(|s| s.split_whitespace()))
        .any(|t| {
            std::path::Path::new(t)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(t)
                .eq_ignore_ascii_case("claude")
        });
```

**After:**

```rust
    let is_claude = std::iter::once(shell)
        .chain(args.iter().flat_map(|s| s.split_whitespace()))
        .any(|t| {
            std::path::Path::new(t)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(t)
                .to_ascii_lowercase()
                .starts_with("claude")
        });
```

This mirrors the predicate in `commands/session.rs` line **419** (`b.starts_with("claude")` against a lowercased basename). Without this change, `claude-mb` recipes never enter the stripping branch, so an auto-injected `--continue` would self-perpetuate into the persisted recipe and defeat `restart_session(skip_auto_resume=true)`.

#### 4.2.2 Loosen the per-token Claude position lookups

**Location:** four sites in the same function:

- Line **492** — `is_cmd` branch, top-level args: `executable_basename(arg) == "claude"`.
- Line **523** — `is_cmd` branch, embedded-token rescan: `executable_basename(token) == "claude"`.
- (No corresponding sites in the non-cmd branch at lines **613–620**: it scans by exact token equality on `--continue`/`--append-system-prompt-file` rather than by claude-position; nothing to change there.)

**Before (representative):**

```rust
            if let Some(idx) = result
                .iter()
                .position(|arg| crate::commands::session::executable_basename(arg) == "claude")
            {
                strip_claude_tokens(&mut result, idx + 1);
            }
```

**After (representative — both sites):**

```rust
            if let Some(idx) = result
                .iter()
                .position(|arg| {
                    crate::commands::session::executable_basename(arg).starts_with("claude")
                })
            {
                strip_claude_tokens(&mut result, idx + 1);
            }
```

Apply the same change to the embedded-token rescan at line **523**.

DO NOT change the `codex` (lines **500**, **533**) or `gemini` (lines **508**, **543**, **558**) position lookups. The reported regression is Claude-only; widening the codex/gemini detectors is unnecessary here and out of scope. See §6 for the parallel-work note.

#### 4.2.3 Tests — extend the existing `#[cfg(test)] mod tests`

Add three regression tests at the end of `sessions_persistence.rs`'s test module:

```rust
    #[test]
    fn strip_auto_injected_args_strips_continue_for_wrapper_basename() {
        // claude-mb invoked directly: `--continue` must be stripped from the
        // saved recipe even though the executable's stem is "claude-mb".
        let stripped = strip_auto_injected_args(
            "claude-mb",
            &[
                "--dangerously-skip-permissions".to_string(),
                "--effort".to_string(),
                "max".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_strips_continue_for_cmd_wrapped_basename() {
        // cmd.exe /K claude-mb ... --continue → strip --continue.
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude-mb".to_string(),
                "--effort".to_string(),
                "max".to_string(),
                "--continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/K".to_string(),
                "claude-mb".to_string(),
                "--effort".to_string(),
                "max".to_string(),
            ]
        );
    }

    #[test]
    fn strip_auto_injected_args_strips_continue_for_embedded_cmd_wrapped_basename() {
        // cmd.exe /K "claude-mb --effort max --continue" → strip --continue.
        let stripped = strip_auto_injected_args(
            "cmd.exe",
            &[
                "/K".to_string(),
                "claude-mb --effort max --continue".to_string(),
            ],
        );
        assert_eq!(
            stripped,
            vec![
                "/K".to_string(),
                "claude-mb --effort max".to_string(),
            ]
        );
    }
```

---

### 4.3 No other files change

- `src-tauri/src/session/session.rs` — `mangle_cwd_for_claude` is the sole consumer of `cwd` mangling here; unchanged.
- `src-tauri/src/phone/mailbox.rs` — `wake_spawn_skip_auto_resume` is unaffected; the boolean it returns still feeds `create_session_inner` at the same call site.
- `src-tauri/src/lib.rs` — startup-restore call site unchanged; it already passes `skip_auto_resume = false`, so the new resolver kicks in and finds the right base.
- `src-tauri/src/web/commands.rs` — fresh-create only, `skip_auto_resume = true`; `claude_project_exists` is irrelevant on this path (early-return inside `should_inject_continue`), but the resolver runs harmlessly.
- `src-tauri/Cargo.toml` — no new dependencies. `which = "7"`, `dirs = "6"`, `tempfile = "3"` are all already declared.

The frontend bundle, IPC types, Tauri command list, and event names are not touched. There are no schema changes to `sessions.toml` or `~/.agentscommander/`.

---

## 5. Edge-case matrix

| # | Launch shape | Resolver returns | Outcome |
|---|---|---|---|
| 1 | `shell="claude"`, no args | `<home>/.claude/projects/<mangled>` | Default-install path preserved exactly. |
| 2 | `shell="claude.exe"` (full path), no args | `<home>/.claude/projects/<mangled>` | `file_stem == "claude"` short-circuit; no wrapper read attempted. |
| 3 | `shell="claude-mb"`, no args | `<custom-base>/projects/<mangled>` if `which::which("claude-mb")` finds the wrapper on PATH; else default fallback. | Real fix path. |
| 4 | `shell="C:\\Users\\maria\\bin\\claude-mb.cmd"`, no args | `<custom-base>/projects/<mangled>` from parsed `set CLAUDE_CONFIG_DIR=…`. | Direct-path branch, no PATH lookup. |
| 5 | `shell="cmd.exe"`, args=`["/K", "claude-mb", "--effort", "max"]` | Resolver scans args, finds `claude-mb` token, resolves via PATH. | Common AC settings layout when default_shell=cmd. |
| 6 | `shell="cmd.exe"`, args=`["/K", "git pull && claude-mb --effort max"]` | Per-arg `split_whitespace` surfaces `claude-mb` from inside the quoted compound. | Embedded compound command. |
| 7 | Wrapper with quoted value `set CLAUDE_CONFIG_DIR="C:\\Path With Spaces\\.claude-x"` | Quotes stripped, internal spaces preserved. | Spaced path support. |
| 8 | Wrapper exists but has no `CLAUDE_CONFIG_DIR=` line | Default fallback. | Transparent passthrough wrappers (e.g. just `claude %*`). |
| 9 | Wrapper file missing / unreadable / >64 KiB | Default fallback. | Defensive against stale settings, perm errors, and binaries-renamed-as-cmd. |
| 10 | Bare `claude.cmd` on PATH that DOES override `CLAUDE_CONFIG_DIR` | Default fallback (because `file_stem == "claude"` triggers the early-return). | Acceptable: this is an exotic setup that would silently work today only by coincidence; not a regression. Documented in `should_inject_continue` doc-comment as out of scope. |
| 11 | Token name starts with `claude` but is not actually a Claude binary (`claudication.exe`?) | Treated as Claude. | Same risk surface as today's `is_claude = b.starts_with("claude")` at session.rs line **419**; consistent. Not introduced by this change. |
| 12 | `home_dir()` returns `None` (rare; corrupted env) | Resolver returns `None` for the default-fallback branches; `claude_project_exists` becomes `false`. | Pre-existing behavior preserved; no `--continue` injection. |
| 13 | Wrapper with mixed `\r\n` / `\n` line endings, or UTF-8 BOM | `lines()` handles both; explicit BOM strip. | Windows editor artifacts tolerated. |
| 14 | PowerShell `.ps1` wrapper using `$env:CLAUDE_CONFIG_DIR = "..."` | `strip_ascii_prefix_ci("$env:")` then standard `=`-split. | Covered by `looks_like_wrapper_extension` accepting `.ps1`. |

---

## 6. Notes, constraints, and out-of-scope follow-ups

### 6.1 Constraints — things the dev MUST NOT do

- **Do not alter** the signature of `should_inject_continue` (`commands/session.rs` line **271**). The existing pure-boolean tests are the regression fence for issue #82 and must keep passing unchanged.
- **Do not alter** `is_claude = cmd_basenames.iter().any(|b| b.starts_with("claude"))` in `create_session_inner` (line **419**). It already accepts `claude-mb`.
- **Do not change** the `cmd /K` injection branch in `create_session_inner` (lines **460–478**). Its `last.to_lowercase().contains("claude")` substring fallback already handles wrapper basenames.
- **Do not** widen `is_codex` / `is_gemini` detectors in `sessions_persistence.rs` in this plan. They are outside the reported regression scope.
- **Do not** invoke any subprocess (`where`, `cmd /C ...`) to resolve PATH. Use `which::which`. We are inside an async hot path during session creation.
- **Do not** add `regex`, `lazy_static`, `once_cell`, or any other crate. The wrapper-grep is small and finite.
- **Do not** read more than 64 KiB of a candidate wrapper. A wrong file (e.g. an `.exe` mis-extensioned) must NOT be slurped wholesale.
- **Do not** persist the resolved CLAUDE_CONFIG_DIR in `sessions.toml` or any other on-disk struct. The resolver is recomputed on every `create_session_inner` call so users can edit/move wrappers between launches.
- **Do not** bake the resolver into `mangle_cwd_for_claude`. The mangle function is shared with the JSONL watcher (see §6.3); adding wrapper resolution there would be a separate, larger change.

### 6.2 Logging

Inside `resolve_claude_projects_dir`, do NOT log on every call (this runs on every session create). The `log::info!` at line **473** of `create_session_inner` already announces successful injection; that is sufficient observability for the success path. The fallback path is silent on purpose to avoid log noise for default-install users.

If a future ticket wants visibility into wrapper detection, add a `log::debug!` (not `info`) inside the wrapper-parsed branch — explicitly out of scope here.

### 6.3 Out-of-scope: `telegram/jsonl_watcher.rs` has the same root cause

`watch_loop` at `src-tauri/src/telegram/jsonl_watcher.rs` lines **191–202** hardcodes `home/.claude/projects/<mangled>` to locate Claude's JSONL transcripts for the Telegram bridge. Wrapper users with custom `CLAUDE_CONFIG_DIR` will see the bridge dormant for the same reason auto-resume is broken today.

**Architect call:** This is a parallel bug. Including it in #186 widens scope beyond what the brief asked for ("auto-resume regression"). The dev should:
1. Implement #186 as specified above.
2. After landing, file a follow-up ticket "Telegram JSONL watcher does not honor `CLAUDE_CONFIG_DIR` wrappers" and reuse `resolve_claude_projects_dir` (which we keep as `pub(crate)` so the watcher can call it).

To make that follow-up trivial, declare the new helper visibility as `pub(crate) fn resolve_claude_projects_dir(...)` rather than private `fn`. (Tag note for the dev: this is the only intentional non-private addition.)

### 6.4 Out-of-scope: parallel codex/gemini wrappers

If a future user ships `codex-foo` or `gemini-foo` wrappers, the persistence stripper will skip their `resume --last` / `--resume latest` for the same `eq_ignore_ascii_case` reason. We are NOT fixing that here. Reasons:
- No reported regression.
- Codex/Gemini do not depend on a filesystem-existence gate, so their auto-resume *injection* still fires; only the *recipe stripping* is broken, and it's silent until the user actually wraps the binary.
- Symmetry repair for codex/gemini belongs in its own ticket.

### 6.5 Phase order

Per Role.md:

- **MVP** — §4.1.1 + §4.1.2 + §4.2.1 + §4.2.2. With these in, default-install Claude users keep working and `claude-mb` users see auto-resume restored without recipe self-perpetuation.
- **Full features** — §4.1.3 (resolver tests) + §4.2.3 (persistence stripper tests). Required before review can sign off.
- **Polish** — Update the `should_inject_continue` doc-comment (lines **254–270**) to mention "(callers should compute `claude_project_exists` via `resolve_claude_projects_dir` to honor wrapper-set `CLAUDE_CONFIG_DIR`)". Single-sentence addition.
- **Extras** — None in this ticket. Telegram watcher fix → follow-up issue per §6.3.

### 6.6 Verification (manual smoke)

After implementation, the dev should confirm on a wrapper setup matching the user's:

1. `agent.command = "claude-mb --dangerously-skip-permissions --effort max"` in settings.
2. `C:\Users\maria\bin\claude-mb.cmd` exists with the canonical `set CLAUDE_CONFIG_DIR=C:\Users\maria\.claude-mb` body (already present on the user's machine).
3. With a session whose CWD has a populated `C:\Users\maria\.claude-mb\projects\<mangled>\` directory: AC restart should respawn the session with `--continue` appended (visible in logs as `Auto-injected --continue for agent '<id>' (prior conversation exists)`).
4. With a fresh CWD (no `<mangled>` dir under either `~/.claude/projects` or the custom base): AC restart should NOT inject `--continue`.
5. With a `claude` (default) agent in a separate workgroup: behavior unchanged from before — regression fence for non-wrapper users.
6. After several restart cycles, `sessions.toml` must NOT accumulate `--continue` in the `shell_args` of the wrapper-launched session.

Per Role.md the dev does NOT push or merge anything; verification stays on the local branch.

---

## 7. Architect verdict

**PENDING_REVIEW** — first-pass plan as requested in the brief (§5 of the tech-lead's message: "for this first pass, create the plan only"). Does not yet carry `READY_FOR_IMPLEMENTATION`. Submit to dev/grinch review rounds.

Reviewers should focus on:
- Correctness of the wrapper-parser (BOM, line endings, quoted values, prefix-strip casing).
- Whether `pub(crate)` exposure of `resolve_claude_projects_dir` is acceptable or should stay private and the JSONL watcher follow-up paid for separately.
- Whether the persistence-stripper alignment in §4.2 should expand to codex/gemini in this same ticket or be deferred per §6.4.
- Coverage of the new test set vs. the existing `should_inject_continue` regression fence.

---

## 9. Grinch review round 1

Verdict: **CHANGES REQUESTED — 2 blocking, 9 non-blocking.**

The plan correctly diagnoses both bugs (resolver-side false negative, stripper-side strict-eq), and the proposed structural changes (new resolver helper, broadened `is_claude` predicate in persistence) are sound. Scope discipline (§6.1, §6.4) is good.

The blockers below are about the wrapper PARSER. As written, the parser will silently fail on the two most common idioms used in real-world `.cmd` wrappers (cmd-quoted assignment, `%VAR%` expansion). Without these, the fix lands but the user still sees the regression on any wrapper they did not hand-write with literal absolute paths. The non-blockers are mostly clarifications and edge cases.

Verified against current source:
- `executable_basename` at `commands/session.rs:1160` lowercases AND strips extension. Persistence detector loosening in §4.2.1 is semantically equivalent to the injection-side detector at `commands/session.rs:419`.
- `mangle_cwd_for_claude` at `session/session.rs:11` is a pure char-substitution; OK to call twice.
- `dirs = "6"`, `which = "7"`, `tempfile = "3"` (dev) all present in `src-tauri/Cargo.toml`.
- `lib.rs:635` startup-restore passes `skip_auto_resume = false`, confirming this is the path the user's regression rides.
- Non-cmd branch at `sessions_persistence.rs:565+` strips by exact-token equality of `--continue`/`--append-system-prompt-file`; only gated on `is_claude`, so §4.2.1 alone is sufficient there. Plan is right that no per-token widening is needed in the non-cmd branch.
- `jsonl_watcher.rs:191-195` confirmed hardcoded to `home/.claude/projects/<mangled>` — same root cause, deferral per §6.3 is justified scope.

### Blocking

#### B1. Cmd-quoted `set "VAR=value"` is not parsed

**What.** After `strip_ascii_prefix_ci(line, "set ")` and `trim_start`, the parser expects `CLAUDE_CONFIG_DIR` as the next bytes. A wrapper line like

```bat
set "CLAUDE_CONFIG_DIR=C:\Path with spaces\.claude-mb"
```

has `"` at byte 0 after the `set ` strip. `strip_ascii_prefix_ci(after_prefix, "CLAUDE_CONFIG_DIR")` fails, and `parse_config_dir_from_wrapper` returns `None`. The resolver falls back to the default base.

**Why.** This is the canonical cmd idiom for any environment variable whose value contains spaces, parentheses, or other delimiter chars. Anyone who copied a wrapper template from Stack Overflow or pulled from `dotfiles` repos uses this form. The user's local repro happens to use a literal absolute path without spaces, but that is a coincidence. The bug surfaces the moment a different user tries the same fix on a wrapper authored idiomatically.

**Fix.** After the `set `/`$env:` prefix strip and `trim_start`, also strip a single leading `"`. After unquoting the value, account for the matching trailing `"` that now closes a `"VAR=value"` rather than `VAR="value"`. Sketch:

```rust
let mut after_prefix = /* ... existing ... */;
// New: support cmd's `set "VAR=value"` form.
let cmd_quoted = after_prefix.starts_with('"');
if cmd_quoted {
    after_prefix = &after_prefix[1..];
}
// ... existing CLAUDE_CONFIG_DIR strip + `=` strip ...
let value = rest.trim();
let unquoted = if cmd_quoted && value.ends_with('"') {
    &value[..value.len() - 1]
} else if /* existing single-pair-of-quotes branch */ { ... } else { value };
```

Add a regression test mirroring `resolve_claude_projects_dir_strips_quotes_around_value` but with the quote OUTSIDE the `=`:

```rust
"@echo off\r\nset \"CLAUDE_CONFIG_DIR=<custom>\"\r\nclaude %*\r\n"
```

#### B2. Environment-variable references in the value are not expanded

**What.** Wrappers commonly write the path with an env-var prefix:

```bat
set CLAUDE_CONFIG_DIR=%USERPROFILE%\.claude-mb
```
or PowerShell:
```ps1
$env:CLAUDE_CONFIG_DIR = "$env:USERPROFILE\.claude-mb"
```

The parser produces `PathBuf::from("%USERPROFILE%\.claude-mb")` (or `$env:USERPROFILE\.claude-mb`). `is_dir()` on that literal returns false, `claude_project_exists` is false, and `--continue` is not injected. The regression surfaces unchanged.

**Why.** Any portable wrapper a user shares between machines uses an env-var prefix (otherwise the wrapper hard-codes someone else's home dir). The parser's literal preservation defeats that idiom entirely.

**Fix.** After unquoting the value, do a single-pass expansion of `%NAME%` (cmd-style) and `$env:NAME` (PowerShell-style) against `std::env::var`. Cover at least `USERPROFILE`, `LOCALAPPDATA`, `APPDATA`, `HOMEDRIVE`+`HOMEPATH`, `USERNAME`. Unknown vars → leave literal (so the resulting `is_dir()` is false rather than panicking). Sketch:

```rust
fn expand_env(s: &str) -> String {
    // Pass 1: %NAME% (cmd).
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => { out.push('%'); out.push_str(name); out.push('%'); }
                }
                rest = &after[end + 1..];
            }
            None => { out.push('%'); out.push_str(after); rest = ""; break; }
        }
    }
    out.push_str(rest);
    // Pass 2: $env:NAME (PowerShell).
    // ... similar, terminator = first non-[A-Za-z0-9_] char ...
}
```

Tests: set a temp env var with `std::env::set_var("AC_TEST_BASE", ...)`, write a wrapper that references `%AC_TEST_BASE%`, assert resolver returns the expanded path. `remove_var` after. **Use a unique var name per test** to avoid collisions if `cargo test` runs in parallel — env-var state is process-global. (Alternatively, gate these tests behind a `mutex` guard, but unique names are simpler.)

### Non-blocking

#### N1. `.sh` wrappers with `export VAR=value` are silently skipped

**What.** `looks_like_wrapper_extension` accepts `sh`, but the prefix-strip chain only knows `set `, `$env:`, and bare `VAR=`. The standard sh idiom `export CLAUDE_CONFIG_DIR=...` matches none of these, so the file is read but no value is extracted.

**Why.** Users running AC under Git Bash / MSYS2 with an `.sh` wrapper see the same regression unfixed. Less common on Windows but not exotic.

**Fix.** Add `strip_ascii_prefix_ci("export ")` to the same prefix chain, OR drop `"sh"` from `looks_like_wrapper_extension` and document the limitation in §6.1. The first option is two lines.

#### N2. `cmd /K` injection branch gap when claude-mb is not the last arg

**What.** §6.1 says: *"Do not change the cmd /K injection branch in `create_session_inner` (lines 460–478). Its `last.to_lowercase().contains("claude")` substring fallback already handles wrapper basenames."* This is true only when the LAST arg is a claude-prefix token. For

```
shell="cmd.exe"  shell_args=["/K", "claude-mb", "--effort", "max"]
```

`last_mut()` yields `"max"`, neither `executable_basename(...) == "claude"` nor `contains("claude")` matches, and `--continue` is never appended.

**Why.** For the user's specific bug, `resolve_agent_command` produces `shell="claude-mb"` directly (verified at `commands/session.rs:1184`), so the cmd branch is not entered and this gap is not exercised. Confirmed at `lib.rs:635` startup-restore (`ps.shell` is the persisted `"claude-mb"`). So the MVP fix solidly addresses the reported regression. The gap matters only if a future user manually configures `default_shell="cmd.exe"` with claude-mb in args, which is a separate, pre-existing class of bug.

**Fix.** Either (a) tighten §6.1 to say *"the cmd-branch substring fallback covers only the case where the last arg is or contains a claude-prefix token; this is sufficient for sessions whose `shell` is the wrapper directly, which is the user's reported configuration"*, or (b) extend the cmd-branch to scan ALL args for a claude-prefix token, not just `last_mut()`. (a) is what I'd ship in this ticket; (b) is a separate enhancement.

#### N3. Function-header visibility inconsistency between §4.1.1 and §6.3

**What.** The function body in §4.1.1 reads `fn resolve_claude_projects_dir(...)`, but §6.3 explicitly mandates `pub(crate)`. A dev who copies §4.1.1 verbatim ships private visibility.

**Fix.** Update the body in §4.1.1 to `pub(crate) fn resolve_claude_projects_dir(...)`. Trivial, but easy to miss when lifting code blocks.

#### N4. Compound-command first-token bias

**What.** The resolver returns the FIRST token whose basename starts with `claude` and short-circuits. For `cmd /K "claude && claude-mb"`, vanilla `claude` matches first → resolver returns the default base, even though the wrapper's CLAUDE_CONFIG_DIR is what the second invocation will use. Acknowledged in §5 row 11 as same surface as the existing line-419 detector.

**Fix.** Optional. If addressed: prefer the first non-vanilla `claude*` token; fall back to vanilla. ~5-line change. Otherwise leave as-is — the case is contrived.

#### N5. Bare-name PATH lookup is silent on miss

**What.** `which::which("claude-mb")` returns `None` when the wrapper is in a directory not on `%PATH%`. `resolve_token_to_file` then returns `None`, and the resolver falls back to the default base — no log, no diagnostic.

**Fix.** §6.6 step 3 should explicitly verify `where claude-mb` returns the wrapper path before testing. Optionally add a `log::debug!` (NOT info, per §6.2) inside `resolve_claude_projects_dir` covering both branches:

```rust
log::debug!(
    "[claude-resume] wrapper-resolved CLAUDE_CONFIG_DIR for token '{}' → {:?}",
    claude_token, resolved
);
```

#### N6. Relative paths in `CLAUDE_CONFIG_DIR` are resolved against AC's CWD, not the wrapper's

**What.** `set CLAUDE_CONFIG_DIR=.claude-mb` (relative) yields `PathBuf::from(".claude-mb")`. `is_dir()` resolves relative to AC's process CWD, not the wrapper's parent. Almost always false-negative.

**Fix.** Either resolve relative paths against the wrapper's parent dir, or document that `CLAUDE_CONFIG_DIR` must be absolute. Documentation is sufficient.

#### N7. Persistence-stripper widening shares the existing detector's false-positive surface

**What.** §4.2.1 changes `eq_ignore_ascii_case("claude")` to `to_ascii_lowercase().starts_with("claude")`. Symmetric with the line-419 detector, but the impact differs: persistence stripping is destructive. If a user has a `claude-derived-tool` whose CLI happens to also accept `--continue` and `--append-system-prompt-file <path>`, the stripper now silently removes those tokens from the saved recipe. Pre-fix, the strict `eq` left them alone.

**Fix.** None required — same trade-off as today's injection-side detector. Mention in §5 row 11 that the persistence-stripper widening shares the same false-positive vector.

#### N8. `--continue=<value>` mismatch between inject-side gate and strip-side detector (pre-existing)

**What.** `should_inject_continue` short-circuits when full_cmd contains `--continue=...` (line 282 `lower.starts_with("--continue=")`), but `strip_claude_tokens` (line 401-418) only matches exact-equality `--continue` — it does NOT strip `--continue=<value>`. Pre-existing, not introduced by this ticket. Worth flagging only because the loosened detector now identifies `claude-mb` as Claude in the persistence path; if AC ever auto-injects `--continue=...` in the future, the stripper would skip it.

**Fix.** None for #186. File a separate symmetry ticket if you want it.

#### N9. Blocking I/O on tokio runtime

**What.** `resolve_claude_projects_dir` does sync `std::fs::metadata`, `std::fs::read`, and `which::which` — all blocking. Runs on a tokio runtime thread inside `create_session_inner` (async).

**Why.** The current code at lines 443-453 already does blocking `is_dir()`. `which::which` is materially slower (cold-PATH stat() over many candidates), so the new helper is a small worst-case regression on cold startup. Acceptable: still single-digit ms, and `create_session_inner` is already non-trivially blocking elsewhere (`discover_teams`, `materialize_agent_context_file`). No fix needed.

**Fix.** None. If perf matters in a future ticket, `tokio::task::spawn_blocking` the helper.

### Recommendation

Fix B1 + B2 in this round. They are small (one helper each, plus a couple tests). Without them the plan ships but the user's actual fix breadth is much narrower than implied.

Sweep N3 (one-character visibility fix). The other Ns are non-blocking notes — handle them in §6.1/§6.6 doc text or defer.

After B1, B2, N3 are addressed, the plan should land at `READY_FOR_IMPLEMENTATION`.

— grinch

---

## 8. Dev-rust review round 1

**Reviewer:** dev-rust (wg-11-dev-team)
**Branch verified against:** `bug/186-claude-config-dir-resume` @ `c35fc55` (last commit before the plan).
**Verdict:** **APPROVED — ready for implementation with the enrichments below.** No blocking issues. The plan compiles in my head, all cited line numbers match the working tree, and every dependency the plan relies on is already declared.

### 8.1 Verification of cited references

Confirmed against the current branch (file/line/code all match the plan):

| Plan citation | Actual location | Status |
|---|---|---|
| `commands/session.rs` line 252 (closing `}` of `inject_codex_resume`) | line 252 | ✓ |
| `commands/session.rs` line 254 (`should_inject_continue` doc start) | line 254 | ✓ |
| `commands/session.rs` line 271 (`should_inject_continue` signature) | line 271 | ✓ |
| `commands/session.rs` line 308–336 (`build_title_prompt_appendage`) | line 308–336 | ✓ |
| `commands/session.rs` line 413 (`let mut shell_args = shell_args;`) | line 413 | ✓ |
| `commands/session.rs` line 419 (`is_claude = ... b.starts_with("claude")`) | line 419 | ✓ |
| `commands/session.rs` lines 443–453 (`claude_project_exists` block) | lines 443–453 | ✓ exact match |
| `commands/session.rs` lines 454–478 (`should_inject_continue` invocation + cmd branch) | lines 454–478 | ✓ |
| `commands/session.rs` test mod opens at line 1925 ish | `#[cfg(test)] mod tests` opens at line 1447, closing `}` at line 1926 | ✓ insertion point is BEFORE line 1926 |
| `commands/session.rs::executable_basename` | line 1160, `pub(crate)`, returns **lowercased** stem (note this for §8.3) | ✓ |
| `config/sessions_persistence.rs` lines 451–459 (`is_claude` definition) | lines 451–459 | ✓ exact match |
| `config/sessions_persistence.rs` line 480–482 (early return) | lines 480–482 | ✓ |
| `config/sessions_persistence.rs` line 484 (`is_cmd`) | line 484 | ✓ |
| `config/sessions_persistence.rs` line 492 (claude position-find, top-level args) | line 492 | ✓ |
| `config/sessions_persistence.rs` line 523 (claude position-find, embedded rescan) | line 523 | ✓ |
| `config/sessions_persistence.rs` lines 613–620 (else-branch: token-equality on `--continue`) | lines 614–620 | ✓ — confirms direct `claude-mb --continue` is handled by `is_claude` change alone (no position-finder change needed in this branch) |
| `config/sessions_persistence.rs` test mod | opens at line 646–647; ends at line 896 | ✓ — but see §8.3.3 for placement nit |
| `session/session.rs` `mangle_cwd_for_claude` | line 11, `pub fn` | ✓ |
| `telegram/jsonl_watcher.rs` lines 191–202 | lines 191–202 | ✓ exact match — same root cause confirmed for the follow-up ticket |
| `Cargo.toml` `which = "7"` | line 32, `[dependencies]` | ✓ — usable from non-test code |
| `Cargo.toml` `dirs = "6"` | line 19, `[dependencies]` | ✓ |
| `Cargo.toml` `tempfile = "3"` | line 47, `[dev-dependencies]` | ✓ — usable only inside `#[cfg(test)]`, which matches plan's placement |

No other deps need adding. The plan's "no new crates" claim holds.

### 8.2 Required enrichments — apply these BEFORE implementation

These are not optional; the plan as written has minor gaps that will surface either at compile time, at review time, or in real-world wrapper inputs.

#### 8.2.1 Resolve the visibility inconsistency — make the helper `pub(crate)`

The plan has two contradictory specs for `resolve_claude_projects_dir`'s visibility:

- §4.1.1 declares `fn resolve_claude_projects_dir(...)` (private).
- §6.3 mandates `pub(crate) fn resolve_claude_projects_dir(...)` so the JSONL-watcher follow-up can reuse it.

§6.3 wins. The implementer must declare it as:

```rust
pub(crate) fn resolve_claude_projects_dir(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> Option<std::path::PathBuf> {
```

No risk to current callers; `pub(crate)` is invisible outside the crate. This avoids a post-merge follow-up PR that just changes a single keyword.

#### 8.2.2 Handle `export ` prefix in the wrapper parser

The plan's `looks_like_wrapper_extension` accepts `cmd | bat | ps1 | sh`. But the parser in `parse_config_dir_from_wrapper` only strips `set ` and `$env:` prefixes. POSIX shell wrappers conventionally write `export CLAUDE_CONFIG_DIR=...`, which the current parser silently skips (the line doesn't begin with `set`/`$env:`/`CLAUDE_CONFIG_DIR`).

Fix: add a third prefix-strip branch.

**Replace:**

```rust
let after_prefix = if let Some(rest) =
    strip_ascii_prefix_ci(line, "set ")
{
    rest.trim_start()
} else if let Some(rest) =
    strip_ascii_prefix_ci(line, "$env:")
{
    rest.trim_start()
} else {
    line
};
```

**With:**

```rust
let after_prefix = if let Some(rest) =
    strip_ascii_prefix_ci(line, "set ")
{
    rest.trim_start()
} else if let Some(rest) =
    strip_ascii_prefix_ci(line, "$env:")
{
    rest.trim_start()
} else if let Some(rest) =
    strip_ascii_prefix_ci(line, "export ")
{
    rest.trim_start()
} else {
    line
};
```

Add a corresponding test:

```rust
    #[test]
    fn resolve_claude_projects_dir_parses_wrapper_with_export_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.sh",
            &format!(
                "#!/usr/bin/env bash\nexport CLAUDE_CONFIG_DIR={}\nexec claude \"$@\"\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "/home/test/repo",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("/home/test/repo"));
        assert_eq!(resolved, Some(expected));
    }
```

This is cheap and closes a real gap: cross-platform parity with the matrix entry that already advertises `.sh` support.

#### 8.2.3 Fix the placement of the new persistence tests

§4.2.3 says "add three regression tests at the end of `sessions_persistence.rs`'s test module." But the test module's actual end currently contains the unrelated `legacy_migration_*` tests (line 813 onward). Inserting at the literal end would scatter Claude-stripper tests across two non-contiguous regions.

**Insert the three new tests AFTER `strip_auto_injected_args_leaves_unrelated_commands_unchanged` (line 810) and BEFORE the `/// Validation #17:` doc-comment that opens `legacy_migration_single_repo_shape` (line 812).** That keeps all `strip_auto_injected_args_*` tests contiguous, matching the existing layout.

#### 8.2.4 Hoist the `use std::path::PathBuf;` to the top of `mod tests`

§4.1.3 introduces `use std::path::PathBuf;` mid-test-module via the `// ── Issue #186 ──` divider. That parses, but it's an outlier in this file — the existing `mod tests` keeps imports at the top (line 1449). Move the `use std::path::PathBuf;` next to the existing `use super::{inject_codex_resume, resolve_actual_agent, should_inject_continue};` so future `cargo fmt` and review noise stays minimal. (No need to add `use tempfile;` — bare `tempfile::tempdir()` is the convention used elsewhere in the workspace.)

#### 8.2.5 Loosen the "embedded cmd string" test against tempdir paths with spaces

The proposed test `resolve_claude_projects_dir_finds_claude_token_in_embedded_cmd_string` builds the embedded command via `format!("git pull && {} --effort max", wrapper.display())` and relies on `split_whitespace` finding `claude-mb.cmd` as a single token. On hosts where `tempfile::tempdir()` yields a path with spaces (macOS user dirs containing spaces, custom `TMPDIR`), the split fragments the path and the resolver finds nothing.

Two acceptable fixes — implementer's choice:

- (a) **Override TMPDIR for this test**: `let tmp = tempfile::Builder::new().prefix("ac186-").tempdir_in(std::env::temp_dir()).unwrap();` and additionally assert `!tmp.path().to_string_lossy().contains(' ')` with an early `return` if the assumption breaks (skip rather than fail on hostile environments — same pattern as the existing `let Some(home) = dirs::home_dir() else { return; };` skips).
- (b) **Replace the embedded form with a direct-path token in this specific test**, since the "compound command containing spaces" path is already exercised by parsing logic; the embedded-token branch is what the test is validating, and that works as long as the token itself doesn't contain whitespace.

I lean (a): preserve the test's intent (whitespace-split surfaces the wrapper) but document the temp-path assumption.

### 8.3 Optional refinements — nice-to-have, low cost

#### 8.3.1 Inline-comment for the `executable_basename` lowercase contract

The resolver's `if executable_basename(&claude_token) == "claude"` early-return is correct *because* `executable_basename` returns a lowercased stem (see line 1165, `.to_lowercase()`). A future refactor that changes that contract would silently break this comparison. Add a one-line `// executable_basename returns a lowercased stem; comparison is case-insensitive by construction.` next to the `==` check. Single-line comment, no behavior change.

#### 8.3.2 The "mirror layout" rationale in §4.1.1 is slightly inaccurate

§4.1.1's style note says "mirror the layout used by `build_title_prompt_appendage` (lines 308–336)". That function does not actually nest helpers — it calls free `crate::*` paths. The nested-helper approach in the plan is fine on its own merits (encapsulation, no namespace pollution), but the comparison is the wrong precedent. Suggest the implementer drop the comparison from any commit message or doc-comment and just present the nesting on its own merits.

#### 8.3.3 Architect's polish in §6.5 is fine as-is — but spell out matrix #10

§6.5 polish says "Update the `should_inject_continue` doc-comment (lines 254–270) to mention `(callers should compute claude_project_exists via resolve_claude_projects_dir to honor wrapper-set CLAUDE_CONFIG_DIR)`". Recommend the implementer ALSO append a single sentence on matrix entry #10:

> Note: a wrapper named exactly `claude.cmd` (or `claude.exe`, `claude.ps1`) that overrides `CLAUDE_CONFIG_DIR` is intentionally NOT honored — the resolver short-circuits when the file_stem equals `claude`. Users who need wrapper overrides should rename to `claude-<suffix>` (e.g. `claude-mb`).

This documents the only deliberate semantic limitation introduced by this fix, in the place a future maintainer will look.

### 8.4 Direct answers to the architect's review-focus questions (§7)

> **Wrapper-parser correctness (BOM, line endings, quoted values, prefix-strip casing):**

OK with one gap — `export ` is not handled despite `.sh` being in the wrapper-extension allowlist. Fix per §8.2.2. Everything else (BOM strip, `\r\n` tolerance via `lines()`, quoted-value unwrap, `eq_ignore_ascii_case` casing, 64 KiB cap) is correct.

Two known limitations of the line-based grep, both acceptable for real-world wrappers and worth a sentence in the helper's doc-comment:

- A line like `set CLAUDE_CONFIG_DIR=foo & next_cmd` will incorrectly include `& next_cmd` in the parsed value (cmd's `&` chaining). The user's actual wrapper is single-statement, so this isn't a regression for the reported case.
- A PowerShell line like `$env:CLAUDE_CONFIG_DIR = "foo"; next-cmd` similarly trails the `;` portion. Same caveat.

These are NOT blocking. The user's wrapper at `C:\Users\maria\bin\claude-mb.cmd` is a clean two-line `set` + `claude %*`.

> **`pub(crate)` exposure of `resolve_claude_projects_dir` — accept now or stay private?**

Accept now. The follow-up ticket (§6.3) is described in enough detail that the JSONL watcher reuse is essentially inevitable. `pub(crate)` is invisible outside the crate, so there's no API-surface cost. A later "promote private → `pub(crate)`" PR adds code-review noise for zero functional benefit. See §8.2.1.

> **Persistence-stripper alignment for codex/gemini in this ticket?**

Defer, per the architect's §6.4 reasoning. The reported regression is Claude-only because Claude is the only family with a filesystem-existence gate on injection. Codex/Gemini auto-resume continues to fire even for renamed wrappers; only their *recipe stripping* is broken, and that's a silent latent bug, not a user-visible regression. Symmetry repair belongs in its own ticket.

> **Coverage of the new test set vs. the existing `should_inject_continue` regression fence:**

Adequate. The existing 9 `should_inject_continue_*` tests at lines 1772–1856 stay intact (the predicate's pure-boolean signature is unchanged), so the issue #82 regression fence holds. The 9 new resolver tests cover:

- Default-install (bare `claude`, full-path `claude.exe`)
- Non-Claude shell (early `None`)
- `set` wrapper, quoted-value, missing directive, missing wrapper file, oversized wrapper
- `cmd /K wrapper.cmd ...` (top-level arg)
- `cmd /K "git pull && wrapper.cmd ..."` (embedded compound)

Plus the 3 new persistence tests cover direct, cmd-wrapped, and embedded-cmd-wrapped wrapper-basename detection. With the §8.2.2 `export ` test added, the matrix is complete.

The deliberate omissions (`which::which` of a bare basename — environment-dependent; `claude.cmd` shim with override — explicitly out of scope per matrix #10) are reasonable and documented.

### 8.5 Implementation order I will follow

1. §4.1.1 — add `pub(crate) fn resolve_claude_projects_dir` (with §8.2.1 visibility + §8.2.2 `export ` branch).
2. §4.1.2 — replace the `claude_project_exists` block in `create_session_inner`.
3. §4.2.1 — loosen `is_claude` in `strip_auto_injected_args` to `.starts_with("claude")`.
4. §4.2.2 — loosen the two `is_cmd`-branch position-finders to `.starts_with("claude")`. (No change to codex/gemini, no change to else-branch.)
5. `cargo check` — must compile.
6. §4.1.3 + §4.2.3 — add tests (with §8.2.3 placement, §8.2.4 import hoist, §8.2.5 robust embedded-cmd test).
7. `cargo test --package agentscommander-new` — all existing + new tests must pass.
8. `cargo clippy --workspace --all-targets -- -D warnings` — must be clean.
9. §6.5 polish — update `should_inject_continue` doc-comment with the §8.3.3 addendum.
10. `tauri.conf.json` version bump (per the team's "bump version on every build" rule) so the user can verify they're running the new build.
11. Smoke per §6.6 (manual). Stop short of merge/push per Role.md.

### 8.6 Out of this round's scope (acknowledging architect's intent)

- Telegram `jsonl_watcher.rs` fix — handled by the follow-up ticket per §6.3, enabled by the `pub(crate)` exposure.
- Codex/Gemini stripper symmetry — deferred per §6.4.
- Adding `log::debug!` on wrapper-parsed branch — explicitly out of scope per §6.2.

### 8.7 Verdict

**READY_FOR_IMPLEMENTATION** once the §8.2 enrichments are folded in. No conceptual blockers, no missing infrastructure, no surprise dependencies. The two-surgical-changes split is correct, the parser is conservative in the right ways, and the test set is sized appropriately for the regression fence.

---

## 10. Architect resolution — round 1

This section is the **design authority's binding response** to the dev-rust (§8) and grinch (§9) reviews. Where this section conflicts with §4 / §5 / §6 / §8 / §9, **this section wins**. The implementer must apply §10 in addition to §4 — the original §4 code blocks are NOT replaced wholesale; specific edits below override specific lines.

### 10.1 Decision matrix

| Source | Item | Verdict | Where the change lands |
|---|---|---|---|
| Grinch B1 | `set "VAR=value"` (cmd-quoted whole-assignment) | **ACCEPTED** | §10.2 parser rewrite + §10.4.1 test |
| Grinch B2 | `%NAME%` / `$env:NAME` expansion in values | **ACCEPTED** | §10.2 parser rewrite + §10.4.2 tests |
| Dev-rust §8.2.1 / Grinch N3 | `pub(crate)` visibility on resolver | **ACCEPTED** | §10.2 (signature change) |
| Dev-rust §8.2.2 / Grinch N1 | `export ` prefix for `.sh` wrappers | **ACCEPTED** | §10.2 parser rewrite + §10.4.3 test |
| Dev-rust §8.2.3 | Persistence test placement (line 810/812) | **ACCEPTED** | §10.5.1 supersedes §4.2.3 placement |
| Dev-rust §8.2.4 | Hoist `use std::path::PathBuf;` to top of `mod tests` | **ACCEPTED** | §10.5.2 supersedes §4.1.3 import location |
| Dev-rust §8.2.5 | Robust embedded-cmd-string test against spaced tempdir | **ACCEPTED** (option a) | §10.4.4 supersedes §4.1.3 test |
| Dev-rust §8.3.1 | Inline comment on `executable_basename` lowercase contract | **ACCEPTED** | §10.5.3 |
| Dev-rust §8.3.2 | Drop "mirror `build_title_prompt_appendage`" comparison | **ACCEPTED** | §10.5.4 supersedes §4.1.1 style notes |
| Dev-rust §8.3.3 | Doc-comment addendum for matrix #10 (`claude.cmd` shim) | **ACCEPTED** | §10.5.5 supersedes §6.5 polish task |
| Grinch N2 | cmd-branch coverage gap when claude-mb is not the last arg | **DEFERRED, doc-only** | §10.5.6 amends §6.1 |
| Grinch N4 | First-token bias (`claude && claude-mb` compound) | **REJECTED** | Unchanged; §10.6 records reason |
| Grinch N5 | `which::which` silent miss | **DEFERRED, doc-only** | §10.5.7 amends §6.6 |
| Grinch N6 | Relative paths in `CLAUDE_CONFIG_DIR` | **DEFERRED, doc-only** | §10.5.8 amends §6.1 |
| Grinch N7 | Persistence-stripper false-positive surface | **ACCEPTED, doc-only** | §10.5.9 amends §5 row 11 |
| Grinch N8 | `--continue=<value>` strip/inject mismatch | **REJECTED** | Pre-existing, separate ticket; §10.6 |
| Grinch N9 | Blocking I/O on tokio runtime | **REJECTED** | Acceptable per grinch's own analysis; §10.6 |

### 10.2 Wrapper parser — full revised body

The implementer MUST use the version below in place of §4.1.1's `parse_config_dir_from_wrapper` and the surrounding signature. Folds in: `pub(crate)` (§8.2.1), `export ` (§8.2.2), cmd-quoted whole-assignment (B1), `%NAME%` / `$env:NAME` expansion (B2). The outer function body, the nested `strip_ascii_prefix_ci`, `looks_like_wrapper_extension`, `resolve_token_to_file`, and the claude-token-search at the bottom of `resolve_claude_projects_dir` remain exactly as in §4.1.1 — only the visibility line, `parse_config_dir_from_wrapper`, and the new `expand_env_vars` helper change.

**Replace the signature line in §4.1.1** (the line that reads `fn resolve_claude_projects_dir(`) with:

```rust
pub(crate) fn resolve_claude_projects_dir(
    shell: &str,
    shell_args: &[String],
    cwd: &str,
) -> Option<std::path::PathBuf> {
```

**Replace the entire `parse_config_dir_from_wrapper` nested function body in §4.1.1** with:

```rust
    fn parse_config_dir_from_wrapper(path: &Path) -> Option<PathBuf> {
        // Cap read at 64 KiB; real wrappers are < 1 KiB. Refusing larger
        // files protects against accidentally treating an exe-renamed-as-cmd
        // as a wrapper.
        const MAX: u64 = 64 * 1024;
        let metadata = std::fs::metadata(path).ok()?;
        if metadata.len() > MAX {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        // Strip UTF-8 BOM if present; tolerate non-UTF-8 by lossy decode.
        let text_bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let text = String::from_utf8_lossy(text_bytes);

        for raw_line in text.lines() {
            let line = raw_line.trim_start();

            // Strip optional shell-prefix introducer:
            //   `cmd`/`.bat`: `set CLAUDE_CONFIG_DIR=...`
            //   `cmd`/`.bat`: `set "CLAUDE_CONFIG_DIR=..."`     (cmd-quoted whole-assignment)
            //   `.ps1`:       `$env:CLAUDE_CONFIG_DIR = ...`
            //   `.sh`:        `export CLAUDE_CONFIG_DIR=...`
            //   Bare:         `CLAUDE_CONFIG_DIR=...`
            let after_prefix = if let Some(rest) = strip_ascii_prefix_ci(line, "set ") {
                rest.trim_start()
            } else if let Some(rest) = strip_ascii_prefix_ci(line, "$env:") {
                rest.trim_start()
            } else if let Some(rest) = strip_ascii_prefix_ci(line, "export ") {
                rest.trim_start()
            } else {
                line
            };

            // Detect cmd's whole-assignment quoting: `set "VAR=value"`. After the
            // `set ` strip we may be sitting on a leading `"`; if so, the matching
            // closing `"` terminates the value (rather than wrapping it).
            let (after_open_quote, cmd_quoted) = match after_prefix.strip_prefix('"') {
                Some(rest) => (rest, true),
                None => (after_prefix, false),
            };

            let Some(rest) =
                strip_ascii_prefix_ci(after_open_quote, "CLAUDE_CONFIG_DIR")
            else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let value = rest.trim();

            // Strip surrounding quotes. Two flavors:
            //   (a) cmd whole-assignment: `set "VAR=value"`     → consume trailing `"`
            //   (b) value-quoted:         `set VAR="value"` or `'value'` → strip matched pair
            let unquoted: &str = if cmd_quoted {
                value.strip_suffix('"').unwrap_or(value)
            } else if value.len() >= 2
                && ((value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\'')))
            {
                &value[1..value.len() - 1]
            } else {
                value
            };

            if unquoted.is_empty() {
                return None;
            }
            let expanded = expand_env_vars(unquoted);
            return Some(PathBuf::from(expanded));
        }
        None
    }
```

**Add a new nested helper inside `resolve_claude_projects_dir`**, alongside `strip_ascii_prefix_ci` / `looks_like_wrapper_extension` / `resolve_token_to_file`:

```rust
    /// Single-pass expansion of `%NAME%` (cmd) and `$env:NAME` (PowerShell)
    /// environment-variable references against `std::env::var`. Unknown names
    /// are preserved literally, so a downstream `is_dir()` check returns
    /// false rather than silently mis-resolving. Names must be ASCII
    /// alphanumeric or `_`; anything else terminates the name.
    ///
    /// Limitations (acceptable for real-world wrappers):
    ///   - No nested expansion: `%A%` whose value contains `%B%` is not re-expanded.
    ///   - No escape syntax (cmd's `^%`, PowerShell's backtick) — wrappers don't use these.
    fn expand_env_vars(input: &str) -> String {
        // Pass 1: %NAME% (cmd-style).
        let mut buf = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find('%') {
            buf.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            match after.find('%') {
                Some(end) => {
                    let name = &after[..end];
                    let valid = !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if valid {
                        if let Ok(v) = std::env::var(name) {
                            buf.push_str(&v);
                        } else {
                            buf.push('%');
                            buf.push_str(name);
                            buf.push('%');
                        }
                    } else {
                        // Not a valid var name (e.g. "100%" literal); preserve.
                        buf.push('%');
                        buf.push_str(name);
                        buf.push('%');
                    }
                    rest = &after[end + 1..];
                }
                None => {
                    buf.push('%');
                    buf.push_str(after);
                    rest = "";
                    break;
                }
            }
        }
        buf.push_str(rest);

        // Pass 2: $env:NAME (PowerShell-style). Case-insensitive prefix; name
        // terminates at the first byte that is not [A-Za-z0-9_].
        let mut out = String::with_capacity(buf.len());
        let bytes = buf.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let remaining = &buf[i..];
            if remaining.len() >= 5
                && remaining.as_bytes()[..5].eq_ignore_ascii_case(b"$env:")
            {
                let name_start = i + 5;
                let mut name_end = name_start;
                while name_end < bytes.len() {
                    let c = bytes[name_end];
                    if c.is_ascii_alphanumeric() || c == b'_' {
                        name_end += 1;
                    } else {
                        break;
                    }
                }
                if name_end > name_start {
                    let name = &buf[name_start..name_end];
                    if let Ok(v) = std::env::var(name) {
                        out.push_str(&v);
                    } else {
                        out.push_str(&buf[i..name_end]);
                    }
                    i = name_end;
                    continue;
                }
            }
            // Default: copy one full UTF-8 char.
            let ch = buf[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }
```

The outer `resolve_claude_projects_dir` body — the claude-token search, the `executable_basename(&claude_token) == "claude"` short-circuit, the wrapper-resolve fallback chain — is unchanged from §4.1.1. Add this one-line comment immediately above the `==` short-circuit per §8.3.1:

```rust
    // executable_basename returns a lowercased stem; comparison is case-insensitive by construction.
    if executable_basename(&claude_token) == "claude" {
```

### 10.3 §4.1.1 style notes — corrected

Replace the §4.1.1 "Style notes for the dev" block in full with:

> Style notes for the dev:
> - Helpers are nested inside the function so we don't pollute the module namespace. Nesting is a deliberate design choice for this resolver, not a project-wide convention.
> - No `unwrap()` outside test code. All errors → `None` → fallback to default base.
> - Do not import `regex`. The `strip_ascii_prefix_ci` and `expand_env_vars` helpers are intentionally tiny.
> - `expand_env_vars` is called only on already-parsed wrapper values; do NOT plumb it elsewhere in this PR.

This drops the inaccurate `build_title_prompt_appendage` precedent comparison flagged in §8.3.2.

### 10.4 New / amended tests

All resolver tests live in `commands/session.rs`'s `mod tests`, appended as in §4.1.3. Persistence tests live in `config/sessions_persistence.rs`'s `mod tests` per §10.5.1. The §4.1.3 import block (`use std::path::PathBuf;` and the `write_wrapper` helper) is moved per §10.5.2.

#### 10.4.1 New test — cmd-quoted whole-assignment (B1)

```rust
    #[test]
    fn resolve_claude_projects_dir_parses_cmd_quoted_whole_assignment() {
        // `set "CLAUDE_CONFIG_DIR=<path with spaces>"` — canonical cmd idiom
        // when the value contains spaces or shell metacharacters.
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join("Path With Spaces").join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset \"CLAUDE_CONFIG_DIR={}\"\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "C:\\x",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }
```

#### 10.4.2 New tests — environment-variable expansion (B2)

Each test uses a **unique env-var name** to avoid contamination if `cargo test` runs in parallel; the var is set before reading and removed at the end of the test. Do NOT reuse standard names like `USERPROFILE` in these tests — they are read-only de-facto on the host.

```rust
    #[test]
    fn resolve_claude_projects_dir_expands_percent_envvar_value() {
        let var = "AC_TEST_186_BASE_PCT";
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        // SAFETY: env state is process-global; unique name avoids cross-test races.
        std::env::set_var(var, custom_base.to_str().unwrap());
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR=%{}%\r\nclaude %*\r\n",
                var
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "C:\\x",
        );
        std::env::remove_var(var);
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_expands_powershell_envvar_value() {
        let var = "AC_TEST_186_BASE_PS";
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        std::env::set_var(var, custom_base.to_str().unwrap());
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.ps1",
            &format!(
                "$env:CLAUDE_CONFIG_DIR = \"$env:{}\"\r\nclaude @args\r\n",
                var
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "C:\\x",
        );
        std::env::remove_var(var);
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }

    #[test]
    fn resolve_claude_projects_dir_preserves_unknown_envvar_literal() {
        // Unknown var → literal preserved → resulting path is_dir() will be
        // false at the call site, but parse must succeed (return Some).
        let tmp = tempfile::tempdir().unwrap();
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            "@echo off\r\nset CLAUDE_CONFIG_DIR=%AC_TEST_186_DEFINITELY_UNSET%\\\\.claude-mb\r\nclaude %*\r\n",
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "C:\\x",
        );
        let resolved = resolved.expect("parser must return Some even when var is unset");
        let s = resolved.to_string_lossy();
        assert!(
            s.contains("%AC_TEST_186_DEFINITELY_UNSET%"),
            "expected literal preservation, got {s}"
        );
    }
```

#### 10.4.3 New test — `export ` prefix (§8.2.2)

Use exactly the test from §8.2.2:

```rust
    #[test]
    fn resolve_claude_projects_dir_parses_wrapper_with_export_directive() {
        let tmp = tempfile::tempdir().unwrap();
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.sh",
            &format!(
                "#!/usr/bin/env bash\nexport CLAUDE_CONFIG_DIR={}\nexec claude \"$@\"\n",
                custom_base.display()
            ),
        );
        let resolved = super::resolve_claude_projects_dir(
            wrapper.to_str().unwrap(),
            &[],
            "/home/test/repo",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("/home/test/repo"));
        assert_eq!(resolved, Some(expected));
    }
```

#### 10.4.4 Amended test — embedded-cmd-string (§8.2.5)

Replace the §4.1.3 test `resolve_claude_projects_dir_finds_claude_token_in_embedded_cmd_string` body with the version below. Skips (does not fail) when the OS temp dir contains spaces, preserving the test's intent without flaking on hostile environments.

```rust
    #[test]
    fn resolve_claude_projects_dir_finds_claude_token_in_embedded_cmd_string() {
        // Embedded form — per-arg whitespace split must surface claude-mb.cmd.
        // Skip on hosts where the temp dir contains spaces (split would
        // fragment the wrapper path); the cmd-wrapped-args test above
        // already covers spaced paths via direct token form.
        let tmp = tempfile::tempdir().unwrap();
        if tmp.path().to_string_lossy().contains(' ') {
            return;
        }
        let custom_base = tmp.path().join(".claude-mb");
        let wrapper = write_wrapper(
            tmp.path(),
            "claude-mb.cmd",
            &format!(
                "@echo off\r\nset CLAUDE_CONFIG_DIR={}\r\nclaude %*\r\n",
                custom_base.display()
            ),
        );
        let combined = format!("git pull && {} --effort max", wrapper.display());
        let resolved = super::resolve_claude_projects_dir(
            "cmd.exe",
            &["/K".to_string(), combined],
            "C:\\x",
        );
        let expected = custom_base
            .join("projects")
            .join(crate::session::session::mangle_cwd_for_claude("C:\\x"));
        assert_eq!(resolved, Some(expected));
    }
```

### 10.5 Plan-text amendments

These are doc-text adjustments to earlier sections. The implementer applies them as-is.

#### 10.5.1 Persistence test placement — supersedes §4.2.3

§4.2.3 says "add three regression tests at the end of `sessions_persistence.rs`'s test module." Override: insert the three new `strip_auto_injected_args_*` tests **after the existing `strip_auto_injected_args_leaves_unrelated_commands_unchanged` test (currently ending at line 810) and BEFORE the `/// Validation #17:` doc-comment that opens `legacy_migration_single_repo_shape` (line 812)**. This keeps all `strip_auto_injected_args_*` tests contiguous, matching the existing layout.

#### 10.5.2 Resolver test imports — supersedes §4.1.3 import location

§4.1.3 places `use std::path::PathBuf;` inside the new test divider. Override: hoist `use std::path::PathBuf;` to the top of `mod tests` in `commands/session.rs`, next to the existing `use super::{inject_codex_resume, resolve_actual_agent, should_inject_continue};` block (currently line ~1449). The `write_wrapper` helper stays inside the divider — it is only referenced by the new tests. Do NOT add `use tempfile;`; bare `tempfile::tempdir()` is the workspace convention.

#### 10.5.3 Inline contract comment on `executable_basename` (§8.3.1)

Add the following one-line comment immediately above the `if executable_basename(&claude_token) == "claude" {` early-return inside `resolve_claude_projects_dir`:

```rust
    // executable_basename returns a lowercased stem; comparison is case-insensitive by construction.
```

#### 10.5.4 §4.1.1 style notes — replaced

See §10.3 above. The "mirror layout used by `build_title_prompt_appendage`" line is dropped.

#### 10.5.5 Doc-comment polish — supersedes §6.5 polish task

§6.5 currently says: *"Update the `should_inject_continue` doc-comment (lines 254–270) to mention `(callers should compute claude_project_exists via resolve_claude_projects_dir to honor wrapper-set CLAUDE_CONFIG_DIR)`"*.

Override: add **two** sentences instead of one to the `should_inject_continue` doc-comment. The first is the existing line; the second documents matrix #10 per §8.3.3:

> Callers should compute `claude_project_exists` via `resolve_claude_projects_dir` to honor wrapper-set `CLAUDE_CONFIG_DIR`. Note: a wrapper named exactly `claude.cmd` / `claude.exe` / `claude.ps1` that overrides `CLAUDE_CONFIG_DIR` is intentionally NOT honored — the resolver short-circuits when the file_stem equals `claude`. Users who need wrapper overrides should rename to `claude-<suffix>` (e.g. `claude-mb`).

#### 10.5.6 §6.1 amendment — cmd-branch coverage clarification (Grinch N2)

In §6.1 (the **Do not change** the `cmd /K` injection branch bullet), append:

> Caveat: this fallback covers the case where the LAST arg is or contains a `claude*` token. If a user manually configures `default_shell="cmd.exe"` with `claude-mb` as a non-last arg (e.g. `["/K", "claude-mb", "--effort", "max"]`), the cmd branch's `last.contains("claude")` check misses. This is a pre-existing latent gap, not a regression introduced or repaired by #186 — the user's reported configuration uses `shell="claude-mb"` directly (verified at `commands/session.rs:1184` and `lib.rs:635`), which routes through the non-cmd branch and is fully covered by §4.1.2. Address the cmd-branch all-args scan in a separate ticket if a user reports the gap.

#### 10.5.7 §6.6 amendment — explicit PATH check (Grinch N5)

Insert a new step between current step 2 and step 3 of §6.6:

> 2b. Confirm `where claude-mb` (cmd) or `Get-Command claude-mb` (PowerShell) returns the expected wrapper path. If it does not, AC's `which::which` lookup will silently fall through to the default base — ensure `%PATH%` includes the wrapper's directory before testing the regression fix.

#### 10.5.8 §6.1 amendment — relative-path requirement (Grinch N6)

Append a new bullet to §6.1:

> - **`CLAUDE_CONFIG_DIR` values must be absolute paths.** A relative value (e.g. `set CLAUDE_CONFIG_DIR=.claude-mb`) is preserved literally and resolved against AC's process CWD, not the wrapper's parent dir — almost always a false-negative. The resolver intentionally does NOT rebase relative paths against the wrapper's directory; document this expectation if a user reports it. Env-var prefixes (`%USERPROFILE%\\.claude-mb`, `$env:USERPROFILE\\.claude-mb`) are the supported way to write portable wrapper paths and ARE expanded — see §10.2 `expand_env_vars`.

#### 10.5.9 §5 row 11 amendment — persistence-stripper false-positive (Grinch N7)

Replace the §5 "Outcome" cell of row 11 with:

> Treated as Claude. Same risk surface as today's `is_claude = b.starts_with("claude")` at session.rs line **419**; consistent. Not introduced by this change. Note the persistence stripper now shares this surface too: if a hypothetical `claude-derived-tool` accepts `--continue` or `--append-system-prompt-file <path>`, the stripper will silently remove those tokens from the saved recipe. Acceptable — the failure mode is symmetric with the inject-side detector and the wrapper namespace is by convention claude-family.

### 10.6 Explicitly rejected items — record of reasoning

- **Grinch N4 (first-token bias on `claude && claude-mb`)** — Rejected. Contrived case (no real wrapper composes `claude` with `claude-mb` in one invocation), already symmetric with the existing line-419 detector, ~5 LOC of speculative logic for zero observed user value. Re-open only if a bug is reported.
- **Grinch N8 (`--continue=<value>` strip/inject mismatch)** — Rejected for #186. Pre-existing, not introduced by this fix, and AC does not currently inject `--continue=<value>` form. File a separate symmetry ticket if desired.
- **Grinch N9 (blocking I/O on tokio runtime)** — Rejected. Grinch's own analysis acknowledges current code at lines 443–453 is already blocking; the addition of `which::which` is single-digit ms in the cold-PATH worst case, comparable to existing blocking calls (`discover_teams`, `materialize_agent_context_file`) on the same hot path. Out of scope for #186; address holistically if a perf ticket lands.

### 10.7 Final implementation order — supersedes §8.5

1. §4.1.1 — add `pub(crate) fn resolve_claude_projects_dir` with §10.2's revised `parse_config_dir_from_wrapper`, new `expand_env_vars` helper, and §10.5.3's inline contract comment. §10.3 style notes apply.
2. §4.1.2 — replace the `claude_project_exists` block in `create_session_inner` (unchanged from original plan).
3. §4.2.1 — loosen `is_claude` in `strip_auto_injected_args` to `.starts_with("claude")` (unchanged from original plan).
4. §4.2.2 — loosen the two `is_cmd`-branch position-finders to `.starts_with("claude")` (unchanged from original plan; codex/gemini and else-branch untouched).
5. `cargo check` — must compile.
6. Add resolver tests — §4.1.3 originals (with §10.5.2 import hoist) + §10.4.1 (B1) + §10.4.2 (B2 ×3) + §10.4.3 (`export `) + §10.4.4 (amended embedded-cmd test).
7. Add persistence tests — §4.2.3's three tests at the §10.5.1 placement.
8. `cargo test --package agentscommander-new` — all existing + new tests must pass.
9. `cargo clippy --workspace --all-targets -- -D warnings` — must be clean.
10. §10.5.5 — update `should_inject_continue` doc-comment (two-sentence version).
11. §10.5.6 / §10.5.7 / §10.5.8 / §10.5.9 — apply doc-text amendments to the plan file itself if the team treats the plan as living docs (optional; the architect will not block on plan housekeeping).
12. `tauri.conf.json` version bump per the team's "bump version on every build" rule, so the user can visually confirm they're running the new build.
13. Smoke per §6.6 (now including step 2b from §10.5.7). Stop short of merge/push per Role.md.

### 10.8 Verdict

**READY_FOR_IMPLEMENTATION**
