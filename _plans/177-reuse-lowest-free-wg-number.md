# Plan: Reuse lowest free workgroup number on creation (#177)

**Branch:** `feature/177-reuse-lowest-free-wg-number`
**Status:** READY_FOR_IMPLEMENTATION (architect round-2 resolved 2026-05-07)

---

## Dev-Rust enrichment (2026-05-07)

Plan reviewed against the current source on
`feature/177-reuse-lowest-free-wg-number`. Verified: file paths, all line
numbers in the affected file, the `HashSet` import on line 2, `use super::*;`
in the test module (line 1791), `tempfile = "3"` dev-dependency
(`Cargo.toml` line 47), and `sanitize_name` normalization (line 90).

**Corrections folded into Change 1 and Change 2 below:**

1. **Latent panic in the existing slicing — fixed in the replacement.**
   The original code uses `&name_str[3..name_str.len() - suffix.len()]` to
   extract the digits between `wg-` and `-{team}`. For a directory named
   `wg-{team}` exactly (no number, e.g. `wg-dev-team`), `name_str.len()`
   equals `3 + suffix.len() - 1`, so the slice becomes `&str[3..2]` which
   **panics at runtime** with `slice index starts after end`. This is a
   latent panic in the existing implementation that the new function
   would otherwise inherit. The replacement now uses `name_str.get(..)`
   for checked slicing — invalid ranges return `None` and the entry is
   silently skipped, matching the plan's intent.

2. **Test 6 comment + new test 9.** Test 6's comment originally claimed
   `wg-dev-team` would be "ignored" by the suffix check, which is wrong
   (it passes the suffix check and panics in the slice). The misleading
   line is removed from test 6's comment, and a new `test 9`
   (`determine_next_wg_number_does_not_panic_on_no_number_dir`) creates
   `wg-dev-team` explicitly and asserts the function returns `1` — locking
   in the no-panic behavior as a regression test.

**Cosmetic-only**: the plan says `mod tests` ends at line 2084; it
actually ends at line 2085 (line 2084 is blank). Append point is
unchanged in practice — just before the final closing `}`.

**Open question for tech-lead (does NOT block implementation):** the
user has a standing preference (memory `feedback_bump_version_on_builds`)
that every feature build bumps `tauri.conf.json` so they can visually
confirm they're running the new build. The plan's "Out of scope" says
not to bump the version in this plan. These can both be true if the
bump happens at build/dispatch time rather than as part of the feature
commit — please confirm whether dev-rust should bump the version as
part of the implementation commit, or leave it to the build flow.

> **Resolved in round 2:** tech-lead confirmed do NOT bump the version
> as part of the allocator implementation. Version-bump-on-build is a
> shipper/release concern handled out-of-band for #177. Dev-rust must
> NOT touch `tauri.conf.json` in this commit.

---

## Round-2 architect resolution (2026-05-07)

Round-1 reviewers (dev-rust, grinch) returned findings against the plan
as enriched by dev-rust. Grinch raised six items (G1–G6); decisions are
below and have been merged into the plan body that follows. The Grinch
Review section at the bottom is preserved verbatim for traceability.

| Finding | Disposition | How resolved in plan body |
|---|---|---|
| G1 — `n > 0` filter rationale wrong | **Drop the filter** (option a) | Change 1 code drops the `if n > 0` guard. "Why this exact shape" no longer lists `Filter n > 0`. Edge case §4 rewritten. Test 7 docstring updated. |
| G2 — Edge case §1 false TOCTOU safety claim | **Documentation-only** (option a) | Edge case §1 rewritten honestly. The `create_dir_all` line is left unchanged — race hardening tracked separately. |
| G3 — Missing test for `.deleting-…` temp dir | **Add test** | Test 10 (`determine_next_wg_number_ignores_deleting_temp_dirs`) appended to Change 2. |
| G4 — Missing test for subset team suffixes | **Add test** | Test 11 (`determine_next_wg_number_distinguishes_subset_team_suffixes`) appended to Change 2. |
| G5 — Banner style drift | **Fix banner** | Change 2 banner becomes `// ── #177 — determine_next_wg_number lowest-free reuse ──`. |
| G6 — `read_dir` silent slot-1 degradation | **Docstring bullet** | Function docstring in Change 1 gains a "Read-error degradation" paragraph. No code change. |

### Rationale for each non-trivial choice

**G1 (drop filter, not rephrase).** A no-op filter dressed up with
"defensive" rhetoric is a load-bearing-for-no-reason hazard for future
maintainers. The smaller code is the better code; correctness is now
explained at the `find` site (search range starts at 1, so `0` is
unreachable from there) — the natural place for the invariant.

