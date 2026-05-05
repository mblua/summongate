# Auto-generate workspace title from BRIEF on Coordinator restart

**Issue**: https://github.com/mblua/AgentsCommander/issues/107
**Branch**: `feature/107-auto-brief-title`
**Repo**: `repo-AgentsCommander`
**Type**: feature (best-effort UX improvement)

---

## 1. Requirement

When a Coordinator agent's PTY is (re)spawned, AC injects a one-shot prompt that
asks the agent to read its workgroup `BRIEF.md` and add a YAML frontmatter
`title:` line summarising the brief. The Rust backend never calls an LLM — it
only writes a prompt into the agent's PTY; the agent (Claude Code, Codex, etc.)
edits the file itself.

The mechanism is gated by a new global setting (default ON) and is fully
idempotent: if `title:` is already present (from a prior run or manual edit) or
the brief is empty, no prompt is injected.

This plan also changes the `BRIEF.md` template applied to **new** workgroups so
the file holds the user's brief verbatim (or is empty), not a markdown template.

---

## 2. Scope (hard constraints)

1. **Backend never calls an LLM.** Only `inject_text_into_session` is used. No
   new HTTP clients, no new SDKs.
2. ~~**Credentials block and title-prompt are SEPARATE PTY writes.** Two distinct
   `inject_text_into_session` calls inside the spawn-time spawned task. Never
   concatenate.~~

   > **Round 4 supersedes**: The original §2.2 contract was "two SEPARATE
   > PTY writes, never concatenate." This was changed in Round 4 to a
   > single combined PTY write when both gates are open (Coordinator +
   > auto-title setting ON + brief has no `title:`). See §R4.2 for
   > rationale and §R4.4 for what's removed.
3. **Trigger = PTY spawn for a session with `is_coordinator == true`.** Hooked
   inside `commands/session.rs::create_session_inner`. `/clear` and `/compact`
   do **not** retrigger title-gen — those paths do not spawn a new PTY (the
   existing `/clear` reinject is credentials-only — see
   `_plans/reinject-credentials-after-clear.md`).
4. **Idempotent.** The injected prompt fires only if BRIEF.md exists, is
   non-empty, and lacks a `title:` field. No retry, no migration, no overwrite
   of an existing title.
5. **No migration.** Existing BRIEF.md files in old workgroups stay as-is. The
   template change only affects new workgroups created after this branch lands.
6. **No new crates.** Use the existing custom frontmatter-parser shape — no
   `serde_yaml`. Justification in §6.
7. **Best-effort.** Failure to read BRIEF.md, locate the workgroup root, or
   inject the prompt → `log::warn!` and abort. Never `error!`, never retry.
8. **Coordinator gate is per-spawn.** `is_coordinator` is read from the local
   variable computed at `session.rs:336` (already in scope before the spawned
   task). No extra lookup required.

---

## 3. Files touched

| File | Action | Detail |
|---|---|---|
| `src-tauri/src/config/settings.rs` | modify | add `auto_generate_brief_title: bool` field + default + Default impl |
| `src-tauri/src/commands/entity_creation.rs` | modify | rewrite `build_brief_content` to drop the template; remove now-unused `default_brief_content`; add `parse_brief_title` helper; add `snapshot_brief_before_edit` helper (R2 fold F6) |
| `src-tauri/src/session/session.rs` | modify | add sibling helper `find_workgroup_brief_path_for_cwd` |
| `src-tauri/src/pty/title_prompt.rs` | **CREATE** | builder for the title-generation prompt block |
| `src-tauri/src/pty/mod.rs` | modify | add `pub mod title_prompt;` |
| `src-tauri/src/commands/session.rs` | modify | extend the existing post-spawn cred-inject task to chain title-gen |
| `src/shared/types.ts` | modify | add `autoGenerateBriefTitle: boolean` to `AppSettings` |
| `src/sidebar/components/SettingsModal.tsx` | modify | add checkbox in General tab |

The TS side has no other code change — `SettingsAPI.get` and `SettingsAPI.update`
already round-trip the full `AppSettings` object generically.

---

## 4. Setting — Rust side

### 4.1 `src-tauri/src/config/settings.rs`

Add the field to `AppSettings` (line 152 area, after `log_level`):

```rust
    /// When true, on Coordinator session spawn AC injects a prompt asking the
    /// agent to add a YAML frontmatter `title:` line to its workgroup
    /// `BRIEF.md` (only if the brief is non-empty and has no `title:` yet).
    /// See plan `_plans/107-auto-brief-title.md`.
    #[serde(default = "default_true")]
    pub auto_generate_brief_title: bool,
```

Add to the `Default` impl (block at lines 186-229), inserted alphabetically near
`coord_sort_by_activity` (line 226):

```rust
            auto_generate_brief_title: true,
```

`default_true()` already exists at line 154 — reuse it.

No change to `commands/config.rs` — `update_settings`/`get_settings` are
generic over `AppSettings`.

### 4.2 `src/shared/types.ts`

Add inside `AppSettings` (lines 126-157), keeping the existing camelCase order
adjacent to `coordSortByActivity`:

```ts
  autoGenerateBriefTitle: boolean;
```

### 4.3 `src/sidebar/components/SettingsModal.tsx`

The General tab section that holds `startOnlyCoordinators` is at lines 296-306.
Insert a new `<label class="settings-checkbox-field">` block immediately
**after** that one (i.e. between line 306 `</label>` and line 307 `<label`):

```tsx
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.autoGenerateBriefTitle}
            onChange={(e) =>
              updateField("autoGenerateBriefTitle", e.currentTarget.checked)
            }
          />
          <span>Auto-generate workspace title from brief</span>
        </label>
```

`updateField` (line 70-76) is already typed against `AppSettings` so the new
key is type-safe with no further changes.

No change to `src/shared/stores/settings.ts` — it stores the whole
`AppSettings` blob.

---

## 5. BRIEF.md template change (new workgroups only)

### 5.1 `src-tauri/src/commands/entity_creation.rs` — lines 179-196

**Before:**

```rust
fn default_brief_content(wg_name: &str) -> String {
    format!(
        "# {}\n\n## Objective\n\n_Describe the goal of this workgroup._\n\n## Scope\n\n_What is in and out of scope._\n\n## Deliverables\n\n- [ ] _List deliverables here_\n",
        wg_name
    )
}

fn build_brief_content(wg_name: &str, brief: Option<String>) -> String {
    let trimmed = brief
        .as_deref()
        .map(str::trim)
        .filter(|content| !content.is_empty());

    match trimmed {
        Some(content) => format!("{}\n", content),
        None => default_brief_content(wg_name),
    }
}
```

**After:**

```rust
/// BRIEF.md content for a brand-new workgroup.
///
/// - User-supplied brief → written verbatim with a single trailing newline.
/// - Nothing supplied → empty file.
///
/// Issue #107: do not auto-template the brief. Empty briefs are a valid state
/// and signal "no title-gen yet" to the Coordinator-spawn flow in
/// `commands/session.rs` (which skips title-gen on empty briefs).
fn build_brief_content(_wg_name: &str, brief: Option<String>) -> String {
    let trimmed = brief
        .as_deref()
        .map(str::trim)
        .filter(|content| !content.is_empty());

    match trimmed {
        Some(content) => format!("{}\n", content),
        None => String::new(),
    }
}
```

Notes:
- `_wg_name` is kept as `_`-prefixed to preserve the call-site signature at line 522 (`build_brief_content(&wg_name, brief)`) — no caller change needed. Avoids touching the call site for a no-op rename.
- `default_brief_content` is removed entirely (sole call site is gone). Verified by grep — no other references.

---

## 6. Frontmatter parsing — extend the custom parser

### 6.1 Decision: custom parser, no `serde_yaml`

**Choice**: extend the existing custom-parser shape from `parse_role_frontmatter`
(`entity_creation.rs:152-177`) by adding a small sibling helper
`parse_brief_title`. **Do NOT add `serde_yaml`.**

**Rationale:**
- Scope is one field (`title:`). The custom parser is ~15 lines, deterministic,
  and consistent with how `Role.md` frontmatter is already handled in the same
  file. No extra dependency, no compile-time hit, no transitive risk.
- The format we ask the agent to emit is the same `---`-delimited shape that
  `parse_role_frontmatter` already handles. Reusing the shape keeps the codebase
  coherent and avoids two parsers for the same on-disk format.
- If the project later needs richer YAML inside frontmatter (lists, nested
  values), `serde_yaml` can be added at that point with no rework here. This
  plan does not preempt that.

### 6.2 New helper in `src-tauri/src/commands/entity_creation.rs`

Insert immediately after `parse_role_frontmatter` (after line 177), before
`default_brief_content` was previously located:

```rust
/// Extract a `title:` field from the YAML frontmatter at the start of `content`.
///
/// Best-effort frontmatter detection — NOT a YAML implementation. Suitable
/// only for the narrow case of one optional scalar field at the top of
/// BRIEF.md.
///
/// Returns `Some(title)` when:
///   - `content` starts with `---`,
///   - a closing `---` exists,
///   - a line of the form `<key>: <value>` exists between the delimiters
///     where `<key>` matches `title` case-insensitively (`title:`, `Title:`,
///     `TITLE:`, mixed casing all accepted).
///
/// The value half is preserved verbatim (case-sensitive), then stripped of
/// surrounding `"` or `'` quote pairs.
///
/// Returns `None` otherwise (no frontmatter, no title key, or empty value).
///
/// Mirrors `parse_role_frontmatter`'s shape — both speak the same on-disk
/// format. See plan `_plans/107-auto-brief-title.md` §6 for why we do not
/// pull in `serde_yaml`.
pub(crate) fn parse_brief_title(content: &str) -> Option<String> {
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end = rest.find("---")?;
    let frontmatter = &rest[..end];

    for line in frontmatter.lines() {
        let trimmed = line.trim();
        // Case-insensitive key match on `title:`. Round 2 fold (F3 / G3):
        // agents stochastically capitalize keys (`Title:`, `TITLE:`); a
        // case-sensitive match would let duplicate `title:` lines accumulate
        // across restarts. Split on the first `:` so we compare just the key.
        let Some((key, value_raw)) = trimmed.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("title") {
            continue;
        }
        let value = value_raw.trim().trim_matches('"').trim_matches('\'').to_string();
        if value.is_empty() {
            return None;
        }
        return Some(value);
    }
    None
}
```

`pub(crate)` so it can be called from `commands/session.rs`. No new imports.

The case-insensitive match is the safety net for F3 / G3. The §8.1 prompt
still asks the agent to emit lowercase `title:` exactly — keeping the prompt
strict + a tolerant parser is the simplest belt-and-braces shape; we never
need to relax the prompt unless empirical drift forces us to.

---

## 7. Workgroup-root resolution — sibling helper

### 7.1 `src-tauri/src/session/session.rs` — add helper

The existing `read_workgroup_brief_for_cwd` (lines 122-142) walks up from `cwd`
looking for the first directory whose name starts with `wg-`, then reads
`BRIEF.md` from it. We need the **path** (for the prompt) AND the content (for
parsing), so we factor out the path-resolution step.

Insert immediately **before** `read_workgroup_brief_for_cwd` (i.e. before line
122):

```rust
/// Walk up from `cwd` to the first ancestor directory whose name starts with
/// `wg-`, and return that directory's `BRIEF.md` path. Returns `None` if no
/// such ancestor exists (does NOT check that the file exists on disk — caller
/// decides how to handle a missing file).
pub(crate) fn find_workgroup_brief_path_for_cwd(cwd: &str) -> Option<std::path::PathBuf> {
    let mut current = Some(Path::new(cwd));
    while let Some(path) = current {
        let is_workgroup_dir = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("wg-"));
        if is_workgroup_dir {
            return Some(path.join("BRIEF.md"));
        }
        current = path.parent();
    }
    None
}
```

### 7.2 Refactor `read_workgroup_brief_for_cwd` to reuse it

Replace lines 122-142 body with:

```rust
pub(crate) fn read_workgroup_brief_for_cwd(cwd: &str) -> Option<String> {
    let path = find_workgroup_brief_path_for_cwd(cwd)?;
    std::fs::read_to_string(&path)
        .ok()
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty())
}
```

Imports: `Path` is already imported at line 3 (`use std::path::Path;`). No
change to imports. `PathBuf` is fully qualified inline — no new `use`.

Behavior is unchanged for the existing call site at session.rs:202.

---

## 8. Title-prompt builder

### 8.1 New file — `src-tauri/src/pty/title_prompt.rs`

Mirrors the shape of `pty/credentials.rs`: pure function, no I/O, byte-stable
output, easy to unit-test.

```rust
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
}
```

### 8.2 `src-tauri/src/pty/mod.rs`

Add `pub mod title_prompt;`. Keep alphabetic order:

```rust
pub mod credentials;
pub mod git_watcher;
pub mod idle_detector;
pub mod inject;
pub mod manager;
pub mod title_prompt;
```

(Insert `title_prompt` after `manager`. The existing module list per
`_plans/reinject-credentials-after-clear.md` §5 already covers `credentials`.)

---

## 9. Spawn-time hook — `commands/session.rs`

### 9.1 Where it goes

The existing post-spawn cred-inject task is at **lines 510-568** inside
`create_session_inner`. It is gated by `if agent_id.is_some()`, captures
`token` + `cwd_clone`, polls for idle (max 30 s), then calls
`inject_text_into_session(&app_clone, session_id, &cred_block, true)`.

We chain title-gen **inside the same spawned task**, **after** the credentials
injection, **only on the success branch** of the cred inject.

This guarantees:
- Single spawned task per session — easy to reason about.
- Strict ordering: idle → creds → idle → title-prompt.
- Title-prompt is a separate `inject_text_into_session` call (separate PTY
  write, satisfies §2.2 constraint).
- Title-gen never runs if creds injection failed (no point — agent has no creds
  anyway and the title prompt would land into a broken context).

### 9.2 Capture additional values into the closure

Today, before the `tokio::spawn(async move { ... })` at line 515, the task
captures:

```rust
        let app_clone = app.clone();
        let session_id = id;
        let token = session.token;
        let cwd_clone = cwd.clone();
```

Add **two new captures** in the same spot:

```rust
        let is_coordinator_clone = is_coordinator;  // bool, Copy
        let auto_title_enabled = {
            let settings_state = app.state::<SettingsState>();
            let cfg = settings_state.read().await;
            cfg.auto_generate_brief_title
        };
```

Notes:
- `is_coordinator` is the local `bool` computed at line 336. `bool` is `Copy`,
  so this is a trivial capture.
- **No live `cfg` exists at line 510** — round 1 of this plan claimed otherwise
  and was wrong. The `cfg` opened at lines 322-323 in `create_session_inner` is
  bound inside an inner `{ … }` block and **dropped at line 331** (verified by
  dev-rust R2 and dev-rust-grinch G1 against the current file). The `cfg` at
  line 630 lives in the outer `create_session` Tauri command, a different
  function. So we open a fresh read guard immediately before the `tokio::spawn`
  to read just the one field we need. One extra `RwLock::read().await` on the
  spawn path only — concurrent readers don't block; deadlock-free (no other
  lock is held at line 510).
- The snapshot semantics are unchanged: the field read happens **once** before
  the `tokio::spawn`, and the captured `bool` is what the spawned task uses. A
  settings toggle that fires mid-spawn is intentionally ignored for the
  in-flight session — same shape as the snapshot pattern at
  `entity_creation.rs:573-576` (issue #84). Mid-flight toggles for already-
  spawned sessions would create surprising user-visible behavior.

> Note (dev-rust-grinch G12): this is the FOURTH `SettingsState` read in
> `create_session_inner` (lines 322, 587, 573, plus this one). All are
> read-locks, so concurrent-reader semantics keep this safe. Folding all four
> into a single top-of-function snapshot is a future code-quality enrichment —
> out of scope for this PR, see §14.

### 9.3 Replacement — extend the success branch

Locate the existing block at session.rs lines 543-566:

```rust
            let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd_clone);

            match crate::pty::inject::inject_text_into_session(
                &app_clone,
                session_id,
                &cred_block,
                true,
            )
            .await
            {
                Ok(()) => {
                    log::info!(
                        "[session] Credentials auto-injected for session {}",
                        session_id
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[session] Failed to auto-inject credentials for {}: {}",
                        session_id,
                        e
                    );
                }
            }
        });
```

Change ONLY the `Ok(()) =>` arm. Replace it with:

```rust
                Ok(()) => {
                    log::info!(
                        "[session] Credentials auto-injected for session {}",
                        session_id
                    );

                    // Issue #107 — Coordinator-only auto-title chain.
                    // Runs sequentially after the credentials inject so the two
                    // PTY writes never collide and the agent already has
                    // credentials in context when the title prompt arrives.
                    if is_coordinator_clone && auto_title_enabled {
                        if let Err(e) = inject_title_prompt_after_idle_static(
                            &app_clone,
                            session_id,
                            &cwd_clone,
                        )
                        .await
                        {
                            log::warn!(
                                "[session] Auto-title skipped for session {}: {}",
                                session_id,
                                e
                            );
                        }
                    }
                }
```

The `Err(e) =>` arm stays as-is — title-gen never runs when creds injection
fails.

### 9.4 New helper function — `inject_title_prompt_after_idle_static`

Add at module scope inside `src-tauri/src/commands/session.rs`. Place it
**immediately before** `pub async fn create_session_inner` (i.e. just before
line 307). It is private (no `pub`), `async`, takes only the values the spawn
captured.

#### 9.4.0 Round 2 amendments folded into this helper

This helper changed shape between rounds. Pre-amendment behaviour and the
review IDs that drove each fold:

- **F2 / R3 / G2 — re-read after the second idle wait.** The pre-idle empty
  and `parse_brief_title` checks were the only guards in round 1; up to 30 s
  could pass before the inject, in which a sibling agent or manual edit could
  invalidate the cached `content`. Round 2 keeps the pre-idle checks as a
  fast short-circuit AND re-runs both guards on a fresh re-read after idle.
- **F4 / G4 — strip `\\?\` UNC prefix from the embedded path** so PTY-bound
  paths normalise the same way `pty/credentials.rs` already does.
- **F6 — `.bak` snapshot before injecting the title prompt.** A timestamped
  copy of BRIEF.md is written next to it before the prompt is sent. If the
  copy fails, the title prompt is **not** injected (better to skip than to
  let an agent edit an unbacked-up file). Idempotent — next restart retries.
  See §16 for the helper this calls (`snapshot_brief_before_edit`).
- **F7 / G6 — `no wg-* ancestor` keeps `Err` + warn.** Tech-lead override of
  dev-rust R9. Reaching this branch means the team config flagged a CWD as
  Coordinator that has no `wg-*` ancestor — that is a config inconsistency
  worth surfacing once per spawn, not noise.

#### 9.4.1 Drafted helper

```rust
/// Issue #107 — Coordinator auto-title.
///
/// Wait for the agent to return to idle (after the credentials inject), then
/// — if the workgroup BRIEF.md exists, is non-empty, and has no `title:` field
/// in its YAML frontmatter — snapshot it to a timestamped `.bak` and inject
/// a one-shot prompt asking the agent to add the title.
///
/// Best-effort. Returns `Err(reason)` for the caller to log at `warn` level;
/// never panics, never retries.
///
/// Gates layered (in order):
///   1. workgroup root resolvable from `cwd` → else `Err` (config issue, F7).
///   2. BRIEF.md exists and read succeeds → else `Err`.
///   3. BRIEF.md non-empty (after trim) → else `Ok(())` (silent skip).
///   4. No `title:` field in existing frontmatter → else `Ok(())` (silent
///      skip).
///   5. Wait for idle (max 30 s, 500 ms poll) → on timeout `Err`.
///   6. RE-READ BRIEF.md (F2 fold). Re-run gates 3 and 4 — sibling writers
///      may have changed the file during the wait.
///   7. Snapshot BRIEF.md to `BRIEF.md.<UTC-ts>.bak` (F6 fold). Snapshot
///      failure → `Err` (do not inject without a backup).
///   8. Inject the title prompt with the absolute, UNC-stripped path (F4
///      fold).
///
/// Idle-poll parameters mirror the credentials path
/// (`session.rs:516-541` — 30 s max, 500 ms poll).
async fn inject_title_prompt_after_idle_static(
    app: &AppHandle,
    session_id: Uuid,
    cwd: &str,
) -> Result<(), String> {
    use crate::commands::entity_creation::{parse_brief_title, snapshot_brief_before_edit};
    use crate::session::session::find_workgroup_brief_path_for_cwd;

    // (1) Resolve workgroup BRIEF.md path. F7: keep `Err` here so a
    //     Coordinator misconfigured to a non-`wg-*` CWD is logged once per
    //     spawn — the warn line names the exact config inconsistency.
    let brief_path = find_workgroup_brief_path_for_cwd(cwd)
        .ok_or_else(|| format!("[auto-title:config] no wg- ancestor in cwd '{}'", cwd))?;

    // (2) Initial read. Missing file or unreadable → warn-and-skip.
    let content = std::fs::read_to_string(&brief_path)
        .map_err(|e| format!("read BRIEF.md at {:?}: {}", brief_path, e))?;

    // (3) Empty brief → silent skip. Documented "no-op" state for newly-
    //     created workgroups where the user did not supply a brief.
    if content.trim().is_empty() {
        log::info!(
            "[session] Auto-title skipped (BRIEF empty) for session {}",
            session_id
        );
        return Ok(());
    }

    // (4) Title already present (manual edit OR previous auto-title run).
    if parse_brief_title(&content).is_some() {
        log::info!(
            "[session] Auto-title skipped (title present) for session {}",
            session_id
        );
        return Ok(());
    }

    // (5) Wait for idle a SECOND time — credentials inject just ran and
    //     likely left the agent processing. Same poll shape as credentials
    //     path. TOCTOU note (parity with phone/mailbox.rs:961-962): the
    //     agent could become busy between the idle observation here and the
    //     write below; acceptable for best-effort title-gen because the
    //     prompt itself instructs the agent to re-check the file at
    //     execution time.
    let max_wait = std::time::Duration::from_secs(30);
    let poll = std::time::Duration::from_millis(500);
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() >= max_wait {
            return Err(format!(
                "timeout ({}s) waiting for idle before title-prompt",
                max_wait.as_secs()
            ));
        }
        tokio::time::sleep(poll).await;

        let session_mgr = app.state::<Arc<tokio::sync::RwLock<SessionManager>>>();
        let mgr = session_mgr.read().await;
        let sessions = mgr.list_sessions().await;
        match sessions.iter().find(|s| s.id == session_id.to_string()) {
            Some(s) if s.waiting_for_input => break,
            Some(_) => {} // still busy
            None => {
                return Err(format!(
                    "session {} destroyed during title-prompt poll",
                    session_id
                ));
            }
        }
    }

    // (6) F2 fold — re-read after the wait. Up to 30 s elapsed; sibling
    //     agents, manual edits, or even our own `restart_session` racing
    //     with this task could have written `title:` already.
    let content = std::fs::read_to_string(&brief_path)
        .map_err(|e| format!("re-read BRIEF.md at {:?}: {}", brief_path, e))?;
    if content.trim().is_empty() {
        log::info!(
            "[session] Auto-title skipped (BRIEF empty post-idle) for session {}",
            session_id
        );
        return Ok(());
    }
    if parse_brief_title(&content).is_some() {
        log::info!(
            "[session] Auto-title skipped (title present post-idle) for session {}",
            session_id
        );
        return Ok(());
    }

    // (7) F6 fold — snapshot BEFORE injecting. If snapshot fails (disk full,
    //     permission error), abort the inject — better to skip the feature
    //     than to let the agent edit an unbacked-up file. Next restart
    //     retries.
    let bak_path = snapshot_brief_before_edit(&brief_path)
        .map_err(|e| format!("snapshot BRIEF.md before edit: {}", e))?;
    log::info!(
        "[session] Auto-title backup created: {:?} (session {})",
        bak_path,
        session_id
    );

    // (8) Build absolute-path string for the prompt. F4 fold: strip the
    //     Windows extended-length `\\?\` prefix to match
    //     `pty/credentials.rs`'s normalisation.
    let raw = brief_path.to_string_lossy().to_string();
    let path_str = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
    let prompt = crate::pty::title_prompt::build_title_prompt(&path_str);

    crate::pty::inject::inject_text_into_session(app, session_id, &prompt, true).await?;

    log::info!(
        "[session] Auto-title prompt injected for session {} (brief={:?}, bak={:?})",
        session_id,
        brief_path,
        bak_path
    );
    Ok(())
}
```

### 9.5 Imports

`commands/session.rs` already imports the items the helper needs:
- `Uuid` (in scope, used elsewhere in the file).
- `AppHandle` (in scope).
- `Arc`, `tokio::sync::RwLock`, `SessionManager` (used by the cred-inject task
  at lines 527-528).

The helper uses `crate::pty::title_prompt`, `crate::pty::inject`,
`crate::commands::entity_creation::parse_brief_title`, and
`crate::session::session::find_workgroup_brief_path_for_cwd` — all by
fully-qualified path, no new `use` lines required.

---

## 10. Sequencing — exact order on a fresh Coordinator spawn

> **Superseded by §R4.2** (Round 4): The two-write timeline + F2
> post-idle re-read described below was replaced by a single combined
> PTY write at spawn-start. The §10 diagram is preserved as audit
> trail; do not implement from this section.

```
1. PTY spawn          (existing — pty_mgr.spawn at session.rs:504)
2. tokio::spawn task starts (existing block, line 510)
   ├─ poll waiting_for_input (max 30 s)
   ├─ idle reached
   ├─ inject_text_into_session(creds, submit=true)        ← PTY write #1
   │    [inject.rs sends text + 2× Enter with 1500/500 ms gaps]
   ├─ on Ok:
   │    ├─ if !is_coordinator OR !auto_title_enabled → DONE
   │    ├─ else call inject_title_prompt_after_idle_static
   │    │    ├─ resolve BRIEF.md path (walk up cwd) — Err if no wg- ancestor
   │    │    │  (F7: tech-lead override; warn-log on config issue)
   │    │    ├─ initial read; empty → skip; title present → skip
   │    │    ├─ poll waiting_for_input (max 30 s) ← waits for creds-inject to settle
   │    │    ├─ idle reached
   │    │    ├─ RE-READ BRIEF.md (F2 fold); re-run empty/title guards
   │    │    ├─ snapshot_brief_before_edit → BRIEF.md.<UTC-ts>.bak (F6)
   │    │    │  on snapshot Err → skip (no .bak, no prompt)
   │    │    ├─ build prompt with absolute path (F4: \\?\ stripped)
   │    │    └─ inject_text_into_session(prompt, submit=true) ← PTY write #2
   └─ on Err: warn, do NOT chain title-gen.
