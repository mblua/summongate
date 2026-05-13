# Dev-Rust Implementation Report — #199

**Date:** 2026-05-10
**Branch:** `feature/191-cli-project-open-create`
**Implementer:** dev-rust
**Status:** COMPLETE — implementation, tests, and review pass clean.

---

## Files changed

| # | File | Nature of change |
|---|---|---|
| 1 | `src-tauri/src/config/session_context.rs` | GOLDEN RULE rewrite per architect resolution: collapsed `allowed_places` to single literal, added `messaging_dir_display` / `messaging_exception` / `messaging_allowed` bindings, replaced `forbidden_scope` (now `String`, references "the entries listed above" + workspace-root narrow-exception qualifier), edited template (concatenation, FORBIDDEN insert, `places`→`entries`, dropped `those`, broadened "NO exceptions" closing line), extended named-arg list, added 3 new tests. |
| 2 | `src-tauri/src/commands/entity_creation.rs` | One-line bootstrap of `<wg_dir>/messaging/` immediately after WG dir creation in `create_workgroup` (R-2.2). |
| 3 | `src-tauri/tauri.conf.json` | Version bump 0.8.16 → 0.8.17 (per repo convention for visual confirmation of new build). |
| 4 | `src-tauri/Cargo.toml` | Version bump 0.8.16 → 0.8.17 (kept aligned with tauri.conf.json). |
| 5 | `src-tauri/Cargo.lock` | Auto-regenerated from Cargo.toml version bump. |
| 6 | `package.json` | Version bump 0.8.16 → 0.8.17 (kept aligned). |
| 7 | `_plans/199-messaging-write-permission.md` | Already on disk (untracked) — committed alongside the implementation. |
| 8 | `_plans/199-grinch-plan-review.md` | Already on disk (untracked) — committed alongside the implementation. |
| 9 | `_plans/199-dev-rust-implementation-report.md` | This report. |