**G2 (docs-only, not `create_dir`).** The race is pre-existing in the
OLD `max+1` allocator and orthogonal to the gap-reuse policy this issue
implements. While #177 does increase the practical hit rate, the race
fix has its own review surface (concurrent caller semantics, the loser's
already-attempted writes to BRIEF.md / repo clones / `__agent_*`, error
mapping) that deserves a dedicated ticket. Per tech-lead's directive to
keep #177 focused on allocator behaviour + tests, the documentation is
corrected and the code is left alone here. A separate issue should be
filed to track the race hardening.

**G3 / G4 (add both tests).** Both lock contracts that #177 implicitly
depends on (the `.deleting-` rename pattern in `try_atomic_delete_wg`
and suffix-overlap disambiguation via `parse::<u32>`). Cheap regression
guards; no reason to omit.

### Test count
Round 1 (dev-rust): 9 tests. Round 2 (G3 + G4 added): **11 tests** total.

---

## Requirement

When a new workgroup is created with the existing `wg-<N>-<team>` naming, allocate
**the lowest free positive integer starting at 1** for that team suffix, instead
of the current `max(existing) + 1` policy.

Concrete example for team `dev-team`:

| Existing dirs in `.ac-new/`                        | Today returns | Must return |
|----------------------------------------------------|---------------|-------------|
| (none)                                             | 1             | 1           |
| `wg-1-dev-team`                                    | 2             | 2           |
| `wg-1-dev-team`, `wg-3-dev-team`                   | **4**         | **2**       |
| `wg-1-dev-team`, `wg-2-dev-team`, `wg-3-dev-team`  | 4             | 4           |
| `wg-2-dev-team`, `wg-3-dev-team`                   | 4             | 1           |

The fix is scoped to the allocator function. The caller and the post-allocate
`exists()` defensive check do not change.

---

## Affected files

| File                                                | Section                          | Action                |
|-----------------------------------------------------|----------------------------------|-----------------------|
| `src-tauri/src/commands/entity_creation.rs`         | `determine_next_wg_number` (1627–1651) | replace body          |
| `src-tauri/src/commands/entity_creation.rs`         | `mod tests` (after line 2084)    | append regression tests |

No other files. No new crates. No new imports (`HashSet` is already imported at line 2).

---

## Change 1 — Replace `determine_next_wg_number` body

**File:** `src-tauri/src/commands/entity_creation.rs`
**Lines:** 1626–1651 (the doc comment + entire function body)

### Current code (to replace verbatim)

```rust
/// Scan .ac-new/ for existing wg-*-{team_name}/ dirs and return the next N.
fn determine_next_wg_number(ac_new_dir: &Path, team_name: &str) -> u32 {
    let suffix = format!("-{}", team_name);
    let mut max_n: u32 = 0;

    if let Ok(entries) = std::fs::read_dir(ac_new_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("wg-") && name_str.ends_with(&suffix) {
                // Extract the number between "wg-" and "-{team_name}"
                let middle = &name_str[3..name_str.len() - suffix.len()];
                if let Ok(n) = middle.parse::<u32>() {
                    if n > max_n {
                        max_n = n;
                    }
                }
            }
        }
    }

    max_n + 1
}
```

### Replacement

```rust
/// Scan `.ac-new/` for existing `wg-<N>-{team_name}/` dirs and return the
/// **lowest free positive integer** starting at 1.
///
/// Issue #177: previously this returned `max(existing) + 1`, which left
/// permanent gaps after a workgroup was destroyed. The new policy reuses
/// any freed numbers so the user-facing sequence stays compact.
///
/// Filtering rules (unchanged from prior behavior):
/// - Only directories are considered (regular files are ignored).
/// - The directory name must match `wg-<digits>-<team_name>` exactly:
///   prefix `wg-`, suffix `-{team_name}`, numeric middle.
/// - Non-numeric middles (e.g. `wg-foo-team`) and other team suffixes
///   are ignored.
///
/// Slot 1 is always reachable because the lowest-free search starts at
/// 1 (see the `find` call below); a stray `wg-0-{team}` directory ends
/// up in `taken` but is never tested by `find` and so cannot displace
/// slot 1.
///
/// Read-error degradation: if `std::fs::read_dir(ac_new_dir)` fails
/// (permission denied, transient I/O, broken junction, path-too-long
/// on Windows), the function returns `1` as a graceful fallback. The
/// post-allocate `wg_dir.exists()` guard in `create_workgroup` will
/// surface the real condition as an "already exists" error if a
/// `wg-1-{team}` is in fact present; otherwise the slot-1 creation
/// succeeds with stale state. Surfacing the read error is tracked
/// separately and is out of scope for #177.
fn determine_next_wg_number(ac_new_dir: &Path, team_name: &str) -> u32 {
    let suffix = format!("-{}", team_name);
    let mut taken: HashSet<u32> = HashSet::new();

    if let Ok(entries) = std::fs::read_dir(ac_new_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("wg-") && name_str.ends_with(&suffix) {
                // Extract the number between "wg-" and "-{team_name}".
                // Use `.get(..)` (checked slicing) — a name like
                // `wg-{team}` (no number) passes both the prefix and
                // suffix checks but produces a slice with start > end,
                // which would panic with `&str[..]`. `.get(..)` returns
                // `None` instead, so such entries are silently ignored.
                if let Some(middle) =
                    name_str.get(3..name_str.len() - suffix.len())
                {
                    if let Ok(n) = middle.parse::<u32>() {
                        taken.insert(n);
                    }
                }
            }
        }
    }

    // Lowest free positive integer ≥ 1. The bounded `..=u32::MAX` form avoids
    // any iterator-overflow footgun in debug builds; `find` short-circuits at
    // the first miss so the actual cost is O(taken.len() + 1) in practice.
    // A `0` may end up in `taken` (from a stray `wg-0-{team}`) but is never
    // tested here — the search starts at 1, so slot 1 is always reachable.
    (1u32..=u32::MAX)
        .find(|n| !taken.contains(n))
        .unwrap_or(1)
}
```

