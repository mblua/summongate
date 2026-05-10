# Plan: Allow writes to `<workgroup-root>/messaging/` in agent context (#199)

**Branch:** `feature/191-cli-project-open-create` (this fix is required to fully validate #191; will be folded into the same branch unless tech-lead asks otherwise)
**Issue:** #199 — Fix agent write permissions for workgroup messaging
**Date:** 2026-05-10

---

## 1. Requirement

The auto-generated agent session context (delivered as `CLAUDE.md` / `AGENTS.md` / `GEMINI.md` to every replica) currently contains a self-contradictory pair of rules:

- **GOLDEN RULE** (write restrictions): only `repo-*`, the agent's own replica root, and the optional Agent Matrix scope are writable. Everything else under the workgroup root is read-only.
- **Inter-Agent Messaging**: requires the agent to *write* a message file at `<workgroup-root>/messaging/<filename>.md` before invoking `<bin> send --send <filename>`.

The CLI (`cli/send.rs:148-203`) only accepts `--send <filename>` (the legacy `--message` / `--message-file` flags were removed by the file-based messaging refactor; see `_plans/messages-always-by-files.md`). So an agent that obeys the GOLDEN RULE literally **cannot reply to any inter-agent message at all**.

Evidence: `__agent_ac-cli-tester/missing-reply-diagnostic.md` — `ac-cli-tester` correctly refused to write to `messaging/` and returned `CONTACT_FAIL`.

**Fix:** add a *narrow*, explicit exception to the GOLDEN RULE that allows agents to create canonical inter-agent message files (and only those) inside `<workgroup-root>/messaging/`. Every other file or path under the workgroup root remains forbidden.

This is required before #191 can be fully validated end-to-end by `ac-cli-tester` (which needs to actually exchange messages with `tech-lead`).

---

## 2. Affected files

| # | File | Change |
|---|---|---|
| 1 | `src-tauri/src/config/session_context.rs` | Compute `messaging_dir_display` from the agent root, inject a "Narrow exception" subsection into the GOLDEN RULE, and add a new "Allowed (narrow)" bullet. Add 2 unit tests. |

No frontend changes. No new Rust modules. No new crates. No type-shape changes (this is plain text generation).

---

## 3. Detailed change — `src-tauri/src/config/session_context.rs`

### 3.1 Add a `use` (optional — fully-qualified paths used inline below; no change required if you keep them qualified)

Not strictly needed. The existing file does not import from `phone`. The plan uses `crate::phone::messaging::workgroup_root` and `crate::phone::messaging::MESSAGING_DIR_NAME` inline (same style as `git_ceiling_directories_for_session_root` callers in `pty/manager.rs:329` and `pty/git_watcher.rs:178`). Keep it qualified to minimize diff.

### 3.2 Compute `messaging_dir_display` and the two new template fragments

**Location:** `default_context` (currently lines 478-622). Insert the new bindings **immediately after** the `matrix_allowed` binding (after the closing `};` on **line 499**) and **before** the `forbidden_scope` binding that starts on **line 500**.

**Current code (lines 493-504), for unambiguous placement:**

```rust
    let matrix_allowed = match matrix_root {
        Some(matrix_root) => format!(
            "- **Allowed**: Full read/write inside your origin Agent Matrix's `memory/`, `plans/`, and `Role.md` ({matrix_root})\n",
            matrix_root = matrix_root,
        ),
        None => String::new(),
    };
    let forbidden_scope = if matrix_root.is_some() {
        "allowed zones — including other agents' replica directories, any other files inside the Agent Matrix, the workspace root, parent project dirs, user home files, or arbitrary paths on disk"
    } else {
        "two zones — including other agents' replica directories, the workspace root, parent project dirs, user home files, or arbitrary paths on disk"
    };
```

**Insert between line 499 (`};` of `matrix_allowed`) and line 500 (`let forbidden_scope = …`):**

```rust
    let messaging_dir_display = crate::phone::messaging::workgroup_root(
        std::path::Path::new(agent_root),
    )
    .ok()
    .map(|wg| {
        let dir = wg.join(crate::phone::messaging::MESSAGING_DIR_NAME);
        display_path(&dir)
    });
    let messaging_exception = match &messaging_dir_display {
        Some(path) => format!(
            "**Narrow exception — workgroup messaging directory:**\n\n\
             You MAY create message files inside this directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Strictly limited to canonical inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-<from_short>-to-<to_short>-<slug>.md` (the CLI rejects any other shape via `phone::messaging::validate_filename_shape`). Used by the two-step protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete files written by other agents. Do NOT write any other kind of file here.\n\n",
            path = path,
        ),
        None => String::new(),
    };
    let messaging_allowed = match &messaging_dir_display {
        Some(path) => format!(
            "- **Allowed (narrow)**: Create canonical inter-agent message files in your workgroup messaging directory ({path}). No other writes there.\n",
            path = path,
        ),
        None => String::new(),
    };
