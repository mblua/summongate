# Grinch plan review — #199 messaging write permission

**Reviewer:** dev-rust-grinch
**Date:** 2026-05-10
**Plan reviewed:** `_plans/199-messaging-write-permission.md`
**Verdict:** **CHANGES REQUESTED** — see findings 1, 3, and 4. Findings 2 and 5 are non-blocking but worth landing as follow-ups.

---

## What I checked

- The plan's exact insertion sites against `src-tauri/src/config/session_context.rs:478-643` (the file as it stands today).
- `phone::messaging::workgroup_root`, `MESSAGING_DIR_NAME`, `messaging_dir`, `validate_filename_shape`, and `create_message_file` (`src-tauri/src/phone/messaging.rs:11-225`).
- `cli/send.rs:148-203` — confirmed only `--send <filename>` is accepted; `--message` / `--message-file` are gone, matching the diagnostic in `__agent_ac-cli-tester/missing-reply-diagnostic.md`.
- `commands/entity_creation.rs:559-788` — confirmed `create_workgroup` does NOT pre-create `messaging/`.
- The four substitution combinations (matrix yes/no × messaging yes/no) against the existing template literal at `session_context.rs:510-621`.
- Existing tests (`default_context_embeds_filename_only_warning`, `workgroup_root_*`) for regression risk.

---

## Findings

### 1. (CHANGES REQUESTED) Internal contradiction remains in the GOLDEN RULE after the change