### Why this exact shape

- **HashSet over Vec+sort:** O(1) membership test inside the search loop;
  matches the existing module style (`HashSet` is already imported on line 2,
  no new dependency).
- **No `n > 0` filter:** an earlier draft filtered zero out of `taken`. It is
  a no-op — the search range starts at 1, so a `0` in `taken` is never tested
  by `find` and cannot affect the result. Carrying a no-op "for defence" would
  invite a future maintainer to read it as load-bearing and miss real changes
  to slot-1 reachability. The invariant is now stated where it actually lives:
  in the comment on the `find` call.
- **Bounded range `1u32..=u32::MAX`:** `(1u32..)` is `RangeFrom<u32>` which
  panics on overflow in debug. The bounded form is overflow-safe and identical
  in performance because `find` short-circuits.
- **`unwrap_or(1)` fallback:** unreachable in practice (would require ~4 B
  taken slots), but keeps the function total without a `panic!`.
- **Checked slicing with `.get(..)`:** the original implementation used
  `&name_str[3..name_str.len() - suffix.len()]` which **panics** when
  `name_str` is exactly `wg-{team}` (no number) — the slice indices come out
  with start > end. `name_str.get(..)` returns `None` for invalid ranges, so
  such directories are silently skipped, matching the plan's intent of
  ignoring non-conforming names without crashing the allocator.

### Caller — no changes

Line 595 in `create_workgroup` continues to call
`determine_next_wg_number(&base, &safe_team)` and the post-allocate guard at
line 599 (`if wg_dir.exists()`) is preserved as a defensive TOCTOU check.

---

## Change 2 — Append regression tests

**File:** `src-tauri/src/commands/entity_creation.rs`
**Location:** inside the existing `#[cfg(test)] mod tests { … }` block. The
last existing `#[test]` ends at line 2083, line 2084 is blank, and line 2085
is the closing `}` of `mod tests`. Append the new tests after line 2083 and
before line 2085 (i.e. inside the trailing blank line, before the final
closing brace of the module).

The existing module already does `use super::*;` at line 1791, which makes the
private `determine_next_wg_number` reachable, and `tempfile::tempdir()` is
already used by the surrounding tests (line 1798), so no extra imports are
required.

### Tests to add (verbatim)