**Explicitly NOT committed:** `package-lock.json` modification (pre-existing dirt per tech-lead's note about prior build setup; left untouched as instructed).

---

## Architect resolution items applied

All 12 steps of the architect's `Implementation order` (advisory) were applied in order, plus the two review-driven fixes captured below.

| Resolution | Site | Status |
|---|---|---|
| R-1.1 — collapse `allowed_places` to literal `"the entries listed below"` | `session_context.rs:479` | DONE |
| §3.2 — `messaging_dir_display`, `messaging_exception` (R-3 wording), `messaging_allowed` bindings | `session_context.rs:496-522` | DONE |
| R-1.2 — `workspace_root_phrase` + `forbidden_scope` rewritten as `String` referencing "the entries listed above" | `session_context.rs:523-538` | DONE |
| §3.3 — concatenate `{matrix_section}{messaging_exception}` (drop literal blank line) | `session_context.rs:560` | DONE |
| R-1.3 — insert `{messaging_allowed}`, drop `those ` before `{forbidden_scope}` | `session_context.rs:566` | DONE |
| R-1.4 — `places` → `entries` on summary line | `session_context.rs:561` | DONE |
| §3.5 — extend named-arg list with `messaging_exception` and `messaging_allowed` | `session_context.rs:652-653` | DONE |
| R-2.2 — bootstrap `<wg_dir>/messaging/` in `create_workgroup` | `entity_creation.rs:604-605` | DONE |
| R-3 — placeholder vocabulary `<wgN>-<you>-to-<wgN>-<peer>`, drop `validate_filename_shape` leak, broaden "any message file once written" | inside `messaging_exception` literal | DONE |
| §4.2, §4.3, §4.5 — three new tests | `session_context.rs:679-?` | DONE — all passing |
| Code-review fix #1 — broaden closing "NO exceptions" sentence | `session_context.rs:570` (template line) | DONE |
| Code-review fix #2 — regression-guard assertion in R-4.1 test | end of `session_context.rs` | DONE |

R-5 (cosmetic test path style) was REJECTED in the architect resolution; no action.

---

## Commands run

| # | Command | Exit | Notes |
|---|---|---|---|
| 1 | `cargo check --message-format=short` (in `src-tauri/`) | 0 | Two pre-existing dead-code warnings in `commands/ac_discovery.rs` (`extract_brief_first_line`, `read_brief_capped`); unrelated to #199. |
| 2 | `cargo clippy --lib --message-format=short` (in `src-tauri/`) | 0 | Same two pre-existing warnings; clippy clean otherwise. |
| 3 | `cargo test --lib session_context` (in `src-tauri/`) | 0 | 4 passed, 0 failed. Existing `default_context_embeds_filename_only_warning` + 3 new tests (`replica_under_wg_includes_messaging_exception`, `non_workgroup_omits_messaging_exception`, `replica_with_matrix_and_messaging_renders_both_sections`). |
| 4 | `cargo test --lib` (full lib suite, in `src-tauri/`) | 1 | 348 passed, 1 failed: `config::projects::tests::absolutise_collapses_dotdot_segments_on_windows`. |
| 5 | Re-run #4's failing test with `--test-threads=1` | 0 | Passes in isolation — confirms it is a pre-existing parallelism flake (CWD swapping in `#[cfg(windows)]` collides with sibling tests), not introduced by #199. |
| 6 | `cargo test --lib session_context` (after applying review fixes #1 + #2) | 0 | 4 passed, 0 failed — all session_context tests still green after the two review-driven edits. |

---

## Feature-dev / code-reviewer result

**Tool:** `feature-dev:code-reviewer` subagent (in lieu of `/feature-dev` which expects an interactive feature-dev workflow — the relevant phase here is review, which `code-reviewer` covers directly).

**Findings reported by the reviewer (confidence / severity / disposition):**

1. **Confidence 88 / Important** — Closing line of GOLDEN RULE section still read "There are NO exceptions." The architect resolution §R-1 rewrote the preamble, summary, and FORBIDDEN bullet, but did not flag this line. After R-1 ships, an agent reading strictly sees "You MAY create message files…" two paragraphs later than "There are NO exceptions." — same exclusivity-by-count failure mode the entire plan was meant to cure.
   **Fix applied:** changed the template literal to "There are NO exceptions beyond those listed above." (one-word edit to the closing sentence). `session_context.rs:570`.

2. **Confidence 80 / Important** — R-4.1 test does not assert the FORBIDDEN bullet contains the new "outside the entries listed above" prefix. A regression that reverts `forbidden_scope` to "two zones" would still pass all four tests today.
   **Fix applied:** added one assertion to the R-4.1 test asserting `out.contains("- **FORBIDDEN**: Any write operation outside the entries listed above")`. Test still passes.

**Confirmed clean by reviewer:** R-1.1 collapse, R-1.2 ordering and `String` type change, R-1.3 `those` drop, R-1.4 `places`→`entries`, R-3 vocabulary alignment, R-2.2 bootstrap correctness/idempotency/failure-mode, format!-placeholder safety with the inner triple-backtick block, named-arg list ordering, R-4.1 ordering and composition assertions.

No CRITICAL findings. Both IMPORTANT findings fixed before commit. No HIGH severity issues remain.

---

## Residual risk

1. **Materialization timing.** Already-running agents (e.g. `ac-cli-tester`) are still operating with the OLD context file written at their session launch. Per the plan's §6.6 / §7.3, this is by design — `materialize_agent_context_file` regenerates the file at the next session launch. Tech-lead must restart `ac-cli-tester` (and any other live agent expected to message) for the fix to take effect for them. Coordinate before the validation step of #191.

2. **Coordinator/Agent-Matrix participation in messaging.** Architect resolution and plan §7 explicitly leave coordinator agents (sessions launched directly from `_agent_*` outside any `wg-N-*`) without a messaging exception, because `workgroup_root` cannot infer which WG they would address. If a future workflow requires coordinator-to-WG messaging, that is a separate design.

3. **Pre-existing parallelism flake** in `config::projects::tests::absolutise_collapses_dotdot_segments_on_windows` — unrelated to #199, but worth a follow-up issue. The test uses `std::env::set_current_dir` inside a `FixtureRoot`, which races with sibling tests sharing the same Temp namespace. Easy fix would be to gate behind `#[serial]` (the `serial_test` crate) or move the assertion to a test that does not require CWD changes. NOT bundled into this PR per the plan's "do not revert unrelated existing worktree changes" instruction.

4. **Build artifact still pending.** Per plan §6.6 the WG-1 binary `agentscommander_standalone_wg-1.exe` should be built via the shipper to give the user a visually-bumped version (0.8.17). Implementation, version bump, and commit are complete; the actual build/ship is the user's call (the role explicitly forbids me from pushing or shipping autonomously, and the shipper-only-to-WG memory note governs which binary target to use). I am leaving the build trigger to the tech-lead / user.

---

## What this unblocks

Once the new binary is shipped (or `ac-cli-tester` is otherwise restarted with the refreshed context), the `CONTACT_OK / CONTACT_FAIL` validation step for #191 should succeed because:
- Strict-reading agents now see consistent "you MAY write canonical messages here" language across the GOLDEN RULE preamble, the Allowed bullets, the FORBIDDEN bullet, and the closing "no exceptions" sentence.
- Fresh workgroups created by `create_workgroup` already have `<wg_dir>/messaging/` on disk, so the first `fs::write` of a message file from any agent succeeds without needing a prior `send` to lazily create the dir.

---

## Verdict

**READY FOR TECH-LEAD REVIEW.** Code compiles clean, clippy clean, all session_context tests green, code-reviewer findings fully addressed. No HIGH severity issues remain. Awaiting tech-lead direction on whether to restart `ac-cli-tester` against the new binary now or defer until after a manual smoke test.
