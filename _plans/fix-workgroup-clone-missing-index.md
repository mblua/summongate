# Plan: Fix workgroup clone missing .git/index

**Branch:** `fix/workgroup-clone-missing-index`
**File:** `src-tauri/src/commands/entity_creation.rs`
**Function:** `git_clone_async()` (lines 1205-1235)

---

## 1. Root Cause Analysis

### Observed symptoms
- `.git/` has HEAD, config, objects/, refs/, packed-refs, shallow — but **no index file**
- **Zero** working tree files on disk
- `git ls-tree HEAD` shows all 7,548 files correctly (object database intact)
- `git status` shows 7,548 staged deletions
- `git reflog` shows only `clone: from <url>` — no post-clone operations
- Exit code was 0 (function returned `Ok(())`)

### What this tells us
Git clone has two phases: **(a) fetch objects** and **(b) checkout working tree**. Phase (a) completed perfectly (objects, refs, HEAD all present). Phase (b) never executed or silently failed — no index was written, no files were checked out, yet git reported success (exit code 0).

### Probable causes (ordered by likelihood)

1. **`CREATE_NO_WINDOW` + console I/O conflict (HIGH)**
   On Windows, `CREATE_NO_WINDOW` (0x08000000) prevents a console from being allocated. Git's checkout phase on Windows can use console handles for progress display and CTRL+C handling. Without a console, the checkout phase may fail silently while the fetch phase (which uses network I/O, not console I/O) succeeds. Other `CREATE_NO_WINDOW` usages in this codebase (`git status`, `git log`, `git rev-parse`) are read-only operations that don't have this issue.

2. **Antivirus file locking during checkout (MEDIUM)**
   Windows Defender real-time scanning locks newly created files. For a 7,548-file checkout, index creation could fail if the AV holds a lock on `.git/index` during writes. This is intermittent and machine-dependent.

3. **Shallow clone edge case (LOW)**
   `--depth 1` creates a shallow clone with a single commit. Some git versions have had bugs where shallow clones on Windows skip checkout under specific conditions. Less likely since `--depth 1` is widely used.

4. **Long path exceeding MAX_PATH (LOW)**
   Workgroup paths are deep (`.ac-new/wg-N-team/repo-Name/...`). If any checked-out file exceeds 260 chars, git might abort checkout. However, this would typically produce an error, not a silent skip of ALL files.

### Investigation approach in the fix
Add detailed logging of git's stderr after clone to capture any checkout warnings that are currently discarded. Log the exit code explicitly. This data will confirm the cause definitively.

---

## 2. Fix Approach for `git_clone_async()`

### 2a. Capture and log stderr even on success

Currently, stderr is only examined on failure. Git writes checkout warnings and progress to stderr. We need this data to diagnose silent failures.

**Change:** After a successful clone, log stderr at `info` level (or `warn` if non-empty). Cap at 1024 bytes to avoid log bloat.

### 2b. Add post-clone validation

After `git clone` returns success, validate the result before returning `Ok(())`:

```rust
// After successful clone, validate working tree
let git_dir = target.join(".git");
let index_path = git_dir.join("index");

if !index_path.exists() {
    log::warn!("[git_clone_async] Clone succeeded but .git/index missing — running recovery checkout");
    // Recovery: run git checkout
    run_git_checkout_recovery(target).await?;
}

// Final validation: index must exist
if !index_path.exists() {
    return Err(format!(
        "git clone produced incomplete repo at {}: .git/index missing after recovery attempt",
        target.display()
    ));
}
```

### 2c. Recovery function: `run_git_checkout_recovery()`

New async helper that runs `git checkout HEAD -- .` inside the cloned repo to regenerate the index and working tree from the object database:

```rust
async fn run_git_checkout_recovery(repo_path: &Path) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["checkout", "HEAD", "--", "."])
        .current_dir(repo_path);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("Failed to spawn git checkout recovery: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git checkout recovery failed: {}", stderr.trim()));
    }

    log::info!("[git_clone_async] Recovery checkout succeeded at {}", repo_path.display());
    Ok(())
}
```

**Why `git checkout HEAD -- .` instead of `git reset --hard`?**
- `git checkout HEAD -- .` regenerates the index AND working tree from the HEAD commit
- `git reset --hard` also works but moves the branch ref — unnecessary since HEAD is already correct
- Both are destructive to working tree (acceptable here since this is a fresh clone)