```rust
// ── #177 — determine_next_wg_number lowest-free reuse ──

/// Helper: create an empty directory at `<root>/<name>` for the test.
fn touch_dir(root: &Path, name: &str) {
    std::fs::create_dir(root.join(name))
        .unwrap_or_else(|e| panic!("create_dir {}: {}", name, e));
}

/// Empty `.ac-new/` returns slot 1 — the lowest positive integer.
#[test]
fn determine_next_wg_number_returns_one_when_no_wg_dirs_exist() {
    let tmp = tempfile::tempdir().expect("tempdir");
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 1);
}

/// Contiguous allocation: `wg-1`, `wg-2`, `wg-3` already exist for the team
/// → next slot is 4 (no internal gap to reuse).
#[test]
fn determine_next_wg_number_returns_next_after_contiguous_block() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(tmp.path(), "wg-2-dev-team");
    touch_dir(tmp.path(), "wg-3-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 4);
}

/// Gap reuse — the load-bearing case from issue #177.
/// `wg-1` and `wg-3` exist (someone destroyed `wg-2`) → next slot is 2.
#[test]
fn determine_next_wg_number_reuses_lowest_internal_gap() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(tmp.path(), "wg-3-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 2);
}

/// Leading gap — `wg-1` is free even though higher slots are taken.
#[test]
fn determine_next_wg_number_reuses_slot_one_when_only_higher_slots_exist() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-2-dev-team");
    touch_dir(tmp.path(), "wg-3-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 1);
}

/// Team scoping: dirs for a different team must not block slot reuse for the
/// requested team. `wg-1-dev-team` and `wg-1-qa-team` coexist → for `qa-team`
/// only slot 1 is taken (by `wg-1-qa-team`), so next is 2; for `dev-team`
/// only slot 1 is taken (by `wg-1-dev-team`), so next is 2.
#[test]
fn determine_next_wg_number_only_considers_matching_team_suffix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(tmp.path(), "wg-1-qa-team");
    touch_dir(tmp.path(), "wg-3-qa-team");
    // For dev-team: only wg-1-dev-team counts → next free is 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 2);
    // For qa-team: wg-1-qa-team and wg-3-qa-team count → next free is 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "qa-team"), 2);
}

/// Invalid `wg-*` directory names must not occupy any slot.
/// - `wg-abc-dev-team`: non-numeric middle → parse fails → ignored.
/// - `wg--dev-team`:    empty middle (`[3..3]` slice) → parse fails → ignored.
/// Only `wg-2-dev-team` is real, so slot 1 is still free.
/// (The `wg-dev-team` no-number case is covered by its own test below
/// because it specifically exercises the checked-slicing guard.)
#[test]
fn determine_next_wg_number_ignores_invalid_directory_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-abc-dev-team");
    touch_dir(tmp.path(), "wg--dev-team");
    touch_dir(tmp.path(), "wg-2-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 1);
}

/// `wg-0-<team>` does not block slot 1. The allocator's lowest-free
/// search starts at 1, so any `0` that ends up in `taken` is never
/// tested by `find` — slot 1 stays reachable. The allocator only ever
/// produces values ≥ 1.
#[test]
fn determine_next_wg_number_ignores_zero_numbered_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-0-dev-team");
    touch_dir(tmp.path(), "wg-2-dev-team");
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 1);
}

/// Files (not directories) named like a workgroup must not occupy a slot —
/// the allocator only considers real workgroup directories.
#[test]
fn determine_next_wg_number_ignores_files_named_like_workgroups() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("wg-1-dev-team"), b"not a dir")
        .expect("write file");
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 1);
}

/// Regression for the suffix-overlaps-prefix slice case: a directory
/// named `wg-{team}` (no number, e.g. `wg-dev-team`) passes both the
/// `starts_with("wg-")` and `ends_with("-{team}")` checks, but the
/// digits slice would be `&name_str[3..2]` — invalid. With `&str[..]`
/// indexing this panics; with `name_str.get(..)` it returns `None` and
/// the entry is silently ignored. This test locks in the no-panic
/// behavior so a future refactor cannot reintroduce the bug.
#[test]
fn determine_next_wg_number_does_not_panic_on_no_number_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-dev-team");
    // Must return slot 1 (the bogus dir is ignored, not counted as taken).
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 1);
}

/// In-flight `.deleting-wg-N-team-<uuid>` directories must NOT be
/// counted as occupying slot N. Locks the contract that #177 relies
/// on: the leading `.` of the temp name (set in `try_atomic_delete_wg`
/// at line 1535 — `.deleting-{wg_name}-{uuid}`) dodges the
/// `starts_with("wg-")` filter, so a freed slot is reusable on the
/// very next allocation tick. A future temp-name refactor that drops
/// the leading `.` would silently re-introduce the gap-leak this issue
/// closes; this test catches that regression.
#[test]
fn determine_next_wg_number_ignores_deleting_temp_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(
        tmp.path(),
        ".deleting-wg-2-dev-team-00000000-0000-0000-0000-000000000000",
    );
    // wg-2 is mid-delete: the `.deleting-…` entry must not block slot 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 2);
}

/// Team `team` is a strict suffix of team `dev-team`. The dir
/// `wg-1-dev-team` ends with `-team` but must NOT count toward team
/// `team` — its middle `1-dev` fails `parse::<u32>()` and is ignored.
/// Test 5 only covered non-overlapping team names; this case locks the
/// suffix-overlap disambiguation that edge case §2 argues for. A future
/// maintainer who relaxed parsing (hex, leading `+`, trailing-char
/// stripping) would silently reintroduce cross-team contamination — and
/// none of the existing tests would catch it.
#[test]
fn determine_next_wg_number_distinguishes_subset_team_suffixes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(tmp.path(), "wg-1-team");
    // For team `team`: only `wg-1-team` counts; `wg-1-dev-team`'s
    // middle `1-dev` is non-numeric and is ignored. Next free is 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "team"), 2);
    // For team `dev-team`: only `wg-1-dev-team` counts. Next free is 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 2);
}
```