```

The two `inject_text_into_session` calls are the two separate PTY writes the
spec requires.

### 10.1 Why a second idle wait

After the credentials block lands (`inject_text_into_session` returns), the
agent goes BUSY for the duration of processing the paste. The cred path's
`inject_text_into_session` adds `\r` keystrokes at +1500 ms and +2000 ms, and
the agent then takes some additional time to acknowledge. A second idle-wait
inside `inject_title_prompt_after_idle_static` is the cleanest way to ensure
the title prompt is processed as a distinct user message, not appended to the
in-flight cred-block paste buffer.

Same poll shape (30 s ceiling, 500 ms tick) as the credentials wait — keeps
two timeouts symmetric and easy to reason about.

---

## 11. Open questions — answers

These map 1:1 to the seven open questions in the request.

### Q1 — Frontmatter parsing: extend custom or pull `serde_yaml`?

**Answer: extend custom.** See §6.1. Single field, ~15-line helper, zero new
deps, format-coherent with `parse_role_frontmatter`.

### Q2 — Trigger detail: "Coordinator session restart" → exact code event?

**Answer:** every PTY spawn for a session whose `is_coordinator == true`. Hooked
inside `commands/session.rs::create_session_inner`, in the existing post-spawn
spawned task gated by `if agent_id.is_some()`. Adds an inner gate
`is_coordinator && auto_title_enabled`.

`/clear` does **not** trigger title-gen — `/clear` does not respawn the PTY,
only the cred block is reinjected (per
`_plans/reinject-credentials-after-clear.md`). `/compact` likewise.

`restart_session` does trigger title-gen — it goes through
`create_session_inner` again. This is intentional: a restart is conceptually a
fresh spawn of the same agent role.

### Q3 — Race condition: agent mid-write to BRIEF.md when next restart fires?

**Answer: idempotent skip handles it.** The guard `parse_brief_title(&content).is_some()`
is read at the LAST possible moment before the prompt is built, so:

- If the agent already wrote `title:` between PTY spawn and this check → we
  skip silently (info log).
- If the agent is in the middle of writing a non-`title:` edit (unrelated) →
  we still inject the prompt. The agent sees it, reads BRIEF.md again at
  prompt-execution time, sees no title, and adds it. Net result: one title
  added; no corruption (the prompt instructs the agent to ONLY add the
  frontmatter, not reflow).

Two restarts back-to-back: each spawns its own task. Both inject their prompt.
PTY queues both — agent processes the first (writes title), then the second
(reads BRIEF.md, sees title now present, the agent's own no-op compliance with
"if title present, do nothing" rule takes effect). One title added, one
no-op. Acceptable.

**No file lock.** Adds complexity for an edge case that the idempotent gate
already covers.

### Q4 — Exact prompt text

**Answer:** see §8.1 (`build_title_prompt`). Drafted to:
- Mark itself with `[AgentsCommander auto-title]` so the agent recognises it as
  a system instruction, not user content.
- Pin the absolute path inline.
- Show the exact frontmatter format with placeholder.
- State the constraints in plain English (≤8 words, no body modification).
- Specify the no-op cases (empty file, title already present) so the agent
  also self-guards even if our backend gate races.

### Q5 — Sequence: creds first, then title-prompt? With a gap?

**Answer:** creds first, **always**, then title-prompt — sequential inside the
same spawned task, with a second idle-wait between them. No fixed sleep — the
second idle-wait IS the gap, and it adapts to the agent's actual response
time. See §10.1.

### Q6 — BRIEF.md doesn't exist (corrupted state)?

**Answer:** silent skip with a `log::warn` from the caller (the `match` in
§9.3 prints the helper's `Err`). The helper returns `Err("read BRIEF.md at
...: <io error>")`. No retry, no recreate. The session is fully usable; only
the auto-title feature is dormant. A fresh workgroup creation would replace
the missing BRIEF.md with an empty file via §5.

### Q7 — Agent doesn't know the workgroup path; CWD is `__agent_<role>`

**Answer:** the prompt embeds the absolute path. Resolved by
`find_workgroup_brief_path_for_cwd` (§7.1) walking up from the session's
`cwd` to the first `wg-*` ancestor. The agent never has to compute or guess.

---

## 12. Test plan (manual — Phase 1 MVP target)

Run on branch `feature/107-auto-brief-title` (already created).

### 12.1 Prereqs

1. Build: from `repo-AgentsCommander`, `cargo check` then `npm run tauri dev`.
2. Have at least one team config that designates a Coordinator agent.
3. Setting `auto_generate_brief_title` defaults to `true` — confirm in the
   General tab of the Settings modal.

### 12.2 Happy path — Coordinator spawn with non-empty BRIEF, no title

1. Create a new workgroup; supply a brief at creation (e.g. "Build the auto-
   title feature for issue 107").
2. Verify `BRIEF.md` on disk contains the brief verbatim with one trailing
   newline, no template, no frontmatter.
3. Start the team's Coordinator session.
4. After the cred block lands and the agent goes idle, observe a second
   message in the agent's PTY: `[AgentsCommander auto-title] Read the
   workgroup brief at \`...\`...`.
5. Within ≤30 s after the agent processes that prompt, BRIEF.md gains a
   `---\ntitle: ...\n---\n` block at the top. Body unchanged.
6. App log contains:
   - `[session] Credentials auto-injected for session <uuid>`
   - `[session] Auto-title prompt injected for session <uuid> (brief=...)`

### 12.3 Idempotent — restart Coordinator with title already present

1. From state at end of 12.2, restart the Coordinator session.
2. Cred block injects normally. NO second auto-title prompt is injected.
3. App log:
   - `[session] Credentials auto-injected for session <uuid>`
   - `[session] Auto-title skipped (title present) for session <uuid>`

### 12.4 Empty brief — Coordinator spawn does nothing

1. Create a new workgroup with no brief (leave the field empty).
2. Verify `BRIEF.md` on disk is exactly 0 bytes.
3. Start the Coordinator. Cred block injects. NO title prompt.
4. App log: `[session] Auto-title skipped (BRIEF empty) for session <uuid>`.
5. BRIEF.md remains empty.

### 12.5 Setting OFF — Coordinator spawn does nothing

1. Open Settings → General. Uncheck "Auto-generate workspace title from brief".
2. Save.
3. Create a new workgroup with a brief; start the Coordinator.
4. Cred block injects. NO title prompt. No log line about auto-title (the
   `is_coordinator_clone && auto_title_enabled` gate short-circuits before
   the helper runs).
5. Re-enable, restart Coordinator → title prompt fires.

### 12.6 Non-Coordinator agent — never triggers

1. From any team that has at least one non-Coordinator member, start that
   member's session.
2. Cred block injects (existing behavior). NO title prompt. No auto-title log
   line.
3. BRIEF.md untouched.

### 12.7 Manual `title:` is respected

1. Create a workgroup with a brief.
2. Before starting the Coordinator, manually edit BRIEF.md to add:
   ```
   ---
   title: My Custom Title
   ---

   <existing body>
   ```
3. Start the Coordinator. Cred block injects. Title-skip log fires.
4. BRIEF.md untouched.

### 12.8 Idle timeout — title-prompt branch

1. Simulate a Coordinator that never returns to idle after the cred inject
   (e.g. inject a long-running tool call right before spawn — TBD by the
   tester; this is hard to simulate cleanly).
2. After 30 s, log: `[session] Auto-title skipped for session <uuid>:
   timeout (30s) waiting for idle before title-prompt`.
3. No crash. Cred block was already injected. Session remains usable.

### 12.9 Missing BRIEF.md — best-effort skip

1. Manually delete BRIEF.md from a workgroup (corrupted state).
2. Start the Coordinator.
3. Cred block injects. Auto-title skip log: `[session] Auto-title skipped for
   session <uuid>: read BRIEF.md at "...": ...`.
4. No crash. Session usable.

### 12.10 BRIEF.md template change — verbatim and empty cases

1. Create new workgroup with brief "Hello world".
2. `BRIEF.md` content: exactly `Hello world\n` (12 bytes). No `# wg-N-team`
   heading, no `## Objective` section.
3. Create new workgroup with no brief.
4. `BRIEF.md` content: exactly empty (0 bytes).
5. Existing workgroups (created before this branch landed) retain their
   templated BRIEF.md unchanged.
6. **R5 / G5 follow-up**: pick at least one pre-existing `wg-*` directory
   from before this branch landed. Restart its Coordinator. Verify the
   templated body is preserved verbatim and a `---\ntitle: ...\n---\n` block
   is prepended. Verify the `.bak` file (see §12.12) was created with the
   pre-edit body.

### 12.11 Settings round-trip

1. Toggle `autoGenerateBriefTitle` in the UI; save; reopen Settings; confirm
   the new value persists.
2. Inspect `~/.agentscommander/settings.toml` (or platform equivalent) and
   confirm the field is present with snake_case key
   `auto_generate_brief_title`.

### 12.12 Backup file is created and is byte-identical (R2 fold F6)

1. From state at end of 12.2 (right after the title-prompt is injected — i.e.
   between PTY-write #2 landing in the agent and the agent finishing its
   edit), inspect the `wg-*` root.
2. Verify a sibling file matching the glob
   `BRIEF.md.[0-9]{8}-[0-9]{6}.bak` exists.
3. Diff its bytes against the pre-edit `BRIEF.md` capture from step 12.2.2
   — they must be byte-identical.
4. App log line: `[session] Auto-title backup created: ... (session <uuid>)`.
5. Restart the Coordinator a second time. Confirm a SECOND `.bak` is **not**
   created (the title-present skip short-circuits before the snapshot step).
6. **Failure-mode probe** (R3 fold F11 / G20): make the `wg-*` dir read-only
   for the running user, then start the Coordinator on a no-title brief.
   Platform-specific recipes — pick the one that matches the dev box:

   ```
   Windows:        icacls "<wg-path>" /deny "<user>:(WD)"
       revert:     icacls "<wg-path>" /grant:r "<user>:(WD)"
   macOS / Linux:  chmod -w <wg-path>
       revert:     chmod u+w <wg-path>
   ```

   (Note: on Windows, `attrib +R <dir>` is advisory and ignored by most APIs
   — do NOT use it. `icacls /deny ...:(WD)` denies write-data on the
   directory itself, which is what `OpenOptions::create_new` will trip on.)

   Expected: warn-log
   `Auto-title skipped for session <uuid>: snapshot BRIEF.md before edit:
   ...`, NO `.bak` file appears, NO title prompt is injected, BRIEF.md
   unchanged. Restart the Coordinator after reverting permissions and verify
   the next attempt succeeds (idempotency).

### 12.13 Case-insensitive title key (R2 fold F3)

1. Create a workgroup with a brief; manually pre-edit BRIEF.md to add:
   ```
   ---
   Title: My Capital-T Title
   ---

   <body>
   ```
2. Start the Coordinator. Cred block injects.
3. Confirm the title-skip log fires (`Auto-title skipped (title present)`).
4. BRIEF.md is untouched. NO duplicate `title:` line is added.
5. Repeat with `TITLE:`, `tItLe:`, and any mixed casing.

---

## 13. Risks / edge cases

| Risk | Mitigation |
|---|---|
| Agent's edit inadvertently modifies body lines (re-quoting, lowercasing a heading, "fixing" what it perceives as a typo, stripping whitespace, reflowing markdown). The prompt says "do not modify or reflow any other line" but the agent is stochastic. | **Round 2 fold F6** — `snapshot_brief_before_edit` writes a timestamped `BRIEF.md.<UTC-ts>.bak` next to BRIEF.md immediately before each title-prompt inject. User can recover the pre-edit body by hand. Backups never auto-deleted; they go with the workgroup directory at destroy time. See §16. |
| Agent wraps the title in markdown / code fence | The custom parser strips `"`/`'` only. If the agent writes ``` `My title` ``` we accept it as a literal title (with backticks). Cosmetic glitch — user can edit; backup is available. |
| Agent emits `Title:` / `TITLE:` / mixed casing | **Round 2 fold F3** — `parse_brief_title` matches the key case-insensitively; agent variation no longer accumulates duplicate `title:` lines. See §6.2. |
| `PathBuf::to_string_lossy` mangles non-UTF-8 bytes on Windows | Real-world paths under `.ac-new` are ASCII-safe (sanitized at workgroup creation, see `entity_creation.rs::sanitize_name`). Lossy round-trip is the same approach used elsewhere in the codebase for PTY-bound strings. |
| Path embedded in prompt carries Windows extended-length `\\?\` prefix | **Round 2 fold F4** — strip the prefix in §9.4 before passing to `build_title_prompt`. Mirrors `pty/credentials.rs`'s normalisation. Path normalisation matches across PTY-injected paths. |
| Agent edits BRIEF.md mid-spawn-of-another-session targeting the same workgroup | Different sessions, different agents. Idempotent guard plus the F2 post-idle re-read catches the case where another writer landed `title:` between our pre-idle check and our inject. Worst case: redundant inject ignored by the agent's self-guard. |
| Two concurrent agent writers on the same BRIEF.md (G9 — startup-restore + manual restart, restart_session race, double-click on "start") | Best-effort: on Windows, `std::fs::write` opens with default sharing flags — one write succeeds, the other may hit a sharing violation. Idempotent retry on next restart picks it up. No file lock by design — overhead would not pay back for the rarity of the case. |
| Settings toggled mid-spawn | Snapshot pattern: bool captured before `tokio::spawn`. In-flight session uses the snapshot. New spawns honor the new value. Same as issue #84's `exclude_global_claude_md` snapshot. |
| Old workgroups (pre-feature) start their Coordinator | Their templated `BRIEF.md` lacks `title:` → auto-title fires and adds one. The body (template) is preserved verbatim. **This is desired** — gives existing workgroups a title without an explicit migration. **R5 follow-up**: a templated BRIEF that already contains `\n---\n` somewhere in the body (extremely unusual — current template has no horizontal rules) could let an instruction-following agent prepend a second `---\ntitle:...\n---\n` block; `parse_brief_title` reads the outer frontmatter correctly, so the file stays idempotent on next restart but is visually broken. Cosmetic; user fix; backup available. |
| Agent's PTY working dir is *not* a `wg-*` descendant (test session, ad-hoc CWD) — only reachable when team config flags a non-workgroup CWD as Coordinator | **Round 2 decision F7 / G6** (overrides dev-rust R9 silent-skip): keep `Err` + `warn`. Reaching this branch under the `is_coordinator` gate means a Coordinator is registered against a non-workgroup directory — a config inconsistency worth surfacing. Log line is prefixed `[auto-title:config]` for filterability. |
| The agent (Codex, Gemini) is NOT a coding agent that edits files | The prompt instructs it to do the edit. If the agent cannot, the file stays untitled. Next restart retries (idempotent). The feature is best-effort across agent families; Claude Code is the primary target. |
| Two simultaneous Coordinator spawns (rare — multi-team workspaces, distinct workgroups) | Independent — different sessions, different `cwd`, different BRIEF.md (one per workgroup). No shared state. The same-workgroup case is covered by the row above. |
| Idle detector false positive — agent quiet for >2500 ms during a long-running tool call gets flagged idle (G14) | Pre-existing behaviour shared with the cred-block path; not new to this plan. Claude Code queues PTY-typed user input and processes it after the tool result, so the title prompt simply lands later. Codex/Gemini behaviour is harder to predict but the worst case is the same as user typing into the PTY mid-tool-call (acceptable). |
| User types into the PTY during the spawn-window (G15) — title-prompt window grows from ~2 s (cred-block today) to up to ~64 s with this feature | If the user types fast enough to keep the agent BUSY through the full 30-s second-idle wait, the title-prompt times out and silent-warns. Title-gen silently gives up — user can restart. Acceptable; same shape as today's cred-block window with a larger magnitude. |
| Frontend renders the raw `---\ntitle: ...\n---\n` frontmatter as part of `workgroupBrief` (R9) | Out of scope for this plan. Once this lands, frontend may want to strip frontmatter when displaying `workgroupBrief`. Cosmetic; downstream UI fix in a separate plan. See §14. |
| Path with embedded backticks (G10 — paranoid; AC-created paths sanitize but user-supplied project root is not sanitized) | Out of scope; AC-created subpaths under `.ac-new/` are ASCII-safe by `sanitize_name`. If a user's outer project root contains a backtick, the prompt's single-backtick code span breaks; agent likely still does the right thing but format is brittle. Acceptable for MVP. |

---

## 14. What this plan does NOT do (out of scope)

- Migration of existing BRIEF.md files (the template-style headings stay).
- Re-generating titles after manual edits.
- Per-workgroup setting override.
- Backward-compat shims for the old `default_brief_content`.
- Polishing the prompt for non-English briefs (the prompt is in English; the
  agent is expected to follow regardless of brief language).
- Adding `serde_yaml` for richer frontmatter — punted to a future plan if/when
  another feature needs it.
- A frontend display affordance for the new title (downstream — frontend can
  parse the frontmatter and show it; that's a separate UX plan). The frontend
  may also want to strip frontmatter from the raw `workgroupBrief` preview
  pane once this lands (R9 / dev-webpage-ui follow-up).
- A generalised "agent edits the BRIEF body" prompt. The user has signalled
  this is coming next: future flows will reuse `snapshot_brief_before_edit`
  (§16) and define their own prompts. This PR keeps §8.1 strict ("title-only,
  no body modification") because the auto-title flow specifically does not
  want body edits — the relaxation arrives with the future flows that DO
  want them (R2 fold F6 §5).
- TS-side drift sweep (`darkfactoryZoom`, `rootToken`, `logLevel` missing
  from `AppSettings` interface — flagged by R6 + G7). Adding
  `autoGenerateBriefTitle` brings the unmodelled-field count to 4. Tech-lead
  to file as a separate follow-up issue after this PR lands.
- Coalescing the four `SettingsState` reads in `create_session_inner` into a
  single top-of-function snapshot (G12). Code-quality enrichment, not a
  blocker.
- Refactoring the post-spawn task to use a shared idle-poll helper (the
  cred-inject path and the title-prompt path now both implement the same
  poll loop inline). Today's codebase does the same in `phone/mailbox.rs`;
  factoring all three is its own refactor.

---

## 15. Implementation phase order

> **Round 2 fold F8 / G8** — **Single PR. Phases 1-4 ship together** on
> `feature/107-auto-brief-title`. The phase numbering below is a compile-
> order guide for the implementer, NOT a shipping schedule. Landing Phase 1
> (the empty/verbatim BRIEF.md template) without Phase 3 (the spawn hook)
> would create a regression window where new workgroups are created with
> empty briefs and no title generation — a visible UX bug. The single-PR
> rule prevents that.

1. **Setting & template** — §4 (Rust + TS) and §5. Compiles, settings round-
   trip works, new workgroups have verbatim/empty BRIEF.md.
2. **Helpers + unit tests** — §6 (`parse_brief_title`), §7
   (`find_workgroup_brief_path_for_cwd`), §8 (`title_prompt.rs` + `mod.rs`),
   §16 (`snapshot_brief_before_edit`). Pure-function helpers ship with the
   unit tests in §17. Easy to unit-test, no temp filesystem needed (except
   the snapshot helper, which uses a `tempfile`-like pattern via Tokio's
   tempdir or a manual `tempdir()` from `std::env::temp_dir`).
3. **Spawn-time hook** — §9 (`inject_title_prompt_after_idle_static` + the
   chain inside the existing spawned task).
4. **Manual test plan** — §12.

Each phase compiles independently. Phase 3 is the only one with runtime
behavior change; Phases 1-2 are inert without the wiring in Phase 3.

---

## 16. BRIEF.md backup helper — `snapshot_brief_before_edit` (R2 fold F6)

### 16.1 Why

The user's product decision (F6) is to make every agent-driven BRIEF.md edit
reversible via on-disk backups, kept until the workgroup is destroyed. This
covers the body-corruption risk dev-rust-grinch raised in G5 (an instruction-
following agent can stochastically mutate body lines despite a "title-only"
prompt) and seeds a generalisable mechanism the user has explicitly said they
will reuse for future "agent edits BRIEF body" flows.

### 16.2 Helper definition

Add to `src-tauri/src/commands/entity_creation.rs` immediately after
`parse_brief_title` (i.e. immediately after the §6.2 helper). `pub(crate)` so
`commands/session.rs` can call it.

```rust
/// Snapshot a BRIEF.md file to a sibling timestamped `.bak` before an
/// agent-driven edit. The caller is expected to invoke this immediately
/// before injecting any prompt that asks an agent to modify the file.
///
/// Filename pattern: `BRIEF.md.<YYYYMMDD-HHMMSS>.bak`, UTC timestamp.
/// Backups accumulate in the workgroup directory and are NOT auto-deleted —
/// they are removed alongside the workgroup when the `wg-*` directory is
/// destroyed.
///
/// Returns the path of the created backup on success.
///
/// Atomicity: the destination is opened with
/// `OpenOptions::new().write(true).create_new(true).open(...)` (R3 fold F9 /
/// G18). On collision the call returns `ErrorKind::AlreadyExists` —
/// `create_new` maps to `O_CREAT | O_EXCL` on POSIX and `CREATE_NEW` on
/// Windows, both atomic against same-name races. Reading-then-overwriting
/// via `std::fs::copy` would silently clobber an existing `.bak` from a
/// same-second collision, breaking F6's reversibility contract.
///
/// Failure modes (all surfaced as `io::Error`):
///   - Source `BRIEF.md` is missing/unreadable → `NotFound`.
///   - Destination directory is read-only or full → `PermissionDenied` /
///     `Other` / `StorageFull`.
///   - Same-second collision (two restarts of the same Coordinator on the
///     same workgroup within the same UTC second) → `AlreadyExists`. Caller
///     treats this as a transient `Err` and skips the title prompt for this
///     restart; idempotent retry next restart.
///
/// Pure I/O helper: no settings access, no PTY, no logging. Caller logs.
///
/// Round 2 fold F6 — see plan `_plans/107-auto-brief-title.md` §16. Designed
/// for reuse by future flows that ask an agent to edit BRIEF.md (the user
/// has said this is coming). Round 3 fold F9 hardened the implementation
/// from `std::fs::copy` (silent overwrite) to atomic `create_new`.
pub(crate) fn snapshot_brief_before_edit(
    brief_path: &std::path::Path,
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write;

    let parent = brief_path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "brief_path has no parent directory",
        )
    })?;
    let stem = brief_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "brief_path has no filename",
            )
        })?;
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let bak_name = format!("{}.{}.bak", stem, timestamp);
    let bak_path = parent.join(bak_name);

    // R3 fold F9 / G18 — atomic create-new. Drop std::fs::copy because it
    // silently overwrites, which breaks F6's reversibility contract on
    // same-second restart collisions.
    let mut source = std::fs::File::open(brief_path)?;
    let mut dest = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)  // atomic: O_EXCL on POSIX, CREATE_NEW on Windows; AlreadyExists on collision
        .open(&bak_path)?;
    std::io::copy(&mut source, &mut dest)?;
    dest.flush()?;  // required — create_new'd File does not auto-flush on drop in all error paths

    Ok(bak_path)
}
```

`chrono` is already a dependency (`Cargo.toml` line 14). No new imports
required at the top of the file — `chrono::Utc::now()` is fully qualified
inline. `std::io::Write` is brought in as a function-local `use` for
`flush()` — local-scope keeps it from cluttering the module's import list.

### 16.3 Lifecycle

| Phase | Behaviour |
|---|---|
| Created | Immediately before each successful title-prompt inject (§9.4 step 7). NEVER created on a skip. |
| Naming | `BRIEF.md.<YYYYMMDD-HHMMSS>.bak` (UTC). Distinct from any other AC artifact; sorts approximately chronologically — a backward NTP step may briefly violate ordering. Backups remain individually valid; the user inspects file `mtime` if exact ordering matters. Example: `BRIEF.md.20260501-013200.bak`. (R3 fold F10 / G19.) |
| Accumulation | Backups accumulate over a workgroup's lifetime — by design. Disk cost is small (BRIEF.md is typically <1 KB). **Worst case** (R3 fold F12 / G21): one `.bak` per Coordinator restart that reaches §9.4 step (7) — i.e. the agent repeatedly fails to actually write the title (every restart re-prompts, every prompt produces a fresh `.bak`). **Typical case**: one `.bak` per workgroup lifetime, after which the title-skip short-circuit at §9.4 gate (4) prevents further snapshots. |
| Cleanup | None. Backups go with the `wg-*` directory at workgroup destroy time. AC has no `wg-*` cleanup workflow that touches files inside the directory; the existing destroy-workgroup paths remove the entire directory. |
| Reuse | Future flows that ask an agent to edit BRIEF.md call the same helper before their inject. The helper is intentionally pure-I/O so it remains a drop-in. |

### 16.4 Failure mode and caller contract

If `snapshot_brief_before_edit` returns `Err`, the caller (§9.4 step 7) must:

1. Map the error into a `String` for the warn-log.
2. Return `Err` from `inject_title_prompt_after_idle_static` so the calling
   `match` in §9.3's `Ok(())` arm logs the warn.
3. NOT inject the title prompt.

This is the "snapshot failure → skip the inject" rule from F6. Better to skip
the feature than to let the agent edit an unbacked-up file. The next
Coordinator restart re-tries the entire helper from scratch — idempotent.

---

## 17. Unit tests (R2 fold F5)

These ship in Phase 2 alongside the helpers. All deterministic, no temp
filesystem (except the snapshot helper test, which uses a per-test `tempdir`
under `std::env::temp_dir()` and cleans up).

### 17.1 `parse_brief_title`

Add `#[cfg(test)] mod tests { … }` at the bottom of
`src-tauri/src/commands/entity_creation.rs` (no module exists today —
this is the first):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_brief_title — dev-rust R7 cases ──

    #[test]
    fn parse_brief_title_returns_some_for_canonical_frontmatter() {
        assert_eq!(parse_brief_title("---\ntitle: Hello world\n---\n\nbody\n"),
                   Some("Hello world".to_string()));
    }

    #[test]
    fn parse_brief_title_strips_double_quotes() {
        assert_eq!(parse_brief_title("---\ntitle: \"Quoted\"\n---\n"),
                   Some("Quoted".to_string()));
    }

    #[test]
    fn parse_brief_title_strips_single_quotes() {
        assert_eq!(parse_brief_title("---\ntitle: 'Quoted'\n---\n"),
                   Some("Quoted".to_string()));
    }

    #[test]
    fn parse_brief_title_returns_none_when_no_frontmatter() {
        assert_eq!(parse_brief_title("# Heading\n\nbody\n"), None);
    }

    #[test]
    fn parse_brief_title_returns_none_for_empty_value() {
        assert_eq!(parse_brief_title("---\ntitle:\n---\n"), None);
    }

    #[test]
    fn parse_brief_title_returns_none_when_closing_delimiter_missing() {
        assert_eq!(parse_brief_title("---\ntitle: foo\nbody only\n"), None);
    }

    #[test]
    fn parse_brief_title_returns_none_when_title_field_absent() {
        assert_eq!(parse_brief_title("---\nname: foo\n---\n"), None);
    }

    #[test]
    fn parse_brief_title_preserves_inner_colon() {
        assert_eq!(parse_brief_title("---\ntitle: a: b\n---\n"),
                   Some("a: b".to_string()));
    }

    #[test]
    fn parse_brief_title_handles_indented_key() {
        assert_eq!(parse_brief_title("---\n  title: foo\n---\n"),
                   Some("foo".to_string()));
    }

    // ── parse_brief_title — dev-rust-grinch G3 / G13 case-insensitivity ──

    #[test]
    fn parse_brief_title_handles_capital_t() {
        assert_eq!(parse_brief_title("---\nTitle: Foo\n---\n"),
                   Some("Foo".to_string()));
    }

    #[test]
    fn parse_brief_title_handles_all_caps_key() {
        assert_eq!(parse_brief_title("---\nTITLE: Foo\n---\n"),
                   Some("Foo".to_string()));
    }

    #[test]
    fn parse_brief_title_handles_mixed_case_key() {
        assert_eq!(parse_brief_title("---\ntItLe: Foo\n---\n"),
                   Some("Foo".to_string()));
    }

    #[test]
    fn parse_brief_title_value_remains_case_sensitive() {
        // The key match is case-insensitive; the value MUST round-trip
        // verbatim (it is user-visible content, not a structural marker).
        assert_eq!(parse_brief_title("---\nTitle: MixedCASE Value\n---\n"),
                   Some("MixedCASE Value".to_string()));
    }

    // ── snapshot_brief_before_edit — F6 ──

    #[test]
    fn snapshot_brief_before_edit_creates_byte_identical_copy() {
        let dir = std::env::temp_dir().join(format!(
            "ac-snapshot-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let brief = dir.join("BRIEF.md");
        let body = b"# A brief\n\nWith some body content.\n";
        std::fs::write(&brief, body).unwrap();

        let bak = snapshot_brief_before_edit(&brief).unwrap();

        assert!(bak.exists());
        let bak_name = bak.file_name().unwrap().to_string_lossy().to_string();
        assert!(bak_name.starts_with("BRIEF.md."));
        assert!(bak_name.ends_with(".bak"));
        // The infix is a YYYYMMDD-HHMMSS UTC timestamp — 15 chars.
        let infix = &bak_name["BRIEF.md.".len()..bak_name.len() - ".bak".len()];
        assert_eq!(infix.len(), 15);
        assert_eq!(infix.chars().nth(8), Some('-'));

        let copied = std::fs::read(&bak).unwrap();
        assert_eq!(copied, body);

        // Cleanup.
        let _ = std::fs::remove_file(&brief);
        let _ = std::fs::remove_file(&bak);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn snapshot_brief_before_edit_errors_when_source_missing() {
        let phantom = std::env::temp_dir().join(format!(
            "ac-snapshot-missing-{}-BRIEF.md",
            std::process::id()
        ));
        let result = snapshot_brief_before_edit(&phantom);
        assert!(result.is_err());
    }

    // R3 fold F9 / G18 — exercise the atomic-create-new collision path.
    // Calling the helper twice within the same UTC second on the same
    // source must yield Err(ErrorKind::AlreadyExists) on the second call;
    // the original .bak from the first call is preserved untouched.
    #[test]
    fn snapshot_brief_before_edit_returns_already_exists_on_collision() {
        let dir = std::env::temp_dir().join(format!(
            "ac-snapshot-collision-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let brief = dir.join("BRIEF.md");
        std::fs::write(&brief, b"first body\n").unwrap();

        let bak1 = snapshot_brief_before_edit(&brief).unwrap();
        let bak1_bytes = std::fs::read(&bak1).unwrap();

        // Mutate source between calls so we can prove the collision did NOT
        // overwrite bak1 with the new body.
        std::fs::write(&brief, b"second body — must not land in bak1\n").unwrap();

        let err = snapshot_brief_before_edit(&brief)
            .expect_err("second call within same UTC second must collide");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

        // bak1 untouched.
        let bak1_after = std::fs::read(&bak1).unwrap();
        assert_eq!(bak1_after, bak1_bytes);
        assert_eq!(bak1_after, b"first body\n");

        // Cleanup.
        let _ = std::fs::remove_file(&brief);
        let _ = std::fs::remove_file(&bak1);
        let _ = std::fs::remove_dir(&dir);
    }
}
```

### 17.2 `find_workgroup_brief_path_for_cwd`

Add to the existing `mod tests` in `src-tauri/src/session/session.rs`:

```rust
#[test]
fn find_workgroup_brief_path_returns_path_when_cwd_is_workgroup_root() {
    let p = find_workgroup_brief_path_for_cwd(r"C:\proj\.ac-new\wg-3-team");
    assert_eq!(
        p,
        Some(std::path::PathBuf::from(r"C:\proj\.ac-new\wg-3-team\BRIEF.md"))
    );
}

#[test]
fn find_workgroup_brief_path_walks_up_from_replica_dir() {
    let p = find_workgroup_brief_path_for_cwd(
        r"C:\proj\.ac-new\wg-3-team\__agent_dev-rust"
    );
    assert_eq!(
        p,
        Some(std::path::PathBuf::from(r"C:\proj\.ac-new\wg-3-team\BRIEF.md"))
    );
}

#[test]
fn find_workgroup_brief_path_returns_none_outside_workgroup() {
    assert_eq!(find_workgroup_brief_path_for_cwd(r"C:\Users\me\misc"), None);
}

#[test]
fn find_workgroup_brief_path_handles_unc_prefix_input() {
    // The helper is a pure path walk; it does not strip `\\?\` itself —
    // §9.4 strips the prefix downstream when embedding into the prompt.
    // This test documents that the walk-up still finds the wg-* ancestor
    // even when the input carries the prefix.
    let p = find_workgroup_brief_path_for_cwd(r"\\?\C:\proj\.ac-new\wg-3-team");
    assert!(p.is_some());
    let p = p.unwrap().to_string_lossy().to_string();
    assert!(p.ends_with(r"\wg-3-team\BRIEF.md"));
}
```

### 17.3 `build_title_prompt`

Already specified inline in §8.1 (two `#[cfg(test)]` cases). Add the
whitespace / special-character case from G13:

```rust
#[test]
fn build_title_prompt_handles_path_with_spaces() {
    let p = build_title_prompt(r"C:\Program Files\Stuff\.ac-new\wg-1-x\BRIEF.md");
    assert!(p.contains(r"C:\Program Files\Stuff\.ac-new\wg-1-x\BRIEF.md"));
    // Format-string template still intact.
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
```

---

## Dev-rust review (added by dev-rust)

I read the plan against the current branch state (`feature/107-auto-brief-title`)
and walked every line/function citation. Summary of what's verified, what's
wrong, and what to enrich.

### R1. Verified — no change needed

| Plan ref | Claim | Status |
|---|---|---|
| §3 | Touched-file list (8 modified + 1 created) | ✅ Complete and correct |
| §4.1 | `log_level: Option<String>` at line 151 in `settings.rs` | ✅ |
| §4.1 | `default_true()` at line 154, reusable | ✅ |
| §4.1 | `Default` impl at lines 186-230, `coord_sort_by_activity` at line 226 | ✅ Off by one — the impl is 186-**230** not 186-229; insertion location still correct |
| §4.2 | `coordSortByActivity` is the last `AppSettings` field (line 156) | ✅ |
| §4.3 | `startOnlyCoordinators` checkbox block at SettingsModal.tsx:296-306 | ✅ |
| §5.1 | `default_brief_content` at lines 179-184, `build_brief_content` at 186-196, call site at line 522 | ✅ Byte-for-byte matches the plan's "Before" block |
| §6.2 | Insert after `parse_role_frontmatter` (line 177); same `---`-delimited shape | ✅ |
| §7.1 | Walk-up logic checks the CWD itself first (`current = Some(Path::new(cwd))` then tests `is_workgroup_dir` before recursing) | ✅ — when CWD is `wg-1-team` itself, helper returns `wg-1-team/BRIEF.md` on the first iteration. Tech-lead question Q7 confirmed. |
| §7.2 | `Path` already imported at session.rs:3, no new `use` | ✅ |
| §7.2 | Existing call site at session.rs:202 (`workgroup_brief: read_workgroup_brief_for_cwd(...)`) — refactor preserves behavior | ✅ |
| §9.1 | Cred-inject task at session.rs:510-568, gated by `if agent_id.is_some()`, idle poll at 516-541, cred build + match at 543-566 | ✅ |
| §9.4 | Polling pattern (max 30s, 500ms tick, `mgr.list_sessions().await`, `s.waiting_for_input` check, `None → return`) is byte-equivalent to cred-inject path | ✅ — this IS the idiomatic pattern in this codebase; no dedicated helper exists. (Tech-lead question Q2 answered.) |
| §9.5 | Imports `Uuid` (line 4), `AppHandle` (line 3 via `tauri::{...}`), `Arc` (line 1), `SessionManager` (line 10) all already present. `tokio::sync::RwLock` is referenced inline elsewhere — same pattern works. | ✅ |
| §11/Q3 | Idempotent skip via `parse_brief_title(&content).is_some()` is sound for the back-to-back-restart race | ✅ |
| Existing tests | Settings round-trip tests (`coord_sort_by_activity_defaults_when_missing_from_json`, `log_level_defaults_to_none_when_missing_from_json`) won't break — the new field has `#[serde(default = "default_true")]`. Session-info tests in session/session.rs don't assert on `workgroup_brief`. No tests in entity_creation.rs to break. | ✅ Cargo test will stay green |

### R2. Critical correction — §9.2 `cfg` is OUT of scope at line 510

**The plan's optimistic path will not compile.** Re-read session.rs:321-331:

```rust
let (agent_id, agent_label) = {
    let settings_state = app.state::<SettingsState>();
    let cfg = settings_state.read().await;
    resolve_actual_agent(...)
};  // ← cfg dropped here
```

`cfg` is bound inside the inner `{ ... }` block and dropped at line 331. There
is no function-level `cfg` binding. The plan's "verified in the Read excerpts"
remark in §9.2 is incorrect — the architect appears to have conflated this
inner-block guard with the one at session.rs:630 which lives in
`create_session` (the outer Tauri command), not in `create_session_inner`.

**Required change:** delete the optimistic capture and treat the contingency
block as the only path. Concretely, in §9.2 replace the two-clause "Add two
new captures … If cfg is no longer in scope …" with this single block,
inserted **immediately before** the `tokio::spawn` at line 515:

```rust
let is_coordinator_clone = is_coordinator;  // bool, Copy
let auto_title_enabled = {
    let settings_state = app.state::<SettingsState>();
    let cfg = settings_state.read().await;
    cfg.auto_generate_brief_title
};
```

No locks are held at line 510 (the function-level `mgr` from line 338 was
dropped before line 351 by the inferred shadowing in the `let mut session =`
binding's expression scope, and the only other recent `await` was
`mgr.set_effective_shell_args(...)` at line 498 which holds the SessionManager
lock briefly and releases it). Adding one settings `read().await` here is
deadlock-free.

The plan's snapshot reasoning (mid-spawn toggle is intentionally ignored) is
unchanged — we just take the snapshot 175 lines later than the plan implied.

### R3. Race resilience — re-read brief after the idle wait

In §9.4 the helper checks `content.trim().is_empty()` and
`parse_brief_title(&content).is_some()` **before** the second idle wait. Up
to 30 seconds can elapse between those checks and the prompt inject. During
that window, anything else writing to BRIEF.md (a sibling agent's auto-title
prompt, manual edit) would not invalidate our cached `content`.

Concretely, the helper as drafted will log
`Auto-title prompt injected for session ...` even when the agent processes
the prompt as a no-op (because the file already gained a title during the
wait). Not broken — the prompt's last rule (`If the file already starts with
'---' and contains a 'title:' field, do nothing`) still makes it idempotent
on the agent side — but the log line lies, and we make the agent do an
unnecessary file read.

**Recommended change:** in §9.4, re-read the brief content **after** the idle
wait succeeds, run the empty/title checks again, and only then build and
inject the prompt. Adds one `std::fs::read_to_string` — negligible cost. Diff
sketch (replaces the section from "// Wait for idle a SECOND time" through
"let path_str = ..."):

```rust
// Wait for idle (same shape as cred path)…
loop { /* unchanged poll loop */ }

// Re-read after the wait — sibling agents or manual edits during the
// 0–30 s wait can change the file under us.
let content = std::fs::read_to_string(&brief_path)
    .map_err(|e| format!("re-read BRIEF.md at {:?}: {}", brief_path, e))?;
if content.trim().is_empty() {
    log::info!("[session] Auto-title skipped (BRIEF empty post-idle) for session {}", session_id);
    return Ok(());
}
if parse_brief_title(&content).is_some() {
    log::info!("[session] Auto-title skipped (title present post-idle) for session {}", session_id);
    return Ok(());
}

let path_str = brief_path.to_string_lossy().to_string();
…
```

The pre-idle checks stay where they are — they let us short-circuit before
even waiting on a stale 30-second timer.

### R4. `parse_brief_title` edge cases — current parser is good enough, document caveats

I walked the seven cases from the tech-lead's Q4 brief. Most resolve correctly.
The parser is a simple sibling of `parse_role_frontmatter` and we reuse its
shape deliberately (§6.1).

| Input | Parser behavior | Verdict |
|---|---|---|
| `---\ntitle: foo: bar\n---\n` (colon in title) | `strip_prefix("title:")` → ` foo: bar`, `trim()` → `foo: bar` | ✅ Preserves the inner colon |
| `  title: foo` (indented) | `line.trim()` runs first, then `strip_prefix("title:")` | ✅ |
| `\r\n` (CRLF) | `lines()` splits on `\n`, the trailing `\r` is removed by `trim()` | ✅ |
| BOM-prefixed file (`\u{FEFF}---\n…`) | `content.starts_with("---")` returns false | ⚠️ Treated as "no title" — agent will be asked to add one. After agent writes, BOM presence is unspecified. Acceptable; AC-created BRIEF.md never has a BOM. |
| YAML block scalar (`title: \|\n  Multi\n  line`) | Captures `\|`, `trim_matches('"')` no-op, returns `Some("|")` | ⚠️ Bogus output. Mitigation: prompt instructs single-line title. Idempotency means a follow-up restart won't fix it (`parse_brief_title` returns `Some("|")` so we skip). User can fix manually. |
| Frontmatter with no closing `---` (`---\ntitle: foo\n` only) | `rest.find("---")?` → `None`, function returns `None` | ⚠️ Treated as "no title" — agent will be asked to insert frontmatter at top, which would result in a doubled-up `---` opener. **Real risk** — see R5. |
| Trailing-colon empty (`---\ntitle:\n---\n`) | `value.is_empty()` → returns `None` | ✅ |
| `title:` literal in body (after closing `---`) | Iteration is over frontmatter slice only | ✅ |

Overall I am OK with the parser as-drafted, **with one note**: §6.2 should
add a brief comment that the parser is "best-effort frontmatter detection,
not a YAML implementation" so future readers don't expect richer YAML.

### R5. Risk row review — old-workgroup pre-existing template (§13)

Tech-lead Q6 asks whether prepending frontmatter to a file starting with
`# wg-N-team\n\n## Objective…` is safe. Walking through what an instruction-
following LLM does with the §8.1 prompt:

1. Reads the file. Sees no `---` at top.
2. Sees the body is the templated heading + sections.
3. Per the prompt: "Format exactly: `---\ntitle: …\n---\n\n<existing brief
   body, unchanged>\n`. Rules: only add the frontmatter — do not modify or
   reflow any other line."
4. Output: `---\ntitle: …\n---\n\n# wg-N-team\n\n## Objective\n…`

This is the desired outcome for a competent agent. The actual risks:

- **Agent merges the H1 heading into the title line.** Mitigation: the
  prompt says "do not modify or reflow any other line." Acceptable risk —
  if it happens, idempotent retry won't help (now there's a title, no
  re-prompt). User edits manually.
- **Agent adds frontmatter but corrupts a line by re-quoting / reflowing
  markdown.** Same mitigation, same residual risk.
- **Agent adds duplicate `---` because the existing file has a `\n---\n`
  in the body** (e.g. an inline horizontal rule). Mitigation: §8.1 prompt
  is unambiguous about prepending. Real-world existing BRIEFs from the
  current template don't have horizontal rules; low risk.

Net: tagging the old-workgroup row in §13 as "low risk, monitor in §12.10
manual test" is accurate. **No change to plan needed**, but the test plan
§12.10 should add a step: "Check at least one pre-existing wg-* dir from
before this branch landed; restart its Coordinator; verify the templated
body is preserved verbatim and a `---\ntitle: …\n---\n` block is prepended."

The "no closing `---`" edge case from R4 deserves explicit acknowledgment in
§13: an agent following the prompt on a file that already has `---\n…\n` (no
closing) would prepend another `---\ntitle: …\n---\n`, producing
`---\ntitle: x\n---\n\n---\nfoo` — visually broken but `parse_brief_title`
would still read `Some("x")` from the outer frontmatter, so the helper would
no-op on next restart. Living with the cosmetic glitch is fine. **Add this
row to §13.**

### R6. TypeScript type drift — pre-existing `logLevel` gap

`AppSettings` in `src/shared/types.ts` (line 126-157) is missing `logLevel:
string | null` even though Rust `AppSettings` carries `log_level:
Option<String>` (settings.rs:151). Round-tripping works because
`SettingsAPI.update`/`get` ship the whole JSON blob — `logLevel` survives as
an unmodeled property — but the TS type is a lie about the runtime shape.

**Out of scope for this issue**, but the dev-rust-grinch reviewer should be
aware that adding `autoGenerateBriefTitle: boolean` to TS without also adding
`logLevel: string | null` deepens the existing drift by exactly one field. We
should NOT include the `logLevel` fix in this PR (clean issue boundary). File
a follow-up issue.

### R7. Test gap — unit-test the two pure helpers

§15 phases the work as MVP-only with manual tests. Two of the new helpers are
trivially unit-testable and worth covering:

**`parse_brief_title`** in `commands/entity_creation.rs` (place inside a
`#[cfg(test)] mod tests { … }` at the bottom of that file — none exists
today, so this is the first):

```rust
#[test]
fn parse_brief_title_returns_some_for_canonical_frontmatter() {
    assert_eq!(parse_brief_title("---\ntitle: Hello world\n---\n\nbody\n"),
               Some("Hello world".to_string()));
}

#[test]
fn parse_brief_title_strips_double_quotes() {
    assert_eq!(parse_brief_title("---\ntitle: \"Quoted\"\n---\n"),
               Some("Quoted".to_string()));
}

#[test]
fn parse_brief_title_strips_single_quotes() {
    assert_eq!(parse_brief_title("---\ntitle: 'Quoted'\n---\n"),
               Some("Quoted".to_string()));
}

#[test]
fn parse_brief_title_returns_none_when_no_frontmatter() {
    assert_eq!(parse_brief_title("# Heading\n\nbody\n"), None);
}

#[test]
fn parse_brief_title_returns_none_for_empty_value() {
    assert_eq!(parse_brief_title("---\ntitle:\n---\n"), None);
}

#[test]
fn parse_brief_title_returns_none_when_closing_delimiter_missing() {
    assert_eq!(parse_brief_title("---\ntitle: foo\nbody only\n"), None);
}

#[test]
fn parse_brief_title_returns_none_when_title_field_absent() {
    assert_eq!(parse_brief_title("---\nname: foo\n---\n"), None);
}

#[test]
fn parse_brief_title_preserves_inner_colon() {
    assert_eq!(parse_brief_title("---\ntitle: a: b\n---\n"),
               Some("a: b".to_string()));
}

#[test]
fn parse_brief_title_handles_indented_key() {
    assert_eq!(parse_brief_title("---\n  title: foo\n---\n"),
               Some("foo".to_string()));
}
```

**`find_workgroup_brief_path_for_cwd`** in `session/session.rs` (extend the
existing `mod tests`):

```rust
#[test]
fn find_workgroup_brief_path_returns_path_when_cwd_is_workgroup_root() {
    // Synthetic path — no FS access needed since helper is pure path walk.
    let p = find_workgroup_brief_path_for_cwd(r"C:\proj\.ac-new\wg-3-team");
    assert_eq!(p, Some(std::path::PathBuf::from(r"C:\proj\.ac-new\wg-3-team\BRIEF.md")));
}

#[test]
fn find_workgroup_brief_path_walks_up_from_replica_dir() {
    let p = find_workgroup_brief_path_for_cwd(
        r"C:\proj\.ac-new\wg-3-team\__agent_dev-rust"
    );
    assert_eq!(p, Some(std::path::PathBuf::from(r"C:\proj\.ac-new\wg-3-team\BRIEF.md")));
}

#[test]
fn find_workgroup_brief_path_returns_none_outside_workgroup() {
    assert_eq!(find_workgroup_brief_path_for_cwd(r"C:\Users\me\misc"), None);
}
```

These tests are deterministic, no tempfs, and run in <1 ms. Worth adding to
§15 as part of Phase 2.

### R8. Phase ordering observation — Phase 1 ships a half-feature

Phase 1 (the BRIEF.md template change) lands BEFORE Phase 3 (the spawn-time
hook). Between those phases, new workgroups will have empty BRIEF.md and no
title generation. That's a one- to two-PR window of "feature half on" UX. If
the team plans to ship phase-by-phase as separate PRs to main, the order
should be flipped: Phase 2 (helpers, all dead code) → Phase 3 (hook, behind
default-on setting) → Phase 1 (template change). Then on the day the
template change lands, the spawn-time hook is already there to fill in the
title.

Alternatively if all four phases ship as a single PR, ordering doesn't
matter. **Tech-lead's call** — flag for the cycle conversation. No code
change either way.

### R9. Two minor nits

**§9.4, log line on path resolution failure.** The helper currently returns
`Err("no wg- ancestor in cwd '{}'", cwd)` and the caller logs at `warn`. For
the common dev case of running an agent in a non-workgroup CWD (root agent
sessions, ad-hoc shells), this would spam the log on every Coordinator
spawn. Drop to `info` level inside the caller arm for this specific Err
variant, or — simpler — have the helper return `Ok(())` for the
"no wg- ancestor" case (it's an expected non-coordinator-deployment shape,
not an error). Recommend the latter; treat "not in a wg-" the same as "BRIEF
empty" — silent skip with an `info` log.

**§13 risks table — add row for restart with prior auto-title.** When
`restart_session` fires after a prior auto-title run, the brief already has
a title, the helper short-circuits at the title-present check, and the
agent's context (built from `read_workgroup_brief_for_cwd` at line 202)
includes the frontmatter as part of the brief content. The frontend may
render the raw `---\ntitle: …\n---\n` as part of `workgroupBrief`. Out of
scope for this plan to fix the rendering, but flag for the dev-webpage-ui
agent: once this lands, frontend may want to strip frontmatter when
displaying `workgroupBrief`. Add a row to §13: "Frontend renders raw
frontmatter in the brief preview pane → cosmetic; downstream UI fix."

### R10. Disagreements with the architect

Just one substantive disagreement: **§9.2 cfg-in-scope assertion is
factually wrong** (see R2). The contingency must become the primary path.

Everything else is enrichment, not disagreement. Plan is solid.

### Ready-to-implement summary

If R2, R3, R5 (§13 row), R7 (tests), and R9 (silent-skip on no-wg-ancestor)
are folded in, this plan is implementable as drafted. I'll wait for
dev-rust-grinch's pass before starting Phase 1.

---

## Dev-rust-grinch review (added by dev-rust-grinch)

I read the plan, the dev-rust review, and the actual code on
`feature/107-auto-brief-title`. Independent verification of dev-rust's load-
bearing claims, plus new findings dev-rust missed. Severities below.

### G1. CONFIRM dev-rust R2 — `cfg` scope (HIGH, blocks implementation)

Verified against `commands/session.rs:321-331`:

```rust
let (agent_id, agent_label) = {
    let settings_state = app.state::<SettingsState>();
    let cfg = settings_state.read().await;
    resolve_actual_agent(&shell, &shell_args, agent_id.as_deref(), agent_label.as_deref(), &cfg)
};  // ← cfg dropped here at line 331
```

`cfg` is a block-local binding. Dropped at end-of-block expression. There is
no function-level `cfg` in `create_session_inner`. The `cfg` at line 630 is
in `create_session` (the outer Tauri command), a different function — the
architect's "verified in the Read excerpts" remark in §9.2 conflated the two.

**Plan rework required.** §9.2 must drop the optimistic path entirely and use
the contingency block (fresh `settings_state.read().await`) as primary, as
dev-rust drafted in R2. No alternative.

### G2. CONFIRM dev-rust R3 — pre-idle/post-idle race (MEDIUM, fold in)

Walking the timeline: helper reads `content` at the entry, runs empty/title
checks, then the second idle-wait can sleep up to 30s before the inject. In
that 30s window, anything else (sibling agent's auto-title prompt landing
ahead of ours, manual edit, restart_session race) can mutate BRIEF.md and
invalidate our cache. R3's re-read after idle-wait is correct — fold in.

I would also fold in the TOCTOU acknowledgement comment that mailbox.rs:961-
962 carries on its analogous helper:

```rust
// TOCTOU: agent could become busy / file could change between the idle
// check and this write. Acceptable for best-effort title-gen — the prompt
// itself instructs the agent to re-check the file at execution time.
```

### G3. CONFIRM dev-rust R4 — but parser case-sensitivity is a real bug (MEDIUM, fold in)

I walked the same seven cases plus the three the tech-lead called out:

**3a. Brief body contains `---\ntitle: foo`.** Two sub-cases:
- File starts with arbitrary text, body has the `---/title/---` block later.
  Parser's `content.starts_with("---")` → false. Returns None. Helper prompts
  agent. Agent prepends frontmatter. Net: body's stray `---/title/---` block
  is preserved as body text. Acceptable.
- File starts with `---/something/---/body-with-another-title-block`. Parser
  reads first frontmatter, may return Some(other-key) or None. If None,
  helper prompts. Agent appends a `title:` to the existing frontmatter (best
  case) or prepends another `---/title/---` block (worst case). The worst
  case yields visually broken file but `parse_brief_title` would correctly
  read the outer frontmatter on next restart, so no infinite re-prompt.
  Acceptable.

**3b. Agent emits `Title:` (capital T).** **REAL BUG.** §6.2 parser is
case-sensitive (`strip_prefix("title:")`). If the agent capitalizes the key:

```
---
Title: My workgroup
---
```

Parser returns `None`. Next restart → helper sees no title → prompts agent
again → agent writes ANOTHER `title:` line (lowercase this time, hopefully)
→ frontmatter ends up with both `Title:` and `title:` keys → parser now
matches the lowercase one → idempotent from this point.

Net damage: cosmetic duplicate field. Not catastrophic, but exactly the
class of agent-stochastic behavior an adversarial reviewer should harden
against — and it costs ~3 lines.

**Recommended fix**: make the prefix match case-insensitive on the key only
(value preserves original case):

```rust
for line in frontmatter.lines() {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("title:") {
        let val = &trimmed["title:".len()..];
        let value = val.trim().trim_matches('"').trim_matches('\'').to_string();
        if value.is_empty() {
            return None;
        }
        return Some(value);
    }
}
```

Add a unit test: `parse_brief_title("---\nTitle: Foo\n---\n")` → `Some("Foo")`.

**3c. Agent emits `title: ` (trailing space, empty value).** Parser's
`value.is_empty()` check returns None. Verified against §6.2:243-245.
Helper would re-prompt. Agent re-tries on retry. Acceptable.

### G4. NEW — `\\?\` UNC prefix not stripped from path (LOW, easy fix)

`find_workgroup_brief_path_for_cwd` (§7.1) returns a `PathBuf` built from a
`cwd: &str` ancestor. On Windows, when `cwd` originates from
`current_exe()` or certain `dirs::home_dir()` paths, it can carry the
`\\?\` extended-path prefix. The existing `pty/credentials.rs:38-44` and
:55-58 strips this prefix before embedding the path in PTY-bound text:

```rust
raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
```

§9.4 currently does `let path_str = brief_path.to_string_lossy().to_string();`
with no strip. If the cwd is `\\?\C:\proj\.ac-new\wg-1-team\__agent_x`, the
prompt embeds `\\?\C:\proj\.ac-new\wg-1-team\BRIEF.md`. Inside a markdown
code span the backslashes are literal — but agents (especially Codex/Gemini)
may pre-normalize the path or treat `\\?\` as garbage. Claude Code accepts
it but logs a warning.

**Recommended fix**: mirror the credentials.rs strip:

```rust
let raw = brief_path.to_string_lossy().to_string();
let path_str = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
```

Note in §13: "Path normalisation matches `pty/credentials.rs`'s prefix-strip
behavior — kept consistent across PTY-injected paths."

### G5. NEW — body corruption risk is under-flagged (MEDIUM, plan-doc fix)

Plan §13 has a one-line risk row: "Agent ignores the prompt or misformats
the frontmatter". This understates the threat. The prompt instructs the
agent to "do not modify or reflow any other line." The agent is stochastic
and can:

- Re-quote a markdown line (`'foo'` → `"foo"`).
- Insert a trailing newline that wasn't there.
- "Fix" what it perceives as a typo.
- Lowercase a heading.
- Strip leading/trailing whitespace from arbitrary lines.

There is **no programmatic guard** in the plan. AC writes the prompt and
walks away; whatever the agent does to BRIEF.md is final. The user has no
diff/recovery affordance — `.ac-new/` is `.gitignore`d so BRIEF.md is not
under VCS in the parent repo.

For a feature defaulting to **ON**, this is a non-trivial data-integrity
risk. Possible mitigations, in increasing order of work:

1. (cheap) Capture body bytes before injecting; on the next session start
   (or on a periodic check), compare body bytes — log warn on divergence.
   Doesn't prevent corruption, but makes it auditable.
2. (medium) Save a `.BRIEF.md.bak` next to BRIEF.md before prompting. User
   can recover manually.
3. (expensive) Don't trust the agent. Have AC parse the `title:` value out
   of the agent's response and write the frontmatter itself. This requires
   the agent to OUTPUT the title rather than EDIT the file — a different
   protocol than the plan describes.

**My recommendation for MVP**: option 1, plus a louder §13 row that calls
out the residual risk and references the issue for a follow-up enhancement.
Don't ship this as default-on without an audit log.

If tech-lead disagrees and ships without a guard, at minimum reword §13's
row from "ignores or misformats frontmatter" to "agent edit may
inadvertently modify body lines — best-effort feature, no guard." Honesty
matters more than wording matters here.

### G6. NEW — silent-skip on no-wg-ancestor is wrong (LOW, disagree with dev-rust R9)

Dev-rust R9 recommends the helper return `Ok(())` when no `wg-*` ancestor
is found, to avoid log spam. **I disagree.**

The path is gated by `is_coordinator_clone && auto_title_enabled` (§9.3).
`is_coordinator_clone` is computed via `is_coordinator_for_cwd` against the
team config. Reaching the helper means: a team config marks this CWD as a
coordinator, AND the CWD has no `wg-*` ancestor.

That state is a config inconsistency. It would mean a Coordinator agent
was registered against a non-workgroup directory. The helper firing this
case at INFO level (silent) hides what is genuinely a misconfiguration
worth surfacing.

The "ad-hoc shell" case dev-rust worries about doesn't reach this code
path — ad-hoc shells aren't coordinators. The "non-coordinator agent"
case is also gated out by `is_coordinator_clone`.

**Recommended fix**: keep the helper returning `Err`, keep the caller's
`log::warn!`. The "spam" is one line per Coordinator session start — and
it indicates a config bug, which is exactly what we want.

If we want to soften the message, prefix it with `[auto-title:config]` so
it's filterable. But don't silence it.

### G7. NEW — TS drift sweep is incomplete (LOW, follow-up issue scope)

Dev-rust R6 flagged `logLevel` as the sole pre-existing TS drift. **There
are at least two more.** I diffed Rust `AppSettings` (settings.rs:47-152)
against TS `AppSettings` (types.ts:126-157):

| Rust field | TS field | Drift |
|---|---|---|
| `darkfactory_zoom: f64` (line 95) | (missing) | TS lacks `darkfactoryZoom` |
| `root_token: Option<String>` (line 137) | (missing) | TS lacks `rootToken` |
| `log_level: Option<String>` (line 151) | (missing) | TS lacks `logLevel` |

Adding `autoGenerateBriefTitle` brings the unmodelled-field count to 4.
Round-trip works (the JSON blob carries them through transparently), but
the TS type is a lie about the runtime shape — a TS consumer reading
`settings.darkfactoryZoom` gets a runtime number with no compile-time
warning that the field exists.

**Out of scope for this PR (clean issue boundary).** The follow-up issue
dev-rust mentioned should cover all three pre-existing fields, not just
`logLevel`. File the follow-up against this issue or a new one.

### G8. NEW — phase ordering is more than a stylistic concern (MEDIUM, tech-lead call)

Dev-rust R8 noted but soft-pedaled the issue. I'm escalating it.

Plan §15 phase order: Phase 1 (template change) → Phase 2 (helpers) →
Phase 3 (spawn hook) → Phase 4 (manual test).

If shipped as a single PR: ordering doesn't matter at runtime — all changes
land together. **This is the only safe option for users.**

If shipped phase-by-phase to main: between Phase 1 and Phase 3, new
workgroups are created with **empty BRIEF.md and no title generation**.
That's a regression — existing users would see new workgroups that look
broken (no template, no title). Even one PR cycle in this state is a
visible UX bug.

**Recommended fix**: tech-lead must commit to single-PR rollout, OR flip
the order to Phase 2 → Phase 3 → Phase 1 (so the spawn hook is in place
when the template change lands).

Adding to plan §15 a note: "Phases 1-4 must ship as a single PR. Do not
land Phase 1 alone."

### G9. NEW — concurrent BRIEF.md writers (LOW, document)

Plan §13 dismisses the "two coordinator spawns same workgroup" case as
"different sessions, different agents, same idempotent guard." Walking
through more carefully:

Coordinators are typically 1:1 with a team / workgroup, but two spawns of
the SAME coordinator can happen via:
- Startup-restore + manual restart in quick succession.
- `restart_session` racing with web/Telegram-triggered spawn.
- User double-clicking "start" before debounce.

Timeline:
1. T+0: Spawn A starts cred-inject task. Reads BRIEF — no title.
2. T+0.1: Spawn B starts cred-inject task. Reads BRIEF — no title.
3. T+5: Both finish second-idle-wait. Both inject title prompt.
4. Two distinct agent processes both attempt to write BRIEF.md.

On Windows, Rust's `std::fs::write` opens with default sharing flags. Two
concurrent writers can collide with a sharing violation — one succeeds,
one returns Err. Best-effort: the failed agent reports the error to the
user. Idempotent retry on next restart picks it up.

**Recommended fix**: add a row to §13:

```
| Two concurrent agent writers on the same BRIEF.md | Best-effort: one
write succeeds, the other may hit a Windows sharing violation. Idempotent
retry on next restart. No file lock by design. |
```

Living with this is fine. Just don't pretend it can't happen.

### G10. NEW — agent recognises the bracket marker, but path delimiter is fragile (LOW, paranoid)

§8.1 wraps the path in single backticks: `` `{path}` ``. Single-backtick
spans don't escape inner backticks — if a path contains `` ` ``, the span
breaks and the rest of the prompt re-formats unpredictably.

`sanitize_name` (entity_creation.rs:90-110) strips non-alphanumeric/hyphen
chars from team and agent names, so AC-created paths are safe. But the
project root (`project_path`) is user-supplied and NOT sanitized for
backticks. If a user's project root happens to contain a backtick (rare
but legal on Windows for ASCII paths), the prompt is malformed.

**Recommended hardening** (cheap):
- Use a fenced code block instead of a single-backtick span (4-backtick
  fence handles paths containing 1-3 backticks).
- OR refuse paths containing backticks at the helper, returning Err with
  a useful message.

I'd accept this as-is for MVP and add a §13 row noting paths with
backticks are out of scope.

### G11. NEW — TOCTOU comment parity with mailbox.rs (LOW, doc)

`phone/mailbox.rs:961-962` explicitly acknowledges the post-idle TOCTOU on
the analogous follow-up helper:

```rust
// Note: same TOCTOU race as the command path — agent could become busy
// between the idle check above and this write. Acceptable for this use case.
```

§9.4's helper has the same race shape, no comment. Even after R3's re-read
fix lands, the agent could become busy between the re-read and the inject.
Add a parity comment so future readers don't think we missed it.

### G12. NEW — repeated settings reads are duplicate work (LOW, code quality)

The proposed fresh `settings_state.read().await` immediately before the
spawn (R2 fix) is the FOURTH read of `SettingsState` in
`create_session_inner`:
- Line 322-323 (read for `resolve_actual_agent`).
- Line 587 (read for `resolve_agent_label` fallback, hot path).
- Line 573-576 (read for `exclude_claude_md`, in workgroup-creation flow).
- New: read for `auto_generate_brief_title`.

Each is a `RwLock::read().await`. Concurrent readers don't block, so this
is correctness-safe. But it's noisy. Future enrichment (NOT in scope for
this PR) could fold all the per-call reads into a single snapshot at the
top of the function. Mention as a follow-up code-quality observation, not
a blocker.

### G13. Test coverage feedback on dev-rust R7

Dev-rust's proposed unit tests for `parse_brief_title` and
`find_workgroup_brief_path_for_cwd` are good. Add these to the list:

```rust
#[test]
fn parse_brief_title_handles_capital_t() {
    // After G3 fix.
    assert_eq!(parse_brief_title("---\nTitle: Foo\n---\n"),
               Some("Foo".to_string()));
}

#[test]
fn parse_brief_title_handles_mixed_case() {
    assert_eq!(parse_brief_title("---\nTITLE: Foo\n---\n"),
               Some("Foo".to_string()));
}

#[test]
fn find_workgroup_brief_path_strips_unc_prefix_input() {
    // After G4 fix in helper, but the helper itself just walks paths;
    // test the prompt builder strips the prefix in title_prompt.rs or in
    // the §9.4 helper.
    let p = find_workgroup_brief_path_for_cwd(r"\\?\C:\proj\.ac-new\wg-3-team");
    assert!(p.is_some());
    // The strip happens in §9.4 — exercise that there.
}
```

Also worth: a unit test for `build_title_prompt` confirming a path with
trailing whitespace doesn't break the format string (it shouldn't, but
verify).