### 2d. Enhanced stderr capture

Replace the current `cmd.output()` with explicit pipe setup to ensure we capture stderr fully, even on success:

```rust
let output = cmd
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .output()
    .await
    .map_err(|e| format!("Failed to spawn git clone: {}", e))?;

// Log stderr regardless of exit code (git writes progress + warnings there)
let stderr = String::from_utf8_lossy(&output.stderr);
if !stderr.trim().is_empty() {
    log::info!("[git_clone_async] git clone stderr for {}: {}", target.display(), &stderr[..stderr.len().min(1024)]);
}
```

---

## 3. Post-Clone Validation Logic

The validation runs **inside `git_clone_async()`** after a successful clone, so the caller (`create_workgroup`) doesn't need changes.

### Validation checklist (sequential):

| Check | How | On failure |
|---|---|---|
| `.git/index` exists | `target.join(".git/index").exists()` | Run recovery checkout |
| Working tree has files | `std::fs::read_dir(target)` has >1 entry (`.git` + at least one file) | Run recovery checkout |
| Recovery index exists | `target.join(".git/index").exists()` after recovery | Return `Err()` — hard failure |

### Why not check `git status --porcelain`?
Running `git status` on a repo with 7,548 deleted files is expensive. File existence checks are O(1) and sufficient to detect this specific failure mode.

---

## 4. Recovery Command for Already-Affected Repos

### Do we need a separate Tauri command?

**Yes.** Existing workgroups with broken repos need a repair mechanism without re-cloning. Two options:

### Option A: Targeted repair command (RECOMMENDED)

Add a new Tauri command `repair_workgroup_repos` that:
1. Scans all `repo-*` dirs in a workgroup
2. For each: checks if `.git/index` exists
3. If missing: runs `git checkout HEAD -- .`
4. Returns a list of repaired repos and any failures

```rust
#[tauri::command]
pub async fn repair_workgroup_repos(wg_path: String) -> Result<Vec<RepairResult>, String> {
    // ... scan and repair logic
}
```

### Option B: Manual instruction

Document that affected repos can be fixed with:
```bash
cd <repo-path>
git checkout HEAD -- .
```

### Recommendation: **Option A** — it integrates with the UI and can be triggered from the workgroup context menu. But Option B costs zero dev time if urgency is high.

---

## 5. Testing Strategy

### 5a. Unit validation (Rust)

The fix is in async code calling external processes — traditional unit tests are impractical. Instead:

1. **Manual test:** Create a workgroup via the UI, verify:
   - `.git/index` exists in every `repo-*` dir
   - `git status` in each repo shows clean working tree
   - Files are present on disk

2. **Forced failure test:** Temporarily add code to delete `.git/index` after clone and verify the recovery path triggers and succeeds.

### 5b. Log verification

After the fix, create a workgroup and check Tauri logs for:
- `[git_clone_async] git clone stderr for ...` — shows what git reported
- No `[git_clone_async] Clone succeeded but .git/index missing` — if this appears, recovery triggered (which means the root cause is still present but now handled)

### 5c. Edge cases to verify

| Scenario | Expected |
|---|---|
| Normal clone (happy path) | Index exists, no recovery triggered, clean working tree |
| Clone of large repo (~8k files) | Same as above, recovery not needed |
| Clone with long workgroup path | Works or produces clear error |
| Network failure during clone | Returns `Err()` as before (no behavior change) |
| Recovery checkout fails | Returns `Err()` with clear message including path |

### 5d. Regression check

The existing `check_workgroup_repos_dirty()` function (lines 1015-1103) already runs `git status --porcelain` on repo-* dirs. After the fix, these should all show clean status for freshly cloned repos.

---

## 6. Implementation Sequence

1. **Modify `git_clone_async()`**: Add stderr logging on success
2. **Add `run_git_checkout_recovery()`**: New helper function
3. **Add post-clone validation**: Index existence check + recovery trigger
4. **Add `repair_workgroup_repos` command**: For already-affected repos (can be Phase 2 if needed)
5. **Register new command**: In Tauri command handlers if repair command is added
6. **Test**: Create workgroup, verify logs and repo state
7. **Verify**: No regression on clean clone path