---

## Edge cases & invariants

1. **TOCTOU between allocator and `create_dir_all`** — there is a real
   race between the `read_dir` scan in `determine_next_wg_number` and the
   `create_dir_all` call in `create_workgroup` (line 602). The post-allocate
   `wg_dir.exists()` check at line 599 catches *sequential* repeats but
   does NOT catch two concurrent `create_workgroup` invocations:
   `std::fs::create_dir_all` returns `Ok(())` when every component already
   exists as a directory (per std contract), so the loser of the race
   silently succeeds and both callers proceed past line 602 to write
   `BRIEF.md`, clone repos, and set up `__agent_*` directories on the same
   `wg-N-team` path. This race is **pre-existing** in the OLD `max+1`
   allocator. The new allocator does not regress it, but gap reuse does
   increase the practical hit rate (a freshly-deleted slot becomes
   reusable on the very next allocation tick). Adding serialisation, or
   switching the leaf creation from `std::fs::create_dir_all` to
   `std::fs::create_dir` (which errors with `AlreadyExists` on a
   pre-existing dir) is **out of scope** for #177 — this issue is
   allocator-policy-only, and the race fix has its own review surface
   (concurrent caller semantics, downstream writes by the loser, error
   mapping). A separate issue should be filed to track race hardening.

2. **Team-suffix ambiguity** — `sanitize_name` (line 90) normalizes team names
   to `[a-z0-9-]+` with no leading/trailing/consecutive hyphens, so suffix
   matching with `ends_with("-{team_name}")` cannot collide across teams whose
   names are not equal. The disambiguation hinges on the *combination* of
   `ends_with("-{team_name}")` plus the `parse::<u32>()` step on the middle
   slice: a non-numeric middle is silently ignored. Test 5
   (`determine_next_wg_number_only_considers_matching_team_suffix`) covers the
   non-overlapping team case; test 11
   (`determine_next_wg_number_distinguishes_subset_team_suffixes`, added in
   round 2) covers the strict-suffix case where one team name is a suffix of
   another (e.g. `team` vs `dev-team`) — that is the case where the
   parse-step disambiguation actually does work.

3. **Non-UTF-8 directory names** — preserved behavior: `to_string_lossy()`
   replaces invalid bytes with U+FFFD which then fails the prefix/suffix +
   parse pipeline, so such entries are silently ignored. No change required.

4. **Allocator never produces 0** — the search range starts at 1, so slot 1
   is always reachable regardless of `taken`. A stray `wg-0-{team}` directory
   ends up as a `0` entry in `taken`, but `find` never tests `0`, so the
   entry cannot block slot 1. Test 7
   (`determine_next_wg_number_ignores_zero_numbered_dir`) locks this in.

---

## Verification

Run from the repo root (`repo-AgentsCommander`):

```bash
cd src-tauri
cargo test --lib commands::entity_creation::tests::determine_next_wg_number
```