### G14. Idle detection false-positive walkthrough (LOW, no fix)

Tech-lead Q9. The idle detector (`pty/idle_detector.rs:7`,
`IDLE_THRESHOLD = 2500ms`) flags a session idle after 2500ms of no PTY
output. The detector tracks PTY OUTPUT, not "is the agent waiting for user
input semantically." If an agent makes a long-running tool call that
produces no PTY output for >2500ms, it gets flagged idle.

For Claude Code: tool calls go via MCP-over-stdio (separate channels from
PTY). PTY output during a tool call is whatever Claude's TUI renders; it
may be quiet for >2500ms. Helper would inject mid-tool-call. Claude Code
queues PTY-typed user input and processes it after the tool result.
**Should be safe.**

For Codex/Gemini: input handling differs. If their PTY input buffer mixes
with in-flight tool messaging, an injected prompt mid-tool-call could be
mis-parsed. Hard to test cleanly without each CLI's source.

**Pre-existing concern** — the cred-block path has the same potential
issue today. Not new to this plan. No fix recommended; flag as known-
behavior in §13.

### G15. UX timeline — user typing during the spawn-window (LOW, pre-existing)

Tech-lead Q15. Today's cred-inject already has a ~2s window in which user
input collides. This plan extends the window to up to ~64s
(idle-poll + cred-inject + idle-poll + title-prompt-inject). User input
during this period interleaves at the byte level with our injections.