```

**Notes on this binding:**

- `workgroup_root` is a pure path operation (no fs touch); see `phone/messaging.rs:50-65`. It walks ancestors of `agent_root` and returns the first one whose basename matches `^wg-\d+-.*$`.
- For replica agents under `wg-N-*`: returns `Ok(<wg-root>)`, so `messaging_dir_display = Some(...)` and the GOLDEN RULE gains the new subsection + bullet.
- For Agent Matrix sessions (e.g. `_agent_architect` directly under `.ac-new/`): no `wg-N-*` ancestor → `Err(NoWorkgroup)` → `messaging_dir_display = None` → both fragments are empty strings, GOLDEN RULE is unchanged. This is correct: matrix-level agents are not part of any workgroup messaging fabric.
- `agent_root` reaching `default_context` was already canonicalized by `ensure_session_context` (line 14-17), so no additional canonicalization is required and `display_path` only strips a leftover `\\?\` UNC prefix if any.

### 3.3 Inject `{messaging_exception}` into the format!() template

**Location:** the `format!(r#"..."#)` macro currently spanning lines 510-621.

**Current template fragment (lines 524-528) for unambiguous placement:**

```text
{replica_usage}

{matrix_section}

Any repository or directory outside the allowed places above is READ-ONLY.
```

**Replace with:**

```text
{replica_usage}

{matrix_section}{messaging_exception}
Any repository or directory outside the allowed places above is READ-ONLY.
```

**Why this exact spacing:** `matrix_section` already terminates with `\n\n` (see line 488), and `messaging_exception` (when non-empty) also terminates with `\n\n`. Removing the literal blank line between `{matrix_section}` and `Any repository` and concatenating both fragments inline preserves a single blank line above "Any repository…" in **all four** combinations (matrix yes/no × messaging yes/no). When both are empty (non-WG, non-matrix — currently impossible in production but covered by the existing test), you get a single newline directly above "Any repository…", which still parses cleanly as Markdown.

### 3.4 Inject `{messaging_allowed}` into the format!() template

**Current template fragment (lines 530-533):**

```text
- **Allowed**: Read-only operations on ANY path (reading files, searching, git log, git status, git diff)
- **Allowed**: Full read/write inside `repo-*` folders
- **Allowed**: Full read/write inside your own replica root ({agent_root}) and its subdirectories
{matrix_allowed}- **FORBIDDEN**: Any write operation outside those {forbidden_scope}
```

**Replace with:**

```text
- **Allowed**: Read-only operations on ANY path (reading files, searching, git log, git status, git diff)
- **Allowed**: Full read/write inside `repo-*` folders
- **Allowed**: Full read/write inside your own replica root ({agent_root}) and its subdirectories
{matrix_allowed}{messaging_allowed}- **FORBIDDEN**: Any write operation outside those {forbidden_scope}
```

`messaging_allowed` ends with `\n` (just like `matrix_allowed`), so the bullet appears as another list item flush against the rest. When empty, the FORBIDDEN bullet sits directly under `matrix_allowed` (or under the replica-root bullet if matrix is also absent), exactly as today.

### 3.5 Pass the two new named args to `format!()`

**Current trailing arg list (lines 614-620):**

```rust
        agent_root = agent_root,
        allowed_places = allowed_places,
        replica_usage = replica_usage,
        matrix_section = matrix_section,
        matrix_allowed = matrix_allowed,
        forbidden_scope = forbidden_scope,
        git_scope = git_scope,
    )
}
```

**Replace with:**

```rust
        agent_root = agent_root,
        allowed_places = allowed_places,
        replica_usage = replica_usage,
        matrix_section = matrix_section,
        matrix_allowed = matrix_allowed,
        messaging_exception = messaging_exception,
        messaging_allowed = messaging_allowed,
        forbidden_scope = forbidden_scope,
        git_scope = git_scope,
    )
}
```

### 3.6 Decisions explicitly NOT made (keep blast radius minimal)

- **No renumbering** of the existing 1/2/3 list. Messaging is a *narrow* exception, not a full write zone, so it sits as a separate "Narrow exception" subsection between the numbered list and the "Any repository… is READ-ONLY" line. This avoids touching `allowed_places` ("two places"/"three places" wording on lines 479-483) and keeps the matrix scope numbered as `3.` exactly as today.
- **No edit to `forbidden_scope`** (lines 500-504). The strings still read "the workspace root, parent project dirs, …". The workspace root *itself* remains forbidden (you cannot write `wg-1-dev-team/foo.md`); only the specific `messaging/` subdirectory is excepted, and that exception is now explicitly listed in the "Allowed" bullets. No ambiguity.
- **No edit to the existing `## Inter-Agent Messaging` section** (lines 577-606). It already documents the protocol correctly — it just has no permission to perform it until this plan ships.

---

## 4. Tests

### 4.1 Existing test — keep unchanged

**File:** `src-tauri/src/config/session_context.rs` lines 636-642
**Test:** `default_context_embeds_filename_only_warning`

Currently passes `"C:/tmp/fake-agent"` (no `wg-N-*` ancestor) and asserts substrings `"filename ONLY"`, `"BAD:"`, `"GOOD:"`. After the change, `messaging_dir_display = None` for this input → both new fragments are empty strings → output still contains all three substrings unchanged. **No edit to this test.**

### 4.2 New test — replica path injects messaging exception

Add inside the existing `mod tests { … }` block (after line 642, before the closing `}` on line 643):

```rust
    #[test]
    fn default_context_replica_under_wg_includes_messaging_exception() {
        let out = default_context("C:/fake/wg-7-dev-team/__agent_architect", None);
        assert!(
            out.contains("Narrow exception — workgroup messaging directory"),
            "expected messaging exception header, got:\n{}",
            out
        );
        assert!(
            out.contains("wg-7-dev-team"),
            "expected workgroup name in messaging path, got:\n{}",
            out
        );
        assert!(
            out.contains("- **Allowed (narrow)**: Create canonical inter-agent message files"),
            "expected narrow-allowed bullet, got:\n{}",
            out
        );
    }
```

### 4.3 New test — non-workgroup path omits messaging exception

```rust
    #[test]
    fn default_context_non_workgroup_omits_messaging_exception() {
        let out = default_context("C:/fake/plain/agent", None);
        assert!(
            !out.contains("Narrow exception — workgroup messaging directory"),
            "expected no messaging exception header for non-WG agent, got:\n{}",
            out
        );
        assert!(
            !out.contains("- **Allowed (narrow)**:"),
            "expected no narrow-allowed bullet for non-WG agent, got:\n{}",
            out
        );
    }
```

### 4.4 Why these path strings work cross-platform

The test paths use `/` separators. `Path::file_name` on both Windows and Unix returns the last segment regardless of separator style (Windows treats both `/` and `\` as separators). The existing test (`"C:/tmp/fake-agent"`) already relies on this. `phone::messaging::workgroup_root` walks `Path::ancestors()` and matches via `is_wg_dir(name)` on the file_name string — separator-agnostic. Confirmed by existing tests `workgroup_root_ok` (`/tmp/wg-7-dev-team/...`) and `workgroup_root_ok_windows_style` (`C:\foo\wg-42-team-x\...`) in `phone/messaging.rs:401-418`.

---

## 5. Dependencies

None. No new crates. No new modules. No new Tauri commands or events. `phone::messaging::workgroup_root` and `MESSAGING_DIR_NAME` are already pub and stable (used in `cli/send.rs`, `cli/brief_set_title.rs`, `cli/brief_append_body.rs`).

---

## 6. Notes / constraints / things the dev must NOT do

1. **Do NOT** widen the exception. Only canonical inter-agent message files (matching the `validate_filename_shape` pattern) belong in `messaging/`. Do not phrase this as "any file relevant to inter-agent communication" or similar.
2. **Do NOT** rephrase the existing GOLDEN RULE numbered list, the "two/three places" preamble, or the `forbidden_scope` strings. The whole point is a surgical insertion.
3. **Do NOT** add a new module, a new helper file, or a new Tauri command. The fix is purely in text generation inside one private function.
4. **Do NOT** auto-create the `messaging/` directory from `default_context`. The directory is created on demand by `phone::messaging::messaging_dir` when an agent first calls `send --send`. Pure text generation must stay pure (no fs side-effects beyond what `ensure_session_context` already does).
5. **Do NOT** drop or restructure the existing `## Inter-Agent Messaging` section. It is the authoritative protocol reference.
6. **Build the WG-specific binary** (per repo convention `_wg-1.exe`) and bump `tauri.conf.json` version so the user can visually confirm the new build is loaded. Replicas pick up the new context the next time `materialize_agent_context_file` runs at session launch — coordinate with tech-lead before re-launching `ac-cli-tester`.
7. **Verification path after build:** restart `ac-cli-tester`, have tech-lead resend the `CONTACT_OK / CONTACT_FAIL` request, confirm the reply lands in `messaging/` and the wake fires. That closes #199 and unblocks final validation of #191.

---

## 7. Out-of-scope follow-ups (do NOT bundle into this PR)

- A `coordinator`/`coord` Agent Matrix is currently emitted with `messaging_dir_display = None` (no WG ancestor). If we ever want coordinators to participate in workgroup messaging directly from `_agent_*`, that needs a separate design — they would need to know which WG to address, and `workgroup_root` cannot infer that. **Not in scope here.**
- `_plans/messages-always-by-files.md` mentions a possible future `list-inbox` / `read-message` helper. Out of scope.
- Regenerating already-launched session context files on the fly. Out of scope — the next session launch refreshes them via `ensure_session_context` (line 22-26).

---

READY_FOR_PLAN_REVIEW

---

## Dev review (dev-rust, 2026-05-10)

**Verdict:** Plan is technically sound and ready to implement. All file paths, line numbers, function references, and call patterns match the current codebase on `feature/191-cli-project-open-create`. Below are minor enrichments; none block implementation.

### Verified against current code

- `src-tauri/src/config/session_context.rs`
  - `default_context` spans lines **478–622** as claimed.
  - `matrix_allowed` block ends at line **499** (`};`); `forbidden_scope` starts at line **500**. Insertion point is unambiguous.
  - `format!()` template runs lines **510–621**.
  - Quoted current fragments at lines **524–528** and **530–533** match byte-for-byte.
  - Trailing named-arg list at lines **614–620** matches.
  - Existing test `default_context_embeds_filename_only_warning` at lines **636–642**, asserting only `"filename ONLY"`, `"BAD:"`, `"GOOD:"` — invariant under the change because `"C:/tmp/fake-agent"` produces no `wg-N-*` ancestor → both new fragments are empty strings → no spurious matches.
- `src-tauri/src/phone/messaging.rs`
  - `MESSAGING_DIR_NAME` is `pub const` at line **11**; `workgroup_root` is `pub fn` at line **54**; both are pure path operations as the plan states.
  - `is_wg_dir` (line 290) confirms `wg-7-dev-team` matches via the `wg-<digits>-...` shape.
  - Unguarded test `workgroup_root_ok` (lines 401–408) already proves `/tmp/wg-7-dev-team/__agent_architect` resolves correctly cross-platform — the new tests using the same shape will work on Windows and Unix.
- Module visibility: `lib.rs:6` has `pub mod phone;` and `phone/mod.rs:3` has `pub mod messaging;`. `crate::phone::messaging::...` is reachable from `config::session_context`.
- Existing call sites use the exact same pattern the plan proposes:
  - `cli/send.rs:151` — `crate::phone::messaging::workgroup_root(agent_root_path)`
  - `cli/brief_set_title.rs:104` — `crate::phone::messaging::workgroup_root(Path::new(&root))`
  - `cli/brief_append_body.rs:104` — same.
  - The "fully-qualified inline" style in §3.1 of the plan is consistent with the codebase. No new `use` import needed.

### Compile risk: low

- Three new bindings (`messaging_dir_display`, `messaging_exception`, `messaging_allowed`) are introduced **before** `forbidden_scope`, so they're in scope at the `format!()` call.
- Template additions `{messaging_exception}` and `{messaging_allowed}` correspond 1:1 to the new named args added at the bottom.
- The new `format!()` for `messaging_exception` is a regular (non-raw) string with `\n` and backslash line-continuations. Triple backticks inside are inert characters — `format!()` does not interpret them. `<from_short>` and `<to_short>` are also inert (only `{name}` is a placeholder). No Rust `{{`/`}}` escaping needed.

### Test risk: low

- New tests use `/`-separator paths. `Path::new("C:/fake/wg-7-dev-team/__agent_architect").ancestors()` correctly produces a `wg-7-dev-team` segment on both Windows and Unix because `Path` treats `/` as a separator universally for parsing on Windows, and natively on Unix. This is already proven by `workgroup_root_ok` in `messaging.rs`.
- The substring assertions (`"Narrow exception — workgroup messaging directory"`, `"wg-7-dev-team"`, `"- **Allowed (narrow)**: Create canonical inter-agent message files"`) survive whatever separator `PathBuf::join` chooses for the joined `messaging` segment, because they only check the workgroup-name token and the literal English text.
- No need to gate the new tests with `#[cfg(windows)]`.

### Enrichments (recommended, non-blocking)

1. **Align terminology with the existing `## Inter-Agent Messaging` section.** The proposed `messaging_exception` text uses the pattern `<from_short>-to-<to_short>-<slug>`, but the pre-existing protocol section (lines 587–589 of the generated context) uses `<wgN>-<you>-to-<wgN>-<peer>-<slug>`. Same canonical filename, two vocabularies — an agent reading top-to-bottom may briefly wonder if these are different patterns. Suggest changing the inserted text to:
   > Strictly limited to canonical inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` (the CLI rejects any other shape). Used by the two-step protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete any message file once written. Do NOT write any other kind of file here.

   Two changes folded in: (a) use `<wgN>-<you>` to match the existing section, (b) drop the internal-naming leak `phone::messaging::validate_filename_shape` from agent-facing context (the agent does not need the function name; the rejection behaviour is what matters), (c) broaden "Do NOT modify or delete files written by other agents" to "Do NOT modify or delete any message file once written" — once `send` fires, the recipient's notification points at the absolute path, and a sender editing their own outgoing file would silently change what the recipient is told to read.

2. **§3.3 spacing claim is slightly imprecise but harmless.** The plan says removing the literal blank line "preserves a single blank line above 'Any repository…' in all four combinations". Actual blank-line count after the change:
   - matrix=Some, messaging=Some: 2 blank lines
   - matrix=Some, messaging=None: 2 blank lines
   - matrix=None, messaging=Some: 2 blank lines
   - matrix=None, messaging=None: 1 blank line

   All are valid Markdown and visually fine. Today's behaviour is 3 blank lines for the rare matrix=None case, so the change actually *tightens* the spacing slightly. No edit to the implementation needed; just noting the prose.

3. **No `use` change needed.** Confirmed — the existing file does not import `phone`, and the proposed inline-qualified `crate::phone::messaging::...` matches the rest of the codebase. Keep it inline.

4. **§3.6's rationale for keeping `forbidden_scope` strings unchanged is correct.** The `messaging/` exception is now explicitly listed in the Allowed bullets, and the workspace-root prohibition still holds for everything else under it (e.g. an agent still cannot write `wg-1-dev-team/foo.md`, only `wg-1-dev-team/messaging/<canonical>.md`). No ambiguity.

### Implementation order I'll follow

1. Add the three bindings (`messaging_dir_display`, `messaging_exception`, `messaging_allowed`) between current lines 499 and 500, applying enrichment #1 above to the `messaging_exception` literal.
2. Update the template at the equivalent of current lines 526 and 533.
3. Append the two new named args to the `format!()` arg list after current line 618.
4. Add the two new tests inside the existing `mod tests {}` block (between lines 642 and 643).
5. `cargo check` → `cargo clippy` (must be clean) → `cargo test -p <crate> session_context` → confirm both new tests pass and the existing one still passes.
6. Bump `tauri.conf.json` version (per repo convention — visual confirmation of new build).
7. Build the WG-specific binary `agentscommander_standalone_wg-1.exe` (shipper-only-to-WG convention; never touch the bare standalone).
8. Commit to `feature/191-cli-project-open-create`. No merge to `main`.
9. Coordinate with tech-lead before re-launching `ac-cli-tester` so it picks up the refreshed context via `materialize_agent_context_file`.

### Out-of-scope items confirmed

§7 of the plan correctly excludes coordinator/matrix participation in messaging, `list-inbox`/`read-message` helpers, and on-the-fly regeneration of materialized session-context files. Agreed — none belong in this PR.

### Note on this review's delivery channel

I am leaving this review in the repo file (allowed: `repo-*` is in the GOLDEN RULE Allowed list) and **not** sending a file-based notification reply. Until #199 ships, my own session context still forbids writes to `<workgroup-root>/messaging/` — the very contradiction this plan fixes. The tech-lead anticipated this in the request ("do not rely only on chat reply").

READY_FOR_IMPLEMENTATION

---

## Grinch Review

**Verdict:** CHANGES REQUESTED. Full report: `_plans/199-grinch-plan-review.md`.

The dev-rust review covers structural soundness — function signatures, line numbers, call patterns. I focused on whether the *resulting text* still trips the strict-reading agent that filed #199. It does, in three places.

1. **Internal contradiction in the GOLDEN RULE text after the change** (CHANGES REQUESTED).
   - **What:** The numbered preamble still says "ONLY two places" (resp. three with matrix). The FORBIDDEN bullet still says "outside those two zones" / "outside those allowed zones". Inserting a `Narrow exception` subsection plus an `Allowed (narrow)` bullet creates a third allowed category that neither exclusivity claim acknowledges.
   - **Why:** This reproduces the failure mode that filed #199. `__agent_ac-cli-tester/missing-reply-diagnostic.md:52` shows the strict reader literally cited "no debo escribir alli sin una autorizacion superior explicita" before returning `CONTACT_FAIL`. After this plan ships, two of the four resulting sentences still tell that same reader "ONLY 2/3 places" — same category of contradiction, just narrower.
   - **Fix:** Lift §3.6's "no edit to `forbidden_scope`" rule. Either rewrite `forbidden_scope` to reference "the allowed entries above (including the messaging exception)" instead of "those two/three zones"; soften `allowed_places` ("the entries listed below") so the preamble does not claim exclusivity over only the numbered list; or number the exception as `2a.` / inside `2.` so it joins the numbered list and `allowed_places` arithmetic stays honest.

2. **Bootstrap of `<wg-root>/messaging/` is unaddressed** (NON-BLOCKING, but land same-PR if cheap).
   - **What:** Step 1 of the protocol writes a file under `<wg-root>/messaging/`; that dir is created by `messaging_dir()` only inside `cli/send.rs:161`, which runs *after* step 1. `commands/entity_creation.rs::create_workgroup` (lines 559–788) does not pre-create it — verified by grep. In a brand-new WG, the agent's first `fs::write` fails because the parent dir does not exist.
   - **Why:** The narrow exception permits writing *files* into `messaging/`, not creating the dir itself. The "you may now reply" promise of #199 is leaky for the first-message case in a fresh WG. (The existing `wg-1-dev-team/messaging/` only exists because of out-of-band bootstrapping that I cannot find in code; an agent that strict-reads the rule will not reproduce that bootstrap.)
   - **Fix:** Add `std::fs::create_dir_all(wg_dir.join("messaging")).map_err(...)?;` to `create_workgroup` after line 603, and document it under §2 as a second affected file. Or punt to a follow-up issue and explicitly note the limitation in §7.

3. **Placeholder shape mismatch between the new exception text and the existing `## Inter-Agent Messaging` section** (CHANGES REQUESTED).
   - **What:** Plan §3.2's `messaging_exception` describes the canonical filename as `YYYYMMDD-HHMMSS-<from_short>-to-<to_short>-<slug>.md`. The existing protocol section in the same template (`session_context.rs:587–589`) uses `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md`. Same regex, different placeholder vocabulary inside one generated document.
   - **Why:** Documentation drift inside one file is the meta-bug behind #199. Fixing one inconsistency by introducing a second is exactly the trap to avoid.
   - **Fix:** Use `<wgN>-<you>-to-<wgN>-<peer>-<slug>` in the `messaging_exception` literal so both sections describe the shape identically.

4. **Test coverage gap — no `(matrix=Some, messaging=Some)` test** (CHANGES REQUESTED).
   - **What:** §4.2 / §4.3 cover (None, Some) and (None, None). The existing `default_context_embeds_filename_only_warning` covers (None, None). The (Some, Some) case — every production replica with `identity` set — is not exercised. The §3.3 template change concatenates `{matrix_section}{messaging_exception}` on one line and relies on both fragments terminating with `\n\n`. A regression that drops a trailing newline from either fragment would only break this combination, and the proposed test set would still pass.
   - **Why:** The only case that breaks is the only case production cares about.
   - **Fix:** Add a third test passing `Some("C:/fake/_agent_architect")` as `matrix_root`, asserting both the matrix section header (`"3. **Your origin Agent Matrix"`) and the messaging exception header are present, plus a composition check on the inter-section spacing. Snippet in `_plans/199-grinch-plan-review.md` §4.

5. **Cosmetic: forward-slash test paths produce mixed separators on Windows** (NON-BLOCKING).
   - **What:** `Path::join("messaging")` yields `C:/fake/wg-7-dev-team\messaging` on Windows. Current assertions (`contains("wg-7-dev-team")`) tolerate this; mentioning so a future maintainer adding a stricter path assertion is not surprised.
   - **Why:** Cosmetic.
   - **Fix:** Optional — use `r"C:\fake\..."` raw strings to match production conventions and improve test-failure legibility.

### Items I confirmed are NOT issues

- `format!` placeholder safety with the inner triple-backtick string. The inner `format!` is evaluated first; the outer raw `format!(r#"..."#)` interpolates the result verbatim with no stray `{` / `}` collisions.
- Cross-platform behavior of the new test paths. `phone::messaging::workgroup_root` is a pure ancestor walk using `file_name()`, separator-agnostic on both Unix and Windows.
- Regression of `default_context_embeds_filename_only_warning`. With both fragments empty, the asserted substrings remain present and the collapsed-newline change does not affect Markdown rendering.
- UNC handling. `agent_root` reaches `default_context` already trimmed of `\\?\` by `display_path` in `ensure_session_context`. `wg.join("messaging")` produces a fresh `PathBuf`; `display_path` is a defensive no-op on it.
- Concurrency, locks, async surface, fs leak risk: nil. Pure text generation.

REVIEW_REQUESTED_CHANGES

---

## Architect resolution (2026-05-10)

**Resolver:** architect
**Reviewing:** dev-rust (READY_FOR_IMPLEMENTATION) + dev-rust-grinch (REVIEW_REQUESTED_CHANGES, full report at `_plans/199-grinch-plan-review.md`).

This resolution is appended rather than edited in place so the prior review history (line numbers, function references) stays intact and reviewable. The original plan (§§1-7) plus this resolution together are the single source of truth for the implementing dev. Where they disagree, **this resolution wins**.

### Verdict per finding

| # | Grinch finding | Disposition |
|---|---|---|
| 1 | GOLDEN RULE retains "ONLY two/three places" + "outside those two zones" exclusivity claims | **ACCEPTED** — see §R-1 below |
| 2 | `<wg-root>/messaging/` not bootstrapped at WG creation | **ACCEPTED, bundled** — see §R-2 below |
| 3 | `<from_short>` vs `<wgN>-<you>` placeholder vocabulary mismatch | **ACCEPTED** — see §R-3 below (also folds dev-rust enrichment #1 b/c) |
| 4 | No `(matrix=Some, messaging=Some)` test | **ACCEPTED** — see §R-4 below |
| 5 | Forward-slash test paths produce mixed separators on Windows (cosmetic) | **REJECTED** — see §R-5 below |

The dev-rust review's enrichment #1 (terminology alignment + drop the internal-naming leak `phone::messaging::validate_filename_shape`) is folded into §R-3 since it overlaps semantically with finding 3.

§3.6's bullet "**No edit to `forbidden_scope`**" is **lifted** — finding 1's resolution explicitly modifies it. The other two bullets of §3.6 (no renumbering of the 1/2/3 list, no edit to the `## Inter-Agent Messaging` section) remain in force.

---

### §R-1 — Resolve finding 1 (residual GOLDEN RULE contradiction)

The strict reader (the actual `ac-cli-tester` whose diagnostic filed #199) cited the exclusivity claim as the deciding factor. Three text sites still claim exclusivity over a fixed count after the original plan ships:

1. Preamble at line 517 → `"You may ONLY modify files in {allowed_places}:"` where `allowed_places ∈ {"two places", "three places"}`.
2. Summary line at line 528 → `"Any repository or directory outside the allowed places above is READ-ONLY."` ("places" reads as a count noun referencing the numbered list).
3. FORBIDDEN bullet at line 533 → `"Any write operation outside those {forbidden_scope}"` where `forbidden_scope` starts with `"two zones —"` or `"allowed zones —"`.

Resolution: rewrite all three so they refer to "the entries listed below/above" with no count, and have the FORBIDDEN bullet explicitly acknowledge the workspace-root narrow exception.

#### §R-1.1 Rewrite `allowed_places` (replaces lines 479-483)

**Old:**
```rust
    let allowed_places = if matrix_root.is_some() {
        "three places"
    } else {
        "two places"
    };
```

**New:**
```rust
    let allowed_places = "the entries listed below";
```

Rationale: collapses both arms to a single literal that does not claim a count. The named arg `allowed_places = allowed_places` in the outer `format!()` (still present) renders the preamble as: `"You may ONLY modify files in the entries listed below:"`. No template change needed for the preamble.

#### §R-1.2 Rewrite `forbidden_scope` (replaces lines 500-504)

**Old:**
```rust
    let forbidden_scope = if matrix_root.is_some() {
        "allowed zones — including other agents' replica directories, any other files inside the Agent Matrix, the workspace root, parent project dirs, user home files, or arbitrary paths on disk"
    } else {
        "two zones — including other agents' replica directories, the workspace root, parent project dirs, user home files, or arbitrary paths on disk"
    };
```

**New:**
```rust
    let workspace_root_phrase = if messaging_dir_display.is_some() {
        "the workspace root (other than the narrow messaging exception above)"
    } else {
        "the workspace root"
    };
    let forbidden_scope = if matrix_root.is_some() {
        format!(
            "the entries listed above — including other agents' replica directories, any other files inside the Agent Matrix, {ws}, parent project dirs, user home files, or arbitrary paths on disk",
            ws = workspace_root_phrase,
        )
    } else {
        format!(
            "the entries listed above — including other agents' replica directories, {ws}, parent project dirs, user home files, or arbitrary paths on disk",
            ws = workspace_root_phrase,
        )
    };
```

Type change: `&'static str → String`. The named arg `forbidden_scope = forbidden_scope` in the outer `format!()` substitutes via `Display`, which both `&str` and `String` implement. No further change needed at the substitution site.

**Placement requirement:** the new bindings must come AFTER the §3.2 `messaging_dir_display` binding so it is in scope. Concretely the final ordering inside `default_context` is:

```
let allowed_places = …;            (R-1.1, replaces 479-483)
let replica_usage = …;             (unchanged, current 484-485)
let matrix_section = …;            (unchanged, current 486-492)
let matrix_allowed = …;            (unchanged, current 493-499)
let messaging_dir_display = …;     (§3.2 — new)
let messaging_exception = …;       (§3.2 — new, with R-3 wording)
let messaging_allowed = …;         (§3.2 — new)
let workspace_root_phrase = …;     (R-1.2 — new)
let forbidden_scope = …;           (R-1.2 — replaces 500-504)
let git_scope = …;                 (unchanged, current 505-509)
```

#### §R-1.3 Rewrite the FORBIDDEN bullet template

This **supersedes** the §3.4 instruction by adding one extra word edit on top of the original `{messaging_allowed}` insertion.

**Current template (line 533):**
```text
{matrix_allowed}- **FORBIDDEN**: Any write operation outside those {forbidden_scope}
```

**Replace with:**
```text
{matrix_allowed}{messaging_allowed}- **FORBIDDEN**: Any write operation outside {forbidden_scope}
```

Two edits in this single template line:
1. Insert `{messaging_allowed}` per §3.4 of the original plan.
2. Drop the literal word `those ` (with trailing space) so the new `forbidden_scope` (which starts with `"the entries listed above —…"`) reads naturally without a dangling demonstrative.

#### §R-1.4 Rewrite the summary-line template

**Current template (line 528):**
```text
Any repository or directory outside the allowed places above is READ-ONLY.
```

**Replace with:**
```text
Any repository or directory outside the allowed entries above is READ-ONLY.
```

One-word edit (`places` → `entries`) to match the preamble's "the entries listed below". A soft phrase that doesn't claim a count, so the messaging exception subsection (which sits "above" this line in the rendered output) is unambiguously included.

#### §R-1.5 Rendered output sanity check (matrix=Some, messaging=Some — production)

After all R-1 edits, the GOLDEN RULE block reads (only relevant fragments shown, with section breaks):

```
**ABSOLUTE AND NON-NEGOTIABLE:** You may ONLY modify files in the entries listed below:

1. **Repositories whose root folder name starts with `repo-`** ...
2. **Your own agent replica directory and its subdirectories** ...

3. **Your origin Agent Matrix, but only for the canonical agent state listed below:**
   ...
   - `Role.md`

**Narrow exception — workgroup messaging directory:**

You MAY create message files inside this directory:
...

Any repository or directory outside the allowed entries above is READ-ONLY.

- **Allowed**: Read-only operations on ANY path ...
- **Allowed**: Full read/write inside `repo-*` folders
- **Allowed**: Full read/write inside your own replica root ...
- **Allowed**: Full read/write inside your origin Agent Matrix's `memory/`, `plans/`, and `Role.md` ...
- **Allowed (narrow)**: Create canonical inter-agent message files in your workgroup messaging directory ...
- **FORBIDDEN**: Any write operation outside the entries listed above — including other agents' replica directories, any other files inside the Agent Matrix, the workspace root (other than the narrow messaging exception above), parent project dirs, user home files, or arbitrary paths on disk
```

Zero exclusivity-by-count phrases remain. The messaging exception is referenced by name in three independent sites (subsection heading, "Allowed (narrow)" bullet, FORBIDDEN bullet's workspace-root qualifier). A pedantic reader cannot pick a "most restrictive" interpretation that contradicts the exception.

---

### §R-2 — Resolve finding 2 (bootstrap `messaging/` at WG creation)

Bundled into this PR. The fix is one line in `commands/entity_creation.rs::create_workgroup`.

#### §R-2.1 Update §2 (Affected files)

| # | File | Change |
|---|---|---|
| 1 | `src-tauri/src/config/session_context.rs` | (per original §2 + §R-1 deltas) |
| 2 | `src-tauri/src/commands/entity_creation.rs` | Pre-create `<wg_dir>/messaging/` immediately after the workgroup root is created. One inserted statement, no new `use`. |

#### §R-2.2 New plan subsection — §3.7 `entity_creation.rs` change

**Location:** `create_workgroup` at line 559. Insert AFTER line 603 (the `?;` of `std::fs::create_dir_all(&wg_dir)`) and BEFORE the blank line at line 604.

**Current code (lines 602-606), for unambiguous placement:**

```rust
    std::fs::create_dir_all(&wg_dir)
        .map_err(|e| format!("Failed to create workgroup directory: {}", e))?;

    // BRIEF.md: use the user-provided brief when present, otherwise seed a template.
    let brief_content = build_brief_content(&wg_name, brief);
```

**Insert between line 603 and line 604:**

```rust
    std::fs::create_dir_all(wg_dir.join(crate::phone::messaging::MESSAGING_DIR_NAME))
        .map_err(|e| format!("Failed to create messaging directory: {}", e))?;
```

**Notes:**

- Fully-qualified `crate::phone::messaging::MESSAGING_DIR_NAME` matches the call style in `cli/send.rs:151`, `cli/brief_set_title.rs:104`, and `cli/brief_append_body.rs:104`. No new `use` line needed; `entity_creation.rs:3` already imports `Path`/`PathBuf`. Do NOT add `use crate::phone::messaging;` — keep the inline qualification consistent with the rest of the codebase.
- Idempotent. `create_dir_all` is a no-op when the directory already exists. Safe to call even though `cli/send.rs::messaging_dir` calls `create_dir_all` again at first-send time. The duplicate is intentional — bootstrap-at-creation, lazy-create-on-first-send. Either alone is sufficient; together they guarantee the dir exists for both fresh and pre-existing WGs.
- Failure mode. If `create_dir_all` fails (disk full, permission denied), WG creation aborts with a clear error. Same severity and pattern as the existing `&wg_dir` creation on line 602.
- No new test. The existing `wg_delete_diagnostic` test at lines 1452-1481 already exercises a `messaging` subdir under `wg_dir`, so cross-module integration is covered. Adding a `wg_create_creates_messaging_dir` unit test would require pulling `create_workgroup`'s many state dependencies (`AppHandle`, settings, sweep_lock, project_path, team config) into a test harness — disproportionate for a one-liner. Manual verification path: create a fresh WG via UI and confirm `<wg-root>/messaging/` exists immediately.

#### §R-2.3 Update §6 (Notes) — add bootstrap rationale

Append as new note 8 after the existing §6.7:

> **8. Bootstrap of `<wg-root>/messaging/` at WG creation time** (per §R-2.2). The narrow exception in the GOLDEN RULE permits agents to create *files* inside `messaging/`, not the directory itself. For brand-new WGs the protocol's step 1 (write file) would otherwise fail because the parent directory does not exist. The one-line `create_dir_all` in `create_workgroup` closes this hole. **Do NOT** also widen the agent-side exception to permit `mkdir` — bootstrap-by-side-effect-of-a-text-rule is exactly the surface area we should not grow.

#### §R-2.4 Out-of-scope follow-up retained

§7's bullet about coordinator/Agent Matrix participation in workgroup messaging stays out of scope. R-2 is a complement to that, not a substitute.

---

### §R-3 — Resolve finding 3 (placeholder vocabulary alignment)

Replaces the original §3.2 `messaging_exception` literal text. Folds in dev-rust enrichment #1 (b) and (c) at the same time.

**Old text inside `messaging_exception` (plan §3.2):**

> Strictly limited to canonical inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-<from_short>-to-<to_short>-<slug>.md` (the CLI rejects any other shape via `phone::messaging::validate_filename_shape`). Used by the two-step protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete files written by other agents. Do NOT write any other kind of file here.

**New text:**

> Strictly limited to canonical inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` (the CLI rejects any other shape). Used by the two-step protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete any message file once written. Do NOT write any other kind of file here.

Three changes folded in:
1. (finding 3) `<from_short>-to-<to_short>` → `<wgN>-<you>-to-<wgN>-<peer>` to match `session_context.rs:587-589` verbatim.
2. (dev-rust enrichment #1b) Drop the internal-naming leak `phone::messaging::validate_filename_shape`. Agent-facing context should describe behavior, not internal function names.
3. (dev-rust enrichment #1c) Broaden "Do NOT modify or delete files written by other agents" → "Do NOT modify or delete any message file once written". Once `send` fires, the recipient's notification points at the absolute path; a sender editing their own outgoing file would silently change what the recipient is told to read.

**Updated `messaging_exception` literal (full replacement of the §3.2 binding):**

```rust
    let messaging_exception = match &messaging_dir_display {
        Some(path) => format!(
            "**Narrow exception — workgroup messaging directory:**\n\n\
             You MAY create message files inside this directory:\n\n\
             ```\n\
             {path}\n\
             ```\n\n\
             Strictly limited to canonical inter-agent message files whose name matches the pattern `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md` (the CLI rejects any other shape). Used by the two-step protocol described in the **Inter-Agent Messaging** section below: write the file, then call `send --send <filename>`. Do NOT modify or delete any message file once written. Do NOT write any other kind of file here.\n\n",
            path = path,
        ),
        None => String::new(),
    };
```

`messaging_dir_display` and `messaging_allowed` bindings remain identical to plan §3.2.

---

### §R-4 — Resolve finding 4 (add (matrix=Some, messaging=Some) test)

Augment §4 with a new test §4.5 that exercises the production combination.

#### §R-4.1 New test — replica path with both matrix and messaging fragments

Add inside the existing `mod tests { … }` block (after the §4.3 test `default_context_non_workgroup_omits_messaging_exception`):

```rust
    #[test]
    fn default_context_replica_with_matrix_and_messaging_renders_both_sections() {
        let out = default_context(
            "C:/fake/wg-7-dev-team/__agent_architect",
            Some("C:/fake/_agent_architect"),
        );
        assert!(
            out.contains("3. **Your origin Agent Matrix"),
            "matrix section header missing, got:\n{}",
            out
        );
        assert!(
            out.contains("Narrow exception — workgroup messaging directory"),
            "messaging exception header missing, got:\n{}",
            out
        );
        // Composition: matrix bullets immediately followed by exception header
        // (single blank line between, matrix_section ends with \n\n).
        assert!(
            out.contains("- `Role.md`\n\n**Narrow exception"),
            "expected matrix → exception boundary, got:\n{}",
            out
        );
        // Composition: ordering of the three structural markers.
        let exception_pos = out
            .find("Narrow exception")
            .expect("messaging exception must be present");
        let summary_pos = out
            .find("Any repository or directory outside the allowed entries above is READ-ONLY.")
            .expect("summary line must be present");
        let forbidden_pos = out
            .find("- **FORBIDDEN**")
            .expect("forbidden bullet must be present");
        assert!(
            exception_pos < summary_pos,
            "exception must precede summary; exception_pos={exception_pos}, summary_pos={summary_pos}"
        );
        assert!(
            summary_pos < forbidden_pos,
            "summary must precede forbidden bullet; summary_pos={summary_pos}, forbidden_pos={forbidden_pos}"
        );
        // The FORBIDDEN bullet acknowledges the messaging exception by name.
        assert!(
            out.contains("the workspace root (other than the narrow messaging exception above)"),
            "FORBIDDEN bullet missing the messaging-exception qualifier, got:\n{}",
            out
        );
    }
```

Why ordering-based assertions instead of literal byte-for-byte composition checks for the summary→FORBIDDEN boundary: the matrix→messaging boundary has stable single-blank-line spacing (both fragments end with `\n\n`, no extra newline interjects). The messaging→summary boundary has `\n\n\n` (two blank lines) because of an extra source-side newline in the template literal between the placeholder line and the summary line — see dev-rust review §2 spacing analysis. Locking the byte-level pattern would over-couple the test to incidental whitespace; the ordering check is what we actually want to enforce.

#### §R-4.2 Existing tests — re-verify after R-1 edits

The R-1 edits change two strings the existing test set must NOT trip on:

- Existing test `default_context_embeds_filename_only_warning` (lines 636-642): asserts `"filename ONLY"`, `"BAD:"`, `"GOOD:"`. These substrings live in the `## Inter-Agent Messaging` section (lines 596-599), untouched by R-1. **Still passes.**
- §4.2 `default_context_replica_under_wg_includes_messaging_exception`: asserts `"Narrow exception — workgroup messaging directory"`, `"wg-7-dev-team"`, `"- **Allowed (narrow)**: Create canonical inter-agent message files"`. None overlap with R-1's edits. **Still passes.**
- §4.3 `default_context_non_workgroup_omits_messaging_exception`: asserts the *absence* of `"Narrow exception — workgroup messaging directory"` and `"- **Allowed (narrow)**:"` for non-WG paths. R-1 does not introduce either string on the non-WG path. **Still passes.**

R-4.1 is the only NEW test required. Existing test set is regression-safe.

---

### §R-5 — Reject finding 5 (cosmetic test path style)

Keep the forward-slash test paths from §4.2/§4.3/§R-4.1. Rationale:

- Plan §4.4 already documents the cross-platform behavior. `Path::file_name` is separator-agnostic and `phone::messaging::workgroup_root` walks `Path::ancestors`, which treats both `/` and `\` as separators on Windows.
- Switching to raw-string Windows paths (`r"C:\fake\..."`) would obscure cross-platform parity and produce Windows-only path output in test failure messages, which is harder to read for a Linux-side maintainer running the suite.
- The grinch confirms the existing assertions tolerate the mixed-separator joined-output (e.g. `C:/fake/wg-7-dev-team\messaging`). The existing test (`default_context_embeds_filename_only_warning`) already establishes this convention.

No edit. The plan stands as-is for §4.4.

---

### Summary of plan deltas (delta-table)

| Plan section | Delta source | Change |
|---|---|---|
| §2 (Affected files) | R-2.1 | Add `entity_creation.rs` as file #2 |
| §3.1 | (unchanged) | No `use` change needed |
| §3.2 | R-3 | Update `messaging_exception` literal text per §R-3 |
| §3.3 | (unchanged) | Original spacing fragment still correct |
| §3.4 | R-1.3 | Drop literal `those ` from the FORBIDDEN bullet template (in addition to the `{messaging_allowed}` insertion) |
| §3.5 | (unchanged) | Named arg list unchanged |
| §3.6 | R-1 | First and third bullets unchanged. Second bullet ("**No edit to `forbidden_scope`**") **SUPERSEDED** by §R-1.2 / §R-1.3 / §R-1.4. |
| §3.7 (NEW) | R-2.2 | Add `entity_creation.rs::create_workgroup` one-liner |
| §3.8 (NEW) | R-1.1 | Replace `allowed_places` block with single literal |
| §3.9 (NEW) | R-1.2 | Replace `forbidden_scope` binding (introduces `workspace_root_phrase`) |
| §3.10 (NEW) | R-1.4 | Edit summary-line template `places` → `entries` |
| §4.5 (NEW) | R-4.1 | Add (matrix=Some, messaging=Some) composition test |
| §6 | R-2.3 | Add note 8 — bootstrap rationale |
| §7 | (unchanged) | Out-of-scope items still apply |

### Implementation order (advisory, supersedes the dev-rust review's "Implementation order I'll follow")

1. Apply §R-1.1 (collapse `allowed_places`).
2. Apply §3.2 (add `messaging_dir_display`, `messaging_exception` per R-3, `messaging_allowed` bindings).
3. Apply §R-1.2 (add `workspace_root_phrase`, replace `forbidden_scope`).
4. Apply §3.3 (concatenate `{matrix_section}{messaging_exception}`, removing the literal blank line).
5. Apply §R-1.3 (insert `{messaging_allowed}`, drop `those ` in the FORBIDDEN template).
6. Apply §R-1.4 (`places` → `entries` in the summary line).
7. Apply §3.5 (extend the named-arg list with `messaging_exception`, `messaging_allowed`).
8. Apply §R-2.2 (one-liner in `create_workgroup`).
9. Add §4.5 test (R-4.1).
10. `cargo check` → `cargo clippy` (must be clean) → `cargo test -p <crate> session_context` → confirm new test passes and existing tests still pass.
11. Bump `tauri.conf.json` version per repo convention.
12. Build the WG-1 binary `agentscommander_standalone_wg-1.exe` (shipper-only-to-WG; never the bare standalone).
13. Coordinate with tech-lead before re-launching `ac-cli-tester` so it picks up the refreshed context via `materialize_agent_context_file`.

### Verdict

**READY_FOR_IMPLEMENTATION**

dev-rust may proceed using the original plan + this resolution as the single source of truth. Where they disagree, this resolution wins.