To also re-run the existing `try_atomic_delete_wg_*` tests that share the
module (sanity check that the new tests didn't break the surrounding module):

```bash
cargo test --lib commands::entity_creation::tests
```

Expected: all **11** new tests pass on Windows (the dev's primary target) and
Linux/macOS. The new tests do **not** depend on Windows-specific APIs, so they
run cross-platform unlike the `#[cfg(windows)]` `try_atomic_delete_wg_blocked_*`
test.

A `cargo build` is not required separately — `cargo test` exercises the build.

---

## Out of scope (do NOT do)

- Do not change the `wg-<N>-<team>` naming scheme, the regex/parse rules, or
  the `team_name` sanitization.
- Do not refactor `create_workgroup` (line 558+). The TOCTOU guard at line 599
  must remain.
- Do not change the suffix-based filter (`name_str.ends_with(&suffix)`) — it
  is the only thing that scopes the allocator per team.
- Do not introduce `BTreeSet`, sorted vectors, or any helper module — the
  HashSet replacement is small enough to live inline.
- Do not add new crates. `HashSet` is already imported (line 2). `tempfile` is
  already a dev-dependency (`Cargo.toml` line 47).
- Do not bump `tauri.conf.json` version in this plan — version bumps are part
  of the build/release flow handled separately by the user's release process.

---

## Verdict

**READY_FOR_IMPLEMENTATION** (round-2 architect resolution applied).

The change is a single-function body replacement plus a contained block of
regression tests, all inside one file. The behavioral spec is fully captured
by the 11-test matrix (empty, contiguous, internal gap, leading gap, team
scoping, invalid names, zero-numbered, file-not-dir, **no-number-no-panic**,
**.deleting-temp-dir lockout**, **subset-team-suffix disambiguation**). All
six grinch findings (G1 filter rationale, G2 TOCTOU claim, G3 deleting-temp
test, G4 subset-suffix test, G5 banner style, G6 read_dir docstring) are
folded into the body above. The `tauri.conf.json` version-bump question was
resolved by tech-lead: do NOT bump in this commit. A dev should be able to
apply this plan as written without further clarification.

---

## Grinch Review (2026-05-07)

**Outcome: NOT READY — return to architect.** Dev-rust's enrichment correctly
closes the latent panic (`.get(..)` + test 9 — confirmed sound: `str::get`
with `start > end` returns `None`, not panic, per std contract). Two
documentation defects and four missing test/spec items remain. None block
the *behavioural* core of #177, but #177 is a small change so the bar for
correctness of its surrounding documentation and test coverage is high.

### Finding G1 — `n > 0` filter rationale is factually wrong (BLOCKER for documentation)

**What.** The "Why this exact shape" section justifies the `if n > 0` guard
as: *"Without this filter, the presence of a stray `wg-0-<team>` (manual
creation, prior buggy state) would make the allocator skip 1."* Edge case
§4 repeats the same claim: *"Combined with the `n > 0` filter when
populating `taken`, slot 1 is always reachable regardless of any
pre-existing `wg-0-*` directory."*

Both are false. Trace:
- `taken = {0}` (filter removed): `(1u32..=u32::MAX).find(|n| !taken.contains(n))` checks `1` first; `!taken.contains(&1)` is `true`; returns `1`. The `0` in `taken` is never tested.
- `taken = {0, 1}` (filter removed): checks `1` (in taken, skip), checks `2` (not in taken), returns `2`.
- Same results with the filter present (`taken = {}` and `taken = {1}` respectively).

The filter has zero effect on the function's result. It is a true no-op. The
test `determine_next_wg_number_ignores_zero_numbered_dir` would pass
identically with or without the filter — so it doesn't pin anything either.

**Why.** Misleading rationale poisons future refactors. A maintainer who
reads "the `n > 0` filter is what keeps slot 1 reachable" may, when later
extending the function (e.g. to support a `start_at` parameter, or to share
the `taken` set with another allocator), preserve the filter as load-bearing
and miss real changes that *would* affect slot-1 reachability. Worse, the
filter's presence will be cited as "the reason" any subsequent slot-1 bug
report doesn't reproduce — sending debugging effort the wrong direction.

**Fix.** Two acceptable resolutions, dev's choice:

(a) **Drop the filter.** It is a no-op; the simpler code is easier to
reason about. Edit Change 1 to remove lines 161–163 and adjust the docstring
to remove the "wg-0 is ignored" bullet. Test 7 still passes (verified by
trace above).

(b) **Keep the filter, fix the rationale.** Replace the "Why this exact
shape — Filter `n > 0`" bullet with:

> *Filter `n > 0`: cosmetic. The search range is `1u32..=u32::MAX` so a
> `0` in `taken` is never tested and the filter does not change the
> function's return value. Excluding `n == 0` keeps `taken` ⊂
> `{1u32..=u32::MAX}` — the set of values the allocator can actually emit
> — which makes the loop invariant trivially true and avoids confusing a
> future reader who sees a `0` entry and wonders if it is load-bearing.*

And replace edge case §4 with: *"Allocator never produces 0 — the search
range starts at 1, so slot 1 is always reachable regardless of `taken`."*
Drop the "Combined with the `n > 0` filter" clause.

Recommendation: **(a)**. The filter is four lines of defensiveness against
a no-op; the smaller code is the better code.

### Finding G2 — TOCTOU edge case §1 still claims a safety the code does not deliver (BLOCKER for documentation)

**What.** Edge case §1 reads:

> *"another caller could create the same `wg-N` dir. The defensive `if
> wg_dir.exists()` check at line 599 already catches this and returns
> `Err`."*

This is incomplete in the worst case. The race window:

```text
T0: A: determine_next_wg_number → 2
T1: A: wg_dir.exists() → false
T2: B: determine_next_wg_number → 2          (race window)
T3: B: wg_dir.exists() → false               (still empty on disk)
T4: A: std::fs::create_dir_all(&wg_dir)      → Ok, dir now created
T5: B: std::fs::create_dir_all(&wg_dir)      → Ok (NOT Err) because the
                                                std contract for create_dir_all
                                                is "Ok(()) if every component
                                                already exists as a directory"
T6: A & B both proceed past line 602, racing on BRIEF.md, __agent_*/,
    repo clones, sweep_lock, etc.
```

`std::fs::create_dir_all` is the union of `create_dir` and "succeed if
already exists as a dir." The post-allocate `exists()` guard at line 599
catches sequential repeats but **does not** catch two interleaved
`create_workgroup` invocations.

The new allocator does not regress this race (the OLD `max+1` allocator has
the same shape), and it is true that #177 is allocator-policy-only, but the
plan as written reassures the reviewer with a false claim of safety.
Gap-reuse also makes the race practically more visible: a freshly-deleted
slot is reusable on the next tick, so a delete-then-create burst now hits
the race more often than the old max+1 policy did.

**Why.** A tech lead approving this plan on the strength of "the existing
guard already catches this" will sign off on a system that is not
race-safe, with the false belief that it is. When the race actually fires
(symptoms: two BRIEF.md collisions, half-cloned repos in one wg, or
duplicate `__agent_*` setup), debugging starts from "but the plan said
this case was handled" and goes in the wrong direction.

**Fix.** Two acceptable resolutions:

(a) **Documentation-only.** Rewrite §1 to be honest:

> *"There is a TOCTOU race between `read_dir` in the allocator and
> `create_dir_all` in `create_workgroup`. The post-allocate
> `wg_dir.exists()` check at line 599 catches sequential repeats but
> NOT two concurrent `create_workgroup` invocations: `create_dir_all`
> returns `Ok(())` when every component already exists as a directory
> (per std contract), so the loser of the race silently succeeds and
> both callers proceed to populate the same `wg-N-team` directory.
> The new allocator does not change this race — it is pre-existing in
> the OLD `max+1` policy too. Gap reuse does increase the practical
> hit-rate (a freshly-deleted slot becomes reusable on the next tick).
> Adding serialisation is tracked separately and is out of scope for
> #177."*

(b) **Two-character code fix.** Replace line 602 (`std::fs::create_dir_all(&wg_dir)`)
with `std::fs::create_dir(&wg_dir)`. `create_dir` (without `_all`)
errors with `AlreadyExists` if the dir already exists, closing the race
for concurrent calls — each loser bails cleanly with that error. The
`.ac-new/` parent already exists (validated at lines 570–572), so no
recursive creation is needed at the leaf.

Recommendation: **(a)** if the team wants #177 to stay strictly
allocator-only. **(b)** if a one-line, surgically scoped race fix is
acceptable in the same patch — it is functionally adjacent to the
allocator change, would be reviewed by the same pair of eyes, and
converts the race from "silent corruption" to "loud error." Either
way, the current §1 wording must change because it is a false safety
claim.

### Finding G3 — Missing test: in-flight `.deleting-wg-N-team-<uuid>` does not occupy slot N (LOCK-IN)

**What.** The `try_atomic_delete_wg` helper (line 1516) renames `wg-N-team`
to `.deleting-{wg-N-team}-{uuid}` *before* the physical removal. Between
the rename and the removal, `read_dir` returns an entry whose name starts
with `.deleting-`. The `starts_with("wg-")` filter in
`determine_next_wg_number` happens to exclude it because of the leading
`.`, so the freed slot N is correctly recognised as free.

But no test pins this. If a future refactor changes the temp-name prefix
(e.g. drops the leading dot to make orphans easier to spot in `ls`, or
matches it case-insensitively), the allocator would silently start
treating in-flight deletes as occupied slots and skip past them — exactly
the gap-leak behaviour #177 closes.

**Why.** The whole point of #177 is to reuse freed slots; the freeing event
is exactly the rename. A regression here re-creates the bug this issue
closes, and it would slip past every test currently in the plan.

**Fix.** Add this test to Change 2:

```rust
/// In-flight `.deleting-wg-N-team-<uuid>` directories must NOT be
/// counted as occupying slot N. Locks the contract that #177 relies
/// on the leading `.` of the temp name to dodge the wg- filter (see
/// `try_atomic_delete_wg` at line 1516, which renames before remove).
#[test]
fn determine_next_wg_number_ignores_deleting_temp_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(
        tmp.path(),
        ".deleting-wg-2-dev-team-00000000-0000-0000-0000-000000000000",
    );
    // wg-2 is mid-delete: the `.deleting-…` entry must not block slot 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 2);
}
```

### Finding G4 — Missing test: subset team-name suffixes (LOCK-IN)

**What.** Edge case §2 argues that suffix matching cannot cross-contaminate
teams because of the sanitised name shape and the `parse::<u32>()` step.
The argument is correct, but it relies on the *combination* of `ends_with`
+ parse — and the proposed test 4
(`determine_next_wg_number_only_considers_matching_team_suffix`) only
covers two **non-overlapping** team names (`dev-team` vs `qa-team`), where
`ends_with` alone disambiguates. The case the argument actually hinges on
— one team name being a strict suffix of another — is not tested.

**Why.** A future maintainer who relaxes parsing (e.g. allows hex, strips
trailing chars before parsing, accepts a leading `+`) would silently
reintroduce cross-team contamination. None of the existing tests would
catch it because none of them set up suffix-overlapping team names.

**Fix.** Add this test to Change 2:

```rust
/// Team `team` is a strict suffix of team `dev-team`. The dir
/// `wg-1-dev-team` ends with `-team` but must NOT count toward team
/// `team` — its middle `1-dev` fails `parse::<u32>()` and is ignored.
/// This locks the suffix-overlap disambiguation that edge case §2
/// argues for.
#[test]
fn determine_next_wg_number_distinguishes_subset_team_suffixes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    touch_dir(tmp.path(), "wg-1-dev-team");
    touch_dir(tmp.path(), "wg-1-team");
    // For team `team`: only `wg-1-team` counts; `wg-1-dev-team`'s
    // middle `1-dev` is non-numeric and is ignored. Next free is 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "team"), 2);
    // For team `dev-team`: only `wg-1-dev-team` counts. Next free is 2.
    assert_eq!(determine_next_wg_number(tmp.path(), "dev-team"), 2);
}
```

### Finding G5 — Test-block style drift from surrounding tests (cosmetic)

**What.** The existing `mod tests` uses `// ── <section> ──` separators
(line 1969 `// ── parse_brief_title — dev-rust R7 cases ──`, line 2031
`// ── parse_brief_title — dev-rust-grinch G3 / G13 case-insensitivity ──`,
line 2067 `// ── parse_brief_title — UTF-8 BOM (grinch MEDIUM) ──`). The
plan's new block uses `// ----...` rules and `// #177 — …` titles instead.