Not new (the cred-block has the same shape) but the magnitude is much
larger. Two scenarios:

1. User types fast post-spawn: their bytes reach PTY before idle-detector
   sees first PTY output; idle-detector flags the session BUSY (their
   typing IS PTY output). Idle never fires. Helper times out at 30s.
   Cred-block was injected anyway (cred path's fallback). Title-prompt
   never fires. **Acceptable** — title-gen is opt-out, user can
   restart.

2. User types after cred-block lands but before title-prompt fires: their
   bytes arrive at agent's input. Agent processes user message, becomes
   busy. Idle never fires for the title-prompt's wait. Title-prompt
   times out at 30s. **Acceptable** — same as 1.

Neither breaks correctness. Just UX: title-gen silently gives up if the
user is "fast enough." Note in §13.

### G16. Q16 walkthrough — title written before second-idle but after pre-checks (LOW)

Tech-lead Q16. The pre-idle checks in §9.4 read content, see no title,
proceed to wait. Within the 30s wait, the agent (acting on Role.md
instructions or its own initiative) writes a title. Idle-wait completes.
After R3's re-read fix lands, helper re-reads, sees title, returns
`Ok(())` with the "title present post-idle" log. **Correct outcome.**

Without R3's fix (current plan as drafted), helper would inject the
prompt anyway. Agent reads the file, sees title is now present, no-ops
per the in-prompt rule. Result is correct but the inject is wasted and
the log line lies ("prompt injected" when it had no effect). R3 already
addresses this — fold it in.

### G17. Disagreements with dev-rust

Two:
1. **R9 (silent-skip on no-wg-ancestor)** — disagree. Keep `Err` and
   `warn`. See G6.
2. **R6 (TS drift scope)** — incomplete. Three drift fields, not one.
   See G7.

Everything else in dev-rust's review I concur with.

### Severity summary and required-for-implementation list

| ID | Severity | Required before impl? |
|---|---|---|
| G1 (cfg scope, dev-rust R2) | **HIGH** | YES — already in dev-rust review |
| G2 (re-read brief, dev-rust R3) | MEDIUM | YES — already in dev-rust review |
| G3 (case-insensitive parser) | **MEDIUM** | YES — fold in |
| G4 (`\\?\` UNC strip) | LOW-MEDIUM | YES — fold in (one line) |
| G5 (body corruption guard / louder §13) | **MEDIUM** | YES (decision required: guard or louder doc) |
| G6 (warn vs silent on no-wg) | LOW | YES — disagreement with dev-rust to resolve |
| G7 (TS drift sweep) | LOW | NO — follow-up issue |
| G8 (phase ordering) | MEDIUM | YES (tech-lead call: single-PR or reordered) |
| G9 (concurrent writers row) | LOW | NO — doc-only nit |
| G10 (path with backticks) | LOW | NO — paranoid |
| G11 (TOCTOU comment parity) | LOW | NO — doc-only |
| G12 (settings read coalesce) | LOW | NO — future cleanup |
| G13 (additional unit tests) | LOW | YES — fold into Phase 2 alongside dev-rust R7 |
| G14 (idle false-positive) | LOW | NO — pre-existing, document |
| G15 (user input collision) | LOW | NO — pre-existing, document |
| G16 (Q16 timing) | LOW | NO — addressed by R3 fix |
| G17 (disagreements logged) | — | resolve in §5 cycle |

**Verdict**: plan is fundamentally sound. Five items (G1, G2, G3, G4, G5)
are required folds before implementation can start. Two items (G6, G8) are
decisions for the tech-lead to resolve. Everything else is enrichment or
known-acceptable.

If G1-G5 are folded and G6/G8 resolved, this plan is implementable.

---

## Round 2 amendments (architect)

Tech-lead's round-2 message
(`messaging/20260501-011833-wg5-tech-lead-to-wg5-architect-auto-brief-title-round2-folds.md`)
required eight fold-ins (F1-F8) — five mandatory technical folds, one user-
decided product fold, two tech-lead decisions on disagreements between
dev-rust and dev-rust-grinch. All applied below; review sections above stay
intact for the audit trail.

| Fold | Source | Where folded | What changed |
|---|---|---|---|
| **F1** | dev-rust R2 + grinch G1 (HIGH, blocks impl) | §9.2 | Dropped the "optimistic capture" path. The `cfg` opened at lines 322-323 of `create_session_inner` is bound inside an inner block and dropped at line 331 — there is no live `cfg` at line 510 (round 1 was wrong; the architect conflated it with the `cfg` at line 630 in the outer `create_session` Tauri command, a different function). The fresh-read pattern is now the **only** documented path: open a `SettingsState::read().await` immediately before `tokio::spawn`, capture the `bool`. Rationale + dev-rust + grinch citations included inline. Added a doc-only note about G12's "four reads" code-quality observation pointing to §14. |
| **F2** | dev-rust R3 + grinch G2 (MEDIUM) | §9.4 | Added step (6) "RE-READ BRIEF.md after the second idle wait" with both guards (`empty` and `parse_brief_title.is_some()`) re-run on the fresh content. The pre-idle checks stay as a fast short-circuit. Added the parity TOCTOU comment grinch G11 / mailbox.rs:961-962 reference. Updated the helper's doc-comment gate list and §10 sequencing diagram implicitly via the renumbered step list in §9.4.0. |
| **F3** | grinch G3 (MEDIUM) | §6.2 | `parse_brief_title` now matches the `title:` key case-insensitively. Implementation: `split_once(':')` then `key.trim().eq_ignore_ascii_case("title")` so we compare just the key; the value half is preserved verbatim (case-sensitive). The §8.1 prompt stays strict ("Format exactly: ...lowercase `title:`...") — strict prompt + tolerant parser is the simplest belt-and-braces shape. Documented the choice inline. |
| **F4** | grinch G4 (LOW-MEDIUM) | §9.4 step 8 + §13 row | Strip the Windows extended-length `\\?\` prefix from `path_str` before passing to `build_title_prompt`. Mirrors `pty/credentials.rs:38-44` exactly (`raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()`). Added a §13 row noting path normalisation parity across PTY-injected paths. |
| **F5** | dev-rust R7 + grinch G13 (LOW) | NEW §17, §15, §3 | New §17 lists every unit test from R7 + G13: `parse_brief_title` (9 R7 cases + 4 G13 case-sensitivity cases), `find_workgroup_brief_path_for_cwd` (3 R7 cases + 1 G13 UNC-prefix case), `build_title_prompt` (2 §8.1 cases + 2 G13 whitespace/special-char cases), `snapshot_brief_before_edit` (2 cases for F6's helper). All deterministic; only `snapshot_brief_before_edit_creates_byte_identical_copy` touches a per-test tempdir under `std::env::temp_dir()`. §15 renamed Phase 2 to "Helpers + unit tests" to bind the ship together. §3 already covers the helper file edits. |
| **F6** | user product decision | NEW §16, §9.4 step 7, §3, §12.12, §13, §14 | New §16 specifies `snapshot_brief_before_edit(brief_path: &Path) -> io::Result<PathBuf>` in `entity_creation.rs`. Filename pattern `BRIEF.md.<UTC-YYYYMMDD-HHMMSS>.bak`. §9.4 calls it as step (7), immediately before the prompt inject, only when ALL guards pass. Snapshot failure aborts the inject (no `.bak`, no prompt — better to skip than to edit unbacked-up). Lifecycle: backups accumulate, never auto-deleted, removed with the workgroup directory. §13 row "agent inadvertently modifies body lines" now references §16 as the recovery affordance. §14 documents that the helper is intentionally reusable for future "agent edits BRIEF body" flows the user has signalled. New test §12.12 verifies `.bak` creation, byte-identity, no-`.bak`-on-skip, and the snapshot-failure abort path. §3 file-touched row updated to mention the helper. |
| **F7** | tech-lead decision (G6 over R9) | §9.4 step 1, §13 row | Tech-lead override of dev-rust R9 silent-skip: keep `Err` and warn for "no `wg-*` ancestor". Reaching that branch under the `is_coordinator` gate means a Coordinator was registered against a non-workgroup CWD — a config inconsistency worth surfacing once per spawn. Log line prefixed `[auto-title:config]` for grep filterability. §13 row rewritten to cite the F7 decision and the rationale. |
| **F8** | tech-lead decision (G8) | §15 | Single-PR rollout. Phases 1-4 ship together on `feature/107-auto-brief-title`. The phase numbering remains as a compile-order guide for the implementer; landing Phase 1 alone (template change without spawn hook) would create a regression window where new workgroups have empty briefs and no title generation. New leading note in §15 enforces "single PR" explicitly. |

### Sections rewritten or added in round 2

- **§3** — file-touched row updated for the new `snapshot_brief_before_edit` helper.
- **§6.2** — `parse_brief_title` reworked for case-insensitivity (F3).
- **§9.2** — optimistic capture removed; fresh-read pattern is the only path (F1). G12 doc note added.
- **§9.4** — new §9.4.0 round-2 amendments preamble; helper rewritten with re-read (F2), `\\?\` strip (F4), `.bak` snapshot (F6), `[auto-title:config]` warn-prefix (F7); TOCTOU parity comment (G11).
- **§12.10** — added step 6 referencing the .bak verification for old-workgroup retro-titling (R5 follow-up).
- **§12.12 (NEW)** — backup file verification test.
- **§12.13 (NEW)** — case-insensitive title-key test.
- **§13** — risk table rewritten: agent-corruption row now points to F6's `.bak`; F3 and F4 rows added; F7 row rewritten; G9 concurrent-writers row added; G14 idle-detector row added; G15 user-input-collision row added; R9 frontend-render row added; G10 path-with-backticks row added; round-1's stale "Race: `cfg` guard scope" row removed (no longer applicable after F1).
- **§14** — extended out-of-scope list: G7 TS drift sweep, G12 settings-read coalescing, R9 frontend frontmatter render, future "agent edits body" generalisation (F6 §5).
- **§15** — single-PR note (F8) added; Phase 2 renamed to include unit tests (F5).
- **§16 (NEW)** — `snapshot_brief_before_edit` helper specification (F6).
- **§17 (NEW)** — full unit-test catalog (F5), grouped by helper.

### Items NOT folded (per tech-lead instruction)

- **G7 / R6** — TS drift (`darkfactoryZoom`, `rootToken`, `logLevel` missing from `AppSettings` interface). Tech-lead will file a follow-up issue. Documented in §14 only; no code change.
- **G9, G10, G11, G14, G15, G16** — accepted as low-severity edge cases. Documented in §13 where they touch existing rows; no new mitigations added.
- **G12** — settings-read coalescing. Code-quality enrichment, deferred. Doc-only note in §9.2 + §14.

### Net delta vs round 1

- 2 new code helpers documented (`snapshot_brief_before_edit`, F3 case-insensitive parser variant).
- 1 step added to the spawn-time helper (post-idle re-read with two guards).
- 1 line of path normalisation added (UNC-prefix strip).
- 4 new test cases for `parse_brief_title` (case-insensitivity), 1 new test for `find_workgroup_brief_path_for_cwd` (UNC), 2 new tests for `build_title_prompt` (path edge cases), 2 new tests for `snapshot_brief_before_edit`.
- 2 new manual test steps (§12.12 backup, §12.13 case-insensitivity); 1 new step in §12.10 (old-workgroup retro-title with backup verification).
- 1 stale row removed from §13 (cfg guard scope), several rewritten or added (F3, F4, F6, F7, G9, G14, G15, R9, G10).
- §14 expanded with 3 additional out-of-scope clarifications.
- §15 enforces single-PR rollout.

No new dependencies in `Cargo.toml`. `chrono` was already present (used by the snapshot helper's UTC timestamp formatter).

The plan is now implementable as written. dev-rust and dev-rust-grinch are the next reviewers — round 2 close-out per the tech-lead's protocol.

---

## Round 3 amendments (architect)

Round 2 ack-pass results: dev-rust ACK, dev-rust-grinch push-back on §16 with one MEDIUM finding (G18) and three LOW doc nits (G19, G20, G21). Tech-lead resolved G18 by picking grinch's Option A (atomic `create_new`) and folding all four into v3. Per grinch's pre-ACK statement, no further round-4 review is needed — once v3 publishes, dev-rust gets the implementation green light (Step 6).

Round 3 message:
`messaging/20260501-013753-wg5-tech-lead-to-wg5-architect-auto-brief-title-round3-folds.md`.

| Fold | Source | Where folded | What changed |
|---|---|---|---|
| **F9** | grinch G18 (MEDIUM) — tech-lead Option A | §16.2 helper body + docstring; §17 new test | Replaced `std::fs::copy` with `OpenOptions::new().write(true).create_new(true).open(...)` + `std::io::copy` + `dest.flush()`. `create_new` maps to `O_CREAT \| O_EXCL` on POSIX and `CREATE_NEW` on Windows — both atomic against same-name races. Same-second restart collisions now correctly return `ErrorKind::AlreadyExists` and the prior `.bak` stays intact (was: silent overwrite, breaking F6's reversibility contract). Docstring updated to call out the atomicity explicitly and to remove the misleading "between path build and `copy`" wording from the failure-modes list. New §17 test `snapshot_brief_before_edit_returns_already_exists_on_collision` exercises the path: writes a brief, snapshots it, mutates source between calls, snapshots again within the same UTC second, asserts `ErrorKind::AlreadyExists` and that the original `.bak` bytes are preserved. |
| **F10** | grinch G19 (LOW) | §16.3 Naming row | Replaced "sorts chronologically" with "sorts approximately chronologically — a backward NTP step may briefly violate ordering. Backups remain individually valid; the user inspects file `mtime` if exact ordering matters." Honesty about wall-clock ordering vs monotonic ordering. |
| **F11** | grinch G20 (LOW) | §12.12 step 6 | Replaced platform-fuzzy "make the `wg-*` dir read-only" with explicit Windows (`icacls /deny ...:(WD)`) and POSIX (`chmod -w`) recipes, including revert commands. Inline note that `attrib +R <dir>` is advisory and ignored by most APIs (do NOT use). Added an idempotency check after revert. |
| **F12** | grinch G21 (LOW) | §16.3 Accumulation row | Made the `.bak` accumulation upper bound explicit: worst case = one per Coordinator restart that reaches §9.4 step (7) (agent repeatedly fails to write the title); typical case = one per workgroup lifetime, after which the title-skip short-circuit at gate (4) prevents further snapshots. |

### Sections rewritten or added in round 3

- **§12.12 step 6** — platform-specific read-only recipes + revert + idempotency (F11).
- **§16.2 helper body** — `std::fs::copy` → atomic `create_new` + `flush` (F9).
- **§16.2 docstring** — atomicity callout, failure-modes corrected (F9).
- **§16.3 Naming row** — chronological-ordering caveat (F10).
- **§16.3 Accumulation row** — worst/typical case bound (F12).
- **§17 snapshot tests** — new `snapshot_brief_before_edit_returns_already_exists_on_collision` (F9).
- **THIS section** — Round 3 amendments (architect).

### Items NOT in scope for v3 (per tech-lead instruction)

- The §16.2 doc nit dev-rust flagged in their round-2 reply (claiming `std::fs::copy` returns `AlreadyExists`) was resolved transitively by F9 — switching the implementation makes the original `AlreadyExists` paragraph correct again. No separate doc-only fix needed.
- Anything else.

### Net delta vs round 2

- ~15 lines of code documented in §16.2 (helper body grew from 4 lines to ~13 lines; docstring grew by ~10 lines).
- 1 new unit test in §17 (collision path).
- 3 doc-only edits to §16.3 + §12.12 step 6.

No new `Cargo.toml` dependencies. `std::io::Write` brought in as a function-local `use` for `flush()` — kept out of the module's import list per minimal-blast-radius §3.

Per grinch's pre-ACK, this v3 is the implementation-ready plan. Tech-lead can hand to dev-rust for Step 6 immediately on receipt.

---

## Round 4 — combined PTY write + GOLDEN RULE amendment

Round 4 is **additive** to v3 — every previously-shipped helper, parser, snapshot, F-fold, and review section above stays. The audit trail of why we built things this way is intact. Round 4 documents the two coupled corrections triggered by a real-world bug observed after v3 shipped.

Round 4 source: `messaging/20260501-143541-wg5-tech-lead-to-wg5-architect-107-r4-combine-and-golden-rule.md`.

### §R4.1 Real-world finding (the bug v3 didn't anticipate)

After v3 (commits `807d863` → `3f7da00` → `e458b85`) shipped on `feature/107-auto-brief-title`, the user built and exercised the feature. The Coordinator-spawn flow injected the title prompt correctly (the agent processed it for ~28 s and even proposed a fitting title — *"Probando el creador de briefs"*) — but **the agent then refused to write to BRIEF.md**. The agent's verbatim reply:

> I cannot modify that file. The BRIEF.md is at `wg-2-a-team\BRIEF.md` — the workgroup root, which is a parent directory of my replica. Per the GOLDEN RULE in my context, I may only write to:
> 1. `repo-*` folders
> 2. My own replica directory: `...\wg-2-a-team\__agent_agent-1\`
> 3. My origin Agent Matrix's `memory/`, `plans/`, `Role.md`

So #107's PTY chain works end-to-end. The title-prompt arrives in the agent's input. The parser, the snapshot, the idle-wait, the gates — all behave correctly. **The agent's CLAUDE.md GOLDEN RULE template forbids the write semantically.** The materialised CLAUDE.md is generated by `default_context()` at `src-tauri/src/config/session_context.rs:478`, and its current shape lists exactly the three zones the agent recited.

The v3 plan never exercised this layer because the title-prompt design assumed the agent would simply do what the prompt asked. The GOLDEN RULE was external to v3's scope. It is no longer.

Two coupled corrections, **single PR** (still on `feature/107-auto-brief-title`):

- **Change A** — combine cred-block + title-prompt into one PTY write (fewer surfaces, smaller TOCTOU window, plausible bypass of the agent's mid-turn refusal).
- **Change B** — amend the GOLDEN RULE template to add a 4th allowed write zone: the workgroup `BRIEF.md` file (and only that file).

Change B is the load-bearing fix. Change A is opportunistic scaffolding that may help on its own but cannot be relied on in isolation.

---

### §R4.2 Change A — combine title-prompt into the cred-block PTY write

#### §R4.2.1 Design

When the auto-title preconditions hold (Coordinator + setting ON + brief non-empty + no `title:` field + snapshot succeeds), the title-prompt is **concatenated into the same `inject_text_into_session` call as the cred-block**. One PTY write. No second idle-wait. No second `inject_text_into_session`.

When ANY precondition fails, the cred-block is injected alone — exactly as today's `agent_id.is_some()` block at `commands/session.rs:702-709` already does for non-Coordinator sessions.

The decision tree collapses from "two awaits + two gates + two writes" to "one decision + one write".

#### §R4.2.2 Concatenation order and shape

Order: **cred-block first**, then a single blank line (`\n`), then title-prompt. Rationale:

- The cred-block ends with `# === End Credentials ===\n` (verified at `pty/credentials.rs:69`).
- The title-prompt starts with `[AgentsCommander auto-title]` (verified at `pty/title_prompt.rs:22`).
- Cred-block-first matches the user's mental model: "here is who you are, then here is your bootstrap task".
- Inserting an extra `\n` between them yields a visible blank line in the agent's transcript and gives the agent a paragraph boundary to parse.

Concrete combined string:

```
\n
# === Session Credentials ===
# Token: ...
# Root: ...
# Binary: ...
# BinaryPath: ...
# LocalDir: ...
# === End Credentials ===
\n
[AgentsCommander auto-title] Read the workgroup brief at `...\BRIEF.md` ...
```

The leading `\n` is already part of `build_credentials_block`'s output — unchanged. The trailing `\n` after `# === End Credentials ===` is the cred-block's own; we add **one** additional `\n` before the title-prompt for the blank-line separator.

`inject_text_into_session(..., submit=true)` then writes the entire string, sleeps 1500 ms, sends `\r`, sleeps 500 ms, sends `\r` again. The two Enter keystrokes are unchanged from today; they apply to the entire pasted block, not to each segment.

#### §R4.2.3 Where the read, snapshot, and concat happen

Inside the existing `tokio::spawn` task at `commands/session.rs:674-746`, **after** the idle-wait at lines 679-700 and **before** the `build_credentials_block` call at line 702. The sequence becomes:

1. Idle-wait (existing, unchanged — lines 679-700).
2. **NEW**: compute `title_appendage: Option<String>` via `build_title_prompt_appendage(&cwd_clone)` — the synchronous helper introduced in §R4.2.4. Gated by `is_coordinator_clone && auto_title_enabled` (existing capture at lines 668-673).
3. Build `cred_block` (existing, unchanged — line 702).
4. Build `combined`:
   - `Some(p)` → `format!("{}\n{}", cred_block, p)`
   - `None` → `cred_block`
5. Single `inject_text_into_session(&app_clone, session_id, &combined, true).await` — replaces the existing call at lines 704-710.
6. On `Ok(())` — log success, **stop**. The chain to `inject_title_prompt_after_idle_static` at lines 718-736 is **deleted** (see §R4.4).
7. On `Err(e)` — log warn (existing behavior, unchanged).

#### §R4.2.4 New helper — `build_title_prompt_appendage`

Add at module scope inside `src-tauri/src/commands/session.rs`. Place **immediately before** `pub async fn create_session_inner` (where `inject_title_prompt_after_idle_static` lives today, at line 287). It is private (no `pub`), **synchronous** (no `async`, no `await`), and takes only `cwd: &str`.

```rust
/// Issue #107 round 4 — build the title-prompt segment to concat with the
/// cred-block, OR `Ok(None)` if the auto-title preconditions don't hold.
///
/// Synchronous: filesystem reads + snapshot only, no PTY, no await. The
/// caller is the post-spawn task in `create_session_inner`; it concatenates
/// the returned `Some((prompt, _))` with the cred-block and issues a single
/// `inject_text_into_session` call (round 4 design — see plan §R4.2). The
/// returned `bak_path` is surfaced to the caller for the success log line —
/// see R4.D3 fold for why this is preferable to dropping it on the floor.
///
/// Gates layered (in order):
///   1. workgroup BRIEF.md path resolvable from `cwd` → else `Err`
///      (config issue, F7 preserved).
///   2. BRIEF.md exists and read succeeds → else `Err`.
///   3. BRIEF.md non-empty (after trim) → else `Ok(None)` (silent skip).
///   4. No `title:` field in existing frontmatter → else `Ok(None)` (silent
///      skip).
///   5. Snapshot BRIEF.md to `BRIEF.md.<UTC-ts>.bak` (F6 preserved). Snapshot
///      failure → `Err` (do not append the prompt without a backup).
///   6. Build title prompt with the absolute, UNC-stripped path (F4
///      preserved). Return `Ok(Some((prompt, bak_path)))`.
///
/// The pre/post-idle re-read pair from v3 §9.4 step (6) is **gone**: with a
/// single PTY write there is no idle-gap between the read and the write,
/// so the F2 fold no longer applies. (R4 supersedes F2.)
fn build_title_prompt_appendage(
    cwd: &str,
) -> Result<Option<(String, std::path::PathBuf)>, String> {
    use crate::commands::entity_creation::{parse_brief_title, snapshot_brief_before_edit};
    use crate::session::session::find_workgroup_brief_path_for_cwd;

    // (1) Resolve workgroup BRIEF.md path. F7 preserved: surface as Err so
    //     a Coordinator misconfigured to a non-`wg-*` CWD is logged once
    //     per spawn under the `[auto-title:config]` prefix.
    let brief_path = find_workgroup_brief_path_for_cwd(cwd)
        .ok_or_else(|| format!("[auto-title:config] no wg- ancestor in cwd '{}'", cwd))?;

    // (2) Read BRIEF.md. Missing/unreadable → Err (warn-and-skip at caller).
    let content = std::fs::read_to_string(&brief_path)
        .map_err(|e| format!("read BRIEF.md at {:?}: {}", brief_path, e))?;

    // (3) Empty brief → silent skip.
    if content.trim().is_empty() {
        return Ok(None);
    }

    // (4) Title already present → silent skip.
    if parse_brief_title(&content).is_some() {
        return Ok(None);
    }

    // (5) F6 preserved — snapshot BEFORE building the prompt. Snapshot
    //     failure aborts: no .bak, no prompt-append. Idempotent next spawn.
    //     R4.D3 fold: bind `bak_path` (not `_bak_path`) so the caller can
    //     surface it in the success log line.
    let bak_path = snapshot_brief_before_edit(&brief_path)
        .map_err(|e| format!("snapshot BRIEF.md before edit: {}", e))?;

    // (6) F4 preserved — strip Windows \\?\ extended-length prefix.
    let raw = brief_path.to_string_lossy().to_string();
    let path_str = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
    let prompt = crate::pty::title_prompt::build_title_prompt(&path_str);

    Ok(Some((prompt, bak_path)))
}
```

The helper does **not** log internally. The caller's match arms log success/skip/warn at appropriate levels — same shape v3 used. The `bak_path` is returned alongside the prompt so the caller's success log line preserves v3's `[session] Auto-title backup created: <path>` signal at INFO level (R4.D3 fold).

#### §R4.2.5 Caller change — exact diff for the spawn task

Replace lines 702-746 of `commands/session.rs` (the `let cred_block = ... match inject_text_into_session(...).await { Ok(()) => { ... } Err(e) => { ... } }` block) with:

```rust
            // Issue #107 round 4 — build the optional title-prompt segment
            // BEFORE the PTY write. Synchronous fs reads + snapshot only;
            // no async work, no second idle-wait. See plan §R4.2.
            let title_appendage = if is_coordinator_clone && auto_title_enabled {
                match build_title_prompt_appendage(&cwd_clone) {
                    Ok(Some((prompt, bak_path))) => {
                        log::info!(
                            "[session] Auto-title appendage built for session {} (bak={:?})",
                            session_id,
                            bak_path
                        );
                        Some(prompt)
                    }
                    Ok(None) => {
                        log::info!(
                            "[session] Auto-title appendage skipped (gate not passed) for session {}",
                            session_id
                        );
                        None
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Auto-title appendage skipped for session {}: {}",
                            session_id,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };

            let auto_title_was_appended = title_appendage.is_some();
            let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd_clone);
            let combined = match title_appendage {
                Some(prompt) => format!("{}\n{}", cred_block, prompt),
                None => cred_block,
            };

            match crate::pty::inject::inject_text_into_session(
                &app_clone,
                session_id,
                &combined,
                true,
            )
            .await
            {
                Ok(()) => {
                    log::info!(
                        "[session] Bootstrap message injected for session {} (auto-title={})",
                        session_id,
                        auto_title_was_appended
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[session] Failed to inject bootstrap for {}: {}",
                        session_id,
                        e
                    );
                }
            }
        });
```

Two key shape choices captured (R4.D2 + R4.D3 folds):

1. **Sibling let** `let auto_title_was_appended = title_appendage.is_some();` is bound BEFORE `let combined = match title_appendage { … }` so the bool survives the `match`'s ownership move. The `&combined`-grep alternative is fragile (a future log-line tweak could lose the marker token).

2. **Helper return tuple** `(prompt, bak_path)` lets the caller include the bak path in the appendage-built INFO line — preserves v3's `[session] Auto-title backup created: <path>` operational signal at the same log level instead of dropping it. Architect's "no internal logging inside the helper" rule is preserved — the caller still does the log.

#### §R4.2.6 What this preserves and what it changes (round 4 vs round 3)

| Item | Round 3 v3 | Round 4 |
|---|---|---|
| Number of PTY writes per Coordinator spawn (when auto-title fires) | 2 (`inject_text_into_session` ×2) | **1** |
| Idle-waits before injecting title prompt | 2 (cred-inject path + `inject_title_prompt_after_idle_static`) | **1** (cred-inject path only) |
| Read BRIEF.md timing | Pre-idle (initial) + post-idle (re-read, F2 fold) | **Once**, immediately before the single PTY write |
| F2 post-idle re-read | Required | **Removed** — superseded by Round 4's single-write design |
| Snapshot timing | After post-idle re-read, before second write | **Before** the single combined write (still synchronous, still F6-compliant) |
| F4 UNC-prefix strip | Applied | **Preserved** |
| F7 no-wg-ancestor → warn | Applied | **Preserved** |
| Snapshot failure → no inject | Applied | **Preserved** — falls back to cred-block-alone (safer than v3 which returned `Err` from the helper and fully skipped the prompt; round 4 still injects creds because the cred-block is an independent obligation) |
| Idempotency on retry | Preserved | **Preserved** (gate 4 short-circuits next spawn once the agent writes a title) |
| Coordinator-only gate | At setting capture | **Preserved** at setting capture |
| `auto_generate_brief_title` setting snapshot | Captured before `tokio::spawn` (F1 fold) | **Preserved** unchanged |

---

### §R4.3 Change B — amend the GOLDEN RULE template (4th allowed write zone)

#### §R4.3.1 Required change

Add a 4th allowed write zone to the `default_context()` template at `src-tauri/src/config/session_context.rs:478`:

> 4. **The workgroup `BRIEF.md` file** (e.g. `<absolute-path-to-wg-root>\BRIEF.md`). Readable AND writable, since AgentsCommander may inject prompts asking the agent to maintain the brief.

**Surgical scope discipline (non-negotiable):** ONLY the `BRIEF.md` file at the workgroup root is writable. NOT the workgroup root directory itself, NOT any other file at the workgroup root, NOT subdirectories of the workgroup root that are not already covered (`repo-*`, the agent's own replica). One file, by absolute path. Tech-lead's brief: *"Surgical exception."*

#### §R4.3.2 Where the new zone applies

`default_context()` is parameterised: it generates either a 3-zone template (replicas, where `matrix_root.is_some()`) or a 2-zone template (Agent Matrix root agents, where `matrix_root.is_none()`). The 4th zone is applicable **only when the agent's `agent_root` has a `wg-*` ancestor** — i.e. the agent runs inside a workgroup.

| `matrix_root` | wg-* ancestor of `agent_root`? | Zones rendered | New `allowed_places` text |
|---|---|---|---|
| `Some(_)` (replica) | yes (standard replica layout) | 1, 2, 3, **4** | "four places" |
| `Some(_)` (replica) | no (anomalous — replica without wg ancestor) | 1, 2, 3 | "three places" (unchanged) |
| `None` (Agent Matrix `_agent_X`) | no (matrix sits at `.ac-new\_agent_X`) | 1, 2 | "two places" (unchanged) |
| `None` (Agent Matrix `_agent_X`) | yes (anomalous — matrix nested inside a wg) | 1, 2, **4** | "three places" |

The expected production cases are rows 1 and 3. Rows 2 and 4 are anomalies the template handles defensively (no panic, no spurious zone).

#### §R4.3.3 Implementation — exact edits to `session_context.rs`

**Edit 1 — derive the BRIEF.md path inside `default_context`.**

At the top of `default_context` (current line 478, just inside the function), add:

```rust
    let brief_display_path = crate::session::session::find_workgroup_brief_path_for_cwd(agent_root)
        .map(|p| {
            let raw = p.to_string_lossy().to_string();
            raw.strip_prefix(r"\\?\")
                .unwrap_or(&raw)
                .to_string()
        });
```

Reuses the existing §7.1 helper (sole source of truth for wg-ancestor walking — see dev-rust R1 verified at `session/session.rs:122-142`). Strips the `\\?\` prefix to match `display_path` and `pty/credentials.rs:38-44`'s normalisation. Returns `Option<String>`.

**Edit 2 — replace the `allowed_places` ternary at lines 479-483.**

```rust
    let allowed_places = match (matrix_root.is_some(), brief_display_path.is_some()) {
        (true, true) => "four places",
        (true, false) | (false, true) => "three places",
        (false, false) => "two places",
    };
```

**Edit 3 — add `brief_section` and `brief_allowed` blocks (after `matrix_section`/`matrix_allowed` at lines 486-499).**

The `brief_section` format string ends with **exactly one** trailing `\n` (R4.D5 fold) so the `{matrix_section}{brief_section}` concatenation in Edit 5 renders a single blank line between zone 3 and zone 4 — visually consistent with the existing zone-1↔2 and zone-2↔3 spacing.

```rust
    let brief_section = match brief_display_path.as_deref() {
        Some(brief_path) => format!(
            "4. **The workgroup `BRIEF.md` file** — readable AND writable, since AgentsCommander may inject prompts asking the agent to maintain the brief:\n   ```\n   {brief_path}\n   ```\n   **Scope discipline**: ONLY this single file is writable in the workgroup root. NOT the workgroup root directory, NOT any other file at the workgroup root.\n",
            brief_path = brief_path,
        ),
        None => String::new(),
    };
    let brief_allowed = match brief_display_path.as_deref() {
        Some(brief_path) => format!(
            "- **Allowed**: Read/write the workgroup `BRIEF.md` file ONLY ({brief_path})\n",
            brief_path = brief_path,
        ),
        None => String::new(),
    };
```

**Edit 4 — broaden `forbidden_scope` so the carve-out is explicit.**

Replace lines 500-504 with:

```rust
    let workgroup_clause = if brief_display_path.is_some() {
        ", other files at the workgroup root (BRIEF.md is the ONLY writable exception),"
    } else {
        ""
    };
    let forbidden_scope = if matrix_root.is_some() {
        format!(
            "allowed zones — including other agents' replica directories, any other files inside the Agent Matrix, the workspace root{workgroup_clause} parent project dirs, user home files, or arbitrary paths on disk",
            workgroup_clause = workgroup_clause,
        )
    } else {
        format!(
            "two zones — including other agents' replica directories, the workspace root{workgroup_clause} parent project dirs, user home files, or arbitrary paths on disk",
            workgroup_clause = workgroup_clause,
        )
    };
```

This changes `forbidden_scope` from a `&'static str` to a `String`. Update the format-string consumer at line 533 (the `{forbidden_scope}` interpolation) — it works with `String` because `format!` calls `Display`, which is implemented for both. No call-site change.

**Edit 5 — embed the new placeholders in the template body.**

Insert `{brief_section}` immediately after `{matrix_section}` at the current template location (after line 526) — keeps the rendered numbering "1, 2, 3, 4" in display order. Mirror the existing `{matrix_allowed}{brief_allowed}` style: NO literal newline between the two section placeholders. Each helper string carries its own trailing `\n`, so adjacent placeholders compose without stacking blank lines (R4.D5 fold):

```rust
        r#"# AgentsCommander Context
...
{matrix_section}{brief_section}
Any repository or directory outside the allowed places above is READ-ONLY.
...
```

Insert `{brief_allowed}` immediately after `{matrix_allowed}` at line 533:

```rust
- **Allowed**: Full read/write inside your own replica root ({agent_root}) and its subdirectories
{matrix_allowed}{brief_allowed}- **FORBIDDEN**: Any write operation outside those {forbidden_scope}
```

**Edit 6 — add `brief_section` and `brief_allowed` to the `format!` call's named-args list at the bottom of the function (current lines 614-620).**

```rust
        agent_root = agent_root,
        allowed_places = allowed_places,
        replica_usage = replica_usage,
        matrix_section = matrix_section,
        brief_section = brief_section,
        matrix_allowed = matrix_allowed,
        brief_allowed = brief_allowed,
        forbidden_scope = forbidden_scope,
        git_scope = git_scope,
```

#### §R4.3.4 Worked example — replica inside a workgroup

For an agent at `C:\proj\.ac-new\wg-2-a-team\__agent_dev-rust`, the rendered fragment becomes:

```markdown
**ABSOLUTE AND NON-NEGOTIABLE:** You may ONLY modify files in four places:

1. **Repositories whose root folder name starts with `repo-`** (...).
2. **Your own agent replica directory and its subdirectories** — your assigned root:
   ```
   C:\proj\.ac-new\wg-2-a-team\__agent_dev-rust
   ```
   ...
3. **Your origin Agent Matrix, but only for the canonical agent state listed below:**
   ```
   C:\proj\.ac-new\_agent_dev-rust
   ```
   Allowed there:
   - `memory/`
   - `plans/`
   - `Role.md`

4. **The workgroup `BRIEF.md` file** — readable AND writable, since AgentsCommander may inject prompts asking the agent to maintain the brief:
   ```
   C:\proj\.ac-new\wg-2-a-team\BRIEF.md
   ```
   **Scope discipline**: ONLY this single file is writable in the workgroup root. NOT the workgroup root directory, NOT any other file at the workgroup root.

Any repository or directory outside the allowed places above is READ-ONLY.

- **Allowed**: Read-only operations on ANY path (...)
- **Allowed**: Full read/write inside `repo-*` folders
- **Allowed**: Full read/write inside your own replica root (...)
- **Allowed**: Full read/write inside your origin Agent Matrix's `memory/`, `plans/`, and `Role.md` (...)
- **Allowed**: Read/write the workgroup `BRIEF.md` file ONLY (C:\proj\.ac-new\wg-2-a-team\BRIEF.md)
- **FORBIDDEN**: Any write operation outside those allowed zones — including other agents' replica directories, any other files inside the Agent Matrix, the workspace root, other files at the workgroup root (BRIEF.md is the ONLY writable exception), parent project dirs, user home files, or arbitrary paths on disk
```

The agent reads zone 4 by absolute path. There is zero ambiguity that any other path under the workgroup root remains read-only.

#### §R4.3.5 Files touched (Change B)

| File | Action | Detail |
|---|---|---|
| `src-tauri/src/config/session_context.rs` | modify | Edits 1-6 in §R4.3.3 — adds `brief_display_path`, `brief_section`, `brief_allowed`, broadens `forbidden_scope` |

No new modules, no new crates, no new TS types (the GOLDEN RULE is renderered inside CLAUDE.md / AGENTS.md / GEMINI.md and never crosses the IPC boundary).

---

### §R4.4 What's removed from #107 v3

The following code & logic ship with v3 (commits up to `e458b85`) and are deleted in Round 4:

| v3 location | Item removed | Why |
|---|---|---|
| `commands/session.rs:287-432` | `inject_title_prompt_after_idle_static` (the entire async helper) | Replaced by the synchronous `build_title_prompt_appendage` (§R4.2.4). The post-idle re-read, the second idle-wait poll loop, and the second `inject_text_into_session` call all go with it. |
| `commands/session.rs:704-710` | The standalone `inject_text_into_session(&app_clone, session_id, &cred_block, true)` for the cred-block | Replaced by the single combined-message inject in §R4.2.5. |
| `commands/session.rs:712-736` (the `Ok(()) =>` arm body) | The post-cred-inject chain to `inject_title_prompt_after_idle_static` | The chain is now done synchronously before the single inject. |
| Plan §10 sequencing diagram | Two-write timeline | Superseded by the §R4.2.3 single-write sequence. (§10 stays in the file for the audit trail; tech-lead may choose to flag it as "round-3 historical" or leave the contradiction implicit — see §R4.7 open questions.) |
| Plan §9.4 step (5) — second idle-wait | Second 30-s poll loop | No second write means no second wait. |
| Plan §9.4 step (6) — F2 post-idle re-read | Pre/post-idle re-read pair | Single read at spawn-completion + single write closes the TOCTOU window without needing a re-read. |

The F-fold IDs F2 (post-idle re-read) is **superseded** but its review-trail rows in §13 stay (the audit trail is whole-file; we don't rewrite history).

---

### §R4.5 What's preserved from #107 v3

Everything below remains exactly as v3 documents it:

- **§4** — `auto_generate_brief_title` setting (Rust + TS + SettingsModal). Unchanged.
- **§5** — BRIEF.md template change (verbatim or empty). Unchanged.
- **§6.2** — `parse_brief_title` with F3 case-insensitive key match. Unchanged. Still called from §R4.2.4 step 4.
- **§7** — `find_workgroup_brief_path_for_cwd` (and its refactor of `read_workgroup_brief_for_cwd`). Unchanged. Now called from BOTH §R4.2.4 AND §R4.3.3 Edit 1.
- **§8** — `pty/title_prompt.rs::build_title_prompt`. Unchanged. Still produces the same string; the difference is only how the caller delivers it.
- **§9.1-§9.3** — the spawn-time hook location (inside the existing `agent_id.is_some()` task) and the `is_coordinator_clone` + `auto_title_enabled` capture (F1). Unchanged.
- **§16** — `snapshot_brief_before_edit` (F6 + F9 atomic `create_new`). Unchanged. Called synchronously inside §R4.2.4 step 5.
- **F4** — UNC `\\?\` prefix strip on the path embedded in the prompt. Preserved at §R4.2.4 step 6.
- **F7** — no-wg-ancestor → `Err` + `[auto-title:config]` warn. Preserved at §R4.2.4 step 1.
- **§17** — all unit tests for `parse_brief_title`, `find_workgroup_brief_path_for_cwd`, `build_title_prompt`, `snapshot_brief_before_edit`. Unchanged.

The dev-rust review, dev-rust-grinch review, Round 2/3 amendment ledgers stay intact above for the audit trail.

---

### §R4.6 Test plan — what changes from §12, what's new

#### §R4.6.1 §12.2 happy path — single combined message

Replace step 4 (currently *"observe a second message in the agent's PTY"*) with:

> 4. After the cred block lands and the agent goes idle, observe **a single PTY paste containing both the cred block and the title prompt**, separated by a blank line. The cred block (`# === Session Credentials ===` ... `# === End Credentials ===`) appears first; immediately after a blank line, `[AgentsCommander auto-title] Read the workgroup brief at \`...\`...` appears.

Replace step 6's expected log lines with:

> 6. App log contains:
>    - `[session] Auto-title appendage built for session <uuid>`
>    - `[session] Bootstrap message injected for session <uuid> (auto-title=true)`

The pre-Round-4 lines (`[session] Credentials auto-injected for session <uuid>`, `[session] Auto-title prompt injected for session <uuid> ...`) **no longer appear** — both the credential inject and the title-prompt inject have been folded into a single bootstrap log line.

#### §R4.6.2 §12.3 idempotent restart — single message, no appendage

Replace step 3's expected log lines with:

> 3. App log:
>    - `[session] Auto-title appendage skipped (gate not passed) for session <uuid>`
>    - `[session] Bootstrap message injected for session <uuid> (auto-title=false)`

#### §R4.6.3 §12.4 empty brief — same change as §12.3

Same log-line rewrite as §R4.6.2. The empty-brief path now skips at gate (3) inside `build_title_prompt_appendage` — same observable behavior, single log line, single PTY write.

#### §R4.6.4 §12.5 setting OFF — same change

The `is_coordinator_clone && auto_title_enabled` outer gate short-circuits BEFORE `build_title_prompt_appendage` is called, so neither the "appendage built" nor "appendage skipped" line appears. The bootstrap log line reports `(auto-title=false)`. No regression — this is still the cleanest "feature off" observable.

#### §R4.6.5 §12.6 non-Coordinator agent — same change

Same as §R4.6.4. The outer gate short-circuits.

#### §R4.6.6 §12.8 idle timeout — change

The §12.8 scenario (agent never returns to idle after spawn) now leaves the bootstrap message itself un-injected — the helper's idle-wait still applies but only ONCE (gate at lines 679-700). Expected log:

> `[session] Timeout waiting for idle before credential injection for session <uuid>`
> `[session] Failed to inject bootstrap for <uuid>: ...` (if the inject path is reached after timeout — current v3 behaviour breaks the loop and continues to inject anyway, see lines 681-682)

The §12.9 missing-BRIEF.md scenario also collapses: `build_title_prompt_appendage` returns `Err` at gate (2), the appendage is skipped, the cred-block-alone is injected. New expected log:

> `[session] Auto-title appendage skipped for session <uuid>: read BRIEF.md at "...": ...`
> `[session] Bootstrap message injected for session <uuid> (auto-title=false)`

The session is still fully usable, the cred-block still lands. Best-effort behavior preserved.

#### §R4.6.7 NEW §12.14 — GOLDEN RULE includes 4th zone (Change B)

1. Create a new workgroup; supply a brief.
2. Trigger context regeneration for the workgroup's Coordinator (start a fresh session — `materialize_agent_context_file` runs at every spawn per `commands/session.rs:592-615`).
3. Inspect the materialised CLAUDE.md (or AGENTS.md/GEMINI.md depending on agent family) at `<replica-root>/CLAUDE.md`.
4. Verify the GOLDEN RULE section reads:
   - `**ABSOLUTE AND NON-NEGOTIABLE:** You may ONLY modify files in four places:` (was: `three places`).
   - Zone `4.` exists, names "The workgroup `BRIEF.md` file", and lists the absolute path to `<wg-root>\BRIEF.md`.
   - The "Scope discipline" sentence appears verbatim: "ONLY this single file is writable in the workgroup root..."
   - The bulleted "Allowed" list contains a `Read/write the workgroup BRIEF.md file ONLY (<path>)` line.
   - The "FORBIDDEN" line includes "other files at the workgroup root (BRIEF.md is the ONLY writable exception)".

#### §R4.6.8 NEW §12.15 — combined message obeys the 4th zone (end-to-end)

Run the same scenario against each of the three coding-agent families. The 4th GOLDEN RULE zone renders identically into CLAUDE.md, AGENTS.md, and GEMINI.md — but each family enforces write rules under its own model, so cross-family acceptance is best-effort:

| Family | Test outcome | Status |
|---|---|---|
| Claude Code | Should write the title cleanly | **Required pass** |
| Codex | Best-effort: writes the title if its own permission model accepts the 4th GOLDEN RULE zone | Best-effort |
| Gemini | Same as Codex | Best-effort |

For each family:

1. Create a new workgroup with a non-empty brief and no `title:`.
2. Start the team's Coordinator session against the chosen family (Claude / Codex / Gemini — covered by AC's family-detection at `commands/session.rs:515-526`).
3. Verify (as in §12.2 R4.6.1) that the agent receives one combined paste.
4. Verify the agent **writes** the title to BRIEF.md without refusing — i.e. the bug from §R4.1 does **not** reproduce.
5. Verify a `.bak` was created in the wg-* directory before the agent's edit.
6. Verify BRIEF.md now starts with `---\ntitle: ...\n---\n` and the body is preserved.

This test is the canonical "Change A + Change B working together" check. If §R4.6.7 passes but §R4.6.8 fails, Change A's combined-message bypass attempt did not fire as hoped — Change B is still the load-bearing fix and the agent should accept the write regardless.

Round 4 does not validate cross-family write enforcement in this PR. Codex/Gemini residual refusals are family-compliance issues, not Round 4 design issues — log the family + behavior in the test result and move on.

#### §R4.6.9 NEW unit test — `default_context_renders_4th_zone_when_wg_ancestor_present`

Add to the existing `mod tests` in `src-tauri/src/config/session_context.rs` (the module already exists at line 632-643).

The test cannot easily fake a wg-* ancestor without writing to `std::env::temp_dir()` — `find_workgroup_brief_path_for_cwd` is a pure path walk, but `default_context` calls it via the `agent_root` string. The cleanest shape:

```rust
#[test]
fn default_context_includes_4th_zone_when_agent_root_inside_wg() {
    let agent_root = r"C:\proj\.ac-new\wg-2-a-team\__agent_dev-rust";
    let matrix = r"C:\proj\.ac-new\_agent_dev-rust";
    let out = default_context(agent_root, Some(matrix));
    assert!(out.contains("four places"));
    assert!(out.contains(r"C:\proj\.ac-new\wg-2-a-team\BRIEF.md"));
    assert!(out.contains("workgroup `BRIEF.md` file"));
    assert!(out.contains("Scope discipline"));
    assert!(out.contains("BRIEF.md is the ONLY writable exception"));
}

#[test]
fn default_context_omits_4th_zone_when_no_wg_ancestor() {
    // Matrix sits at .ac-new\_agent_X — no wg ancestor.
    let agent_root = r"C:\proj\.ac-new\_agent_dev-rust";
    let out = default_context(agent_root, None);
    assert!(out.contains("two places"));
    assert!(!out.contains("workgroup `BRIEF.md` file"));
    assert!(!out.contains("Scope discipline"));
}

#[test]
fn default_context_three_places_for_replica_without_wg_ancestor() {
    // Anomalous: replica without a wg-* ancestor in its agent_root.
    let agent_root = r"C:\proj\.ac-new\__agent_orphan";
    let matrix = r"C:\proj\.ac-new\_agent_orphan";
    let out = default_context(agent_root, Some(matrix));
    assert!(out.contains("three places"));
    assert!(!out.contains("workgroup `BRIEF.md` file"));
}
```

The pure-path-walk nature of `find_workgroup_brief_path_for_cwd` means these tests require no filesystem fixtures. Deterministic, <1 ms each.

#### §R4.6.10 §17 unit-test catalog — what changes

`build_title_prompt_appendage` is **not** unit-tested directly for gates 1-4 — that logic is the union of `parse_brief_title` (tested), `find_workgroup_brief_path_for_cwd` (tested), and direct `std::fs::read_to_string` (no test needed). The helper is a thin orchestrator over those.

**One narrow exception** (R4.D8 / F5 fold): the post-snapshot bail contract — when `snapshot_brief_before_edit` returns `Err`, the helper must return `Err` and the caller must fall back to cred-block alone. This is a behaviour change vs v3 (timing differs) and a future refactor could plausibly swallow the snapshot `Err` and inject the prompt without a backup. A 15-line test pins the contract — see §R4.6.12.

`inject_title_prompt_after_idle_static` had no unit tests in v3 (it was an `async` helper that touched PTY state — out of unit-test scope). Removing it changes nothing in `cargo test`.

#### §R4.6.11 Dev-rust verification items during round 4 enrichment

These were not new tests but things dev-rust verified by reading code during the round 4 enrichment pass (Step 3). **All three passed** — see the `R4.D-summary` in dev-rust's review section below.

- **Combined-message size budget vs ConPTY input buffer.** ✅ PASS. `pty/manager.rs` has no input-side buffer cap; `PtyManager::write` calls `write_all(data)` + `flush()` directly. The only sized buffer (`[0u8; 4096]`) is the read loop. ConPTY's stdin pipe on Windows is well above 64 KB. ~900 B is comfortable.
- **Sibling let for `auto_title_was_appended` (§R4.2.5).** ✅ PASS with the F1 fold above — the sibling-let pattern is now the canonical body in §R4.2.5.
- **`forbidden_scope` type change in `session_context.rs`** (§R4.3.3 Edit 4). ✅ PASS — `format!` named-arg interpolation calls `Display`, implemented for both `&'static str` and `String`. No call-site change. Single caller of `default_context` plus the existing test, both consume the returned `String` and care only about its content.

#### §R4.6.12 NEW unit test — snapshot-collision aborts the appendage (R4.D8 / F5 fold)

Pin the post-snapshot bail contract — when `snapshot_brief_before_edit` returns `Err`, `build_title_prompt_appendage` returns `Err` and the prompt is NOT built. Catches the regression "future refactor swallows snapshot Err and injects the prompt without a backup".

Add to the existing `mod tests` in `src-tauri/src/commands/session.rs` (or wherever the helper lives). Marked `#[test]` (NOT `#[ignore]`) — runs in default `cargo test`.

```rust
#[test]
fn build_title_prompt_appendage_returns_err_when_snapshot_collides() {
    let dir = std::env::temp_dir().join(format!(
        "ac-r4-snap-collide-{}-wg-1-x", std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let brief = dir.join("BRIEF.md");
    std::fs::write(&brief, b"Some brief body, no title.\n").unwrap();

    // Pre-populate a same-second `.bak` so the helper's snapshot collides
    // (atomic create_new returns AlreadyExists per F9 fold).
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let preexisting = dir.join(format!("BRIEF.md.{}.bak", ts));
    std::fs::write(&preexisting, b"prior").unwrap();

    let cwd = dir.to_string_lossy().to_string();
    let result = build_title_prompt_appendage(&cwd);
    assert!(matches!(result, Err(ref e) if e.starts_with("snapshot BRIEF.md before edit:")));

    let _ = std::fs::remove_file(&brief);
    let _ = std::fs::remove_file(&preexisting);
    let _ = std::fs::remove_dir(&dir);
}
```

Note on the test's path-walk dependency: `find_workgroup_brief_path_for_cwd` walks ancestors looking for a `wg-` prefix. The tempdir name `"ac-r4-snap-collide-<pid>-wg-1-x"` does NOT start with `wg-`, so the gate (1) check would short-circuit before reaching the snapshot. To exercise the snapshot-failure path, the test must use a tempdir whose name itself starts with `wg-`, OR construct a wg-prefixed parent. Simpler: rename the tempdir to `format!("wg-r4-snap-collide-{}", std::process::id())` — the gate-1 walk-up finds the cwd itself as the wg ancestor, and the test exercises gate (5) directly.

```rust
let dir = std::env::temp_dir().join(format!(
    "wg-r4-snap-collide-{}", std::process::id()
));
```

Helper for dev-rust during impl: this is a 15-line cheap test, not a 50-line integration scaffold. The point is to wire `Err(snapshot ...)` → `Err` round-trip; the gate-1 path-walk is incidental.

---

### §R4.7 Migration note — pre-existing replicas

**The GOLDEN RULE is regenerated at every session spawn**, via `materialize_agent_context_file` at `commands/session.rs:592-615`, which calls `default_context` (or the replica context builder) and writes a fresh `CLAUDE.md` / `AGENTS.md` / `GEMINI.md` into the replica's directory. The previous file is removed first (see `MANAGED_CONTEXT_FILENAMES` at `session_context.rs:36-37`).

#### §R4.7.1 User-visible effect (quick mental model)

- **New replicas** (created after this change ships): pick up the 4th zone on first spawn — automatic.
- **Active sessions on pre-existing replicas**: pick up the 4th zone on next spawn, `/clear`, or session restart. **No action needed by the user**. If they want immediate adoption mid-session, `/clear` is the one-liner — the cred-block reinject path materialises a fresh CLAUDE.md.
- **Pre-existing replicas with no active session**: pick up the 4th zone on next session start.

No batch-regeneration command is needed. Round 4 confirms (per tech-lead Q2 decision) that the existing per-session rewrite flow IS the migration mechanism. The one-liner mid-session escape hatch (`/clear`) is the only documentation users need.

#### §R4.7.2 Detailed mechanics

- **Replicas spawned AFTER this change ships** automatically get the new GOLDEN RULE on their next session start. No migration step needed.
- **Replicas with an active session at the time of upgrade** still hold the OLD rule in their agent's in-memory context (the agent has already read CLAUDE.md once at boot). Restarting the session — or any of the existing reinject paths (`/clear` reinject, manual restart, mailbox wake-from-cold) — calls `create_session_inner` → `materialize_agent_context_file` → fresh CLAUDE.md with the new rule.
- **The on-disk CLAUDE.md** is updated on the next session start regardless of whether the in-memory context refreshes. So the *next* time the agent's context is rebuilt (most commonly: `/clear`), it reads the new file.

**No batch-regeneration step is required.** The existing per-session regeneration flow is the migration mechanism. Round 4 does not introduce a "rewrite all CLAUDE.md files" command — the per-session refresh is sufficient and avoids the failure modes of a sweeping rewrite (active sessions interrupted, file-busy errors on Windows, etc.).

---

### §R4.8 Failure modes — exhaustive matrix for Round 4

| Scenario | Coordinator? | Setting ON? | Has wg-* ancestor? | Brief state | Snapshot succeeds? | Outcome |
|---|---|---|---|---|---|---|
| Standard happy path | yes | yes | yes | non-empty, no title | yes | Combined message injected. Agent writes title to BRIEF.md. `.bak` exists in wg-root. |
| Setting OFF | yes | **no** | yes | any | n/a | Cred-block alone. No appendage attempt. Bootstrap log: `auto-title=false`. |
| Non-Coordinator | **no** | any | yes | any | n/a | Cred-block alone. No appendage attempt. |
| No wg-* ancestor (config issue, F7) | yes | yes | **no** | n/a | n/a | Cred-block alone. Helper returns `Err("[auto-title:config] no wg- ancestor in cwd '...'")`. Caller logs at `warn`. Indicates a Coordinator misregistered against a non-workgroup CWD. |
| BRIEF.md missing | yes | yes | yes | **read fails** | n/a | Cred-block alone. Helper returns `Err("read BRIEF.md at ...: <io error>")`. Warn-log. |
| BRIEF.md empty | yes | yes | yes | **empty (0 bytes)** | n/a | Cred-block alone. Helper returns `Ok(None)`. Info-log: "appendage skipped (gate not passed)". |
| Title already present | yes | yes | yes | non-empty, **has title** | n/a | Cred-block alone. Helper returns `Ok(None)`. Info-log same as empty. Idempotent on retry. |
| Snapshot fails (disk full, perm denied, same-second collision) | yes | yes | yes | non-empty, no title | **no** | Cred-block alone. Helper returns `Err("snapshot BRIEF.md before edit: ...")`. Warn-log. Next spawn retries. **Different from v3**: v3 also abandoned the title-prompt inject on snapshot failure but had already injected the cred block; round 4 still injects the cred block (just without the appendage). Net behavior parity, fewer PTY writes. |
| First idle-wait times out (30 s) | yes | yes | yes | any | any | Per existing v3 behavior (`session.rs:681-682`), the wait breaks but the inject still proceeds as a "fallback". Combined message goes regardless. Same as today; no regression. |
| Two restarts back-to-back, same UTC second | yes | yes | yes | non-empty, no title (initially) | first succeeds, second `AlreadyExists` (F9) | First spawn: combined message injected, agent writes title. Second spawn: gate (4) sees title now present → `Ok(None)` → cred-block alone. The same-second `AlreadyExists` only matters if BOTH spawns reach gate 5 — which requires a tight timing where the agent has not yet committed the title between spawns. If that happens, the second spawn's snapshot returns `Err`, helper returns `Err`, cred-block alone. Caller logs warn. Idempotent — third spawn sees title (assuming agent eventually wrote it). |
| GOLDEN RULE refusal returns | yes | yes | yes | non-empty, no title | yes | **Should not happen post-Change-B**, but if it does (Codex/Gemini agents that don't fully follow CLAUDE.md, or an agent variant that ignores the 4th zone): the cred-block-and-title-prompt arrived as one paste. The agent processes the title prompt as a regular user message. On compliant agents the new GOLDEN RULE permits the write. On non-compliant agents, no write happens; on the next spawn the same combined message fires again (gate 4 still sees no title) — best-effort behavior, no infinite loop because the user can disable `auto_generate_brief_title` if it bothers them. |
| User edits BRIEF.md mid-spawn (between read and PTY write) | yes | yes | yes | changes mid-flow | n/a | The new design's "read once, write once, no idle gap" closes the TOCTOU window to milliseconds — the gap from `build_title_prompt_appendage`'s `read_to_string` to `inject_text_into_session` is dominated by the cred-block's `format!` (~1 ms). If the user manages to write between those two operations, the agent receives a stale prompt that points to a now-edited file; the agent's own self-guard rule from §8.1 ("If the file already starts with `---` and contains a `title:` field, do nothing") catches it. No corruption. |
| Concurrent-writer (two AC processes, same workgroup, same time) | yes | yes | yes | racing | snapshot may collide | Separate `tokio::spawn` tasks in separate processes. Each builds its own combined message. The `.bak` snapshot's `create_new` (F9) gives one process the slot; the other gets `AlreadyExists` and falls back to cred-block alone. The agent receives one prompt OR two (one per process); idempotency at gate 4 ensures only one title is added across both runs. |

The matrix is exhaustive against the gates defined in §R4.2.4 and the v3-preserved semantics from §R4.5. Every "Cred-block alone" outcome above results in a fully usable session — the auto-title feature is best-effort and never blocks credentials delivery.

---

### §R4.9 Files touched (Round 4 summary)

| File | Action | Detail |
|---|---|---|
| `src-tauri/src/commands/session.rs` | modify | Delete `inject_title_prompt_after_idle_static` (lines 287-432). Add `build_title_prompt_appendage` at the same location (~35 lines, sync). Replace the spawn-task body at lines 702-746 with the §R4.2.5 single-write block. |
| `src-tauri/src/config/session_context.rs` | modify | §R4.3.3 Edits 1-6 — add `brief_display_path`, `brief_section`, `brief_allowed`, broaden `forbidden_scope`, embed placeholders, add to `format!` named-args. Add §R4.6.9 unit tests to the existing `mod tests`. |

`src-tauri/src/pty/title_prompt.rs` — **unchanged** (still produces the same prompt string).
`src-tauri/src/pty/credentials.rs` — **unchanged** (still produces the same cred-block string).
`src-tauri/src/commands/entity_creation.rs` — **unchanged** (`parse_brief_title`, `snapshot_brief_before_edit` both unchanged).
`src-tauri/src/session/session.rs` — **unchanged** (`find_workgroup_brief_path_for_cwd` now has a second caller in `session_context.rs::default_context`, but the function itself is unchanged).
`src-tauri/src/config/settings.rs`, `src/shared/types.ts`, `src/sidebar/components/SettingsModal.tsx` — **unchanged**.

No new dependencies. No new modules. Net code delta: ~145 lines deleted (`inject_title_prompt_after_idle_static`), ~35 lines added (`build_title_prompt_appendage`), ~50 lines added in `session_context.rs` (Change B edits + tests), ~15 lines added in the spawn-task body. **Net: ~−45 lines.**

---

### §R4.10 Open questions for tech-lead

> **All six resolved** by tech-lead message
> `messaging/20260501-144814-wg5-tech-lead-to-wg5-architect-107-r4-decisions.md`.
> Resolutions folded into the plan: §2.2 strikethrough + Round-4-supersedes
> callout (Q1, applied), §R4.7 expanded with triple-bullet (Q2, applied),
> §10 superseded callout (Q3 / Q5, applied), §R4.6.8 3-family table (Q4,
> applied), §R4.12 logging changes (Q5, applied), §R4.6.11 dev-rust verify
> ConPTY buffer note (Q6, applied). Original open-questions text preserved
> below for the audit trail.

1. **§2 hard-constraint conflict.** Round 1 §2.2 hard-constraint says: *"Credentials block and title-prompt are SEPARATE PTY writes. Two distinct `inject_text_into_session` calls inside the spawn-time spawned task. Never concatenate."* Round 4 directly violates this. Two options:
   - **(a) Amend §2.2** in-place to *"... credentials and title-prompt are sent in a SINGLE concatenated PTY write at spawn time. Round 4 supersedes the round-1 separation."* Most readable for future maintainers.
   - **(b) Leave §2.2 as historical**, add a sentinel pointer at §2.2 saying "see §R4.2 — Round 4 supersedes". Preserves the audit trail intact.
   I recommend **(a) — surgical edit to §2.2** with a `~~`-strikethrough or `<del>` HTML so the original text is preserved visually. Tech-lead's call.

2. **Pre-existing-replicas refresh.** Round 4 §R4.7 says no batch-regeneration is needed — every session spawn rewrites CLAUDE.md. Confirm this is the intended behaviour, OR specify an opt-in CLI command (e.g. `agentscommander_mb refresh-context --all-replicas` or a UI button) to force-regenerate without restarting sessions. Most users will not need it but power users with many active replicas might.

3. **Logging line shape.** §R4.2.5's success log line includes an `auto-title=true|false` boolean. This is a behavioural-observability change — anyone grepping `[session] Auto-title prompt injected for session` to count successes will need a new query. Worth flagging in the PR description; tech-lead may also want a discrete `[session] Auto-title written` follow-up log fired from a future "verify the agent did the write" check (out of scope for Round 4).

4. **Combined-message size budget.** Cred-block (~250 bytes) + blank line + title-prompt (~600 bytes) = ~900 bytes single PTY write. ConPTY's input buffer on Windows is well above 1 KB, but if anyone has touched the buffer-size constants in `pty/manager.rs` recently, this is worth a verifying read. (I haven't checked them in Round 4 — calling it out as a "verify before merge" item.)

5. **Conflict with v3 §10 sequencing diagram.** §10 documents the two-write timeline and the F2 post-idle re-read. Round 4's §R4.4 explicitly removes that step but the §10 prose remains in the file. Two options:
   - **(a)** Add a one-line callout at §10 top: "**Superseded by §R4.2.3 in Round 4.**" Keep the original prose for audit.
   - **(b)** Replace §10's content with a redirect to §R4.2.3.
   I recommend **(a)**. Tech-lead's call.

6. **Codex/Gemini behaviour with the new GOLDEN RULE.** The 4th zone is rendered identically into AGENTS.md (Codex) and GEMINI.md (Gemini). Both families have varying degrees of "follow the GOLDEN RULE". Worth a manual-test step on at least one non-Claude family — if Codex still refuses despite the new rule, that's a Codex compliance issue, not a Round 4 design issue. Decision needed: is the test plan §12 expected to cover all three families, or only Claude for round 4 and other families left as known-best-effort?

---

### §R4.11 Constraint conflicts I am pushing back on

- **§2.2 (Round 1) — "credentials and title-prompt are SEPARATE PTY writes ... never concatenate"** is in direct conflict with Round 4's design. Tech-lead's Round 4 brief explicitly authorises the change ("New design: ... the title-prompt is concatenated into the same PTY message as the cred-block. ONE PTY write."). I am surfacing this as Open Question §R4.10.1 above; the Round 4 design proceeds as briefed but the §2.2 text needs an explicit edit (Option a or b in §R4.10.1) for the plan to be self-consistent. I have NOT silently rewritten §2.2 — the audit-trail preservation rule applies.

- **§9 entire section** (Round 1) describes the asynchronous chain to `inject_title_prompt_after_idle_static`. Round 4's §R4.4 removes that helper. §9.1-§9.3 (the spawn-time hook location and the captured values) survive almost verbatim — only the body inside the `Ok(()) =>` arm changes. §9.4 (the helper itself) is fully removed in code but its specification stays in the plan as audit. **No silent rewrite of §9 either.**

No other conflicts. Round 4 is consistent with the F1 fold (settings snapshot before `tokio::spawn` — preserved), F3 (case-insensitive parser — preserved), F4 (UNC strip — preserved), F6 (snapshot before edit — preserved at a slightly earlier point in the timeline), F7 (no-wg-ancestor warn — preserved), F8 (single PR — preserved), F9 (atomic `create_new` — preserved). F2 (post-idle re-read) is **superseded** but its review-trail entries stay.

---

### §R4.12 Logging changes (PR description callout)

Round 4 collapses the v3 cred-inject success log + auto-title prompt-injected log into a single bootstrap log line. Anyone grepping the old strings for ops dashboards or alerting needs to update their queries.

**Locked log-line shape** (R4.D4 tech-lead decision — Option (a) collapse):
- Single bootstrap log line per spawn: `[session] Bootstrap message injected for <uuid> (auto-title=true|false)`.
- Single argument: a boolean (`true|false`) — NOT a typed enum, NOT a "with reason=…" suffix.
- Skip-reason granularity is intentionally NOT preserved — operators counting "no brief" vs "already titled" must read BRIEF.md state directly. Rationale: gates are documented in the spec, git/PR description preserves the audit trail, and adding a typed `SkipReason` enum solves a debugging scenario that may never come up. If we ever need it, adding it later is one struct field + one format arg.

| Old (v3) | New (Round 4) |
|---|---|
| `[session] Credentials auto-injected for session <uuid>` | `[session] Bootstrap message injected for session <uuid> (auto-title=true\|false)` |
| `[session] Auto-title prompt injected for session <uuid> (brief=..., bak=...)` | (folded into the line above) |
| `[session] Auto-title backup created: <path> (session <uuid>)` | folded into `[session] Auto-title appendage built for session <uuid> (bak={:?})` (R4.D3 fold — the helper returns the bak path alongside the prompt; the caller's appendage-built INFO line carries it at the same level as v3's standalone backup-created line) |
| `[session] Auto-title skipped (BRIEF empty) for session <uuid>` | `[session] Auto-title appendage skipped (gate not passed) for session <uuid>` (combined `BRIEF empty` and `title present` cases — the helper returns `Ok(None)` in both; intentional collapse per R4.D4) |
| `[session] Auto-title skipped (title present) for session <uuid>` | (folded into the line above) |
| `[session] Auto-title skipped for session <uuid>: <reason>` | `[session] Auto-title appendage skipped for session <uuid>: <reason>` (now refers to skip-with-reason, not silent skip) |
| `[session] Failed to auto-inject credentials for <uuid>: <e>` | `[session] Failed to inject bootstrap for <uuid>: <e>` (covers cred-block-alone OR combined-message failure — same warn level) |

The PR description must repeat this table verbatim so ops/dashboard maintainers can update queries before merge. Tech-lead's call: also include in the changelog if AC ships one.

A future "verify the agent did the write" check could fire a discrete `[session] Auto-title written for session <uuid>` log line — out of scope for Round 4, design TBD.

---

### §R4.13 Net delta vs Round 3

- **Code lines removed**: ~145 (the entire `inject_title_prompt_after_idle_static` helper + its caller chain).
- **Code lines added**: ~100 (`build_title_prompt_appendage` + spawn-task body rewrite + `session_context.rs` Change B edits + new tests).
- **Net code change**: ~−45 lines, with much simpler control flow (one PTY write instead of two; one idle-wait instead of two; no F2 post-idle re-read pair).
- **Plan delta**: this entire `Round 4 — combined PTY write + GOLDEN RULE amendment` section appended; nothing above edited (audit trail preserved).
- **Test delta**: §R4.6.7-§R4.6.9 new tests; §12.2-§12.6 + §12.8-§12.9 log-line expectations updated (described diff-only in §R4.6.1-§R4.6.6).
- **Cargo dependencies**: zero added.
- **TS / frontend**: zero changes.

Round 4 is implementable as written, **subject to tech-lead's resolution of §R4.10.1, §R4.10.2, and §R4.10.5**. dev-rust is the next reviewer — Round 4 enrichment pass (Step 3) per the protocol.

---

## Round 4 — dev-rust review (added by dev-rust)

I read §R4.1-§R4.13 against the current branch state (`feature/107-auto-brief-title`, HEAD `e458b85`) and walked every line/function citation. Convention mirrors the round-1 review above: `R4.Dn`-prefixed findings, a verified-no-change table, then numbered actionable items.

### R4.D-summary — outcome of architect's three flagged checks (§R4.6.11)

| Check | Outcome | Notes |
|---|---|---|
| 1. ConPTY input buffer fits ~900 B | ✅ Pass | `pty/manager.rs` has no input-side buffer constant. `PtyManager::write` (line 464-479) calls the inner writer's `write_all(data)` then `flush()` directly — no Rust-side cap. The only sized buffer (`[0u8; 4096]` at line 388) is the **read** loop (PTY → app), unrelated to writes. ConPTY's stdin pipe on Windows is well above 64 KB by default. ~900 B is comfortable. |
| 2. Sibling-let scoping in the new combined-message build | ✅ Pass with one concrete fix — see R4.D2 | All four values (`auto_title_enabled`, `is_coordinator_clone`, helper result, cred-block) are read in the same scope before the `format!`. F1's pre-`tokio::spawn` capture is preserved exactly (current code 668-673). The only spot needing tightening is the illustrative `title_appendage_was_some_indicator` placeholder in §R4.2.5 — concretised in R4.D2 below. |
| 3. `forbidden_scope` type change `&'static str` → `String` | ✅ Pass | `format!`'s named-arg interpolation (line 510-621) calls `Display`, implemented for both. The named-args list passes-by-value once; `String` works identically to `&str`. No call-site change. Verified: only one caller of `default_context` (`ensure_session_context` at line 24) plus the existing test (line 638) — both consume the returned `String` and care only about its content. |

### R4.D1. Verified — no change needed

| Plan ref | Claim | Status |
|---|---|---|
| §R4.1 | Real-world bug — agent refused write because GOLDEN RULE template lists 3 zones at `session_context.rs:478` | ✅ Verified — current `default_context` renders "three places" for replicas (line 480) and the four explicit "Allowed/FORBIDDEN" bullets at lines 530-533 do not include workgroup-`BRIEF.md`. |
| §R4.2.1 | When auto-title preconditions fail, cred-block is injected alone — exactly as today's `agent_id.is_some()` block does for non-Coordinator sessions | ✅ Verified — current behavior at `commands/session.rs:702-709` injects `cred_block` alone via one `inject_text_into_session` call. |
| §R4.2.2 | Cred-block ends with `# === End Credentials ===\n` (one trailing `\n`) | ✅ Verified at `pty/credentials.rs:69` (last line of the `concat!` block). |
| §R4.2.2 | Title-prompt starts with `[AgentsCommander auto-title]` | ✅ Verified at `pty/title_prompt.rs:22`. |
| §R4.2.2 | Concat shape `format!("{}\n{}", cred_block, p)` produces a visible blank line between END marker and title-prompt header | ✅ Verified — cred_block's trailing `\n` plus format!'s inserted `\n` totals two `\n` between `# === End Credentials ===` and `[AgentsCommander auto-title]`, which renders as exactly one blank line. Architect's "one additional `\n`" wording is from the source-code POV; consistent with the rendered POV. |
| §R4.2.3 | Spawn-task lines 679-700 idle-wait stays unchanged | ✅ Verified — F1-captured values are read before `tokio::spawn`; helper runs after the idle-wait succeeds. |
| §R4.2.3 step 6 | "On `Ok(())` — log success, **stop**. The chain to `inject_title_prompt_after_idle_static` is **deleted**" | ✅ Verified — current chain at `session.rs:718-736`. Removing it is the v3-cleanup step. |
| §R4.2.4 | Helper imports `parse_brief_title`, `snapshot_brief_before_edit` from `entity_creation` (both `pub(crate)` at lines 200, 267) and `find_workgroup_brief_path_for_cwd` from `session::session` (`pub(crate)` at line 126) | ✅ Verified — all three are `pub(crate)`, callable from both `commands::session` (helper) and `config::session_context` (Edit 1). |
| §R4.2.4 | Helper is synchronous (no `async`, no `await`) | ✅ Sound — all three called helpers are sync (fs read, snapshot is `std::io::copy`, `build_title_prompt` is pure format!). No PTY/SessionManager state needed. |
| §R4.3.2 | 4-row matrix (zone-rendering combinations) | ✅ Verified — matches the existing parameterisation at `session_context.rs:479-499`. The new `(matrix_root, brief_display_path)` 2-tuple cleanly extends it. |
| §R4.3.3 Edit 1 | `find_workgroup_brief_path_for_cwd(agent_root)` defensive UNC strip | ✅ Verified — `agent_root` is already `\\?\`-stripped via `display_path` at `session_context.rs:14-16, 67-68`, but a second strip is idempotent and harmless. |
| §R4.3.3 Edit 4 | `format!`-built `String` interoperates with the existing `{forbidden_scope}` interpolation at line 533 | ✅ Verified — both `&str` and `String` impl `Display`. No caller change. |
| §R4.5 | F1, F3, F4, F6, F7, F9 preserved | ✅ Verified — each helper called by `build_title_prompt_appendage` is the same function it was in v3, untouched. |
| §R4.7 | Per-spawn rewrite via `materialize_agent_context_file` is sufficient migration | ✅ Verified — `commands/session.rs:592-615` calls into `materialize_agent_context_file`, which calls `resolve_session_context_content` → `ensure_session_context` → `default_context` → fresh CLAUDE.md/AGENTS.md/GEMINI.md every spawn. The MANAGED_CONTEXT_FILENAMES sweep at `session_context.rs:440-451` removes any prior file before writing the new one. Pre-existing replicas pick up the 4th zone on next spawn / `/clear` / restart. |
| §R4.6.9 | Unit tests for `default_context_*_when_*_wg_ancestor` work without filesystem fixtures | ✅ Verified — `find_workgroup_brief_path_for_cwd` is a pure path walk (no `std::fs` calls); existing tests at `session/session.rs:278-317` already exercise it without fixtures. The proposed tests use fabricated absolute paths (`r"C:\proj\.ac-new\..."`) — Rust's `Path::ancestors()` walks them without touching disk. |
| §R4.9 | Net delta ~−45 lines | ✅ Plausible — counted: deleting `inject_title_prompt_after_idle_static` (lines 287-432 = 146 lines, including the doc comment), adding `build_title_prompt_appendage` (~50 lines incl. doc comment), spawn-task body rewrite (~+15 net lines for the `if`/`match` shapes), `session_context.rs` edits (~+50 lines incl. tests). |
| §R4.6.11 (architect's checks) | All three pass — see R4.D-summary above | ✅ |

### R4.D2. Recommendation — concretise §R4.2.5's `title_appendage_was_some_indicator` placeholder via sibling let

The architect's §R4.2.5 already names the cleaner pattern. Lock it in. The exact spawn-task body to apply:

```rust
            // Issue #107 round 4 — build the optional title-prompt segment
            // BEFORE the PTY write. Synchronous fs reads + snapshot only;
            // no async work, no second idle-wait. See plan §R4.2.
            let title_appendage = if is_coordinator_clone && auto_title_enabled {
                match build_title_prompt_appendage(&cwd_clone) {
                    Ok(Some((prompt, bak_path))) => {
                        log::info!(
                            "[session] Auto-title appendage built for session {} (bak={:?})",
                            session_id,
                            bak_path
                        );
                        Some(prompt)
                    }
                    Ok(None) => {
                        log::info!(
                            "[session] Auto-title appendage skipped (gate not passed) for session {}",
                            session_id
                        );
                        None
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Auto-title appendage skipped for session {}: {}",
                            session_id,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };

            let auto_title_was_appended = title_appendage.is_some();
            let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd_clone);
            let combined = match title_appendage {
                Some(prompt) => format!("{}\n{}", cred_block, prompt),
                None => cred_block,
            };

            match crate::pty::inject::inject_text_into_session(
                &app_clone,
                session_id,
                &combined,
                true,
            )
            .await
            {
                Ok(()) => {
                    log::info!(
                        "[session] Bootstrap message injected for session {} (auto-title={})",
                        session_id,
                        auto_title_was_appended
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[session] Failed to inject bootstrap for {}: {}",
                        session_id,
                        e
                    );
                }
            }
        });
```

Two key choices captured:

1. **Sibling let** `let auto_title_was_appended = title_appendage.is_some();` is bound BEFORE `let combined = match title_appendage { … }` so the bool survives the `match`'s ownership move. The `&combined`-grep alternative in the architect's draft works too but is fragile (a future log-line tweak could lose the marker token). Sibling let is the safer pattern.

2. **Helper return type bump** from `Result<Option<String>, String>` to `Result<Option<(String, PathBuf)>, String>` — see R4.D3.

### R4.D3. Recommendation — bump helper return type to preserve `.bak`-path observability

Architect's §R4.2.4 helper drops the snapshot path on the floor (`let _bak_path = snapshot_brief_before_edit(&brief_path)…?;`). v3 logs `[session] Auto-title backup created: <path> (session <uuid>)` at line 410 — operators currently grep for it to verify the backup exists. R4 deletes that signal and §R4.12 admits the gap ("still emitted by `build_title_prompt_appendage` indirectly via `snapshot_brief_before_edit`" — but `snapshot_brief_before_edit` does not log internally; verified at `entity_creation.rs:267-302`).

Cleanest fix: helper returns the snapshot path alongside the prompt. Architect's "no internal logging" rule still holds — the caller logs.

Concrete signature swap for §R4.2.4:

```rust
fn build_title_prompt_appendage(
    cwd: &str,
) -> Result<Option<(String, std::path::PathBuf)>, String> {
    // … gates 1-4 unchanged …
    let bak_path = snapshot_brief_before_edit(&brief_path)
        .map_err(|e| format!("snapshot BRIEF.md before edit: {}", e))?;
    // … gate 6 unchanged …
    Ok(Some((prompt, bak_path)))
}
```

Caller-side already shown in R4.D2 — destructures `(prompt, bak_path)` and includes the bak path in the success log line:

```
[session] Auto-title appendage built for session <uuid> (bak="C:\proj\.ac-new\wg-2\BRIEF.md.20260501-145322.bak")
```

Cost: one extra field in the return tuple. Benefit: ops parity with v3, plus the bak path appears at a **higher** log level (info on the appendage-built line), not buried in a separate INFO line that may be filtered out independently.

Update §R4.12 logging table accordingly:

| Old (v3) | New (Round 4 — with R4.D3 fold) |
|---|---|
| `[session] Auto-title backup created: <path> (session <uuid>)` | folded into `[session] Auto-title appendage built for session <uuid> (bak=<path>)` |

### R4.D4. Question for tech-lead — collapsed skip-reason log line

§R4.12 acknowledges that v3's two distinct skip log lines (`Auto-title skipped (BRIEF empty)`, `Auto-title skipped (title present)`) collapse into one R4 line: `Auto-title appendage skipped (gate not passed)`. The helper returns `Ok(None)` in both cases and the caller cannot disambiguate.

Two options:

- **(a) Accept the collapse** (architect's choice). Operators counting "no brief" vs "already titled" can no longer distinguish them via logs — they have to read BRIEF.md state directly. Code is minimal.
- **(b) Restore distinguishability** with a typed enum:
  ```rust
  enum AppendageOutcome {
      Built { prompt: String, bak_path: std::path::PathBuf },
      SkippedEmptyBrief,
      SkippedTitleAlreadyPresent,
  }
  fn build_title_prompt_appendage(cwd: &str) -> Result<AppendageOutcome, String>
  ```
  Caller matches all three `Ok(_)` arms and emits distinct INFO lines mirroring v3's strings. ~10 extra lines.

I recommend **(a)** — these are infrequent, low-stakes events; the loss of distinction is small and the overall log volume is already quieter under R4 (one bootstrap line vs two). But I want tech-lead's explicit thumbs-up before locking it in, since it's a (minor) observability regression vs v3.

### R4.D5. Recommendation — mirror `{matrix_section}{brief_section}` template style for cleaner rendering

Architect's §R4.3.3 Edit 5 inserts `{brief_section}` on its own line in the template:

```text
{matrix_section}
{brief_section}

Any repository or directory outside the allowed places above is READ-ONLY.
```

But `matrix_section` already terminates with `\n\n` (per its own format string at the existing line 488). With the literal newline between placeholders, plus `brief_section` also ending in `\n\n`, plus the template's existing blank line at the current line 527, the rendered output between zone 3 and "Any repository..." gets up to **3 blank lines** stacked. Cosmetic only; not broken — but inconsistent with the cleaner pattern Edit 5 already uses for the bullets section: `{matrix_allowed}{brief_allowed}- **FORBIDDEN**: ...` (no literal newline between placeholders; each helper string ends with exactly one `\n`).

Apply the same shape to the section block:

```text
{matrix_section}{brief_section}
Any repository or directory outside the allowed places above is READ-ONLY.
```

And shorten `brief_section`'s trailing whitespace from `\n\n` to `\n` (one trailing newline) so the adjacent placeholders compose without stacking blank lines:

```rust
let brief_section = match brief_display_path.as_deref() {
    Some(brief_path) => format!(
        "4. **The workgroup `BRIEF.md` file** — readable AND writable, since AgentsCommander may inject prompts asking the agent to maintain the brief:\n   ```\n   {brief_path}\n   ```\n   **Scope discipline**: ONLY this single file is writable in the workgroup root. NOT the workgroup root directory, NOT any other file at the workgroup root.\n",
        brief_path = brief_path,
    ),
    None => String::new(),
};
```

Drops one `\n` at the end. Combined with the `{matrix_section}{brief_section}` concatenation, the rendered separator becomes a single blank line between zone 3 and zone 4 — visually consistent with the existing single-blank-line gap between zones 1↔2 and 2↔3 in the current template.

Minor / cosmetic, but worth doing once at code time rather than discovering misaligned rendering later.

### R4.D6. Verified — `read_workgroup_brief_for_cwd` wrapper survives R4

The wrapper at `session/session.rs:141-147` is **not** dead code post-R4. It has another caller at `session/session.rs:207` (`workgroup_brief: read_workgroup_brief_for_cwd(&s.working_directory)`), which feeds the `SessionInfo` IPC payload. Don't touch it. R4's helper does its own read directly via `std::fs::read_to_string` — that's intentional separation (the helper needs the raw content for parsing AND the path for snapshot/prompt; the wrapper only returns trimmed content).

### R4.D7. Verified — idempotency byte-for-byte match when title_appendage is None

Constraint from the tech-lead brief: "if `auto-title` is OFF and we DON'T append the title prompt, are we 100% sure the cred-block path remains byte-for-byte identical to v3?"

In R4.D2's body:

```rust
let combined = match title_appendage {
    Some(prompt) => format!("{}\n{}", cred_block, prompt),
    None => cred_block,
};
inject_text_into_session(&app_clone, session_id, &combined, true).await
```

When `title_appendage` is `None`, `combined = cred_block` (move, no copy, no transformation). The payload to `inject_text_into_session` is bit-identical to today's v3 cred-block-only path at line 702-709. The Enter-keystroke timing in `inject.rs:79-111` is unchanged. ✅ Byte-for-byte identical when auto-title is OFF.

### R4.D8. Recommendation — add a low-cost unit test for the snapshot-failure-aborts contract

§R4.6.10 argues `build_title_prompt_appendage` doesn't need its own unit test ("thin orchestrator"). Mostly true — the gates 1-4 are individual function calls already tested. The one piece NOT covered by composition is the **post-snapshot bail**: if `snapshot_brief_before_edit` returns `Err`, the helper returns `Err`, the caller falls back to cred-block alone. This is a behaviour change vs v3 (§R4.5 row "Snapshot fails" notes parity but the timing differs).

A 15-line test pins the behaviour:

```rust
#[test]
fn build_title_prompt_appendage_returns_err_when_snapshot_collides() {
    let dir = std::env::temp_dir().join(format!(
        "ac-r4-snap-collide-{}-wg-1-x", std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let brief = dir.join("BRIEF.md");
    std::fs::write(&brief, b"Some brief body, no title.\n").unwrap();

    // Pre-populate a same-second `.bak` so the helper's snapshot collides.
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let preexisting = dir.join(format!("BRIEF.md.{}.bak", ts));
    std::fs::write(&preexisting, b"prior").unwrap();

    let cwd = dir.to_string_lossy().to_string();
    let result = build_title_prompt_appendage(&cwd);
    assert!(matches!(result, Err(ref e) if e.starts_with("snapshot BRIEF.md before edit:")));

    let _ = std::fs::remove_file(&brief);
    let _ = std::fs::remove_file(&preexisting);
    let _ = std::fs::remove_dir(&dir);
}
```

Catches the regression "future refactor swallows snapshot Err and injects the prompt without a backup" — exactly the kind of subtle drift v3's F6/F9 folds were defending against. Optional; tech-lead's call.

### R4.D9. Disagreements with the architect

None substantive. The two pieces I'd push on (R4.D3 helper return type, R4.D5 template cosmetics) are refinements, not corrections. The architect explicitly invited dev-rust to pick the §R4.2.5 sibling-let shape (R4.D2) so that's not a disagreement either.

### R4.D-ready — Ready-to-implement summary

If R4.D2 (sibling-let body), R4.D3 (helper returns `(prompt, bak_path)` tuple), R4.D5 (template `{matrix_section}{brief_section}` + single trailing `\n` in `brief_section`), and tech-lead's call on R4.D4 (a vs b) are folded, R4 is implementable as written. R4.D8 (snapshot-collision unit test) is a polish item; recommend including but not blocking.

Architect's three flagged checks (§R4.6.11) all pass — see R4.D-summary at the top of this section. No conflicts with §R4.10 open questions; tech-lead's resolutions noted at the §R4.10 preamble are consistent with the implementation surface verified above.

I read the round-4 plan, the messaging chain, and the actual code on `feature/107-auto-brief-title` HEAD `e458b85`. No additional verification gaps surfaced.

---

## Round 5 — post-137 refresh

Round 5 is **a refresh on top of Round 4**, triggered by issue #137 landing on `main` (PR #145, merged 2026-05-03). #137 introduced two coordinator-only CLI verbs that write `BRIEF.md` on the agent's behalf — `agentscommander_mb brief-set-title "<text>"` and `agentscommander_mb brief-append-body "<text>"` — both with caller validation, atomic write, timestamped backup, and advisory locking. The binary holds the write privilege, not the agent.

This eliminates Round 4 §R4.3 (Change B — GOLDEN RULE 4th-zone amendment) entirely, because the agent never writes `BRIEF.md` directly. It also removes the backend-side snapshot helper, because the CLI verb does its own backup atomically.

Round 5 source: `messaging/20260505-025423-wg5-tech-lead-to-wg5-architect-107-refresh-plan-post-137.md`.

The audit trail above (v3, Round 4, dev-rust review of Round 4) stays intact. Round 5 documents only the deltas.

### §R5.1 Why a Round 5

Round 4's load-bearing fix (Change B) was: *amend the GOLDEN RULE template so the agent is permitted to write `BRIEF.md`*. That fix was the entire reason Round 4 existed — without it, the agent refused the write (§R4.1).

#137 supersedes Change B. The new shape:

```
agent receives prompt ───►  agent runs:                                  ──► BRIEF.md
                            "<BinaryPath>" brief-set-title \             updated
                              --token <T> --root <R> --title "<text>"    by binary
                            (binary validates caller, writes file,
                             creates timestamped backup, exits 0/1)
```

Because the agent invokes a binary that writes the file in its name (gated by `is_coordinator` + token validation), the agent itself never modifies `BRIEF.md`. The GOLDEN RULE template stays at three zones; no 4th zone, no `brief_section` placeholder, no `forbidden_scope` carve-out. `session_context.rs` is **not touched** by Round 5.

Change A (single combined PTY write) survives Round 5 — it was opportunistic in Round 4, but it remains the right design with #137 because:
- It still reduces TOCTOU surface (one inject vs two).
- Cred block + prompt in one paste is naturally referenceable from the prompt body — the prompt says *"use the values from your `# === Session Credentials ===` block above"*, and the values are literally above in the same paste.
- v3's two-write model required a second idle-wait; Round 4 simplified to one wait; Round 5 keeps that simplification.

### §R5.2 What Round 5 keeps from Round 4

| Round 4 element | Round 5 |
|---|---|
| Single combined PTY write at spawn (cred-block + title-prompt) | **Kept** — same wire shape (`format!("{}\n{}", cred_block, prompt)`). |
| `is_coordinator_clone` + `auto_title_enabled` capture before `tokio::spawn` (F1) | **Kept** unchanged. |
| Synchronous helper `build_title_prompt_appendage(cwd: &str)` placed before `create_session_inner` | **Kept** — but signature simplified (see §R5.3). |
| Gate (1) — workgroup root resolvable; F7 → `Err` + `[auto-title:config]` warn | **Kept**. |
| Gate (2) — BRIEF.md exists and read succeeds → else `Err` | **Kept**. |
| Gate (3) — non-empty brief → else `Ok(None)` (silent skip) | **Kept**. |
| Gate (4) — no `title:` field → else `Ok(None)` (silent skip) | **Kept**. Backend gate stays the FIRST line of idempotence; the CLI verb's `EditOutcome::NoOp` is the SECOND (TOCTOU safety net). See §R5.10. |
| F4 — UNC `\\?\` prefix strip on the path embedded in the prompt | **Kept** — needed because the prompt still references the absolute path for context. |
| §R4.2.5 sibling-let shape (`auto_title_was_appended` bound before the move-out match) | **Kept** as the canonical body. |
| Best-effort failure semantics — warn + fall back to cred-block alone | **Kept** unchanged. |
| `parse_brief_title` + `find_workgroup_brief_path_for_cwd` helpers from v3 | **Kept** unchanged. |

### §R5.3 What Round 5 removes from Round 4 (and from v3)

| Element | Why it goes |
|---|---|
| **§R4.3 — Change B (GOLDEN RULE 4th-zone amendment)** | Superseded by #137. The agent never writes `BRIEF.md`; the binary does. No template change needed. |
| **`brief_display_path`, `brief_section`, `brief_allowed`, `workgroup_clause` derivations in `default_context`** | Same reason. `session_context.rs` is not touched. |
| **§R4.6.7 (GOLDEN RULE 4th-zone manual test)** | No 4th zone exists; nothing to verify. |
| **§R4.6.9 (`default_context_*_when_*_wg_ancestor` unit tests)** | Same. |
| **§R4.6.8 (combined message obeys 4th zone, 3-family table)** | Replaced by §R5.8.4 (CLI-verb end-to-end test). |
| **§R4.2.4 step 5 — `snapshot_brief_before_edit` call** | The CLI verb (`brief-set-title`) creates its own atomic timestamped backup before every successful write that had a prior file (`cli/brief_ops.rs:454-492`). A backend-side snapshot is redundant — and worse, it produces TWO backup files per spawn (one from the helper, one from the verb), polluting the workgroup root. |
| **`bak_path` field in `build_title_prompt_appendage`'s return tuple** (R4.D3 fold) | The backend never produces a backup, so there's nothing to surface to the caller. Helper return type collapses from `Result<Option<(String, PathBuf)>, String>` back to `Result<Option<String>, String>`. |
| **`snapshot_brief_before_edit` helper itself** (v3 §16; `entity_creation.rs::snapshot_brief_before_edit`) | No remaining caller after the spawn-chain stops calling it. Delete the function and its tests. The only artifact left is a comment in §16 of v3 explaining why it existed — leave the §16 prose for the audit trail. |
| **§R4.6.12 / R4.D8 — snapshot-collision aborts the appendage unit test** | No snapshot in the appendage anymore; nothing to test. |
| **F6 / F9 folds — relevant only to `snapshot_brief_before_edit`** | Audit-trail rows in §13 stay (history is whole-file); the helper they apply to is gone. |
| **v3 §17.2 / §17.3 tests for `snapshot_brief_before_edit`** | Removed alongside the helper. `parse_brief_title` and `build_title_prompt` tests stay. |

### §R5.4 New title-prompt content (the load-bearing change)

#### §R5.4.1 Decisions on prompt shape

Tech-lead's seven decision points from the Round 5 brief (`20260505-025423`):

| Q | Decision | Rationale |
|---|---|---|
| Q1 — Form of the new prompt | Reference BRIEF.md by absolute path (for context) + instruct the agent to invoke `brief-set-title`. Constraints (≤8 words, single line) move from "format the YAML directly" to "pass the title text to `--title`". The CLI handles YAML escaping. | Agent never sees a frontmatter template — that's the binary's job. Shorter prompt; clearer agent task. |
| Q2 — Literal `--token X --root Y` vs agent composes from credentials | **Option 2: agent composes from `# === Session Credentials ===`.** Prompt embeds literal placeholders `<YOUR_TOKEN>`, `<YOUR_ROOT>`, `<YOUR_BINARY_PATH>` and instructs the agent to substitute from the cred block immediately above. | (1) The cred block is in the SAME PTY write as the prompt — values are literally above. (2) The pattern matches the existing `send`/`list-peers` examples in CLAUDE.md (`session_context.rs:577-612`). (3) Embedding the literal token leaks it into the agent's transcript twice (creds + prompt). (4) Shorter prompt. The "more error-prone" risk is real but small — agents already execute the `send` template flawlessly with the same composition pattern. |
| Q3 — How does the agent know which BinaryPath to use | Same as Q2 — `<YOUR_BINARY_PATH>` placeholder, resolved from creds. | The cred block contains `# BinaryPath: <abs_path>`. Agent reads it; no backend pass-through needed. Backend doesn't even know which binary the user launched — `BinaryPath` is computed by the cred-block builder from `std::env::current_exe()`. Pre-computing it server-side and embedding it in the prompt would duplicate that resolution. |
| Q4 — CLI failure handling | **Stay best-effort.** Agent invokes the verb; verb prints `Error: ...` to stdout (visible in agent transcript) and exits 1 on failure. Backend does NOT poll BRIEF.md, does NOT verify success, does NOT retry. On next Coordinator spawn, gate (4) sees no `title:` and re-injects the prompt. The agent's natural reporting ("I tried to set the title but the CLI returned an authorization error") is sufficient observability. | Mirrors v3's "best-effort, never retry" philosophy (§2.7). Adds NO new verification machinery. |
| Q5 — Idempotence post-137 | **Backend gate (4) stays AS-IS.** The CLI verb's NoOp behavior (`EditOutcome::NoOp` when title value matches, `cli/brief_ops.rs:445-449`) is a SECOND idempotence layer for TOCTOU safety, not a replacement for the gate. See §R5.10. | The §2.4 gate "fires only if BRIEF.md ... lacks a `title:` field" is preserved verbatim. The verb's NoOp catches the rare case where a sibling agent or manual edit added a `title:` between gate-check and verb-invocation. |
| Q6 — End-to-end test | New §R5.8.4 — spawn a real Coordinator with non-empty brief, no title; verify (a) single combined paste lands; (b) agent transcript shows the verb invocation with non-placeholder values; (c) BRIEF.md ends with `title: '...'` matching the agent's chosen text; (d) `BRIEF.<UTC-ts>.bak.md` exists in the workgroup root (created by the verb, not by the backend); (e) no other files modified. | Replaces v3 §12.2 happy-path. |
| Q7 — Rebase conflicts | **Owned by dev-rust.** The rebase paused at conflicts in `entity_creation.rs`, `settings.rs`, and `src/shared/types.ts`. Round 5's plan ASSUMES those are resolved before Step 6 (implementation). | The rebase is mechanical 3-way merge work; it does not require architectural decisions. Round 5 does not re-litigate any v3 settings or types choices. |

#### §R5.4.2 `build_title_prompt` rewrite

`src-tauri/src/pty/title_prompt.rs::build_title_prompt` is **rewritten** (still at the same file, same function name, same signature `(brief_absolute_path: &str) -> String`). The new body:

```rust
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
```

The R2 fold tests for `8 words or fewer` and exact-format `---\ntitle: ...\n---` checks are **removed** — the new prompt does not emit a YAML template (the CLI does that), and the "8 words" guidance is now phrased as part of "(8 words or fewer, single line, no trailing period)" inline. The new test set covers the structural invariants of the rewritten prompt: marker prefix, path interpolation, CLI-verb syntax presence, credentials-block reference, direct-edit prohibition, skip conditions.

The pure-function shape (`(&str) -> String`, no I/O) and module location (`src-tauri/src/pty/title_prompt.rs`) are unchanged. `pty/mod.rs::pub mod title_prompt;` is unchanged.

### §R5.5 Helper changes — `build_title_prompt_appendage` simplification

The helper from Round 4 §R4.2.4 is **simplified**. Final Round 5 shape:

```rust
/// Issue #107 round 5 — build the title-prompt segment to concat with the
/// cred-block, OR `Ok(None)` if the auto-title preconditions don't hold.
///
/// Synchronous: filesystem reads only, no PTY, no await, no snapshot.
/// (#137 introduced `brief-set-title` which creates its own atomic backup;
/// the backend no longer snapshots before injection.)
///
/// The caller is the post-spawn task in `create_session_inner`; it
/// concatenates the returned `Some(prompt)` with the cred-block and issues a
/// single `inject_text_into_session` call (Round 4 §R4.2.3 — preserved in
/// Round 5).
///
/// Gates layered (in order):
///   1. workgroup BRIEF.md path resolvable from `cwd` → else `Err`
///      (config issue, F7 preserved).
///   2. BRIEF.md exists and read succeeds → else `Err`.
///   3. BRIEF.md non-empty (after trim) → else `Ok(None)` (silent skip).
///   4. No `title:` field in existing frontmatter → else `Ok(None)` (silent
///      skip).
///   5. Build title prompt with the absolute, UNC-stripped path (F4
///      preserved). Return `Ok(Some(prompt))`.
///
/// (Round 4's gate 5 — `snapshot_brief_before_edit` — is removed in Round 5
/// per §R5.3.)
fn build_title_prompt_appendage(cwd: &str) -> Result<Option<String>, String> {
    use crate::commands::entity_creation::parse_brief_title;
    use crate::session::session::find_workgroup_brief_path_for_cwd;

    // (1) Resolve workgroup BRIEF.md path. F7 preserved.
    let brief_path = find_workgroup_brief_path_for_cwd(cwd)
        .ok_or_else(|| format!("[auto-title:config] no wg- ancestor in cwd '{}'", cwd))?;

    // (2) Read BRIEF.md. Missing/unreadable → Err (warn-and-skip at caller).
    let content = std::fs::read_to_string(&brief_path)
        .map_err(|e| format!("read BRIEF.md at {:?}: {}", brief_path, e))?;

    // (3) Empty brief → silent skip.
    if content.trim().is_empty() {
        return Ok(None);
    }

    // (4) Title already present → silent skip.
    if parse_brief_title(&content).is_some() {
        return Ok(None);
    }

    // (5) F4 preserved — strip Windows \\?\ extended-length prefix.
    let raw = brief_path.to_string_lossy().to_string();
    let path_str = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
    let prompt = crate::pty::title_prompt::build_title_prompt(&path_str);

    Ok(Some(prompt))
}
```

Net delta vs Round 4 §R4.2.4 helper:
- Return type: `Result<Option<(String, PathBuf)>, String>` → `Result<Option<String>, String>`. (No PathBuf surfaced.)
- One `use` line removed (`snapshot_brief_before_edit`).
- Five lines deleted (the snapshot call + error wrap).
- Helper body is ~25 lines (down from ~35).

The caller in `commands/session.rs` (Round 4 §R4.2.5 + R4.D2) becomes:

```rust
            // Issue #107 round 5 — build the optional title-prompt segment
            // BEFORE the PTY write. Synchronous fs reads only; no async
            // work, no snapshot, no second idle-wait. See plan §R5.5.
            let title_appendage = if is_coordinator_clone && auto_title_enabled {
                match build_title_prompt_appendage(&cwd_clone) {
                    Ok(Some(prompt)) => {
                        log::info!(
                            "[session] Auto-title appendage built for session {}",
                            session_id
                        );
                        Some(prompt)
                    }
                    Ok(None) => {
                        log::info!(
                            "[session] Auto-title appendage skipped (gate not passed) for session {}",
                            session_id
                        );
                        None
                    }
                    Err(e) => {
                        log::warn!(
                            "[session] Auto-title appendage skipped for session {}: {}",
                            session_id,
                            e
                        );
                        None
                    }
                }
            } else {
                None
            };

            let auto_title_was_appended = title_appendage.is_some();
            let cred_block = crate::pty::credentials::build_credentials_block(&token, &cwd_clone);
            let combined = match title_appendage {
                Some(prompt) => format!("{}\n{}", cred_block, prompt),
                None => cred_block,
            };

            match crate::pty::inject::inject_text_into_session(
                &app_clone,
                session_id,
                &combined,
            )
            .await
            {
                Ok(()) => {
                    log::info!(
                        "[session] Bootstrap message injected for session {} (auto-title={})",
                        session_id,
                        auto_title_was_appended
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[session] Failed to inject bootstrap for {}: {}",
                        session_id,
                        e
                    );
                }
            }
        });
```

Two adjustments vs Round 4 §R4.2.5:

1. **`Ok(Some(prompt))`** instead of `Ok(Some((prompt, bak_path)))` — matches the simplified helper return.
2. **`inject_text_into_session(&app_clone, session_id, &combined)`** — three args, NOT four. The current `inject.rs:36-40` signature on `main` is `(app, session_id, text)`; v3 §9.3 and Round 4 §R4.2.5 wrote `(.., true)` four-arg, but that `submit: bool` parameter does not exist in the function. The R4 reviewers did not catch this — it would have failed to compile. Round 5 corrects: 3-arg. The function already submits with two `\r` keystrokes (1500 ms / 500 ms gaps) — see `inject.rs:79-111`; no `submit` flag is needed.

The "appendage built" log line **drops** the Round 4 R4.D3 `(bak={:?})` suffix because there's no backend-side bak path. Operators who want to confirm the backup exists can grep the agent's PTY transcript for `BRIEF.md title updated; backup: <path>` (the verb's success line — `cli/brief_set_title.rs:128`) or list `BRIEF.<UTC-ts>.bak.md` in the workgroup root. See §R5.11.

### §R5.6 Files touched (Round 5 net)

| File | Action | Detail |
|---|---|---|
| `src-tauri/src/config/settings.rs` | modify | Add `auto_generate_brief_title: bool` field (UNCHANGED from v3 §4.1; rebase conflict here is owned by dev-rust). |
| `src-tauri/src/commands/entity_creation.rs` | modify | (a) `build_brief_content` template change (UNCHANGED from v3 §5.1). (b) Add `parse_brief_title` (UNCHANGED from v3 §6.2). (c) **Do NOT add `snapshot_brief_before_edit`** — Round 5 supersedes v3 §16 entirely. If the v3 commit `3f7da00` introduced the helper, **dev-rust deletes it** during rebase resolution: remove the function body, its tests, and the v3 §17.2 unit tests. |
| `src-tauri/src/session/session.rs` | modify | Add `find_workgroup_brief_path_for_cwd` (UNCHANGED from v3 §7.1). |
| `src-tauri/src/pty/title_prompt.rs` | **rewrite** | New body per §R5.4.2. Same file, same function signature, new prompt content + new test set. |
| `src-tauri/src/pty/mod.rs` | modify | Add `pub mod title_prompt;` (UNCHANGED from v3 §8.2). |
| `src-tauri/src/commands/session.rs` | modify | Add `build_title_prompt_appendage` (Round 5 §R5.5 helper). Replace the single `inject_text_into_session(.., &cred_block)` call at lines 545-565 with the combined-message block (§R5.5 caller code). NO snapshot call, NO `inject_title_prompt_after_idle_static` async helper. |
| `src/shared/types.ts` | modify | Add `autoGenerateBriefTitle: boolean` (UNCHANGED from v3 §4.2). |
| `src/sidebar/components/SettingsModal.tsx` | modify | Add checkbox (UNCHANGED from v3 §4.3). |
| `src-tauri/src/config/session_context.rs` | **NOT TOUCHED** | Reverses Round 4 §R4.3 entirely. The GOLDEN RULE template stays at three zones (replicas) / two zones (matrix roots). |

No new crates. No new modules. No frontend changes beyond the existing v3 setting toggle.

### §R5.7 What's removed from #107 (full audit list, post-Round-5)

Items deleted from the in-flight branch state (commits `807d863` → `3f7da00` → `e458b85` → `d4d4e07`):

| Source | Item | Replacement |
|---|---|---|
| `commands/entity_creation.rs` | `snapshot_brief_before_edit` helper (introduced by `3f7da00`) | None — CLI verb's atomic backup. |
| `commands/entity_creation.rs` | Tests for `snapshot_brief_before_edit` (v3 §17.2) | Removed. |
| `commands/session.rs` | `inject_title_prompt_after_idle_static` async helper (introduced by `e458b85`) | `build_title_prompt_appendage` synchronous helper (§R5.5). |
| `commands/session.rs` | Two-write spawn-task body (introduced by `e458b85`) | Single combined-write spawn-task body (§R5.5). |
| `pty/title_prompt.rs` | Original prompt body (introduced by `3f7da00`) | New prompt body (§R5.4.2). |
| `_plans/107-auto-brief-title.md` Round 4 §R4.3 | All `session_context.rs` edits | Not implemented. |
| `_plans/107-auto-brief-title.md` Round 4 §R4.6.7-§R4.6.9, §R4.6.12 | GOLDEN RULE 4th-zone tests, snapshot-collision test | Replaced by §R5.8.4 (CLI-verb end-to-end), §R5.8.6 (backend gate idempotence test). |

These are removals from the **plan's intent**, executed during the rebase (commits get re-shaped) or as the dev-rust implementation pass (Step 6).

### §R5.8 Test plan delta vs Round 4

Most v3 §12 / Round 4 §R4.6 manual scenarios stay in shape; the observable artifacts shift from "BRIEF.md edited by the agent + a `.bak` from `snapshot_brief_before_edit`" to "BRIEF.md edited by the binary + a `BRIEF.<UTC-ts>.bak.md` from the verb".

#### §R5.8.1 §12.2 happy path — single combined message + CLI verb invocation

Replaces v3 §12.2 step 4 (Round 4 §R4.6.1 already replaced step 4 once). Round 5 step 4:

> 4. After the cred block lands and the agent goes idle, observe **a single PTY paste containing both the cred block and the title prompt**, separated by a blank line. Cred block first (`# === Session Credentials ===` ... `# === End Credentials ===`), then immediately a `[AgentsCommander auto-title]` prompt instructing a `brief-set-title` invocation.
> 5. The agent reads BRIEF.md, picks a title, and runs the `brief-set-title` verb with `--token`, `--root`, `--title` resolved from the cred block. The verb's stdout (`BRIEF.md title updated; backup: <path>`) is visible in the agent's transcript.
> 6. App log contains:
>    - `[session] Auto-title appendage built for session <uuid>` (NO `bak=...` suffix vs Round 4).
>    - `[session] Bootstrap message injected for session <uuid> (auto-title=true)`.
> 7. BRIEF.md now starts with `---\ntitle: '<the agent's chosen title>'\n---\n` (canonical single-quoted YAML form per `brief_ops::apply_set_title`). Body preserved byte-for-byte modulo line-ending normalisation.
> 8. A backup file `BRIEF.<YYYYMMDD-HHMMSS>.bak.md` exists in the workgroup root, created by the verb (NOT by the backend). Same wall-clock second as the verb invocation.
> 9. No other files modified in the workgroup root.

#### §R5.8.2 §12.3 idempotent restart — same as Round 4 §R4.6.2

Unchanged from Round 4. The backend gate (4) short-circuits; helper returns `Ok(None)`; `auto-title=false` in the bootstrap log line.

Additional verification (Round 5-specific): even if the gate misfires (e.g. clock-skew TOCTOU), the CLI verb's `EditOutcome::NoOp` would catch it and print `BRIEF.md unchanged (title value already matches)` to the agent's transcript. The verb does NOT create a backup in NoOp mode (`brief_ops::perform_inner` returns at line 446-448 before the backup branch).

#### §R5.8.3 §12.4 / §12.5 / §12.6 — same shape as Round 4

Empty brief, setting OFF, non-Coordinator agent — all gate at the same points as Round 4. No differences observable except the missing `bak=...` suffix in any "appendage built" log lines (which don't fire in these scenarios anyway).

#### §R5.8.4 NEW §12.16 — End-to-end CLI-verb integration

This is the Round 5 canonical happy-path test. Replaces Round 4's §R4.6.8 (the "combined message obeys 4th zone" 3-family table — superseded entirely by #137).

**Setup**:
1. Create a fresh workgroup (e.g. `wg-9-test`) with a non-empty user-supplied brief, no `title:`.
2. Configure a Coordinator agent for the workgroup.
3. Confirm `auto_generate_brief_title` is true in settings (default).

**Execution**:
4. Spawn the Coordinator session.
5. Wait for the agent to receive the combined paste.
6. Wait for the agent to invoke the verb (visible in PTY transcript).

**Verification**:
7. App log contains:
   - `[session] Auto-title appendage built for session <uuid>`.
   - `[session] Bootstrap message injected for session <uuid> (auto-title=true)`.
   - `[brief] set-title: sender=<agent_name> wg=<wg-root> pid=<pid> backup=<bak_path>` (emitted by `cli/brief_set_title.rs:121-127` on the binary's success path — visible in the app log because the CLI verb runs a `validate_cli_token` → `load_settings` flow that initialises the same logger; cf. commit `f77aa34` "fix(cli): init logger in CLI path").
8. `BRIEF.md` byte-content begins with `---\ntitle: '<chosen title>'\n---\n` followed by the verbatim original body. `<chosen title>` is single-quoted (canonical YAML form) and YAML-escapes any single-quote characters in the agent's text.
9. `BRIEF.<YYYYMMDD-HHMMSS>.bak.md` exists in the workgroup root, byte-identical to the pre-edit BRIEF.md.
10. No `BRIEF.md.lock` file remains (`brief_ops::LockGuard::Drop` cleans it up).
11. No `BRIEF.md.tmp.<pid>` file remains (verb's atomic-publish step always cleans up — `brief_ops::perform_inner` at lines 559 and 498).

**Cross-family note**: this scenario should pass on Claude Code. Codex and Gemini should also pass because the agent invokes a CLI verb (a routine "run shell command" operation) rather than directly modifying a file outside its allowed write zones — the family-specific permission models all allow CLI invocations regardless of the GOLDEN RULE wording. Round 5 makes Codex/Gemini compliance a normal-path expectation, not a best-effort fallback.

#### §R5.8.5 NEW §12.17 — CLI authorization failure path

Verifies that a misconfigured agent doesn't silently succeed.

**Setup**:
1. Same as §12.16, but the agent's `--token` is corrupted before invocation (e.g. user manually typoed it during testing — simulated via a wrapper script that intercepts the agent's command line).

**Verification**:
2. Verb exits 1 with stdout `Error: invalid token '<prefix>...'. Expected a valid session token (UUID) or root token.` (cf. `cli/mod.rs:124-129`).
3. Agent observes the failure and reports it back into the session ("I tried to set the title but the CLI rejected my token — see error above").
4. BRIEF.md is unchanged. No `BRIEF.<UTC-ts>.bak.md` was created (the verb aborts before the backup loop, since `validate_cli_token` runs first — `brief_set_title.rs:50-56`).
5. Backend log: NO follow-up entries — the backend doesn't poll BRIEF.md and never observes the failure.
6. Next Coordinator spawn triggers the same prompt (gate (4) still sees no title), and if the token is now valid, the verb succeeds.

This pin is intentionally low-cost — it does not require a fault-injection test harness in `cargo test`. Manual verification at integration time is sufficient for the best-effort surface.

#### §R5.8.6 NEW §17.4 — `build_title_prompt_appendage` idempotence unit test

Add to `src-tauri/src/commands/session.rs` `mod tests` (the helper is co-located).

```rust
#[test]
fn build_title_prompt_appendage_returns_none_when_title_already_present() {
    use std::env;
    let dir = env::temp_dir().join(format!(
        "wg-r5-idempotent-{}", std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let brief = dir.join("BRIEF.md");
    std::fs::write(&brief, b"---\ntitle: 'Pre-existing'\n---\nBody.\n").unwrap();
    let cwd = dir.to_string_lossy().to_string();
    let result = build_title_prompt_appendage(&cwd);
    assert!(matches!(result, Ok(None)), "expected Ok(None), got {:?}", result);
    let _ = std::fs::remove_file(&brief);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn build_title_prompt_appendage_returns_none_when_brief_empty() {
    use std::env;
    let dir = env::temp_dir().join(format!(
        "wg-r5-empty-{}", std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let brief = dir.join("BRIEF.md");
    std::fs::write(&brief, b"   \n\n\t\n").unwrap();
    let cwd = dir.to_string_lossy().to_string();
    let result = build_title_prompt_appendage(&cwd);
    assert!(matches!(result, Ok(None)), "expected Ok(None), got {:?}", result);
    let _ = std::fs::remove_file(&brief);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn build_title_prompt_appendage_returns_some_when_brief_has_no_title() {
    use std::env;
    let dir = env::temp_dir().join(format!(
        "wg-r5-some-{}", std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let brief = dir.join("BRIEF.md");
    std::fs::write(&brief, b"# A real brief with body content.\n").unwrap();
    let cwd = dir.to_string_lossy().to_string();
    let result = build_title_prompt_appendage(&cwd);
    let prompt = match result {
        Ok(Some(p)) => p,
        other => panic!("expected Ok(Some(_)), got {:?}", other),
    };
    assert!(prompt.contains("brief-set-title"));
    assert!(prompt.contains("<YOUR_BINARY_PATH>"));
    let brief_str = brief.to_string_lossy().to_string();
    assert!(prompt.contains(&brief_str));
    let _ = std::fs::remove_file(&brief);
    let _ = std::fs::remove_dir(&dir);
}
```

Tempdir naming starts with `wg-` so `find_workgroup_brief_path_for_cwd`'s ancestor walk finds the cwd itself as the wg ancestor (mirrors Round 4 §R4.6.12's path-walk workaround note). The three tests pin gates (3), (4), and the happy path. Path-walk gate (1) failure is exercised by the existing v3 §17.1 tests on `find_workgroup_brief_path_for_cwd`. Read-failure gate (2) requires fault-injecting `std::fs::read_to_string`, which is not worth the harness for a thin orchestrator.

### §R5.9 Failure modes — exhaustive matrix for Round 5

| Scenario | Coordinator? | Setting ON? | wg-* ancestor? | Brief state | CLI verb outcome | Net behavior |
|---|---|---|---|---|---|---|
| Standard happy path | yes | yes | yes | non-empty, no title | success | Combined message injected. Agent runs verb. Verb writes title + creates `.bak`. |
| Setting OFF | yes | no | yes | any | not invoked | Cred-block alone. `auto-title=false`. |
| Non-Coordinator | no | any | yes | any | not invoked | Cred-block alone. |
| No wg-* ancestor (config issue, F7) | yes | yes | no | n/a | not invoked | Cred-block alone. Helper `Err`. Warn-log. |
| BRIEF.md missing (read error) | yes | yes | yes | read fails | not invoked | Cred-block alone. Helper `Err`. Warn-log. |
| BRIEF.md empty | yes | yes | yes | empty | not invoked | Cred-block alone. Helper `Ok(None)`. Info-log "appendage skipped". |
| Title already present | yes | yes | yes | non-empty, has title | not invoked | Cred-block alone. Helper `Ok(None)`. Idempotent. |
| Verb succeeds | yes | yes | yes | non-empty, no title | exit 0, `Wrote{Some(bak)}` | Title written, backup created. Agent reports success. |
| Verb succeeds with NoOp (TOCTOU race — sibling agent set title between gate and verb) | yes | yes | yes | non-empty, no title at gate; has title at verb | exit 0, `NoOp` | No write, no backup. Verb prints `BRIEF.md unchanged (title value already matches)`. Agent reports the no-op. |
| Verb fails — auth (token rejected, agent not coordinator, etc.) | yes | yes | yes | non-empty, no title | exit 1 | BRIEF.md unchanged. Agent surfaces the error in transcript. Backend doesn't observe. Next spawn retries. |
| Verb fails — backup collision (100 retries exhausted same-second) | yes | yes | yes | non-empty, no title | exit 1, `BackupExhausted` | BRIEF.md unchanged. Verb prints `Error: 100 collision retries exhausted ...`. Manual cleanup needed if backups accumulate. |
| Verb fails — lock timeout (concurrent writer holds lock >5 s) | yes | yes | yes | non-empty, no title | exit 1, `LockTimeout` | BRIEF.md unchanged. Verb prints `Error: BRIEF.md is locked by another writer (5s timeout). Try again.` Next spawn retries. |
| Verb fails — external write detected (sentinel mismatch) | yes | yes | yes | non-empty, no title | exit 1, `ExternalWrite(bak)` | BRIEF.md may or may not be unchanged (depends on the external editor). Verb prints `... aborting. Backup at <path> retains the externally-modified state.` Backup IS still produced (defensively). Agent surfaces error. |
| Two AC processes spawn the same Coordinator simultaneously | yes | yes | yes | non-empty, no title | first verb exits 0, second exits 0 with NoOp (or LockTimeout, low-probability) | Title set once. Second verb either NoOps or times out on the lock. Idempotent overall. |
| Setting toggled OFF mid-spawn | yes | (toggle) | yes | any | n/a | F1 captures setting BEFORE `tokio::spawn`; in-flight session uses the captured value. New spawn picks up the toggle. |
| Agent ignores the prompt | yes | yes | yes | non-empty, no title | not invoked | BRIEF.md unchanged. Backend doesn't observe. Next spawn retries (gate (4) still passes). User can disable `auto_generate_brief_title` if it bothers them. |

Round 4's "Snapshot fails" row is removed (no snapshot in Round 5). Round 4's "GOLDEN RULE refusal returns" row is removed (Round 5 does not depend on the GOLDEN RULE template).

### §R5.10 Idempotence layering

Two layers, in order:

**Layer 1 — Backend gate (4) in `build_title_prompt_appendage`**: `parse_brief_title(&content).is_some() → Ok(None)`. Short-circuits BEFORE the prompt is built. Saves ~28 s of agent processing time + a PTY write + an unnecessary CLI verb invocation. This is the v3 §2.4 contract preserved verbatim.

**Layer 2 — CLI verb's `EditOutcome::NoOp`**: `brief_ops::perform_inner` checks `title_value_of(&new_parsed) == title_value_of(&parsed)` after parse + apply_edit; if equal, returns `NoOp` without writing or backing up. Saves the actual file modification + backup creation in the rare TOCTOU window where a sibling agent or manual edit added a title between gate (4) and the verb invocation.

Layer 1 catches the common case (~99.9%). Layer 2 catches concurrent-coordinator races and clock-skew TOCTOU. Together they make Round 5 strictly more idempotent than v3, which had only Layer 1.

The two layers are **not redundant**: removing Layer 1 would expose every Coordinator spawn to a wasted ~28 s prompt cycle. Removing Layer 2 would risk a duplicate write (with backup) on TOCTOU. Both stay.

### §R5.11 Logging changes vs Round 4

Round 4 §R4.12's table is **superseded** in two cells:

| Old (Round 4) | New (Round 5) |
|---|---|
| `[session] Auto-title appendage built for session <uuid> (bak=<path>)` | `[session] Auto-title appendage built for session <uuid>` (no `bak=` suffix — backend doesn't produce a backup) |
| (no equivalent in Round 4 — the verb didn't exist) | `[brief] set-title: sender=<agent_name> wg=<wg-root> pid=<pid> backup=<bak_path>` (emitted by the binary at the SAME log file via the CLI logger init at commit `f77aa34` — visible to operators querying app.log) |

All other log-line shapes from Round 4 §R4.12 (the `(auto-title=true|false)` boolean, the appendage-skipped lines, the bootstrap-failure warn) remain unchanged.

For ops dashboards counting "auto-title successes": switch from grepping `Auto-title prompt injected for session` (v3) or `Auto-title appendage built for session <uuid> (bak=` (Round 4) to grepping `[brief] set-title:`. The v3/R4 shapes give "we sent the prompt"; the Round 5 shape gives "the binary wrote the file" — strictly stronger. The two correlate by `<uuid>` ↔ `<wg-root>` (look up the session's CWD via `[session] Auto-title appendage built for session <uuid>` line and match the workgroup root in the brief log line).

### §R5.12 Rebase note (informational; dev-rust owns the resolution)

The branch `feature/107-auto-brief-title` is mid-rebase onto `main` HEAD `7115888` (post-#137 merge). State at the time of Round 5 authoring:

```
Last commands done (2 commands done):
   pick 807d863 # plans: add design doc for issue #107 auto-brief-title
   pick 3f7da00 # feat(#107): settings field, BRIEF.md template, and pure helpers + tests

Next commands to do (2 remaining commands):
   pick e458b85 # feat(#107): wire Coordinator auto-title chain into post-spawn task
   pick d4d4e07 # docs(plan): #107 Round 4 — document BRIEF.md write failure + proposed fixes

Unmerged paths (conflicts):
   src-tauri/src/commands/entity_creation.rs
   src-tauri/src/config/settings.rs
   src/shared/types.ts
```

The current on-disk plan (this file) already contains Round 4 — so dev-rust's pick of `d4d4e07` will either no-op or fast-forward, depending on whether the working tree has been committed in between.

Round 5 is **agnostic to the rebase mechanics**. Dev-rust may choose any of:
- (a) Resolve conflicts → continue rebase → apply Round 5's deltas as additional commits on top.
- (b) Abort rebase → cherry-pick a fresh sequence (settings/template helpers + Round 5 wiring) atop `main`.
- (c) Squash the in-flight `e458b85` (Round 4 §R4.5 implementation) into a Round 5-shaped single commit during the rebase via `edit`.

(b) is probably cleanest given how much of `e458b85`'s implementation Round 5 changes (the async helper goes, the spawn-task body goes, the snapshot call goes). Tech-lead's call.

Round 5 does **not** prescribe the rebase strategy. The plan is implementable as written from any post-rebase state where:
- `src-tauri/src/config/settings.rs` has `auto_generate_brief_title` (v3 §4.1).
- `src-tauri/src/commands/entity_creation.rs` has the BRIEF template change (v3 §5.1) + `parse_brief_title` (v3 §6.2).
- `src-tauri/src/session/session.rs` has `find_workgroup_brief_path_for_cwd` (v3 §7.1).
- `src/shared/types.ts` has `autoGenerateBriefTitle` (v3 §4.2).
- `src/sidebar/components/SettingsModal.tsx` has the checkbox (v3 §4.3).

`pty/title_prompt.rs` and `commands/session.rs`'s spawn-task body are dev-rust's main code-edit surface for Step 6.

### §R5.13 Net delta vs Round 4

| Surface | Round 4 | Round 5 | Difference |
|---|---|---|---|
| Code lines added | ~100 (helper + spawn-task body + Change B edits + tests) | ~50 (helper + spawn-task body + new prompt body + new tests) | ~−50 |
| Code lines removed | ~145 (`inject_title_prompt_after_idle_static` chain) | ~145 (same) **+** ~30 (`snapshot_brief_before_edit` + its tests) | ~−30 more deletion |
| Net code change | ~−45 | ~−125 | ~−80 |
| Files touched | 2 (`commands/session.rs`, `config/session_context.rs`) plus the inherited v3 surface | 2 (`commands/session.rs`, `pty/title_prompt.rs`) plus the inherited v3 surface | `session_context.rs` no longer touched |
| New tests | 3 (Change B unit tests + snapshot-collision) | 3 (helper idempotence × 2 + helper happy path) + 6 (new `build_title_prompt` invariant tests, replacing v3's 4) | similar count |
| Cargo deps | 0 | 0 | unchanged |
| TS / frontend | 0 | 0 | unchanged |
| Plan delta | This Round 5 section appended; nothing above edited (audit trail preserved). |

Round 5 is **strictly simpler** than Round 4 in code and surface area. The price was paid by #137 — it took the load-bearing complexity out of the auto-title chain entirely.

### §R5.14 Verdict

`READY_FOR_IMPLEMENTATION`.

Rationale:
- Every code surface in Round 5 has been verified against the actual source on `main` HEAD `7115888` (post-#137) AND against the in-flight `feature/107-auto-brief-title` rebase state.
- The CLI verb behaviour (`brief-set-title` exit codes, idempotence, backup format, error matrix) is fixed by #137 and well-tested (`cli/brief_set_title.rs::tests` + `cli/brief_ops.rs::tests` + integration test `cli_brief_logger.rs`). Round 5 only consumes the verb's contract; it does not modify it.
- The seven open questions from tech-lead's brief are all resolved with stated rationale (§R5.4.1).
- The single notable code surprise — the 3-arg vs 4-arg `inject_text_into_session` signature mismatch in v3/R4 — is documented in §R5.5 and corrected.
- The rebase is mechanical (§R5.12); resolution does not require new architectural decisions.
- All v3 review folds (F1, F3, F4, F7) survive Round 5; F2 (Round 4 superseded), F6, F9 (Round 5 superseded — no snapshot helper); audit trail in §13 stays whole.

No items needing another round with dev-rust + grinch. Dev-rust may proceed to Step 6 (implementation) directly with this plan.