### Files to modify
- `src-tauri/src/commands/entity_creation.rs` — main fix (steps 1-4)
- `src-tauri/src/lib.rs` — register new command if repair command is added
- `src-tauri/gen/schemas/*.json` — auto-generated on build if new command added

### Estimated scope
- Core fix (steps 1-3): ~40 lines of new code in `entity_creation.rs`
- Repair command (step 4): ~60 lines additional
- No frontend changes required for the core fix

---

## Dev-Rust Review

### Agreement

The root cause analysis is thorough and well-reasoned. The `CREATE_NO_WINDOW` hypothesis (cause #1) is the most likely. I verified all `CREATE_NO_WINDOW` usages in the codebase — there are 6 total across 4 files. The other 5 are ALL read-only git operations (`git branch --show-current`, `git rev-parse --abbrev-ref HEAD`, `git status --porcelain`, `git log`). Only `git_clone_async` performs write operations (checkout phase), which is where the console handle absence could matter.

The post-clone validation via `.git/index` existence check is the right approach — lightweight, direct, and catches the exact failure mode observed.

The choice of `git checkout HEAD -- .` over `git reset --hard` for recovery is correct.

### Issue: Recovery function re-uses CREATE_NO_WINDOW

**This is the most critical concern.** The proposed `run_git_checkout_recovery()` in section 2c applies `CREATE_NO_WINDOW` to the recovery checkout command. If `CREATE_NO_WINDOW` is truly the root cause (and the plan ranks it as HIGH probability), then the recovery checkout will fail for the **exact same reason** — it performs the same kind of working tree write + index generation that failed during the original clone.

**Recommendation:** The recovery function should either:
- (a) NOT use `CREATE_NO_WINDOW` — accept the brief console window flash as a tradeoff for correctness, OR
- (b) Try with `CREATE_NO_WINDOW` first, and if `.git/index` still doesn't exist after recovery, retry WITHOUT the flag, OR
- (c) Use environment variables to suppress console interaction without `CREATE_NO_WINDOW` (see suggestion below)

If we go with (a), the console flash is acceptable because recovery only triggers on failure, which should be rare.

### Issue: Working tree file count check is unnecessary

Section 3 proposes checking `std::fs::read_dir(target)` for >1 entry alongside the `.git/index` check. This adds complexity without value — a partial checkout could have some files but a corrupt/missing index. The `.git/index` check alone is sufficient and directly tests the observed failure mode. Drop the file count check to keep validation simple.

### Concern: stderr cap inconsistency

The plan proposes capping stderr at 1024 bytes for success logging. The existing error path caps at 512 bytes (line 1230). Use 512 for consistency.

### Suggestion: Add `--progress` flag to clone

`git clone --progress` forces progress output to stderr regardless of terminal detection. Without this flag, git skips progress output when stderr is not a TTY (which it isn't, since `output()` pipes it). Adding `--progress` forces git into a code path that doesn't query the console for terminal capabilities. This might prevent the checkout failure entirely, making the recovery path unnecessary. Worth testing before or alongside the validation approach.

### Suggestion: Set `GIT_TERMINAL_PROMPT=0`

Adding `cmd.env("GIT_TERMINAL_PROMPT", "0")` tells git to never prompt for terminal input. This prevents any internal console queries that might interfere with checkout. Combined with `--progress`, this could eliminate the root cause rather than just recovering from it.

### Note: tokio::process::Command and output()

The plan correctly uses `tokio::process::Command` + `.output().await`. One nuance: `output()` implicitly pipes stdout and stderr (`Stdio::piped()`). The explicit pipe setup shown in section 2d is technically redundant but doesn't hurt — I'd keep it only if we want the code to be self-documenting about intent. Otherwise, `output()` alone is sufficient.

### Note: Repair command can be deferred

Agree with the plan's framing — the repair command (Option A, section 4) is nice-to-have but not needed for the core fix. Affected repos can be fixed manually with `git checkout HEAD -- .` (Option B). Recommend implementing the core fix first, deferring the repair command to a follow-up if the issue recurs.

### Summary

The plan is solid. Two actionable changes before implementation:
1. **Do NOT use `CREATE_NO_WINDOW` in the recovery function** — this would defeat the purpose if it's the root cause
2. **Drop the working tree file count check** — `.git/index` alone is sufficient
3. **Consider adding `--progress` and `GIT_TERMINAL_PROMPT=0`** to the clone command as a preventive measure alongside the validation