**Why.** Diff/PR noise; future readers grep `// ──` to navigate sections.
Easier to spot one ruler in a green-field block than to fix it later.

**Fix.** Replace the proposed banner

```rust
// ---------------------------------------------------------------------
// #177 — `determine_next_wg_number` reuses the lowest free integer.
// ---------------------------------------------------------------------
```

with the existing convention:

```rust
// ── #177 — determine_next_wg_number lowest-free reuse ──
```

### Finding G6 — `read_dir` failure silently degrades to slot 1 (PRE-EXISTING — acknowledge in the plan)

**What.** Both the OLD and the NEW function bodies use
`if let Ok(entries) = std::fs::read_dir(ac_new_dir)` and silently treat
any read error (permission denied, transient I/O, broken junction,
ERROR_PATH_NOT_FOUND on a deep `.ac-new`) as "no entries" — the function
returns `1`. The user-visible failure is then either
*"Workgroup directory already exists"* (caught by line 599 if a
`wg-1-team` is present) or a corrupted second creation (if no
`wg-1-team` is present). The original cause is never surfaced.

**Why.** Pre-existing behaviour; not introduced by this plan. But the
plan's claim that the function is *"fully captured by the test matrix"*
is not quite right — none of the tests exercise the read-error branch,
and the docstring doesn't acknowledge it either.

