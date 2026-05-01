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
2. **Credentials block and title-prompt are SEPARATE PTY writes.** Two distinct
   `inject_text_into_session` calls inside the spawn-time spawned task. Never
   concatenate.
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