**What.** After the proposed edits, the GOLDEN RULE text is internally inconsistent in three places, all of which a strict-reading agent (exactly the kind of agent #199 was filed for — see `ac-cli-tester`'s diagnostic) will trip over:

1. The numbered preamble still says **"You may ONLY modify files in two places"** (resp. "three" if matrix). The new "Narrow exception" subsection introduces a third (resp. fourth) category, but the preamble's claim of exclusivity is unchanged.
2. The bullet list now contains an `Allowed (narrow)` entry, but the trailing **"FORBIDDEN: Any write operation outside those two zones …"** (line 500-504, `forbidden_scope`) still says "two zones" / "allowed zones" with no acknowledgement of the messaging exception.
3. The summary line **"Any repository or directory outside the allowed places above is READ-ONLY"** still references "the allowed places above" — what counts as "above"? Just the numbered 1/2/(3)? Or also the narrow exception subsection sandwiched between?

**Why.** This is not theoretical. `__agent_ac-cli-tester/missing-reply-diagnostic.md:52` shows the exact failure mode: the strict reader cited *"`messaging\` es una carpeta hermana de mi raiz, no una subcarpeta, por lo que no debo escribir alli sin una autorizacion superior explicita"* and returned `CONTACT_FAIL`. After this plan ships, the same agent will read:

- "ONLY two places" (preamble — exclusivity claim)
- *Narrow exception* (subsection)
- "outside those two zones" (FORBIDDEN bullet — exclusivity claim)
- "Allowed (narrow)" (bullet — permission)

Two of those four say "you may ONLY do X with Y zones"; the other two introduce a Z. A pedantic reader picks the most restrictive interpretation, which is exactly the bug we are fixing. Plan section §3.6 acknowledges this by claiming "The workspace root *itself* remains forbidden … only the specific `messaging/` subdirectory is excepted, and that exception is now explicitly listed in the 'Allowed' bullets. No ambiguity." That argument is correct *for a sympathetic reader*. The agent we are designing for is not a sympathetic reader; that is the entire premise of #199.

**Fix.** Pick one of these (in order of minimal-blast-radius):

- **(preferred) Rewrite the FORBIDDEN bullet's `forbidden_scope`** to: `"the allowed entries above — including other agents' replica directories, [the Agent Matrix scope outside memory/plans/Role.md,] the workspace root (other than the messaging/ exception above), parent project dirs, user home files, or arbitrary paths on disk"`. This costs one extra branch in `forbidden_scope` (or a parameterized inline) but eliminates the "two zones" / "three zones" arithmetic mismatch.
- **(alternative) Soften the preamble** so "ONLY in two places" reads "ONLY in the entries listed below (numbered list + the narrow exception)". Keeps the numbered list intact; just replaces the `allowed_places` "two/three places" string with something like "the entries listed below".
- **(alternative) Number the exception** as `2a.` or move it inside item `2.` — explicitly extends the numbered list rather than living between the list and the summary line. This is what the plan section §3.6 says NOT to do, but it most directly fixes the contradiction.

Plan section §3.6's instruction "no edit to `forbidden_scope`" should be lifted; that is the source of the residual contradiction.

---

### 2. (NON-BLOCKING, document) Bootstrap of `<wg-root>/messaging/` is unaddressed

**What.** The protocol's step 1 is "Write your message to a new file in the workgroup messaging directory". Step 2 is `send --send <filename>`. `cli/send.rs:161` calls `messaging::messaging_dir(&wg_root)`, which creates the dir via `create_dir_all`. But that call happens *during* step 2, after step 1 already required the dir to exist. No call to `messaging_dir()` (or `create_dir_all` of `<wg>/messaging`) exists in `commands/entity_creation.rs::create_workgroup` (lines 559-788) — verified by grep.

In a freshly-created workgroup, the very first message therefore fails at step 1: `fs::write` (or the equivalent agent tool) cannot create a file under a non-existent parent. Existing WGs on this machine (including `wg-1-dev-team`) only work because the dir was bootstrapped by some prior interaction — most likely a tech-lead's loose-interpretation `mkdir` before the GOLDEN RULE was being enforced, since I cannot find a code path that creates it during WG creation.

**Why.** For #199's stated scope (have an existing replica reply to an existing message), the dir already exists, so this does not block validation. But the plan's *narrow exception* permits the agent to "create message files inside this directory" without permitting the agent to create the directory itself, so the failure mode is just shifted: the first ever inter-agent message in a brand-new WG hits the same `CONTACT_FAIL` as before.

**Fix.** Either:

- (in-scope, cheap) Add a one-liner to `entity_creation.rs::create_workgroup` after line 603: `std::fs::create_dir_all(wg_dir.join("messaging")).map_err(...)?;` and document it under §2 of the plan as a second affected file.
- (out-of-scope, document) Note in plan §7 that brand-new WGs still require an external bootstrap of `messaging/` and link a follow-up issue. Alternatively widen the narrow exception to also permit `mkdir <wg-root>/messaging/` if missing — but I would not pick this path; bootstrap-by-side-effect-of-a-text-rule is exactly the surface area we should not be growing.

This is a pre-existing issue that the plan does not make worse, but it is the natural follow-up to land at the same time.

---

### 3. (CHANGES REQUESTED) Placeholder shape mismatch between the new exception text and the existing protocol section

**What.** The plan's `messaging_exception` describes the canonical filename shape as:

> `YYYYMMDD-HHMMSS-<from_short>-to-<to_short>-<slug>.md`

But the *existing* `## Inter-Agent Messaging` section in the same file (`session_context.rs:587-589`) describes it as:

> `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md`

These are the same regex, but the placeholder vocabulary differs (`<from_short>` vs `<wgN>-<you>`). An agent reading the GOLDEN RULE first and the messaging section second sees two different-looking patterns and has to guess whether they collapse to the same thing. (`agent_short_name` confirms they do: `from_short = "wg<N>-<you>"` after the function in `phone/messaging.rs:76-85`.)

**Why.** Documentation drift inside one generated file is exactly the class of bug that produced #199. The whole reason this plan exists is that two sections of the same template were mutually inconsistent.

**Fix.** Align the new text with the existing wording. Replace `<from_short>-to-<to_short>-<slug>` with `<wgN>-<you>-to-<wgN>-<peer>-<slug>` in the `messaging_exception` literal (plan §3.2). Costs nothing.

---

### 4. (CHANGES REQUESTED) Test coverage gap — production case `(matrix=Some, messaging=Some)` is not exercised

**What.** The plan adds two tests (§4.2, §4.3) covering:

- `(matrix=None, messaging=Some)` — `default_context_replica_under_wg_includes_messaging_exception`
- `(matrix=None, messaging=None)` — `default_context_non_workgroup_omits_messaging_exception`

The existing test `default_context_embeds_filename_only_warning` covers `(matrix=None, messaging=None)`. There is **no** test covering `(matrix=Some, messaging=Some)`, which is *the* production scenario for every WG replica with a configured `identity` (i.e. all the agents in `wg-1-dev-team`).

**Why.** The §3.3 template change concatenates `{matrix_section}{messaging_exception}` on a single line, relying on both fragments terminating with `\n\n`. If a future maintainer drops a trailing `\n` from either fragment, only the (Some, Some) combination breaks (the others have at least one empty fragment masking the issue). The existing test set would still pass.

**Fix.** Add a third test:

```rust
#[test]
fn default_context_replica_with_matrix_and_messaging_renders_both_sections() {
    let out = default_context(
        "C:/fake/wg-7-dev-team/__agent_architect",
        Some("C:/fake/_agent_architect"),
    );
    assert!(out.contains("3. **Your origin Agent Matrix"), "matrix section missing");
    assert!(out.contains("Narrow exception — workgroup messaging directory"), "messaging exception missing");
    // Composition check: matrix bullet immediately followed by allowed-narrow bullet,
    // no orphaned blank lines between them.
    assert!(
        out.contains("- `Role.md`\n\n**Narrow exception"),
        "expected single blank line between matrix bullets and exception header"
    );
    assert!(
        out.contains("- **Allowed (narrow)**:"),
        "narrow-allowed bullet missing"
    );
}
```

This locks in the intended spacing for the (Some, Some) case so regressions show up immediately.

---

### 5. (NON-BLOCKING) Unrealistic test path style

**What.** §4.2 and §4.3 use forward-slash test paths (`"C:/fake/wg-7-dev-team/__agent_architect"`). On Windows, `Path::join("messaging")` produces a backslash separator, so `messaging_dir_display` ends up as `C:/fake/wg-7-dev-team\messaging` (mixed separators). Production paths (post-canonicalize) are pure backslash on Windows. The current tests' assertions (`out.contains("wg-7-dev-team")`) tolerate this, but a future maintainer adding a stricter path assertion will be surprised.

**Why.** Cosmetic only; not a correctness issue. Plan §4.4 documents the rationale ("`Path::file_name` on both Windows and Unix returns the last segment regardless of separator style") and is correct. Mentioning here so the reviewer is aware.

**Fix.** None required. Optionally use `r"C:\fake\wg-7-dev-team\__agent_architect"` to match production conventions and make the rendered path in test failure messages legible to a Windows-side debugger.

---

## Items I confirmed are NOT issues

- **`format!` placeholder safety.** `messaging_exception` contains literal triple-backticks and `<...>` placeholders. None of those introduce stray `{` / `}` that would collide with the outer `format!(r#"..."#)`. The inner `format!` is evaluated first; the outer raw string interpolates the result verbatim. ✅
- **Test path determinism cross-platform.** `phone::messaging::workgroup_root` is a pure ancestor walk and uses `file_name()`, which is separator-agnostic on both Unix and Windows. `(C:/fake/wg-7-dev-team/__agent_architect)` resolves identically on either OS. ✅
- **Existing test regression.** `default_context_embeds_filename_only_warning` passes a non-WG path; both new fragments evaluate to empty strings, so its asserted substrings remain present. The single-newline collapse in the matrix-section / messaging-exception line does not affect Markdown rendering. ✅
- **`messaging_dir_display` UNC handling.** `agent_root` reaches `default_context` already trimmed of `\\?\` (via `display_path` in `ensure_session_context:14-17`). `wg.join("messaging")` produces a fresh `PathBuf` with no UNC re-prefixing. `display_path` is a defensive no-op here. ✅
- **No new lock acquisition or async surface.** Pure text generation; no concurrency considerations. ✅
- **No new resource leaks.** No fs handles are opened during this code path. ✅

---

## Better minimal verification path?

The plan's verification (rebuild WG-binary → relaunch `ac-cli-tester` → tech-lead resends `CONTACT_OK / CONTACT_FAIL`) is sound but slow. A faster, complementary check before the live run:

1. After the build, manually inspect the regenerated context cache file directly:
   `%LOCALAPPDATA%\..\<LocalDir>\context-cache\ac-context-<hash>.md`
   (or whatever resolves from `super::config_dir()` on this machine).
2. Confirm the file contains:
   - The "Narrow exception — workgroup messaging directory" heading.
   - The literal absolute path of `<wg>/messaging`.
   - The `- **Allowed (narrow)**:` bullet.
3. Confirm the *agent's* in-replica `CLAUDE.md` / `AGENTS.md` (rewritten on session launch by `materialize_agent_context_file`) matches.

This is a 30-second filesystem check that catches build/cache issues before spending the agent's context on a live message round-trip.

---

## Summary

The plan is structurally sound and surgical. The single concrete bug a strict-reading agent will hit is finding 1 (the residual "ONLY two places" / "those two zones" wording). Fixing finding 1 plus aligning placeholder vocabulary (finding 3) and adding the `(Some, Some)` test (finding 4) closes the loop completely. Findings 2 and 5 are non-blocking; finding 2 should land as a same-PR or follow-up to avoid leaving a chicken-and-egg in fresh WGs.

Stopping here as instructed. No code changes from me.