**Fix.** No code change required for #177. Append one bullet to the
function docstring:

> *"If `read_dir` fails (permission denied, transient I/O, path-too-long
> on Windows), the function returns `1` as a graceful degradation. The
> post-allocate `wg_dir.exists()` guard in `create_workgroup` will
> surface the real condition as `AlreadyExists` if a `wg-1-{team}` is
> in fact present; otherwise the slot-1 creation will succeed with
> stale state. Logging the read error here is tracked separately."*

This is a documentation-only nudge; the dev does not need to change
behaviour to land #177.

---

### Summary for the architect

- **G1** and **G2** are documentation defects in the plan's own narrative
  that misdirect future maintainers. Pick option (a) or (b) for each
  before sending to dev.
- **G3** and **G4** are missing regression tests that lock the contracts
  #177 already depends on — both should be added to Change 2.
- **G5** is a cosmetic style nit; trivial to fold in while editing
  Change 2.
- **G6** is a docstring acknowledgement of pre-existing degraded
  behaviour; no code change.

Sanity-check on dev-rust's enrichment that I did want to flag explicitly:
the `.get(..)` substitution **is** the right primitive. `str::get` with a
range whose `start > end` returns `None` (per std contract); and a
`name_str.len() < suffix.len()` case is unreachable here because
`ends_with(&suffix)` already gates entry into the block. Test 9
(`determine_next_wg_number_does_not_panic_on_no_number_dir`) covers the
exact panic shape. No further code change needed for the panic itself.

Recommended next step: architect amends the plan to (a) resolve G1 and
G2 with chosen wording (or one-line code change for G2), (b) append the
G3 and G4 tests verbatim to Change 2, (c) fix the G5 banner style, and
(d) add the G6 docstring bullet — then re-issue.
