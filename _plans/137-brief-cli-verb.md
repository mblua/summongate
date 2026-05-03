# Plan: CLI verb for coordinators to edit workgroup BRIEF.md (#137)

**Branch:** `feature/137-brief-cli-verb`
**Issue:** https://github.com/mblua/AgentsCommander/issues/137
**Companion (frozen):** #107 on `feature/107-auto-brief-title` — refactors atop this once landed.

---

## 1. Goal & non-goals

### Goal
Provide an authenticated CLI surface that lets a Coordinator agent (running inside its sandbox) request two specific edits to its workgroup's `BRIEF.md` — set the YAML-frontmatter `title:` field, and append text to the body — without granting the agent direct write access to the workgroup root (which would weaken the GOLDEN RULE in `default_context()` at `src-tauri/src/config/session_context.rs:478`). The binary mediates: validates token + coordinator role, takes a lock, writes a timestamped backup, applies the edit atomically, releases the lock.

### Non-goals
This plan does NOT design body-replace, unified-diff patching, frontmatter fields beyond `title:`, per-section/per-line editing, non-coordinator authorisation, retroactive migration of existing `BRIEF.md` files, GUI surface for these operations, or any change to the GOLDEN RULE template. It also does NOT design the #107 PTY-prompt change — that is a one-line refactor handled in #107's branch after this lands (see §10).

---

## 2. Final verb signature(s)

Two **flat** subcommands (matches existing pattern: `send`, `close-session`, `list-peers`, `list-sessions`, `create-agent` — none nest sub-subcommands; one verb-with-subcommands would introduce a new clap pattern unused elsewhere).

### `<bin> brief-set-title`

```
<bin> brief-set-title --token <TOKEN> --root <PATH> --title <TEXT>
```

| Flag        | Required | Type         | Description                                                                 |
|-------------|----------|--------------|-----------------------------------------------------------------------------|
| `--token`   | yes      | `String`     | Session token from `# === Session Credentials ===` block                    |
| `--root`    | yes      | `String`     | Caller's agent root (CWD); used to derive caller FQN and locate workgroup   |
| `--title`   | yes      | `String`     | New title text. Single string argument, single-quoted YAML escape on write  |

**Stdout on success (exit 0):** `BRIEF.md title updated; backup: <abs path>` or, when no prior file existed, `BRIEF.md created; no prior content to back up`.
**Stderr on error (exit 1):** prefixed `Error: <message>` (see error matrix in §3).

### `<bin> brief-append-body`

```
<bin> brief-append-body --token <TOKEN> --root <PATH> --text <TEXT>
```

| Flag        | Required | Type         | Description                                                                 |
|-------------|----------|--------------|-----------------------------------------------------------------------------|
| `--token`   | yes      | `String`     | Session token from `# === Session Credentials ===` block                    |
| `--root`    | yes      | `String`     | Caller's agent root (CWD); used to derive caller FQN and locate workgroup   |
| `--text`    | yes      | `String`     | Body text to append. Newline-normalised per §6.                             |

**Stdout on success (exit 0):** `BRIEF.md body appended; backup: <abs path>` or, when no prior file existed, `BRIEF.md created; no prior content to back up`.
**Stderr on error (exit 1):** as above.

### Why two verbs (not `brief set --title` / `brief append`)
- All existing AC subcommands are flat (`cli/mod.rs:27-38`). Introducing a nested `Subcommand` for two operations is gratuitous complexity.
- Two flat verbs are self-discoverable in `<bin> --help` without an extra hop into `<bin> brief --help`.
- Naming follows the existing `verb-noun` style (`close-session`, `list-peers`, `create-agent`); the "what" (set-title / append-body) is in the verb itself, not a positional discriminator. This is also kinder to the #107 PTY-prompt template — a single literal verb to instruct (no nested-subcommand confusion for the agent).

### Exit codes
`0` on success. `1` on every error (matches every other CLI subcommand — see `cli/send.rs`, `cli/close_session.rs`, etc.).

### Error matrix (exact strings, all stderr, all exit 1)

| Cause                                                                                                          | Exact error string                                                                                                                                              |
|----------------------------------------------------------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `--root` missing                                                                                               | `Error: --root is required. Specify your agent's root directory.`                                                                                                |
| `--token` missing/empty                                                                                        | (delegate to `validate_cli_token` — same string as `send`/`close-session`)                                                                                       |
| `--token` invalid (not UUID, not root/master)                                                                  | (delegate to `validate_cli_token` — same string)                                                                                                                 |
| `--title` empty (after trim) for `brief-set-title`                                                             | `Error: --title cannot be empty.`                                                                                                                                |
| `--text` empty (after trim) for `brief-append-body`                                                            | `Error: --text cannot be empty.`                                                                                                                                 |
| `--title` contains a control character other than `\t` (incl. `\n`/`\r`)                                       | `Error: --title must be a single line of printable characters (control characters other than tab are not allowed).`                                              |
| `--text` contains a control character other than `\n`/`\r`/`\t`                                                | `Error: --text contains a control character that is not allowed (only newline, carriage return, and tab are permitted).`                                         |
| `--root` not under any `wg-<N>-*` ancestor                                                                     | `Error: --root is not under a wg-<N>-* ancestor; cannot locate the workgroup BRIEF.md.`                                                                          |
| Caller is not a coordinator of any team                                                                        | `Error: authorization denied — '<sender_fqn>' is not a coordinator of any team. Only coordinators can edit BRIEF.md.`                                            |
| Lock acquisition timeout (5 s)                                                                                 | `Error: BRIEF.md is locked by another writer (5s timeout). Try again.`                                                                                            |
| Lock-file create fails for any reason other than `AlreadyExists` (read-only FS, ENOENT on parent, ACL denial…) | `Error: failed to acquire BRIEF.md lock at <path>: <io::Error>. Aborting; BRIEF.md left unchanged.`                                                              |
| Read of existing BRIEF.md fails (non-NotFound IO error)                                                        | `Error: failed to read BRIEF.md at <path>: <io::Error>`                                                                                                          |
| Backup `copy` fails                                                                                            | `Error: failed to write backup at <path>: <io::Error>. Aborting; BRIEF.md left unchanged.`                                                                       |
| Backup-collision suffix loop exhausted (≥100 collisions in one second — cf. §B.2)                              | `Error: failed to write backup at <path>: 100 collision retries exhausted in the same second. Aborting; BRIEF.md left unchanged.`                                |
| Tmp-file write fails                                                                                           | `Error: failed to write <absolute-tmp-path>: <io::Error>. Aborting; BRIEF.md left unchanged.` (the path is `<wg_root>/BRIEF.md.tmp.<pid>` rendered absolute by `PathBuf::display`) |
| External writer modified BRIEF.md between read and rename (size+mtime sentinel — cf. §HIGH-4 in §7)            | `Error: BRIEF.md was modified externally between read and write; aborting. Backup at <path> retains the externally-modified state.`                              |
| Atomic rename fails (after 3-attempt retry on Windows AV/Explorer transient holds — cf. §MED-4 in §7)          | `Error: failed to publish BRIEF.md (rename): <io::Error>. Backup at <path> retains the prior state.`                                                              |

---

## 3. Auth flow

Mirrors `cli/close_session.rs::execute` lines 41–101 (single-source-of-truth for the token+root+coordinator pattern). No new auth primitives.

```text
fn execute(args) -> i32:
    # 1. --root presence
    let root = args.root.ok_or(eprintln "Error: --root is required..."; return 1)

    # 2. token validation (existing helper; same error strings as send/close-session)
    let (token, is_root_token) = match crate::cli::validate_cli_token(&args.token):
        Ok(pair)  => pair
        Err(msg)  => eprintln msg; return 1

    # 3. derive sender FQN from caller's --root (project-aware, WG-aware)
    #    e.g. "<project>:wg-19-dev-team/architect"
    let sender = crate::cli::send::agent_name_from_root(&root)
                   # which delegates to config::teams::agent_fqn_from_path

    # 4. operation-specific arg validation
    #    4a. non-empty after trim
    if args.title.trim().is_empty():
        eprintln "Error: --title cannot be empty."; return 1
    # (analogous for --text in brief-append-body)
    #
    #    4b. control-character rejection (defends against shell-delivered LFs and
    #        invisible bytes that would silently break YAML / parser invariants —
    #        see §D.2 + LOW-1):
    #          --title:  reject any c.is_control() && c != '\t'
    #                    (covers \n, \r, NUL, \x01-\x1F except \t — title is single-line)
    #          --text:   reject any c.is_control() && c != '\n' && c != '\r' && c != '\t'
    #                    (allows multi-paragraph body text per §6; rejects only the
    #                     "invisible byte" class — NUL, \x01-\x08, \x0b-\x0c, \x0e-\x1f)
    #        Use the exact strings from §3 error matrix.

    # 5. coordinator gate (skipped for root/master token; mirrors close_session.rs:89-101)
    if !is_root_token:
        let discovered = crate::config::teams::discover_teams()
        if discovered.is_empty()
           || !crate::config::teams::is_any_coordinator(&sender, &discovered):
            eprintln "Error: authorization denied — '{sender}' is not a coordinator of any team. \
                      Only coordinators can edit BRIEF.md."
            return 1

    # 6. locate workgroup root (same primitive used by `send --send`)
    let wg_root = match crate::phone::messaging::workgroup_root(Path::new(&root)):
        Ok(p)  => p
        Err(_) => eprintln "Error: --root is not under a wg-<N>-* ancestor; \
                            cannot locate the workgroup BRIEF.md."; return 1

    # 7. hand off to brief_ops::apply_*  (see §4)
```

### Why `is_any_coordinator` (and not `is_coordinator_of(sender, target_agent, ...)`)
The verb has no target *agent* — it edits a workgroup's file. The natural narrowing is: "is the sender a coordinator of any team?" + "is the file inside the sender's own workgroup?". The second clause is enforced **structurally** by `workgroup_root(--root)` — the file we touch is, by construction, inside the WG that contains the caller's `--root`. A coordinator-of-WG-1 with `--root` honestly pointing at their own WG-1 dir cannot reach WG-19's `BRIEF.md` because `workgroup_root` walks up from THEIR root.

The trust boundary on the caller honestly reporting `--root` is identical to the one `close-session` already accepts (`close_session.rs:59` derives `sender` purely from `--root`); we inherit it without amplification. The GOLDEN RULE keeps a malicious agent out of another agent's session credentials, which is the upstream guarantee.

### Project-strict matters
`is_any_coordinator` calls into `is_coordinator` (`config/teams.rs:403`) which is **§AR2-strict** for WG-aware matches: an unqualified agent name CANNOT hold coordinator authority, and cross-project coordinator flags never leak. `agent_fqn_from_path` always returns a project-qualified FQN for a WG replica, so the strict path is exercised. No regression vs. close-session.

---

## 3a. Inherited weakness (escalated for #137)

> **Status:** acknowledged, accepted for #137 scope, follow-up issue recommended. See §"Round 2 — Architect Resolution" for the full reasoning behind this position; this section is the user-facing record of the gap.

### The gap

`validate_cli_token` (`cli/mod.rs:87-97`) accepts any well-formed UUID as a non-root token. There is **no check** that the UUID was actually issued by AC for some session. `agent_fqn_from_path` (`config/teams.rs:62`) and `workgroup_root` (`phone/messaging.rs:54`) are **pure string operations** — they never touch the filesystem. So a non-coordinator agent who knows (or `list-peers`-discovers) the team name + coordinator-suffix of any sibling workgroup in the same project can:

1. Mint a fresh UUIDv4 → passes `validate_cli_token`.
2. Pass `--root <fake-path>` shaped like the target team's coordinator replica → `agent_fqn_from_path` returns the target coordinator's FQN.
3. `is_any_coordinator` accepts that FQN.
4. `workgroup_root` returns the (existing) target WG dir.
5. The verb writes to the target team's `BRIEF.md`.

### Why this is qualitatively worse than the same hole in `send` and `close-session`

`send` produces a transient message; recipients treat sender as untrusted. `close-session` kills sessions, which can be respawned. Brief verbs **persistently re-program** every agent in the target WG via the BRIEF.md they read on session start (per #137 design + the #107 auto-brief-title chain). The blast radius is durable and broad.

### What bounds the attack surface today

The attack requires a `--root` whose ancestors include a real `wg-N-<team>` directory on disk — otherwise `workgroup_root` returns `Err`, or `wg_root.join("BRIEF.md")` writes to a non-existent parent and `fs::write` fails. So the writable target set is bounded to **extant sibling `wg-N-*` directories under the same project** (the workgroups AC has already laid out). `list-peers` enumerates this set directly, so naming is trivial.

### Why the GOLDEN RULE is **not** part of this bound

The GOLDEN RULE in `default_context()` (`config/session_context.rs:478`) is a **prompt-level instruction** to the LLM running inside an agent session. It tells well-behaved agents to confine writes to their own replica root. It is **not** binary-level enforcement: a malicious agent that crafts a `<bin> brief-set-title --token <fresh-uuid> --root <forged-path>` invocation is making a tool call, and `validate_cli_token` / `agent_fqn_from_path` / `workgroup_root` are all **pure path operations** (verified at `cli/mod.rs:87-97`, `config/teams.rs:62`, `phone/messaging.rs:54`) that never check ownership of the path. So the GOLDEN RULE bounds **honest agent behavior**, not the attack surface — a malicious agent ignoring the rule is precisely the threat we are modeling. Calling the GOLDEN RULE "necessary but not sufficient" overstates its defensive role; it is **orthogonal** to this attack.

### Net attack surface

**Any agent in the same project** can rewrite the BRIEF of **any sibling workgroup whose `wg-N-*` directory exists on disk**. The bound comes from the layout AC creates, not from any check in the brief verbs.

### Why we accept this for #137 and what should follow

Closing this requires changing `validate_cli_token` to consult an authoritative session/credentials registry — that registry doesn't exist today, and its design is non-trivial (where does it live? how does it survive AC restarts? is it per-project or per-instance?). The fix is **architectural** and benefits `send`, `close-session`, **and** the new brief verbs simultaneously. Pulling it into #137's scope would balloon the issue and delay delivery.

**Position for #137:** ship the verbs with the inherited risk, document it loudly here (this section), and recommend a follow-up issue titled approximately *"Bind CLI tokens to issued sessions to prevent UUID-mint forgery (#XXX)"* tracked separately. The follow-up plan should:

- Make `validate_cli_token` consult a session-scoped registry (in-memory or on-disk under `~/.agentscommander/`).
- Bind `(token, root)` at issuance time and verify the binding.
- Apply uniformly to `send`, `close-session`, `brief-set-title`, `brief-append-body`.

### Operational mitigation in the meantime

The verb logs a single `log::info!` at the success path with `sender=` and `wg=` fields (§12). A coordinator (or a future audit tool) can grep their AC logs for `[brief]` entries with `wg=` mismatching the caller's home workgroup. This is detect-after-the-fact, not prevent. The collision-resistant timestamped backups (`*.bak.md`, see §B.2) are the recovery surface — a wronged team can roll back to the most recent legitimate state.

---

## 4. File-touch flow

Pseudocode for the post-auth path. All steps inside `cli/brief_ops.rs`. Both verbs share this scaffold; only step 5 ("apply edit") differs.

```text
fn perform(wg_root, op) -> Result<EditOutcome, BriefOpError>:
    perform_inner(wg_root, op, chrono::Utc::now)   # production wrapper; see §G.1

fn perform_inner<F: FnOnce() -> chrono::DateTime<chrono::Utc>>(
        wg_root: &Path, op: BriefOp, now: F) -> Result<EditOutcome, BriefOpError>:
    let brief_path  = wg_root.join("BRIEF.md")
    let lock_path   = wg_root.join("BRIEF.md.lock")
    # Per-PID tmp suffix eliminates the tmp-collision race when stale-lock recovery
    # fires while a previous writer is still blocked in fs::write (see §HIGH-2 in §7).
    let tmp_path    = wg_root.join(format!("BRIEF.md.tmp.{}", std::process::id()))

    # ─── 1. Acquire advisory file lock ────────────────────────────────────────
    let _lock = LockGuard::acquire(&lock_path, LOCK_TIMEOUT_5S, LOCK_STALE_AFTER_5M)?
    # Drop guard removes lock_path on every exit (success, error, panic)

    # ─── 2. Read existing content (treat NotFound as empty string) ────────────
    let (existing, file_existed) = match std::fs::read_to_string(&brief_path):
        Ok(s)                                      => (s, true)
        Err(e) if e.kind() == NotFound             => (String::new(), false)
        Err(e)                                     => return Err(ReadFailed(brief_path, e))

    # ─── 2a. Capture pre-edit sentinel (for HIGH-4 external-writer detection) ─
    #        size+mtime snapshot of BRIEF.md taken IMMEDIATELY after the read.
    #        At step 7 we re-stat and abort if either differs.
    #        NOTE on the mtime field: keep it as Option<SystemTime> so that a
    #        transient `modified()` failure (rare on local NTFS, possible on
    #        SMB/NFS) does not asymmetrically read as UNIX_EPOCH on one side and
    #        a real timestamp on the other (would produce a false-positive
    #        ExternalWrite). At step 7a we always compare `len`; we only compare
    #        mtimes when both sides are `Some`.
    let pre_sentinel: Option<(u64, Option<SystemTime>)> = if file_existed:
        match std::fs::metadata(&brief_path):
            Ok(m)  => Some((m.len(), m.modified().ok()))
            Err(_) => None     # file vanished between read and stat — treat as "no sentinel"
    else:
        None

    # ─── 3. Parse frontmatter (hand-rolled; see §5 — strict line-aware) ──────
    let parsed = parse_brief(&existing)
    # parsed = ParsedBrief { bom, line_ending, has_frontmatter, frontmatter: Vec<String>, body }
    #   - bom:             true iff content begins with U+FEFF (HIGH-3)
    #   - line_ending:     "\r\n" if first newline in input is preceded by \r else "\n" (LOW-3)
    #   - frontmatter:     lines BETWEEN the opening and closing `---` (no fences)
    #   - has_frontmatter: true iff first line trimmed equals "---" AND a subsequent
    #                      line trimmed equals "---" (D.1 + HIGH-3)
    #   - body:            everything after the closing `---<eol>` (or the whole input
    #                      after BOM if no frontmatter)

    # ─── 4. Apply edit (per-op; see §5 + §6) — returns post-edit ParsedBrief ─
    #        apply_edit operates on the parsed structure and returns a new
    #        ParsedBrief; rendering to a String is deferred until step 5b so the
    #        idempotence check can compare title values without re-parsing.
    let new_parsed = apply_edit(&parsed, op)?

    # ─── 5. Idempotence short-circuit (set-title only; see §H.6 + MED-3) ──────
    #        Compare SEMANTICALLY, not byte-exact: a CRLF-styled file that round-trips
    #        through render() with the same title is still a NoOp even though the
    #        rendered bytes may differ from `existing` if line-ending preservation
    #        wasn't perfect on some odd input.
    if op.is_set_title() && title_value_of(&new_parsed) == title_value_of(&parsed):
        return Ok(EditOutcome::NoOp)

    # ─── 5b. Render the post-edit ParsedBrief to bytes for the upcoming write ─
    let new_content = render(&new_parsed)

    # ─── 6. Backup with collision-suffix loop (only if file existed pre-edit) ─
    let backup_path: Option<PathBuf> = if file_existed:
        let ts = now().format("%Y%m%d-%H%M%S").to_string()
        let mut chosen: Option<PathBuf> = None
        for n in 0..=99:
            let candidate = if n == 0 {
                wg_root.join(format!("BRIEF.{}.bak.md", ts))
            } else {
                wg_root.join(format!("BRIEF.{}.{}.bak.md", ts, n))
            };
            # create_new for collision detection; close immediately, then copy.
            match OpenOptions::new().write(true).create_new(true).open(&candidate):
                Ok(_file) => { drop(_file); chosen = Some(candidate); break; }
                Err(e) if e.kind() == AlreadyExists => continue,
                Err(e) => return Err(BackupFailed(candidate, e)),
        let bp = chosen.ok_or(BackupExhausted(wg_root.join(format!("BRIEF.{}.bak.md", ts))))?
        # The create_new produced a 0-byte file at `bp`. Now stream the actual content.
        # On copy failure: explicitly remove the partial/empty file (§C.1) — fs::copy
        # makes NO guarantee of partial-file cleanup.
        match std::fs::copy(&brief_path, &bp):
            Ok(_) => Some(bp)
            Err(copy_err) =>
                let _ = std::fs::remove_file(&bp);   # best-effort; preserves original error
                return Err(BackupFailed(bp, copy_err))
    else:
        None

    # ─── 7. Atomic write: tmp + sentinel-check + rename ───────────────────────
    match std::fs::write(&tmp_path, &new_content):
        Ok(_) => ()
        Err(e) =>
            let _ = std::fs::remove_file(&tmp_path)   # MED-6 cleanup on ENOSPC etc.
            return Err(TmpWriteFailed(tmp_path.clone(), e))

    # 7a. Sentinel check: detect external writer (HIGH-4). Realistic case (editor
    #     save events seconds apart) is caught; sub-millisecond TOCTOU remains
    #     (specifically, the read→metadata window of ~µs at step 2a — the snapshot
    #     is taken AFTER the read, so an external write that lands between the
    #     read and the metadata call is reflected in the captured snapshot rather
    #     than detected here. Do NOT "tighten" by moving the snapshot BEFORE the
    #     read: that would introduce an unbounded write-between-snapshot-and-read
    #     window).
    if let Some((pre_len, pre_mtime)) = pre_sentinel:
        match std::fs::metadata(&brief_path):
            Ok(now_meta):
                let now_mtime = now_meta.modified().ok()
                let len_changed = now_meta.len() != pre_len
                let mtime_changed = match (pre_mtime, now_mtime):
                    (Some(a), Some(b)) => a != b
                    _                  => false   # one side missing — fall back to len-only
                if len_changed || mtime_changed:
                    let _ = std::fs::remove_file(&tmp_path)   # tidy
                    let bp = backup_path.clone().expect("file_existed → backup_path is Some")
                    return Err(ExternalWrite(bp))
            Err(e) if e.kind() == NotFound:
                # External delete between read and rename. Without this branch,
                # rename creates the destination silently (rename to a vanished
                # destination is a normal create on both Windows and Unix), so the
                # external delete would be silently undone. Treat as ExternalWrite
                # so the user is told what happened and pointed at the backup that
                # captured pre-delete content.
                let _ = std::fs::remove_file(&tmp_path)   # tidy
                let bp = backup_path.clone().expect("file_existed → backup_path is Some")
                return Err(ExternalWrite(bp))
            Err(_) => ()   # other transient FS error — let rename surface the real error

    # 7b. Rename with retry on Windows transient AV/Explorer holds (MED-4).
    #     Both error returns clean up the per-PID tmp file (mirrors §C.1 / MED-6
    #     pattern) so the rename-failure path leaves no `BRIEF.md.tmp.<pid>`
    #     litter — required by I20 and §H.7 ("clean operational artifacts must
    #     be absent post-call"; only crashed-writer litter is acceptable).
    for attempt in 0..=2:
        match std::fs::rename(&tmp_path, &brief_path):
            Ok(_) => break
            Err(e) if e.kind() == PermissionDenied
                   || e.raw_os_error() == Some(32)    # ERROR_SHARING_VIOLATION
                   || e.raw_os_error() == Some(5):    # ERROR_ACCESS_DENIED
                if attempt < 2: thread::sleep(Duration::from_millis(100)); continue
                else:
                    let _ = std::fs::remove_file(&tmp_path)   # MED-1 cleanup
                    return Err(RenameFailed(e, backup_path.clone()))
            Err(e) =>
                let _ = std::fs::remove_file(&tmp_path)       # MED-1 cleanup
                return Err(RenameFailed(e, backup_path.clone()))

    # ─── 8. Lock released by Drop on _lock when scope ends ────────────────────
    Ok(EditOutcome::Wrote { backup: backup_path })
```

### Backup-failure path (explicit)
If the `OpenOptions::create_new` step exhausts 100 attempts in the same second, the function returns `BackupExhausted(...)` (new variant per §B.2). If `std::fs::copy(&brief_path, &bp)` returns `Err`, we **explicitly delete the partial backup** with `let _ = std::fs::remove_file(&bp)` (per §C.1 — `fs::copy` does NOT guarantee partial-file cleanup) and then return `BackupFailed(...)`. In both cases the function returns **before** any tmp-write or rename. The lock is released by `LockGuard::Drop`. `BRIEF.md` is bit-for-bit unchanged; the partial backup file is removed; no `BRIEF.md.tmp.<pid>` is created.

### External-writer abort path (HIGH-4 sentinel)
The `pre_sentinel` snapshot of `(len, mtime)` taken right after the read is re-checked just before the rename. If either field changed — OR the file was deleted (`Err(NotFound)` is treated as `ExternalWrite` rather than letting rename silently re-create the destination) — we abort with `ExternalWrite(backup_path)` and the user's externally-modified content is preserved as the timestamped backup (which `fs::copy` captured **after** the external write, since the sentinel-trigger means the sequence was: our read → external write → our backup → our sentinel-check). The user gets a clear error pointing at the backup.

The sentinel is still racy at sub-millisecond resolution: specifically the **read→metadata window of ~µs** at step 2a (the snapshot is taken AFTER the read, so an external write that lands in that window is reflected in the captured snapshot rather than detected). The realistic case (editor save events seconds apart, AV scans hundreds of ms) is caught.

**Note on FAT32:** `Metadata::modified()` on FAT32 has 2-second granularity. Two writes inside the same 2-second bucket that don't change file size produce equal `(len, mtime)` snapshots, so the sentinel does not fire. In practice BRIEF.md lives in `.ac-new/` on the user's project drive (NTFS / EXT4 / APFS — all sub-second), so this is a documented edge case for unusual layouts (USB stick, very old SD card), not a v1 blocker. The mtime field is stored as `Option<SystemTime>` and skipped from comparison when either side is `None`, so a transient `modified()` failure (rare on local NTFS, possible on SMB / NFS) does not produce a false-positive `ExternalWrite` — `len` is always compared.

### Lock guard details (`LockGuard`)
```text
struct LockGuard { path: PathBuf }

impl LockGuard:
    fn acquire(path, timeout, stale_after) -> Result<Self>:
        let start = Instant::now()
        loop:
            match OpenOptions::new().write(true).create_new(true).open(path):
                Ok(file):
                    let _ = writeln!(file, "pid={} ts={}", process::id(), Utc::now().to_rfc3339())
                    return Ok(LockGuard { path: path.clone() })
                Err(e) if e.kind() == AlreadyExists:
                    # Stale-lock recovery (HIGH-2: 5-minute window — was 60s).
                    # Two writers can race here: A and B both detect stale, both call
                    # remove_file (one succeeds, the other gets NotFound and ignores it
                    # via `let _ =`), then both call create_new. The kernel's CREATE_NEW
                    # is the mutex — exactly one wins; the loser falls through to the
                    # AlreadyExists branch with the new (fresh) lockfile and waits.
                    # Per-PID tmp paths (see perform_inner) prevent the secondary
                    # tmp-collision race that the old shared `BRIEF.md.tmp` path had.
                    # NOTE: a writer who is still alive but blocked >5 min in fs::write
                    # will lose its rename to the new lock-holder (rename overwrite
                    # race). A liveness check (parse pid → OpenProcess on Windows /
                    # kill(pid,0) on Unix) would close this; deferred to a follow-up
                    # to avoid a new windows-crate dependency in v1. The 5-min window
                    # is wide enough that this is extremely rare in practice.
                    if let Ok(meta) = fs::metadata(path):
                        if meta.modified().ok().and_then(|m| m.elapsed().ok())
                              .map(|d| d > stale_after).unwrap_or(false):
                            log::warn!("[brief] removing stale lock at {:?}", path)
                            let _ = fs::remove_file(path)
                            continue
                    if start.elapsed() >= timeout:
                        return Err(LockTimeout)
                    thread::sleep(Duration::from_millis(50))
                Err(e):
                    return Err(LockIo(path.clone(), e))

impl Drop for LockGuard:
    fn drop(&mut self):
        let _ = std::fs::remove_file(&self.path)   # best-effort; never panic
```

Constants: `LOCK_TIMEOUT_5S = Duration::from_secs(5)`, `LOCK_STALE_AFTER_5M = Duration::from_secs(300)` (extended from 60s per HIGH-2). Both private to `brief_ops`; tests can construct `LockGuard::acquire` with shorter values.

### `EditOutcome` (single source for the success-line text)
```text
enum EditOutcome {
    Wrote { backup: Option<PathBuf> },   # backup=None when file_existed=false
    NoOp,                                # only emitted by the idempotence short-circuit
}
```

The CLI translates this to stdout in the per-verb `execute()`:
- `Wrote { backup: Some(p) }` → `"BRIEF.md <op> updated; backup: {p}"` where `<op>` is `title` or `body appended`
- `Wrote { backup: None }` → `"BRIEF.md created; no prior content to back up"`
- `NoOp` → `"BRIEF.md unchanged ({op} value already matches)"`

---

## 5. Frontmatter rules

Hand-rolled parser/editor in `cli/brief_ops.rs`, **inspired by** `parse_role_frontmatter` at `commands/entity_creation.rs:152` but with stricter line-aware open/close detection (the existing helper would mis-parse `---blob---` as a frontmatter block — see §D.3). YAML values written as **single-quoted** strings, `'` escaped as `''` (matches the existing convention at `commands/entity_creation.rs:232`). Parser is robust against three real-world artifacts of user-edited input:

1. **UTF-8 BOM** at the start (Notepad-on-Windows default — see HIGH-3).
2. **Trailing whitespace on `---` markers** — `--- \n` (invisible space) is common (see D.1).
3. **Mixed/CRLF line endings** — Windows editors write CRLF; we preserve the dominant style (LOW-3).

### Parsing

```text
struct ParsedBrief {
    bom: bool,                  # input began with U+FEFF (HIGH-3)
    line_ending: &'static str,  # "\r\n" if first observed newline was CRLF, else "\n" (LOW-3)
    has_frontmatter: bool,
    frontmatter: Vec<String>,   # raw frontmatter lines (eol-stripped)
    body: String,               # byte-for-byte slice after the closing `---<eol>`
}

fn parse_brief(s_in: &str) -> ParsedBrief:
    # ─── BOM peel (HIGH-3) ────────────────────────────────────────────────────
    let (bom, s) = if s_in.starts_with('\u{FEFF}') {
        (true, &s_in['\u{FEFF}'.len_utf8()..])     # 3 bytes
    } else {
        (false, s_in)
    };

    # ─── Line-ending detection (LOW-3) ────────────────────────────────────────
    # Look at the first newline in the post-BOM content. If preceded by \r → CRLF.
    let line_ending: &'static str = match s.find('\n') {
        Some(i) if i > 0 && s.as_bytes()[i-1] == b'\r' => "\r\n",
        _ => "\n",
    };

    # ─── Pull the opening line out of the iterator (CRIT-1 Form B fix) ───────
    # The opening's actual byte length is whatever split_inclusive gives us —
    # 4 bytes for "---\n", 5 for "---\r\n", 7 for "--- \r\n", etc.  Hard-coding
    # `consumed = "---\n".len()` was wrong for CRLF (CRIT-1).
    let mut iter = s.split_inclusive('\n');
    let opening = match iter.next() {
        Some(line) if line.trim() == "---" => line,
        _ => return ParsedBrief {
                bom, line_ending,
                has_frontmatter: false, frontmatter: vec![],
                body: s.to_string()        # NB: body is BOM-less here; render re-emits BOM
            },
    };
    let mut consumed = opening.len();      # exact byte count of the actual opening line

    # ─── Walk to the closing `---` (D.1: trim full whitespace, not just \r\n) ─
    let mut fm_lines: Vec<String> = vec![]
    let mut closed = false
    for line in iter:
        consumed += line.len()
        let stripped = line.trim_end_matches(['\r','\n'])
        if stripped.trim() == "---":         # D.1: tolerate `--- \n`, `\t---\n`, etc.
            closed = true
            break
        fm_lines.push(stripped.to_string())

    if !closed:
        # Malformed frontmatter — treat as no frontmatter (preserve as body verbatim)
        return ParsedBrief {
            bom, line_ending,
            has_frontmatter: false, frontmatter: vec![],
            body: s.to_string()
        }

    let body = s[consumed..].to_string()
    ParsedBrief { bom, line_ending, has_frontmatter: true, frontmatter: fm_lines, body }
```

**Notes on the parsing decisions:**

- BOM is peeled at the very top so that `s.starts_with("---")`-style checks work uniformly. The BOM is stored on the struct so render can re-emit it (Windows-friendly, principle-of-least-surprise — see HIGH-3 in §"Round 2 — Architect Resolution").
- The opening-line length is read from the iterator yield, not hard-coded — fixes CRIT-1 (a 5-byte CRLF opening was being treated as 4 bytes, leaking a stray `\n` into the body on every CRLF-opened file).
- `line.trim_end_matches(['\r','\n']).trim() == "---"` makes the close-detection robust against trailing whitespace AND CRLF (D.1).
- `body` is preserved byte-for-byte from the post-BOM input — internal newlines (LF/CRLF/mixed) within the body survive untouched.

### Render

```text
fn render(parsed: &ParsedBrief) -> String:
    let eol = parsed.line_ending      # "\n" or "\r\n" — preserves dominant style (LOW-3)
    let mut out = String::with_capacity(parsed.body.len() + 64)
    if parsed.bom { out.push('\u{FEFF}'); }                           # HIGH-3 re-emit
    if !parsed.has_frontmatter:
        out.push_str(&parsed.body);
        return out;
    out.push_str("---"); out.push_str(eol);
    for line in &parsed.frontmatter:
        out.push_str(line);
        out.push_str(eol);
    out.push_str("---"); out.push_str(eol);
    out.push_str(&parsed.body);
    out
```

**Why preserving the BOM and line ending matters:** users who open BRIEF.md in Notepad/VS Code expect the file to keep its existing style. Silently dropping the BOM forces a re-encoding diff in the user's git client; flipping LF↔CRLF triggers their editor's "mixed line endings" warning. Both are trivially avoided by carrying the metadata on `ParsedBrief`.

### Set-title behaviour matrix (exhaustive)

Let `escaped = title.replace('\'', "''")`, `new_title_line = format!("title: '{}'", escaped)`.

| Pre-edit state                                            | Action                                                                                                              |
|-----------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------|
| File missing (NotFound) or `existing.is_empty()`          | Set `parsed = { bom:false, line_ending:"\n", has_frontmatter:true, frontmatter: vec![new_title_line], body: String::new() }`. New file = `---\ntitle: 'X'\n---\n`. BRIEF.md is born LF-only and BOM-less per `entity_creation.rs:193` convention. |
| `parsed.has_frontmatter == false` (no leading `---<eol>`) | Set `parsed.has_frontmatter = true`, `parsed.frontmatter = vec![new_title_line]`. Body unchanged. `bom` and `line_ending` preserved (HIGH-3 + LOW-3). New file gains a frontmatter block ahead of the body. |
| `parsed.has_frontmatter == true`, `title:` line exists    | Replace the FIRST line whose `trim_start()` starts with `title:` (case-sensitive, matching existing skills.md convention). Other lines preserved verbatim, including order. |
| `parsed.has_frontmatter == true`, `title:` line absent    | Insert `new_title_line` at index 0 of `parsed.frontmatter`. Other lines preserved verbatim, including order.        |

**Detection of "is this a `title:` line"**: `line.trim_start().starts_with("title:")` (matches `parse_role_frontmatter` style at `entity_creation.rs:169`). Replacement preserves the leading whitespace exactly, so an indented `  title: x` becomes `  title: 'NewTitle'`.

**Duplicate `title:` lines (NIT-5).** A hand-edited or merge-conflict-resolved BRIEF.md may contain multiple `title:` lines. The "FIRST line" rule above is intentional — changing it to "replace ALL" would surprise users who deliberately have multiple title-shaped lines (e.g., a `title: x` inside a YAML literal block). When `apply_set_title` detects more than one frontmatter line whose `trim_start().starts_with("title:")`, emit a single `log::warn!("BRIEF.md frontmatter contains N title: lines; replacing the first only — downstream YAML parsers may pick a different one")` BEFORE the replacement. The replacement itself proceeds unchanged. `title_value_of` (used for the idempotence check) also reads the FIRST title line, so a subsequent set-title with the same value will idempotence-skip even if duplicates remain.

**Idempotence short-circuit (semantic, not byte-exact — MED-3):** the short-circuit compares the **value of the `title:` field** before and after, not the byte-equality of the rendered file. This matters for CRLF-styled files: even with line-ending preservation (LOW-3), a parse→render round-trip on an unusual input could byte-differ from the original while being semantically a no-op. Concretely:

```text
fn title_value_of(parsed: &ParsedBrief) -> Option<String>:
    parsed.frontmatter.iter()
        .find(|line| line.trim_start().starts_with("title:"))
        .map(|line| extract_yaml_single_quoted(line.trim_start().strip_prefix("title:").unwrap().trim()))

# In perform_inner step 5:
if matches!(op, BriefOp::SetTitle(_))
   && title_value_of(&existing_parsed) == title_value_of(&new_parsed):
    return Ok(EditOutcome::NoOp)
```

`extract_yaml_single_quoted` parses the canonical `'value with '' escapes'` form; for non-canonical inputs (bare scalar, double-quoted) it falls back to the raw post-`title:` substring trimmed. This is sufficient for "did the user-visible title change?" — the conservative direction (return false → write a new backup) is harmless audit-trail noise; the unsafe direction (return true → skip a real edit) is impossible because the parsed-after-edit form is always canonical single-quoted.

`apply_append_body` does not benefit from idempotence (an append always changes the file) — the short-circuit applies only to set-title. See §H.6.

### Append-body behaviour matrix (exhaustive)

| Pre-edit state                                            | Action (after parse)                                                                                                      |
|-----------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------|
| File missing or `existing.is_empty()`                     | `parsed = { bom:false, line_ending:"\n", has_frontmatter:false, frontmatter:vec![], body: String::new() }`. Body becomes `format!("{}\n", text.trim_end())`. |
| `parsed.body.trim().is_empty()`                           | Body becomes `format!("{}\n", text.trim_end())`. `bom` and `line_ending` preserved (HIGH-3 + LOW-3). (No leading blank line — there's nothing above.) |
| `parsed.body` is non-empty                                | `body = format!("{}\n\n{}\n", parsed.body.trim_end(), text.trim_end())`. (Trim trailing whitespace of the existing tail; insert exactly one blank line; ensure single trailing `\n`.) `bom` and `line_ending` preserved. |

Frontmatter (if present) is **never** touched by `brief-append-body`.

**Note on body line-endings:** the appended `text` and the body separator are written with literal `\n`. This is intentional for v1 — the body's existing line-ending style is preserved byte-for-byte (the body slice is taken raw from `parse_brief`), but the *appended portion* is always LF. A CRLF-styled BRIEF.md will end up with mixed line endings inside the body after an append. This is acceptable per §6 — the alternative (rewriting the entire body to use `parsed.line_ending`) violates the "preserve user's body verbatim" guarantee. Frontmatter consistently uses `parsed.line_ending` because we re-render it; body content we never re-render.

### YAML escaping rationale (single-quoted form)

The single-quoted YAML scalar tolerates every printable character — including `:`, `#`, `"`, `[`, `]`, leading `-`, etc. — except the single quote itself, which doubles to `''`. This is exactly the convention `commands/entity_creation.rs:232-235` already uses for Role.md descriptions, so the codebase has a single escape style. We do not need a YAML library and we do not need to handle multiline titles (titles are single-line by spec — `--title` is one CLI argument; embedded newlines from a shell would be a user error).

---

## 6. Append newline normalisation (exact rule)

Captured above for completeness:

> **Rule:** Trim trailing whitespace from the existing body's tail. If the trimmed tail is non-empty, append `"\n\n" + text.trim_end() + "\n"`. If the trimmed tail is empty (or the file did not exist / had no body after the frontmatter), append `text.trim_end() + "\n"`. The file always ends with exactly one `\n` after the operation.

Properties:
- Exactly one blank line separates pre-existing content from the appended block.
- No accumulating trailing newlines across repeated appends.
- The appended `text` is preserved verbatim except for trailing-whitespace trim (so users don't bleed CRLF / trailing spaces into the file). Internal newlines inside `text` are preserved, allowing multi-paragraph appends.
- Frontmatter is not in the body, so it is unaffected.

---

## 7. Concurrency proof

### Mechanism: filesystem advisory lockfile + atomic publish via `tmp + rename`

Decision: **exclusive lockfile via `OpenOptions::new().write(true).create_new(true).open(...)`**, plus the existing `tmp + rename` publish pattern from `config/settings.rs::save_settings` (line 459, `// Atomic write (tmp + rename) per G.14`). Both primitives are already in use in the codebase; no new crate added. Stale-lock recovery (5 min — `LOCK_STALE_AFTER_5M`, raised from 60 s per HIGH-2) keeps a crashed coordinator from permanently blocking writes.

### Why not bare `tmp + rename` only?
`tmp + rename` is atomic for the *publish* — readers see either the old or the new file, never a partial one. It does NOT prevent **lost updates**: two concurrent callers could both `read` the old content, both compute their own "new" content (one with new title, one with appended body), both write distinct tmp files, and the second `rename` would silently overwrite the first caller's edit. The issue's acceptance criterion "concurrent writes from two coordinators don't corrupt the file" demands that BOTH edits land — corruption-free is necessary but not sufficient; lost-update is also a form of unwanted result. The lockfile serialises read–modify–write so both edits are applied in some sequence.

### Why not `fs2`/`fd-lock`/`file-guard`?
None are in `Cargo.toml`. Adding a dep for one verb is unjustified when `OpenOptions::create_new` already provides exclusive creation atomically (the kernel guarantees it, on Windows via `CREATE_NEW`, on Unix via `O_CREAT | O_EXCL`). The existing `phone/messaging.rs::create_message_file` (line 215) uses exactly this primitive for collision-safe message-file creation — same pattern, same guarantees.

### Failure modes (and what saves us in each)

| Failure                                                       | Outcome                                                                                                                                                          |
|---------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Two coordinators run `brief-set-title` simultaneously         | First wins the lock; second polls every 50 ms for up to 5 s. Either succeeds in order (both edits land) or second exits with the lock-timeout error (no corruption, no lost update). |
| Coordinator runs `brief-set-title` while another runs `brief-append-body` | Same — serialised by the lock; both edits applied in arrival order.                                                                                            |
| Coordinator process crashes mid-edit AFTER taking lock        | Lock file is left behind. Next caller within 5 min gets `LockTimeout`. After 5 min, stale-lock recovery removes the lock and proceeds. (Also: `tmp + rename` means `BRIEF.md` is bit-identical to its pre-crash state since rename is the commit point.) The per-PID `BRIEF.md.tmp.<pid>` from the dead writer remains as litter — addressable by a follow-up sweep at lock acquire if it ever bothers anyone (defer per role's "minimal blast radius"). |
| Stale-lock recovery fires while the prior writer is still alive but blocked >5 min in `fs::write` (HIGH-2) | Per-PID tmp paths (`BRIEF.md.tmp.<pid>`) eliminate the most likely failure mode (two writers colliding on the same tmp file). The remaining race — A's eventual `rename` overwriting B's just-completed `rename` — is **not** prevented by the per-PID fix; it requires a writer-liveness check (parse pid from lockfile → `OpenProcess`/`kill(pid,0)`) deferred to a follow-up. The 5-minute stale window (raised from 60 s) makes the window for this race extremely narrow in practice (Defender scans, OneDrive interception, sleepy SATA drives — none typically block writes that long). Backup captures pre-A state, so data is recoverable from `*.bak.md` even if a rename-race silently drops B's edit. |
| OS crashes mid-edit                                            | After reboot, `BRIEF.md` is bit-identical to its pre-crash state (rename hadn't fired). `BRIEF.md.tmp.<pid>` may exist as garbage — next successful run with the same PID overwrites it via `std::fs::write`; otherwise it persists until the wg dir is cleaned. Backup also retains pre-crash state. |
| `std::fs::rename` fails on Windows (file in use)              | Lock prevents another writer holding it. Antivirus / Explorer transient holds are the realistic case — `perform` retries `rename` 3× with 100 ms backoff (MED-4) on `ERROR_SHARING_VIOLATION`/`ERROR_ACCESS_DENIED`. If all retries fail, the error string includes the backup path so the user can diagnose. |
| External non-cooperating writer modifies BRIEF.md between our read and our rename (HIGH-4) — e.g. user has BRIEF.md open in VS Code and saves during the operation | The advisory lockfile does NOT block external writers. `perform` captures `(len, mtime)` of BRIEF.md right after the read (step 2a), and re-checks it just before the rename (step 7a). If either changed, abort with `ExternalWrite` error pointing the user at the timestamped backup that captured the externally-modified state. Sub-millisecond TOCTOU remains theoretically open, but realistic editor save events (seconds-apart) and AV scans (hundreds of ms) are caught. The user-facing message is explicit about what happened and where the backup is. |
| Backup `copy` fails                                           | Abort BEFORE any tmp-write. Partial backup file is explicitly removed (§C.1 — `fs::copy` does NOT guarantee partial-file cleanup). `BRIEF.md` unchanged. Lock released by Drop. |
| Backup-name collision (1-second timestamp resolution + lock-released-then-reacquired window — §B.2) | `OpenOptions::create_new` collision-suffix loop tries `BRIEF.{ts}.bak.md`, then `BRIEF.{ts}.1.bak.md`, …, up to `.99.bak.md`. After 100 collisions in the same wall-clock second (would require >100 calls/second — implausible under the lock-serialised workload) we abort with `BackupExhausted`. |
| Tmp write fails mid-stream (e.g. ENOSPC — MED-6)              | Best-effort `let _ = remove_file(&tmp_path)` to avoid leaving partial-content garbage that confuses a human inspecting the wg dir. Lock released by Drop. `BRIEF.md` unchanged. |
| Lockfile created but the process is killed -9                  | Same as crash above — stale-lock recovery handles after 5 min.                                                                                                    |

### Justification summary
- **Atomic publish** comes from `tmp + rename` — already proven in `settings.rs:459`.
- **Lost-update prevention (cooperative)** comes from the lockfile — same primitive as `messaging.rs:215`.
- **Backup audit-trail integrity** comes from the collision-suffix loop (B.2) plus explicit partial-cleanup (C.1).
- **External-writer detection** comes from the size+mtime sentinel between read and rename (HIGH-4 — caught at user-realistic timescales).
- **Crash recovery** comes from the 5 min stale-lock window — bounded delay, no manual intervention. Per-PID tmp paths (HIGH-2) prevent the most-likely tmp-collision race; the remaining writer-liveness race is documented and deferred.
- **Windows AV/Explorer transient holds** mitigated by the 3× rename-retry with 100 ms backoff (MED-4).
- **Zero new dependencies.**

---

## 8. File map

### New files

| Path                                                | Purpose                                                                                                                          |
|-----------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------|
| `src-tauri/src/cli/brief_ops.rs`                    | Pure logic: `parse_brief`, `render`, `apply_set_title`, `apply_append_body`, `LockGuard`, `BriefOpError` (thiserror), `EditOutcome`. No clap, no I/O on `BRIEF.md` outside the locked critical section. Unit-tested standalone with tempdirs. |
| `src-tauri/src/cli/brief_set_title.rs`              | clap `BriefSetTitleArgs` + `execute(args) -> i32`. Owns the auth flow (§3), then calls into `brief_ops::perform`.                |
| `src-tauri/src/cli/brief_append_body.rs`            | clap `BriefAppendBodyArgs` + `execute(args) -> i32`. Same shape as the set-title verb.                                            |

### Modified files

| Path                                                | Change                                                                                                                            |
|-----------------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------|
| `src-tauri/src/cli/mod.rs` lines 1–5                | Add `pub mod brief_append_body;`, `pub mod brief_ops;`, `pub mod brief_set_title;` (alphabetical order).                          |
| `src-tauri/src/cli/mod.rs` lines 27–38 (`Commands`) | Add two variants: `BriefSetTitle(brief_set_title::BriefSetTitleArgs)` and `BriefAppendBody(brief_append_body::BriefAppendBodyArgs)`. Place them after `CloseSession` to keep the existing entries' positions stable. |
| `src-tauri/src/cli/mod.rs` lines 104–110 (`handle_cli`) | Add two `Commands::*` arms calling `brief_set_title::execute(args)` and `brief_append_body::execute(args)`.                   |

No changes elsewhere. Specifically:
- `src-tauri/Cargo.toml` — **no new dependencies**. Reuse `clap`, `chrono`, `thiserror`, `log`, `std::fs`, plus existing helpers from `crate::cli`, `crate::config::teams`, `crate::phone::messaging`.
- `src-tauri/src/main.rs` — no change. `handle_cli` is the single dispatch point; the new verbs surface automatically.
- `src-tauri/src/config/session_context.rs` (the GOLDEN RULE template) — **no change**. The whole point of the verb is to avoid weakening the rule.
- `src-tauri/src/commands/entity_creation.rs::build_brief_content` — no change here; #107 will land BRIEF-creation policy changes on its own branch.
- Frontend — **no change**. These verbs are agent-facing only; not exposed via Tauri commands or events. No `src/shared/types.ts` changes, no `src/shared/ipc.ts` changes.

### Test files

| Path                                                | Hosts                                                                                                                            |
|-----------------------------------------------------|----------------------------------------------------------------------------------------------------------------------------------|
| `src-tauri/src/cli/brief_ops.rs` (`#[cfg(test)] mod tests`) | All pure-logic unit tests: parser, edit application, lock acquisition, atomic publish. Uses tempdir convention from `phone/messaging.rs:349` (`unique_tmp` helper — copy locally; matches the no-new-dep style). |
| `src-tauri/src/cli/brief_set_title.rs` (`#[cfg(test)] mod tests`) | Per-verb integration around the auth boundary using `args` constructed in-test. Mocks token via the root-token bypass. |
| `src-tauri/src/cli/brief_append_body.rs` (`#[cfg(test)] mod tests`) | Same as set-title.                                                                                                            |

No top-level `tests/` directory entries are added — the codebase keeps tests inline (`config/teams.rs` line 720+, `phone/messaging.rs` line 343+), so we follow the convention.

---

## 9. Test plan

Maps every acceptance criterion in #137 to one or more concrete tests. **All test names are exact `#[test] fn` names; the developer should not rename without coordinating.**

### Unit tests in `cli/brief_ops.rs::tests`

| # | Test fn name                                            | Asserts                                                                                              |
|---|---------------------------------------------------------|------------------------------------------------------------------------------------------------------|
| U1 | `parse_brief_no_frontmatter`                           | `existing = "# Body"` → `has_frontmatter:false, body:"# Body"`                                       |
| U2 | `parse_brief_empty_string`                              | `existing = ""` → `has_frontmatter:false, body:""`                                                    |
| U3 | `parse_brief_well_formed_frontmatter`                   | `"---\ntitle: x\n---\nbody"` → `has_frontmatter:true, frontmatter:["title: x"], body:"body"`           |
| U4 | `parse_brief_frontmatter_no_title_field`                | `"---\nfoo: bar\n---\nbody"` → fm = `["foo: bar"]`                                                    |
| U5 | `parse_brief_unclosed_frontmatter_treated_as_body`      | `"---\ntitle: x\n(no closer)\n"` → `has_frontmatter:false, body == input`                            |
| U6 | `parse_brief_tolerates_crlf`                            | `"---\r\ntitle: x\r\n---\r\nbody"` → assert `p.has_frontmatter && p.body == "body" && p.line_ending == "\r\n"`. **Strict body equality is required to catch CRIT-1** (the original off-by-one would make `body == "\nbody"` — see MED-1 for why "preserves CRLF" was too vague). |
| U7 | `apply_set_title_creates_frontmatter_when_absent`       | empty input → `"---\ntitle: 'X'\n---\n"`                                                              |
| U8 | `apply_set_title_replaces_existing_title_value`         | `"---\ntitle: old\n---\nbody\n"` + title `"new"` → title line replaced, body intact                  |
| U9 | `apply_set_title_inserts_into_existing_frontmatter`     | `"---\nfoo: bar\n---\nbody\n"` + title `"x"` → fm now `["title: 'x'", "foo: bar"]`                    |
| U10 | `apply_set_title_preserves_other_frontmatter_fields`    | `"---\nfoo: 1\ntitle: old\nbar: 2\n---\nbody"` + title `"new"` → only title line changes, order kept |
| U11 | `apply_set_title_yaml_escapes_single_quote`             | title `"won't"` → on disk `title: 'won''t'`                                                          |
| U12 | `apply_set_title_yaml_safe_with_colon_and_hash`         | title `"v1.0: stable #release"` → unmodified inside single-quotes; round-trips via parser            |
| U13 | `apply_set_title_idempotent_when_value_matches`         | running set-title twice with same value yields `NoOp` on the second call                              |
| U14 | `apply_append_body_to_empty_file`                       | empty input + text `"hello"` → `"hello\n"`                                                            |
| U15 | `apply_append_body_preserves_prior_content`             | `"---\ntitle: x\n---\nold\n"` + text `"new"` → `"---\ntitle: x\n---\nold\n\nnew\n"`                   |
| U16 | `apply_append_body_normalizes_blank_line_separator`     | `"old\n\n\n\n"` + text `"new"` → `"old\n\nnew\n"` (collapses to exactly one blank line)              |
| U17 | `apply_append_body_does_not_touch_frontmatter`          | frontmatter bytes pre/post-edit are byte-equal                                                       |
| U18 | `apply_append_body_strips_trailing_whitespace_from_text`| text `"hello   \n\n"` → final tail `"hello\n"`                                                       |
| U19 | `lock_guard_creates_and_removes_lockfile`               | `acquire` then `drop` leaves no `BRIEF.md.lock`                                                       |
| U20 | `lock_guard_blocks_concurrent_acquisition`              | second `acquire` with 100 ms timeout returns `LockTimeout` while first still held                     |
| U21 | `lock_guard_recovers_stale_lockfile`                    | Test approach (std-only — no `filetime` / no FFI): pre-create the lockfile via `OpenOptions::new().create_new(true).write(true).open(&lock_path)` and drop it; sleep ~20 ms; call `acquire(&lock_path, LOCK_TIMEOUT_5S, Duration::from_millis(10))` — the `stale_after` is configurable per-call so the test uses a small value. Asserts: stale lock removed, `log::warn!` emitted, second acquire succeeds. The production constant is `LOCK_STALE_AFTER_5M = Duration::from_secs(300)` (raised from 60 s per HIGH-2); the test uses a smaller value because std-only Rust cannot easily fake file mtimes without an FFI call (`filetime` is NOT a current dep and adding it for one test crosses the "no new crates" bar). |
| U22 | `atomic_publish_via_rename_round_trip`                  | call `perform` with set-title; after success, assert `wg_root/BRIEF.md.tmp.<pid>` does not exist (per-PID tmp naming per HIGH-2; use `std::process::id()`). Also assert no other `BRIEF.md.tmp.*` files exist. |
| U23 | `backup_filename_uses_utc_timestamp_format`             | regex match on `BRIEF\.\d{8}-\d{6}(\.\d{1,2})?\.bak\.md` (the optional `.N` suffix accommodates the §B.2 collision loop) |
| U24 | `backup_failure_aborts_write_and_preserves_brief`       | use the `perform_inner` clock-injection seam (§G.1): pin `now()` to a fixed UTC, **pre-create a directory** at the predicted backup path (e.g. `BRIEF.20260101-000000.bak.md/`), then call `perform_inner`. `OpenOptions::create_new(true).open()` against the existing directory returns `AlreadyExists` → loop tries `.1.bak.md`, etc. (To force the actual `BackupFailed` from `fs::copy`, pre-create AND make the parent dir read-only — but on Windows that's clumsy. Simpler reliable path: pre-create the directory at the candidate, then create a 0-byte file at `.1.bak.md` so the loop's `create_new` succeeds at `.1.bak.md`, then have `fs::copy` fail by making the source unreadable — also clumsy. **Recommended actually:** make the FIRST candidate be a path whose parent does not exist by setting up the wg_root with the right structure, so `OpenOptions::create_new` returns NotFound, which propagates as `BackupFailed`.) Assert `BRIEF.md` bytes unchanged, lock file removed, no `BRIEF.md.tmp.*` files. |
| U25 | `concurrent_set_title_and_append_body_both_apply`       | spawn two `std::thread::spawn` workers synchronized by `std::sync::Barrier::new(2)` so they call `perform` at the **same instant** (per MED-2: without the barrier, the test passes for the wrong reason — Rust does not parallelise within a single `#[test]`). Both succeed; final content has the title AND the appended text. Loop 50 iterations with random ordering of set-title-vs-append-body to maximise regression-catch probability. |
| U26 | `parse_brief_tolerates_trailing_space_on_markers`       | per §D.1 — input `"--- \ntitle: x\n--- \nbody"` → `has_frontmatter:true, frontmatter:["title: x"], body:"body"`. |
| U27 | `parse_brief_unicode_in_body_preserved_byte_for_byte`   | round-trip a body with non-ASCII (`"café\n"`, plus a 4-byte emoji like `"🎉\n"`) — guards the byte-offset slice in `parse_brief` after CRIT-1 fix. |
| U28 | `apply_set_title_preserves_indentation_of_existing_title_line` | `"---\n  title: old\n---\n"` + `"new"` → `"---\n  title: 'new'\n---\n"` (leading whitespace preserved per §5). |
| U29 | `backup_collision_within_same_second_does_not_clobber_prior_backup` | per §B.2 — uses §G.1 clock-injection seam to pin two `perform_inner` calls to the same UTC second; assert TWO distinct backup files exist on disk (`BRIEF.{ts}.bak.md` and `BRIEF.{ts}.1.bak.md`), each with the correct pre-edit content for its respective call. |
| U30 | `backup_failure_releases_lockfile`                      | per §C.2 — companion to U24: after the injected backup-failure returns, assert `wg_root/BRIEF.md.lock` does **not** exist. |
| U31 | `parse_brief_strips_and_re_emits_leading_bom`           | per HIGH-3 — input `"\u{FEFF}---\ntitle: x\n---\nbody"` → `p.bom == true && p.has_frontmatter == true && p.frontmatter == ["title: x"] && p.body == "body"`. Round-trip `render(parse_brief(input))` reconstructs the leading BOM byte-for-byte. |
| U32 | `set_title_round_trip_preserves_crlf_no_extra_blank_line` | per CRIT-1 — input `"---\r\ntitle: old\r\n---\r\nbody\r\n"`, apply `set_title("new")`, render. Assert the rendered output contains exactly one `---\r\n` after the title line followed immediately by `body\r\n` (no spurious blank line between the closing `---` and the start of body). This is the byte-exact regression test for the CRIT-1 off-by-one. |
| U33 | `parse_brief_preserves_dominant_line_ending`            | per LOW-3 — CRLF input → `p.line_ending == "\r\n"`; LF input → `p.line_ending == "\n"`. After `render`, frontmatter delimiters use the preserved style. |
| U34 | `apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss` | per NIT-E (round 2 dev-rust + grinch agreement) — pins the documented mixed-line-ending trade-off in §5 row 510. Input: `parsed.body == "Line1\r\nLine2\r\n"` plus `text == "NewLine"`. After `apply_append_body`, assert `result.body == "Line1\r\nLine2\n\nNewLine\n"` — Line1's CRLF preserved, Line2's trailing CRLF replaced by `\n\n` separator (the existing `trim_end()` + `format!` pattern), NewLine ends with LF. This pins the trade-off so a future contributor cannot silently "fix" the body to all-LF rendering and regress the byte-for-byte body-preservation guarantee. |

### Integration tests in `cli/brief_set_title.rs::tests` and `cli/brief_append_body.rs::tests`

These exercise the auth boundary using the **root-token bypass** (set `settings.root_token = Some(known_uuid)` in a test settings dir, point `validate_cli_token` at it via env var or a test-only `crate::config::config_dir` override) so we do not need a fully-bootstrapped team. For the non-coordinator-rejection test, omit the root-token bypass and use a UUID token + a fixture `wg-99-X/__agent_member` root that does NOT match any team's coordinator.

| # | Test fn name                                            | Asserts (issue acceptance criterion → test mapping)                                                                            |
|---|---------------------------------------------------------|--------------------------------------------------------------------------------------------------------------------------------|
| I1 | `set_title_root_token_bypasses_coord_check_and_writes`  | AC: "coordinator agent ... can invoke ... and have the title appear" (root-token shortcut covers the success path)             |
| I2 | `append_body_root_token_bypass_appends_text`            | AC: "coordinator agent can invoke the body-append verb"                                                                         |
| I3 | `set_title_rejects_non_coordinator_with_uuid_token`     | AC: "non-coordinator agent invoking either verb is rejected with a clear error and non-zero exit code" — exit 1, exact string  |
| I4 | `append_body_rejects_non_coordinator_with_uuid_token`   | Same, append verb                                                                                                               |
| I5 | `set_title_rejects_invalid_token`                       | AC: "invalid token is rejected with a clear error" — pass `--token notauuid`, expect exit 1 + exact string                     |
| I6 | `set_title_rejects_unresolvable_root`                   | AC: "unresolvable root is rejected with a clear error" — pass `--root /tmp/no-wg-here`, expect exit 1 + exact string           |
| I7 | `set_title_creates_timestamped_backup`                  | AC: "each write ... creates a timestamped backup in the same directory as BRIEF.md"                                             |
| I8 | `append_body_creates_timestamped_backup`                | Same, append verb                                                                                                               |
| I9 | `set_title_does_not_create_backup_for_brand_new_file`   | When `BRIEF.md` does not exist, success message is `"BRIEF.md created; no prior content to back up"` and no `*.bak.md` exists  |
| I10 | `set_title_aborts_on_simulated_backup_failure`         | AC: "if the backup fails, the write is aborted and exit code is non-zero" — see U24 for injection technique                    |
| I11 | `set_title_only_touches_brief_md_and_bak_md`           | AC: "no file other than BRIEF.md and its `*.bak.md` siblings is touched" — snapshot wg_root contents pre/post; only BRIEF.md and one `*.bak.md` differ |
| I12 | `append_body_only_touches_brief_md_and_bak_md`         | Same, append verb                                                                                                               |
| I13 | `set_title_preserves_other_frontmatter_when_replacing` | AC: "existing BRIEF.md frontmatter (other fields) is preserved when only updating `title:`"                                     |
| I14 | `set_title_creates_frontmatter_when_brief_has_none`    | AC: "if BRIEF.md has no frontmatter, the title-set verb creates one cleanly"                                                    |
| I15 | `concurrent_writes_dont_corrupt_file`                  | AC: "concurrent writes from two coordinators don't corrupt the file" — covered structurally by U25. CLI-layer variant: two `std::thread::spawn` workers synchronized by `std::sync::Barrier::new(2)` so both `execute()` calls hit the lock at the same instant (per MED-2). Both report exit 0; final BRIEF.md contains the title AND the appended text. **Without the barrier this test is dishonest** — Rust does not parallelise within a single `#[test]`, so a sequential call elides the lock contention entirely. |
| I16 | `help_text_documents_new_verbs`                        | AC: "new CLI verb(s) exist and are documented via `--help`" — `Cli::command().render_help()` contains both verb names + their flag descriptions |
| I17 | `set_title_rejects_embedded_newlines`                  | per §D.2 — `--title "a\nb"` returns exit 1 with the new control-char error string; BRIEF.md unchanged on disk. |
| I18 | `set_title_rejects_when_root_is_workgroup_root_directly` | per §F.2 — `--root` set to a `wg-N-team/` dir directly (no `__agent_*` parent) returns exit 1 with the standard authorization-denied error. |
| I19 | `append_body_preserves_internal_newlines_in_text`      | append `"line1\nline2\n\nline4"` (intentional blank line inside) → all three internal newlines survive verbatim into the body. |
| I20 | `set_title_aborts_on_readonly_brief_md_with_clean_state` | per NIT-3 — mark BRIEF.md read-only via `std::fs::set_permissions` (Unix `0o444`; Windows `OpenOptionsExt::attributes(FILE_ATTRIBUTE_READONLY)` or `attrib +R`), call set-title, assert exit 1 with `RenameFailed` error string, lock file removed, no `BRIEF.md.tmp.*` litter. |
| I21 | `set_title_aborts_on_external_modification_between_read_and_rename` | per HIGH-4 — the test pins `perform_inner`'s timestamp clock and inserts a sleep + spawned-thread external write between read and rename (use a thread-park/unpark or a small artificial delay; the production code's read→rename window is sub-millisecond, so the test must inject the gap deliberately). External write changes BRIEF.md size; the sentinel-check fires; verb exits 1 with `ExternalWrite` error pointing at the timestamped backup. The user's externally-modified content is recoverable from that backup. (If a clean injection seam proves too invasive, downgrade to a unit test in `brief_ops::tests` that calls `perform_inner` directly with a hook — acceptable; the AC is "external write detected & user warned", not a specific test layer.) |

### Mapping from issue acceptance checklist → tests
- "verb(s) exist and documented via --help" → I16
- "coordinator can set title" → I1
- "coordinator can append body" → I2
- "non-coordinator rejected" → I3, I4, I18
- "invalid token rejected" → I5
- "unresolvable root rejected" → I6
- "each write creates timestamped backup" → I7, I8 + U23, U29
- "backup failure aborts write" → I10 + U24, U30
- "concurrent writes don't corrupt file" → I15 + U20, U21, U25
- "frontmatter other fields preserved on title-update" → I13 + U10, U28
- "no frontmatter → title-set creates one cleanly" → I14 + U7
- "no file other than BRIEF.md and *.bak.md touched" → I11, I12 (note: per §H.7, transient `BRIEF.md.lock` and `BRIEF.md.tmp.<pid>` are operational artifacts that exist only under the lock — both must be absent from the post-call snapshot)
- "title input rejected when not single-line / contains control chars" (D.2 + LOW-1) → I17, U31 (the unit-level title-validation companion in `brief_set_title.rs::tests`)
- "BOM-prefixed BRIEF round-trips without prepending duplicate frontmatter blocks" (HIGH-3) → U31, U32
- "CRLF-opened BRIEF doesn't gain a stray blank line on first edit" (CRIT-1) → U6 (strict), U32
- "external editor write between our read and our rename is detected" (HIGH-4) → I21
- "Windows AV transient sharing-violation on rename is retried" (MED-4) → covered by code path; explicit test deferred to follow-up if no Windows CI is wired up
- "read-only BRIEF.md fails cleanly with no litter" (NIT-3) → I20

### Test infrastructure helpers (developer to extract as needed)
- `unique_tmp(prefix)` — copy from `phone/messaging.rs:349`. Self-cleaning via a `Drop` guard (see `FixtureRoot` in `config/teams.rs:787`).
- A `make_wg_fixture(tmp: &Path) -> PathBuf` helper that creates `<tmp>/proj/.ac-new/wg-1-team/__agent_a/` and returns the agent root.
- For tests that need a root-token, set `crate::config::settings::AppSettings { root_token: Some(uuid), ... }` via a test-only `config_dir` override (the existing test helpers in `config/settings.rs:483+` give the pattern).

---

## 10. Integration with #107 (forward-looking note, no design here)

After this issue lands on `main`, the #107 PTY-prompt template (the message the backend injects on Coordinator session restart, instructing the agent to read `BRIEF.md` and produce a title) will be modified to instruct the Coordinator: **after deciding on a title, run**

```
"<BinaryPath>" brief-set-title --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --title "<the title text>"
```

> **Note for the #107 implementer (NIT-4):** the angle brackets `<…>` above are documentation placeholders, not template syntax. The actual #107 PTY template must substitute concrete values **before** the line is injected into the agent's terminal — `<` and `>` are shell redirect operators in `cmd.exe` and PowerShell and would error if passed through verbatim. Use whatever placeholder convention #107's existing templates use (e.g. `{BinaryPath}` if the codebase uses brace-style, or direct string interpolation in Rust). The four parameters that need substitution are: `BinaryPath` (already in the credentials block), `YOUR_TOKEN` (already in the credentials block), `YOUR_ROOT` (already in the credentials block), `the title text` (the agent's chosen title). #107 should also instruct the agent that the title must be **single-line** with no embedded `\n`/`\r`/control chars (per §D.2 + LOW-1), or the verb will reject it.

Why this signature is PTY-clean:
- All four arguments are flag-named with explicit single-string values — no positionals, no shell expansion gotchas.
- `<YOUR_TOKEN>` and `<YOUR_ROOT>` are already in the agent's `# === Session Credentials ===` block (`session_context.rs:566-571`), so the prompt template can reference them by name without the backend computing or interpolating per-agent values.
- Title is a single double-quoted string; cmd.exe's PTY (ConPTY on Windows) accepts it identically to `send --to "..."` and `close-session --target "..."`, both of which work today. If the title contains a literal `"`, the agent must escape per cmd.exe rules (`\"`); this is an agent-side concern, not a verb-API concern.
- No nested subcommand path means the agent does not need to know an extra disambiguator.
- The agent already understands `--token` and `--root` from the GOLDEN RULE template — no new vocabulary.

The #107 branch will also handle: resolving the two known merge conflicts (`src-tauri/src/config/settings.rs`, `src/shared/types.ts`) against the new `main`, and an end-to-end PTY test of a Coordinator that has a brief. Out of scope for this plan.

---

## 11. Out-of-scope reaffirmation

Per #137 "Scope — out", the following are explicitly NOT in this plan:

- Body **replace** (overwriting the user's content) — risky semantics; defer until real usage of body-append surfaces a need.
- Body **unified-diff patching** — too complex for v1.
- Frontmatter fields **beyond `title:`** (e.g. `tags:`, `status:`) — add when concrete need surfaces.
- Per-line / per-section editing of the body — defer until append proves insufficient.
- **Non-coordinator** agents editing `BRIEF.md` — explicitly rejected.
- **Migration** of existing `BRIEF.md` files — out of scope per #107.
- **GUI** surface for these operations — agent-facing CLI only.
- **Backwards-compatibility shims** — the verbs are net-new; no compatibility layer needed.

---

## 12. Notes for the implementer (devs / grinch)

- **Re-use existing helpers, do not shadow them.** `agent_name_from_root` is already exported by `cli::send`; `is_any_coordinator` and `agent_fqn_from_path` are in `config::teams`; `workgroup_root` is in `phone::messaging`. None of these need wrappers — call them directly. (See `cli/close_session.rs:59,90,92` for the canonical pattern.)
- **Lock guard ordering.** Acquire the lock BEFORE reading the file. Releasing on Drop means an early `?` return on a parse error still releases. Do not attempt manual lock release inside the success path — let the guard handle it.
- **Do NOT add `serde_yaml`, `gray_matter`, `fs2`, `fd-lock`, `file-guard`, or `tempfile` to `Cargo.toml`.** All needed primitives are in `std` + already-present crates (`chrono`, `thiserror`, `log`).
- **Do NOT change `default_context()`** (`config/session_context.rs:478`). The whole architectural rationale of #137 is to keep that template stable.
- **Do NOT touch frontend code.** No `src/shared/types.ts` change, no `src/shared/ipc.ts` change, no Tauri command, no event.
- **Do NOT modify the `feature/107-auto-brief-title` branch.** That refactor lives in its own PR after this lands on `main`.
- **Windows specifics.** `std::fs::rename` on Windows replaces an existing destination via `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` — this is what `settings.rs::save_settings` relies on. No special casing needed. Lockfile creation via `OpenOptions::create_new(true)` maps to `CREATE_NEW`, atomic on NTFS.
- **Logging.** Add a single `log::info!("[brief] set-title: sender={} wg={} backup={:?}", sender, wg_root.display(), backup_path)` at the success point of each verb (mirrors the `log::info!` style at `session_context.rs:457`). Do not log the title or appended text — they may contain user-sensitive content.
- **clap derive.** Use `#[derive(clap::Args)]` exactly as `SendArgs`/`CloseSessionArgs` do. Use `#[arg(long)]` (not positional) for every flag, for consistency with the rest of the CLI surface.
- **Idempotence short-circuit (set-title `NoOp`).** Required for set-title (compare title VALUE semantically per MED-3 — see §5 idempotence section). Do it before backup creation. **Do NOT** apply to append-body — see §H.6.
- **Per-PID tmp path (HIGH-2).** Use `wg_root.join(format!("BRIEF.md.tmp.{}", std::process::id()))` for the tmp filename. Do NOT use the bare `BRIEF.md.tmp` of the original plan — that path is shared across writers and races with stale-lock recovery (HIGH-2). Each invocation gets its own PID-tagged tmp; old tmps from crashed writers are litter the directory may accumulate (acceptable trade-off for v1; sweep is a follow-up).
- **External-writer sentinel (HIGH-4).** After the read in step 2, immediately stat BRIEF.md and capture `(len, mtime)`. Just before the rename in step 7, re-stat and compare. If different, abort with `ExternalWrite(backup_path)`. The sentinel is racy at sub-millisecond resolution but catches the realistic editor-save case. Do NOT attempt exclusive-share-mode locking on BRIEF.md — too user-hostile.
- **BOM preservation (HIGH-3).** Carry `bom: bool` on `ParsedBrief`. Peel U+FEFF at the start of `parse_brief`; re-emit it at the start of `render`. Default for new files is `bom: false` (BRIEF.md is born BOM-less per `entity_creation.rs:193`).
- **Line-ending preservation (LOW-3).** Carry `line_ending: &'static str` ("\n" or "\r\n") on `ParsedBrief`. Detect from the first observed newline post-BOM. Use it for frontmatter delimiter and inter-line separators in `render`. The body slice is preserved byte-for-byte and never re-rendered, so its line-ending style is whatever the user had — appended text always uses `\n` literals (acceptable per §6 note).
- **Trim, don't `trim_end_matches(['\r','\n'])` for marker comparison (D.1).** Both at open-line check and at close-line check, use `.trim() == "---"` to tolerate trailing whitespace (`--- \n`, `\t---\n`).
- **Form B opening-offset (CRIT-1).** Hard-coding `consumed = "---\n".len()` is wrong for CRLF input. Pull the opening line out of `split_inclusive('\n')` first and use its actual `.len()` as the initial `consumed`. See §5 for the exact pseudocode.
- **Backup collision-suffix loop (B.2).** `OpenOptions::new().write(true).create_new(true).open(&candidate)` for `BRIEF.{ts}.bak.md`, then `BRIEF.{ts}.1.bak.md`, …, up to `.99.bak.md`. After the `create_new` succeeds, drop the file handle and call `std::fs::copy(&brief_path, &candidate)` to stream content. **On copy failure, explicitly `let _ = std::fs::remove_file(&candidate)` (§C.1) — `fs::copy` does NOT guarantee partial-file cleanup.**
- **Tmp-write partial-cleanup (MED-6).** On `fs::write(&tmp_path, &content)` failure (e.g. ENOSPC), `let _ = std::fs::remove_file(&tmp_path)` before returning `TmpWriteFailed`. Best-effort; do not propagate the cleanup error.
- **Rename retry loop (MED-4).** Wrap `fs::rename` in `for attempt in 0..=2:` with `thread::sleep(Duration::from_millis(100))` between attempts, retrying only on `ErrorKind::PermissionDenied`, `raw_os_error() == Some(32)` (ERROR_SHARING_VIOLATION), or `raw_os_error() == Some(5)` (ERROR_ACCESS_DENIED). Other errors fail immediately.
- **Control-char rejection at the verb boundary (D.2 + LOW-1).** In `brief_set_title::execute`: reject `--title` if any character `c.is_control() && c != '\t'`. In `brief_append_body::execute`: reject `--text` if any character `c.is_control() && c != '\n' && c != '\r' && c != '\t'`. Use the exact strings from the §3 error matrix.
- **Lock-stale window is 5 minutes** (`LOCK_STALE_AFTER_5M = Duration::from_secs(300)`), raised from 60 s per HIGH-2.
- **Clock-injection seam (G.1).** `pub fn perform(...)` calls `perform_inner(..., chrono::Utc::now)`. `perform_inner` is `pub(crate)` and takes a `now: F: FnOnce() -> chrono::DateTime<chrono::Utc>` argument. Tests call `perform_inner` with a deterministic `now` closure. Production callers go through `perform` and pay no overhead.
- **Trust model docstring on each verb module.** At the top of `brief_set_title.rs` and `brief_append_body.rs`, add a module-level doc-comment paragraph: *"Trust model: caller honestly reports their own `--root` and `--token`. The same model is inherited from `send`/`close-session` and has a known weakness (any well-formed UUID is accepted as a token, and `--root` is unverified). See plan #137 §3a for the escalation analysis. A follow-up issue is recommended to bind tokens to issued sessions, closing the hole for all CLI verbs simultaneously."*
- **Optional `// NOTE:` comment for LOW-2 clock-rewind.** A line near the timestamp formatting in `perform_inner`: `// NOTE: backup filenames sort by wall-clock; an NTP backward correction can break chronological ordering. Acceptable per spec; see plan #137 LOW-2.`

---

## Dev-Rust Round 1 Additions

> Reviewer: dev-rust. All references below verified against `feature/137-brief-cli-verb` HEAD (which tracks `origin/main` exactly — the plan file is the only diff).

### A. Reference verification — every cited file:line confirmed

| Architect cite                                       | Status     | Notes                                                                                              |
|------------------------------------------------------|------------|----------------------------------------------------------------------------------------------------|
| `phone/messaging.rs:215` (lockfile primitive)        | [ok] exact   | `OpenOptions::new().write(true).create_new(true).open(&path)` — actually used for collision-safe message-file CREATE (with numeric suffix retry), not as a lockfile, but the *primitive* is identical and the kernel-level atomicity guarantee transfers. |
| `config/settings.rs:459` (atomic tmp+rename)         | [ok] exact   | `pub fn save_settings` is at line 459; tmp+rename pattern at lines 470–474.                        |
| `commands/entity_creation.rs:152` (parse_role_frontmatter) | [ok-with-caveat] | The reference exists and shape is similar, but **the existing parser is more lenient** than the plan's `parse_brief`: it tests `content.starts_with("---")` (no `\n`!) and finds the close via naive `find("---")` — would mis-parse `---blob---` as frontmatter. The plan's parser is **a strict superset**, not a mirror; see §D below. |
| `commands/entity_creation.rs:232-235` (YAML escape)  | [ok] exact   | `description.replace('\'', "''")` then `format!("description: '{}'", desc_yaml)`.                  |
| `commands/entity_creation.rs:193` (LF-only)          | [ok] exact   | `Some(content) => format!("{}\n", content)`.                                                       |
| `commands/entity_creation.rs:169` (title-line detection) | [note] uses `strip_prefix("name:")` not `starts_with("title:")`  | Both work; the plan correctly chose `trim_start().starts_with("title:")` because it must preserve indentation when re-rendering the line, which `strip_prefix` would discard. |
| `cli/close_session.rs::execute` lines 41–101         | [ok] exact   | All cited lines (--root presence: 42–48, token validate: 50–57, sender derivation: 59, coord gate: 89–101) match. |
| `cli/close_session.rs:59,90,92` (canonical pattern)  | [ok] exact   | Note: close_session uses `is_coordinator_of(sender, target, teams)` at line 92, NOT `is_any_coordinator`. The plan correctly chose `is_any_coordinator` for brief-* (justified in plan §3) — the divergence is intentional. |
| `config::teams::is_any_coordinator`                  | [ok] exists  | At `teams.rs:480`. §AR2-strict via `is_coordinator` at `teams.rs:403`. The hot-path regression guard `is_any_coordinator_requires_qualified_fqn` at `teams.rs:1046` confirms the strict-FQN behaviour the plan relies on. |
| `config::teams::agent_fqn_from_path`                 | [ok] exists  | At `teams.rs:62`. Subdirectory-CWD test `agent_fqn_from_path_deeper_cwd_returns_replica_fqn` at `teams.rs:734` confirms a deep CWD inside a replica still resolves to the replica FQN — relevant to §F below. |
| `config::teams::discover_teams`                      | [ok] exists  | At `teams.rs:540`. Walks `settings.project_paths` + immediate non-dot children. Already invoked by `send` and `close-session`; cost is amortised. |
| `phone::messaging::workgroup_root`                   | [ok] exists  | At `messaging.rs:54`. Pure path operation (no filesystem touch — *important* for §F). Walks `ancestors()`, so a path that *is* a `wg-N-*` dir resolves to itself. |
| `cli::send::agent_name_from_root`                    | [ok] exists  | At `send.rs:72`. Thin wrapper over `agent_fqn_from_path`.                                          |
| `phone/messaging.rs:349` (`unique_tmp` helper)       | [ok] exact   | At line 349 inside `mod tests`. PID + thread-id + nanos hash. Self-cleaning is **not** built in — the test must rely on `FixtureRoot`-style Drop cleanup. |
| `config/teams.rs:787` (`FixtureRoot`)                | [ok] exact   | Drop-based cleanup at 788–792; `FixtureRoot::new` at 793–810. Both patterns are inside `#[cfg(test)] mod tests` — the plan needs them re-imported per-module or copied locally; cannot cross-module-share without making them `pub(crate)`. |
| `config/session_context.rs:478` (`default_context`, GOLDEN RULE) | [ok] exact   | Untouched by this plan — confirmed.                                                                |
| `session_context.rs:457` (`log::info!` style)        | [ok] approximate | Line 457 is the `Materialized managed agent context` log; style is `log::info!("…", …)`. The plan's success log at §12 follows this style. |
| `session_context.rs:566–571` (credentials block)     | [ok] documented in CLAUDE.md, generated by `default_context()`; the plan's #107 PTY-prompt note in §10 reads correctly against this. |
| `cli/mod.rs:1-5` (mod declarations)                  | [ok] exact   | Currently 5 lines (close_session, create_agent, list_peers, list_sessions, send). See §H.1 for placement specifics. |
| `cli/mod.rs:27-38` (Commands enum)                   | [ok] exact   | `Commands` has 5 variants at lines 27–38.                                                          |
| `cli/mod.rs:104-110` (handle_cli)                    | [ok] exact   | Match arms at lines 104–110. See §H.3.                                                             |
| `Cargo.toml`                                         | [ok] verified | `chrono = "0.4"` (with `serde` feature), `thiserror = "2"`, `clap = "4"` (with `derive`), `log = "0.4"`. No new deps required by this plan. |

**Why the table:** the implementer's first task is to apply edits to the lines the architect named. If any cite were stale, the diff would land in the wrong place silently. This audit costs ~10 minutes and saves a debug hour.

### B. Concurrency model — additional findings

#### B.1. Stale-lock recovery is bounded by `create_new` atomicity (no actual hazard)

**Why:** A future reader might worry: "two writers race in `remove_file` → both `create_new` after detecting the same stale lock → corruption?" Trace:

1. A and B both detect stale lock (mtime > 60s).
2. A: `let _ = fs::remove_file(path)` → succeeds.
3. B: `let _ = fs::remove_file(path)` → returns `NotFound`; ignored by `let _ =`.
4. A: `OpenOptions::new().create_new(true).open(path)` → succeeds, A holds lock.
5. B: `OpenOptions::new().create_new(true).open(path)` → returns `AlreadyExists`. mtime is fresh (just set by A). B falls through to the timeout/sleep branch and waits. No corruption.

`create_new` is the kernel-level mutex; one and only one process succeeds. The `let _ = remove_file` swallowing the NotFound error is correct — it's exactly the case "someone else removed the stale file before us, which is fine."

**How to apply:** No change needed. Document the analysis as a comment at the `LockGuard::acquire` impl so future maintainers don't re-derive it.

#### B.2. **Backup-timestamp collision risk — change requested**

**The hazard:** `BRIEF.{ts}.bak.md` with `ts = Utc::now().format("%Y%m%d-%H%M%S")` has 1-second resolution. The lock serialises writes, but two consecutive coordinator invocations 100ms apart will **share the same timestamp**. `std::fs::copy` overwrites the destination by default → second backup obliterates first. Audit-trail loss.

**Why this matters:** Issue #137 requires "each write … creates a timestamped backup" — implicitly, **each backup must persist as a distinct artifact**. Silent overwrite violates the spirit of the criterion.

**How to apply:** mirror `phone/messaging.rs:208-220`'s collision-suffix loop. Pseudocode:

```text
for n in 0..=99:
    let candidate = if n == 0 {
        wg_root.join(format!("BRIEF.{}.bak.md", ts))
    } else {
        wg_root.join(format!("BRIEF.{}.{}.bak.md", ts, n))
    };
    match OpenOptions::new().write(true).create_new(true).open(&candidate):
        Ok(file) => {
            // Stream BRIEF.md into `file`, then close. (Alternative: build the path,
            // close `file` immediately, then std::fs::copy(brief_path, &candidate) —
            // but that re-opens with truncate(true) which loses the create_new
            // exclusivity. Prefer manual stream copy.)
            return Ok(candidate);
        }
        Err(e) if e.kind() == AlreadyExists => continue,
        Err(e) => return Err(BackupFailed(candidate, e)),
}
return Err(BackupExhausted);
```

99 retries is overkill but consistent with the messaging-module convention. In practice, even 2 calls/second never hit `n=2`.

Add new error variant `BriefOpError::BackupExhausted` and new error string: `Error: failed to write backup at <path>: 100 collision retries exhausted in the same second. Aborting; BRIEF.md left unchanged.` Update the error matrix in §3.

Add unit test `backup_collision_within_same_second_does_not_clobber_prior_backup`: mock the clock (see §G.1), run two `perform`s with the same `ts`, assert two distinct backup files exist on disk after.

### C. Backup failure semantics — explicit cleanup

#### C.1. **`std::fs::copy` does NOT guarantee partial-file cleanup on failure — change requested**

**The hazard:** Plan §4 line 189 claims: *"no `BRIEF.md.tmp` or partial backup is left behind because `copy`'s default behaviour creates the target as a new file (failures clean up via the OS)."* The Rust stdlib **does not document any such guarantee**. `fs::copy` opens the destination with `create(true).truncate(true).write(true)`, then writes from source. If the source read fails mid-stream (e.g., disk-block error), the destination remains as a partially-written file; the OS does **not** unlink it.

(Confirmation: `std::fs::copy` is a single function in Rust — the *only* current consumer in this codebase is `phone/mailbox.rs:1752` and it's used as a *fallback for cross-volume rename*, with no concern for partial-file cleanup. We have no precedent here.)

**Why this matters:** Issue #137: "If the backup fails for any reason, ABORT the write and exit non-zero so the caller is aware." The plan correctly aborts, but a corrupted/partial `BRIEF.{ts}.bak.md` left on disk *looks* like a successful backup to a future reader, masking the failure. Worse, it can collide with a subsequent legitimate backup attempt at the same timestamp (compounding §B.2).

**How to apply:** On `fs::copy` error, explicitly delete the partial file before returning. Pseudocode:

```text
match std::fs::copy(&brief_path, &bp):
    Ok(_) => Some(bp),
    Err(copy_err) => {
        let _ = std::fs::remove_file(&bp);   // best-effort; don't shadow original error
        return Err(BackupFailed(bp, copy_err));
    }
```

Note: `let _ = remove_file(&bp)` is intentional — if the partial file never got created, `remove_file` returns NotFound; we don't care. The original copy error is what we surface.

If we adopt the §B.2 collision-suffix loop, the cleanup becomes the inner `Err(e)` branch of that loop.

#### C.2. Lockfile cleanup on backup failure — confirmed correct

The lock is acquired at the top of `perform`. The `BackupFailed` error is returned via `?`, which unwinds the function scope; `LockGuard::Drop` fires; lockfile is removed. Confirmed.

**Why this matters:** confirms a tech-lead question explicitly. Worth pinning in a unit test:

Add unit test `backup_failure_releases_lockfile` (alongside U24): inject a backup failure, assert `BRIEF.md.lock` does not exist on disk after the call returns.

### D. Frontmatter parser — extra edge cases

#### D.1. **Trailing-whitespace tolerance on `---` markers — change requested**

**The hazard:** The plan's `parse_brief` (§5) tests `s.starts_with("---\n")` for the open and `line.trim_end_matches(['\r','\n']) == "---"` for the close. Real-world files often have trailing whitespace on these markers — `--- \n` (single trailing space) is invisible in editors but breaks both checks:

- Open: `--- \n` does not start with `---\n` → parser treats whole file as bodyless. **A subsequent `brief-set-title` invocation prepends a NEW frontmatter ABOVE the existing pseudo-frontmatter, leaving the file with two `---` blocks.** Latent corruption.
- Close: `--- \n` does not equal `"---"` after `trim_end_matches(['\r','\n'])` (it's `"--- "`) → parser keeps walking; if no other line is exactly `---`, returns `has_frontmatter:false` and the file is mis-classified.

**Why this matters:** Robustness against user-edited files. We cannot assume `BRIEF.md` was always written by our binary — the issue is explicit that pre-existing user-edited content must be preserved.

**How to apply:** trim whitespace before comparing markers, both at open and close.

```text
# Open: tolerate "---" + any trailing whitespace + newline
let first_line_end = s.find('\n').unwrap_or(s.len());
let first_line = &s[..first_line_end];
if first_line.trim() != "---":
    return ParsedBrief { has_frontmatter: false, ..., body: s.to_string() }

# Close: trim full whitespace (not just \r\n)
if line.trim() == "---":
    closed = true; break
```

Add unit test `parse_brief_tolerates_trailing_space_on_markers`: input `"--- \ntitle: x\n--- \nbody"` → `has_frontmatter:true, frontmatter:["title: x"]`.

#### D.2. **Reject embedded `\r`/`\n` in `--title` — change requested**

**The hazard:** Plan §5 line 319 dismisses multiline titles as "user error". However, with `bash -c 'binary brief-set-title --title "$(echo -e line1\\nline2)"'`, the shell delivers a real `LF` byte in the argv. The plan's YAML escape does NOT escape newlines. The output `title: 'line1\nline2'` (with literal LF) would either (a) confuse YAML parsers — single-quoted YAML scalars per spec 1.2 don't permit unescaped newlines except as folding — or (b) silently land as a multi-line scalar that breaks `is_title_line` detection on a subsequent invocation (because the `title:` prefix is on line N but line N+1 is the second half of the value, which doesn't start with `title:`).

**Why this matters:** silent file corruption from an undetectable input. Defensive validation costs one line.

**How to apply:** in `brief_set_title::execute`, before passing through to `perform`, add:

```text
if args.title.contains('\n') || args.title.contains('\r'):
    eprintln!("Error: --title must be a single line (no newlines).")
    return 1
```

Add the new error string to the §3 matrix. Add unit test `set_title_rejects_embedded_newlines`.

`brief-append-body --text` does NOT need this restriction — embedded newlines in body text are explicitly supported by §6 ("Internal newlines inside `text` are preserved, allowing multi-paragraph appends").

#### D.3. The plan's `parse_brief` is a strict superset of `parse_role_frontmatter`, not a mirror

**Why this matters:** future maintainers reading "mirroring `parse_role_frontmatter`" might assume behavioural parity. They are NOT identical:

| Aspect                                         | `parse_role_frontmatter` (existing)    | `parse_brief` (this plan, with §D.1 fix)  |
|------------------------------------------------|----------------------------------------|--------------------------------------------|
| Open marker                                    | `content.starts_with("---")`           | First line trimmed equals `---` (newline-aware) |
| Close marker                                   | `rest.find("---")` (substring match)   | First line trimmed equal to `---` (line-aware) |
| Output                                         | `(Option<String>, Option<String>)` (just name + description) | `ParsedBrief { has_frontmatter, frontmatter: Vec<String>, body }` (preserves all lines + body) |
| CRLF tolerance                                 | implicit / accidental                  | explicit |
| Quote-stripping on values                      | yes (`trim_matches('"').trim_matches('\'')`) | no — preserves raw lines verbatim because we re-write them |

**How to apply:** change the plan's wording in §5 from *"mirroring `parse_role_frontmatter`"* to *"inspired by `parse_role_frontmatter`'s shape, but with stricter line-aware open/close detection (existing helper would mis-parse `---blob---` as a frontmatter block)."* The implementer should NOT consult `parse_role_frontmatter` for behavioural parity.

### E. CLI surface coherence

#### E.1. clap kebab-case conversion — confirmed automatic

**Why:** clap derive translates `BriefSetTitle` PascalCase → `brief-set-title` kebab-case automatically (default `rename_all = "kebab-case"`). The existing `CloseSession` → `close-session` confirms this. No `#[command(name = "brief-set-title")]` annotation is needed unless we want override.

**How to apply:** rely on the default; add a doc-comment `/// Set the title field in the workgroup BRIEF.md frontmatter (coordinator-only)` so the auto-generated `--help` block reads cleanly. Match the docstring style of `/// Send a message to another agent` at `cli/mod.rs:28`.

#### E.2. Multi-line `--text` from PTY chain (#107) — acceptable

**Why:** `cmd.exe` and PowerShell both deliver embedded LF/CRLF in a quoted argument as literal newline bytes in argv. The plan's `--text` accepts arbitrary strings; `apply_append_body` preserves internal newlines (§6). End-to-end this works. The only friction is shell quoting on the *agent's* side, which is the agent's concern (analogous to `send --to "..."`).

**Note for #107:** the auto-brief-title chain's PTY prompt should instruct the agent to put the title on **one line** with no embedded newlines (per §D.2). #107's branch will own that prompt-template tweak.

#### E.3. Token-root binding is NOT enforced — inherited trust assumption

**Why this matters for awareness:** `validate_cli_token` accepts any valid UUID as a non-root token, with no binding to `--root`. A coordinator-of-WG-1 could in theory pass `--root <fake-path-impersonating-wg-19-coordinator>` + their own UUID token, derive the wg-19 coordinator's FQN via `agent_fqn_from_path`, and pass `is_any_coordinator`. The structural defense is the GOLDEN RULE: agents can only create files inside their own replica root, so they cannot synthesise a believable path on disk. The token+root pair is trusted because both come from the agent's own credentials block, which lives inside its own replica.

**Same trust model is inherited by `send` and `close-session`** — this is not new attack surface introduced by #137.

**How to apply:** no change. Document in `brief_set_title.rs` and `brief_append_body.rs` module-level doc comments: *"Trust model: caller honestly reports their own --root and --token (per the GOLDEN RULE confinement and the credentials-block contract). This matches `send` and `close-session`."*

### F. Path resolution edge cases

#### F.1. Subdirectory `--root` resolves to the replica FQN — confirmed

**Why:** `agent_fqn_from_path` uses `rposition` to find the right-most `.ac-new` segment, so a CWD like `<...>/.ac-new/wg-19-team/__agent_alice/some/deep/dir` resolves to `<project>:wg-19-team/alice` (test `agent_fqn_from_path_deeper_cwd_returns_replica_fqn`). `workgroup_root` likewise walks ancestors so it returns `<...>/.ac-new/wg-19-team` regardless of how deep the CWD is. **No issue.**

#### F.2. `--root` at workgroup-root (no `__agent_*` parent) — gate fails closed

**The case:** a Coordinator agent whose CWD is `<...>/.ac-new/wg-19-dev-team/` directly (not `<...>/__agent_X/`). Walk-through:

- `workgroup_root` walks ancestors, matches the WG dir on the very first iteration, returns it. OK.
- `agent_fqn_from_path` requires `parts[ac_idx+1].starts_with("wg-")` AND `parts[ac_idx+2].starts_with("__agent_")` — second clause fails because `parts[ac_idx+2]` doesn't exist (or is something arbitrary). Falls through to `agent_name_from_path`, which strips `__agent_/_agent_` prefixes and returns `<parent>/<last>`. For `.../.ac-new/wg-19-dev-team/`, `agent_name_from_path` returns `.ac-new/wg-19-dev-team` — clearly NOT a coordinator FQN.
- `is_any_coordinator(".ac-new/wg-19-dev-team", teams)` → false → reject with the standard authorization error.

**Verdict:** safe. A Coordinator running directly from a WG-root CWD will be rejected with the same error a non-coordinator gets. This is correct behaviour — Coordinators are agents and live in `__agent_*` replicas; running from a WG-root CWD is an irregular situation.

**How to apply:** no change. Add a clarifying integration test `set_title_rejects_when_root_is_workgroup_root_directly` to pin the behaviour.

#### F.3. Symlinked `--root` is text-resolved — inherited

**Why:** `workgroup_root` is documented "no canonicalization." A symlinked `--root` is matched on its textual ancestors, not the symlink target. Same model as `send --send`. Inherit.

**How to apply:** no change.

### G. Test plan additions

#### G.1. **U24 injection technique needs clarification**

**The plan's wording:** *"inject a `copy` failure (point backup_path at a directory that doesn't exist for `copy`'s parent)"*. This does NOT work — `backup_path`'s parent IS `wg_root`, which the test creates. We need a real injection mechanism.

**Recommended approach (least invasive):** make the timestamp injectable via a `TestClock` parameter on `perform` — but in a way that production code doesn't pay any cost. Concretely:

```rust
// In brief_ops.rs:
fn perform_inner<F: FnOnce() -> chrono::DateTime<chrono::Utc>>(
    wg_root: &Path, op: BriefOp, now: F
) -> Result<EditOutcome, BriefOpError> { … }

pub fn perform(wg_root: &Path, op: BriefOp) -> Result<EditOutcome, BriefOpError> {
    perform_inner(wg_root, op, chrono::Utc::now)
}
```

**Why:** with a deterministic timestamp, the test pre-creates a *directory* at the predictable backup path before calling `perform_inner` — `std::fs::copy` to a path that's a directory returns `Err(io::Error)` reliably on both Windows (`ERROR_ACCESS_DENIED` or `ERROR_INVALID_NAME`) and Unix (`EISDIR`).

```rust
#[test]
fn backup_failure_aborts_write_and_preserves_brief() {
    let fixture = FixtureRoot::new("brief-u24");
    let wg = fixture.0.join("wg-1-test");
    std::fs::create_dir_all(&wg).unwrap();
    let brief = wg.join("BRIEF.md");
    std::fs::write(&brief, "old\n").unwrap();
    let original = std::fs::read(&brief).unwrap();
    let fixed_now = || chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let backup_path = wg.join("BRIEF.20260101-000000.bak.md");
    std::fs::create_dir(&backup_path).unwrap();   // <-- the trap
    let result = perform_inner(&wg, BriefOp::SetTitle("x".into()), fixed_now);
    assert!(matches!(result, Err(BriefOpError::BackupFailed(_, _))));
    assert_eq!(std::fs::read(&brief).unwrap(), original);
    assert!(!wg.join("BRIEF.md.lock").exists());   // also covers C.2
    assert!(!wg.join("BRIEF.md.tmp").exists());
}
```

Same `perform_inner` seam supports B.2's `backup_collision_within_same_second_does_not_clobber_prior_backup` test.

**How to apply:** add §H.4 with the exact `BriefOp` enum and the `perform`/`perform_inner` split.

#### G.2. New unit tests (additions to plan §9)

| #   | Test fn name                                                   | Asserts                                                                                                                |
|-----|----------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------|
| U26 | `parse_brief_tolerates_trailing_space_on_markers`              | per §D.1                                                                                                               |
| U27 | `parse_brief_unicode_in_body_preserved_byte_for_byte`          | round-trip a body with non-ASCII (`"café\n"`) — guards the byte-offset-slice approach in `parse_brief`                  |
| U28 | `apply_set_title_preserves_indentation_of_existing_title_line` | `"---\n  title: old\n---\n"` + `"new"` → `"---\n  title: 'new'\n---\n"` (leading whitespace preserved per plan §5)     |
| U29 | `backup_collision_within_same_second_does_not_clobber_prior_backup` | per §B.2 — requires §G.1 clock-injection seam                                                                          |
| U30 | `backup_failure_releases_lockfile`                             | per §C.2                                                                                                               |
| U31 | `apply_set_title_rejects_yaml_breaking_value_with_real_newline` | (lives at the `execute` layer, technically — but a `BriefOp::SetTitle("a\nb")` in `perform` should also reject upstream of the file system. Place in `brief_set_title.rs::tests`.) |

| #   | Test fn name                                                   | Asserts                                                                                                                |
|-----|----------------------------------------------------------------|------------------------------------------------------------------------------------------------------------------------|
| I17 | `set_title_rejects_embedded_newlines`                          | per §D.2 — `--title "a\nb"` returns exit 1 with the new error string, BRIEF.md unchanged                              |
| I18 | `set_title_rejects_when_root_is_workgroup_root_directly`       | per §F.2 — `--root <wg_root>` returns exit 1 with the standard authorization error                                     |
| I19 | `append_body_preserves_internal_newlines_in_text`              | append `"line1\nline2\n\nline4"` → all three internal newlines survive verbatim into the body                          |

Update §9's "Mapping from issue acceptance checklist → tests" table to include U30 alongside U24 for "backup failure aborts write," and U29 alongside U23 for "each write creates timestamped backup".

#### G.3. Acceptance-criteria-13 (issue's "Unit tests cover at minimum" list) — fully covered

Cross-checked the issue-13 minimum-coverage list against the plan + my additions:

| Issue-13 minimum                           | Plan tests                              |
|--------------------------------------------|-----------------------------------------|
| auth pass                                  | I1, I2                                  |
| auth fail (non-coordinator)                | I3, I4                                  |
| auth fail (invalid token)                  | I5                                      |
| backup creation                            | I7, I8 + U23                            |
| no-frontmatter case (creates one)          | I14 + U7                                |
| existing-frontmatter case (preserves others)| I13 + U10                              |
| body append preserves prior content        | U15 + I19 (new)                         |
| concurrent write atomicity                 | I15 + U20, U21, U25                     |

All covered.

### H. Implementation specifics for the impl phase

#### H.1. Module placement in `cli/mod.rs:1-5`

Plan §8 says "alphabetical order" but doesn't specify location. With the existing five mods sorted, `brief_*` sorts BEFORE `close_session`, so the FINAL `cli/mod.rs:1-8` becomes:

```rust
pub mod brief_append_body;
pub mod brief_ops;
pub mod brief_set_title;
pub mod close_session;
pub mod create_agent;
pub mod list_peers;
pub mod list_sessions;
pub mod send;
```

**Why explicit:** prevents an "interpret alphabetical order yourself" mismerge if two implementers touch this file simultaneously.

#### H.2. `Commands` enum variant ordering

Plan §8 says "place them after `CloseSession` to keep the existing entries' positions stable." Confirmed sound — clap renders subcommands in declaration order in `--help`, and inserting at the end avoids visual reflow of pre-existing help output.

```rust
#[derive(Subcommand)]
pub enum Commands {
    Send(send::SendArgs),
    ListPeers(list_peers::ListPeersArgs),
    ListSessions(list_sessions::ListSessionsArgs),
    CreateAgent(create_agent::CreateAgentArgs),
    CloseSession(close_session::CloseSessionArgs),
    /// Set the title field in the workgroup BRIEF.md frontmatter (coordinator-only)
    BriefSetTitle(brief_set_title::BriefSetTitleArgs),
    /// Append text to the body of the workgroup BRIEF.md (coordinator-only)
    BriefAppendBody(brief_append_body::BriefAppendBodyArgs),
}
```

#### H.3. `handle_cli` match arms — exact lines to add

After `Commands::CloseSession(args) => close_session::execute(args),` at line 109, add:

```rust
Commands::BriefSetTitle(args) => brief_set_title::execute(args),
Commands::BriefAppendBody(args) => brief_append_body::execute(args),
```

#### H.4. `BriefOp` enum + `BriefOpError` thiserror enum (concrete)

The plan implies a unified `perform(wg_root, op)` but does not name the `op` type. Concrete proposal:

```rust
pub enum BriefOp {
    SetTitle(String),
    AppendBody(String),
}

#[derive(Debug, thiserror::Error)]
pub enum BriefOpError {
    #[error("BRIEF.md is locked by another writer (5s timeout). Try again.")]
    LockTimeout,
    #[error("failed to acquire BRIEF.md lock at {0}: {1}. Aborting; BRIEF.md left unchanged.")]
    LockIo(PathBuf, std::io::Error),                                                // MED-5: matrix-aligned wording
    #[error("failed to read BRIEF.md at {0}: {1}")]
    ReadFailed(PathBuf, std::io::Error),
    #[error("failed to write backup at {0}: {1}. Aborting; BRIEF.md left unchanged.")]
    BackupFailed(PathBuf, std::io::Error),
    #[error("failed to write backup at {0}: 100 collision retries exhausted in the same second. Aborting; BRIEF.md left unchanged.")]
    BackupExhausted(PathBuf),                                                       // §B.2
    #[error("failed to write {0}: {1}. Aborting; BRIEF.md left unchanged.")]
    TmpWriteFailed(PathBuf, std::io::Error),                                        // path now carries the per-PID tmp name (HIGH-2)
    #[error("BRIEF.md was modified externally between read and write; aborting. Backup at {0} retains the externally-modified state.")]
    ExternalWrite(PathBuf),                                                         // HIGH-4 — invariant-Some (sentinel only fires when file_existed → backup_path is Some)
    // RenameFailed: custom Display impl below — `Option<PathBuf>` cannot use the
    // derived `#[error("...{1}...")]` formatting because `{1}` would Debug-print
    // `Some("path")` / `None`. Variant declared without a derive template.
    RenameFailed(std::io::Error, Option<PathBuf>),                                  // MED-4: emitted only after 3-attempt retry exhausted
}

// Custom Display for RenameFailed (the Option<PathBuf> case — `None` happens for
// the brand-new-file path, where rename of the per-PID tmp into a non-existent
// BRIEF.md fails before any backup is written; `Some(p)` is the normal case):
impl std::fmt::Display for BriefOpError:
    # ... other arms forwarded to thiserror's derived impls ...
    BriefOpError::RenameFailed(io_err, Some(bp)) =>
        write!(f, "failed to publish BRIEF.md (rename): {}. Backup at {} retains the prior state.", io_err, bp.display()),
    BriefOpError::RenameFailed(io_err, None) =>
        write!(f, "failed to publish BRIEF.md (rename): {}. No backup (BRIEF.md did not exist before).", io_err),
```

The `Display` impls are wired so that `eprintln!("Error: {}", e)` in `execute()` produces exactly the strings in §3's error matrix (with the §B.2, MED-5, MED-6, and HIGH-4 additions). Note the signature changes since dev-rust round 1:
- `LockIo` Display now matches the §3 matrix wording.
- `TmpWriteFailed` carries the actual `PathBuf` (per-PID tmp file) so the user-facing string names the exact file that failed. The Display uses `{0}` (which forwards to `Path`'s `Display` via `PathBuf: Display`-equivalent through `thiserror`'s field formatter) — the matrix placeholder `<absolute-tmp-path>` reflects this.
- `ExternalWrite(PathBuf)` (round-3 tightening): always-`Some` by invariant (sentinel-fired ⇒ `file_existed` ⇒ `backup_path: Some(_)`). Storing `PathBuf` instead of `Option<PathBuf>` lets the derived Display use `{0}` (clean path string) instead of `{0:?}` on an Option (which would render as `Some("path")`).
- `RenameFailed(std::io::Error, Option<PathBuf>)` keeps the Option because the brand-new-file path has no prior backup. The custom `impl Display` above covers both arms with clean wording (no `{1:?}` Debug formatting).

#### H.5. Logging — exact log line at success point

Per role's logging rules and plan §12:

```rust
log::info!(
    "[brief] set-title: sender={} wg={} backup={}",
    sender,
    wg_root.display(),
    backup_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<no prior file>".into())
);
```

Same shape for append-body. **Do NOT log the title or text payload** — they may contain user-sensitive content (the role's logging rules require this restraint).

#### H.6. Success-message string consistency between §2 and §4

Minor: plan §2 says append-body's success line is `"BRIEF.md body appended; backup: <abs path>"` (no "updated"), but plan §4's template `"BRIEF.md <op> updated; backup: {p}"` with `op = "body appended"` would produce `"BRIEF.md body appended updated; backup: …"` — grammatically broken.

**Resolution:** drop the shared template; emit the strings per-verb:

| Verb               | EditOutcome::Wrote{Some(p)}                        | Wrote{None}                                          | NoOp                                                 |
|--------------------|----------------------------------------------------|------------------------------------------------------|------------------------------------------------------|
| `brief-set-title`   | `"BRIEF.md title updated; backup: {p}"`           | `"BRIEF.md created; no prior content to back up"`   | `"BRIEF.md unchanged (title value already matches)"`|
| `brief-append-body` | `"BRIEF.md body appended; backup: {p}"`           | `"BRIEF.md created; no prior content to back up"`   | (NoOp not produced by append-body — see below)       |

**Why no-NoOp for append-body:** an append always changes the file, even if `text` is the same as last time. The idempotence short-circuit applies only to set-title.

#### H.7. Lockfile and tmp paths — clarification

Plan §4 uses `BRIEF.md.lock` and `BRIEF.md.tmp.<pid>` siblings of `BRIEF.md` (the latter changed from bare `BRIEF.md.tmp` per HIGH-2 in round 2 — per-PID suffix prevents tmp-collision races during stale-lock recovery). Issue #137 says *"No file other than BRIEF.md and its `*.bak.md` siblings is touched."* — both extra siblings are operational artifacts that exist only transiently under the lock. The integration tests I11/I12 must verify that, in the post-call snapshot, `BRIEF.md.lock` is absent AND no `BRIEF.md.tmp.*` files exist. Their absence proves the operational invariant.

**Crashed-writer litter:** if a writer dies mid-`fs::write(BRIEF.md.tmp.<pid>)`, the per-PID tmp file remains on disk after the lock is recovered by the next writer. This is acceptable for v1 (the next time the same PID is reused — which on Windows is unlikely — its `fs::write` will overwrite). A best-effort cleanup at lock-acquire (`for entry in read_dir(wg_root): if entry starts with "BRIEF.md.tmp." and the suffix is a numeric PID that's no longer alive, remove`) is a reasonable follow-up but not required for #137.

---

### Round-2 consensus items (architect, please weigh in)

The following items push back against the plan and need the architect's call before implementation. They are listed in priority order (highest first):

1. **§B.2 — Backup-timestamp collision risk.** 1-second resolution + lock-released-then-reacquired creates an audit-trail gap. Recommend collision-suffix loop (mirrors `phone/messaging.rs:208`). Adds `BackupExhausted` error variant + new unit test U29. **Hard pushback** — affects acceptance criterion "each write creates a timestamped backup."

2. **§C.1 — Backup partial-file cleanup.** The plan's claim that `fs::copy` cleans up on failure is unsupported. Recommend explicit `let _ = remove_file(&bp)` on copy failure. Small change, but correctness-critical.

3. **§D.2 — `--title` newline rejection.** Real risk of silent YAML breakage from shell-delivered LFs. One-line check at the verb boundary; new error string in §3 matrix. **Hard pushback** — silent corruption surface.

4. **§D.1 — Frontmatter parser tolerance.** Trailing-whitespace on `---` markers is a real footgun for user-edited files. `line.trim() == "---"` instead of `trim_end_matches(['\r','\n'])` for both open and close. Soft-strong; user-affecting.

5. **§G.1 — U24 injection mechanism.** Plan as written doesn't actually inject a failure. Recommend the `perform_inner` clock-injection seam — also used by U29.

6. **§H.6 — Success-message inconsistency between §2 and §4.** §4's template generates a broken string for append-body. Per-verb strings instead.

The remaining items in §A–§H are clarifications and reasoning — not pushback — and can be applied to the plan as documentation without architect re-review.

---

## Grinch Round 1 Findings

> Reviewer: dev-rust-grinch (adversarial). Read against `feature/137-brief-cli-verb` HEAD = `origin/main` (the plan file is the only diff). Severity scale: **CRIT** = system-broken / architect must amend before code; **HIGH** = real failure mode that must be addressed before merge; **MED** = correctness or robustness gap; **LOW** = polish or rare edge case; **NIT** = wording / completeness. Each finding is **input → behavior → consequence → fix**.

### Severity counts

- **CRIT:** 1
- **HIGH:** 4
- **MED:** 6
- **LOW:** 3
- **NIT:** 4

### CRIT-1 — `parse_brief` byte-offset bug for CRLF-opened files

**Where:** plan §5, the `parse_brief` pseudocode line `let mut consumed = "---\n".len()`.

**Input:** any `BRIEF.md` whose opening line is `"---\r\n"` (5 bytes), e.g. a file edited and saved by Notepad, vanilla VS Code on Windows without an .editorconfig override, or any tool that follows the OS native line-ending convention.

**Behavior:** `consumed` is initialized to `"---\n".len() == 4`, but the actual opening consumed by `split_inclusive('\n').skip(1)` is 5 bytes. Walk:
- Input bytes: `"---\r\n"`(0..5) `"title: x\r\n"`(5..15) `"---\r\n"`(15..20) `"body"`(20..24).
- Loop accumulates `consumed = 4 + 10 + 5 = 19` when the close is matched.
- `body = s[consumed..] = s[19..] = "\nbody"` — i.e. the `\n` that belongs to the closing `---\r\n` is **incorrectly included as the first byte of the body**.

**Consequence:** silent file mutation. Every CRLF-opened `BRIEF.md` that goes through `brief-set-title` (or any future op that calls `render`) gains a stray leading blank line between the closing `---` and the original body. Cumulative damage stops after the first invocation (subsequent invocations parse correctly because the now-LF opening is exactly 4 bytes), but the user-visible damage is permanent: a `# Heading` immediately under the closing `---` is shifted down by an extra blank line. Backup captures the pre-damage state but the user is not told to consult it.

dev-rust did not catch this in round 1.

**Fix:** initialize `consumed` from the actual length of the first split-inclusive yield instead of hard-coding it. Two equivalent forms; either is fine, the second is slightly clearer:

```text
# Form A — explicit branch
let opening_len = if s.starts_with("---\r\n") { 5 } else { 4 };
let mut consumed = opening_len;

# Form B — pull the opening out of the iterator first
let mut iter = s.split_inclusive('\n');
let opening = iter.next().expect("starts_with check above guarantees ≥1 line");
let mut consumed = opening.len();
let after_open = iter;   # already advanced past the opening
```

**Pin in tests:** rewrite U6 to assert the body byte-exactly: `assert_eq!(p.body, "body")` for input `"---\r\ntitle: x\r\n---\r\nbody"`. Vague wording "body preserves CRLF" passes for the wrong reason (see MED-1). Also add a positive round-trip test:

```text
U-CRLF-RoundTrip: input "---\r\ntitle: old\r\n---\r\nbody\r\n";
  apply set_title("new"); render; assert no extra blank line between
  closing `---\n` and the start of "body".
```

**Architect must weigh in before code starts.**

### HIGH-1 — Token-to-`--root` binding NOT enforced (inherited, **AMPLIFIED**)

**Where:** plan §3 auth flow + §E.3 (acknowledged but dismissed by dev-rust).

**Input:** any agent that holds a valid session token (any UUIDv4 it has minted itself satisfies `validate_cli_token` — see `cli/mod.rs:88`, which only checks UUID parseability, never that the UUID corresponds to an actual issued session). Caller is wg-19-dev-team/dev-rust (a regular member, not a coordinator). Caller invokes:

```
<bin> brief-set-title \
  --token <freshly-minted UUIDv4> \
  --root "C:/proj/.ac-new/wg-7-tech-lead-team/__agent_tech-lead" \
  --title "evil"
```

**Behavior:**
1. `validate_cli_token` accepts any well-formed UUID with `is_root=false` (`cli/mod.rs:87-97`).
2. `agent_fqn_from_path` is a **pure string operation** (`teams.rs:62`) — it never touches the filesystem. Returns `proj:wg-7-tech-lead-team/tech-lead` regardless of whether that path exists.
3. `is_any_coordinator` succeeds: WG-aware branch in `is_coordinator` (`teams.rs:416-437`) matches by suffix when project + team name + suffix line up. The forged FQN is built precisely to make them line up.
4. `workgroup_root` is also a **pure string operation** (`messaging.rs:54`, walks `ancestors()`) — returns `C:/proj/.ac-new/wg-7-tech-lead-team` even if the directory does not exist (it's only used as a write target later).
5. As long as the target wg-7 directory **does exist on disk in the same project** (the typical AC layout), `wg_root.join("BRIEF.md")` is a writable real path, and the verb proceeds to mutate that team's BRIEF.

**Consequence:** any non-coordinator agent that knows (or guesses, or `list-peers`-discovers) the team name + coordinator suffix of a peer workgroup can rewrite that workgroup's BRIEF. Because BRIEF is loaded by every agent in the WG on session start (per #137 design + #107 plan), this **re-programs the entire peer workgroup** with arbitrary attacker-chosen text. This is a privilege escalation from "team member" to "team coordinator of any sibling team in the same project".

The same primitive weakness exists for `send` and `close-session`, as dev-rust correctly noted — but the blast radius for those is bounded:
- `send`: impersonate-as in a single message; recipients see attacker-chosen `from`. Bad, but agent prompts treat sender as untrusted.
- `close-session`: kill someone's running sessions. Disruptive but transient — sessions can be respawned.
- `brief-set-title` / `brief-append-body`: **persistent semantic re-programming** of every agent in the target WG via their BRIEF.md.

**Fix (one of):**
- **(preferred, minimum)** Strengthen `validate_cli_token` so UUID-tokens MUST exist in a per-session credentials registry (e.g. the same place the credentials block is generated from). Reject UUIDs that aren't currently issued. This closes send/close-session/brief simultaneously and is the right fix for the long-running soft hole.
- **(weaker, workable)** Bind `(token, --root)` at session-issue time and verify the binding inside `validate_cli_token`. The pair is what's already in the credentials block, so the data exists; today nothing is checked.
- **(weakest, do-nothing)** Accept the inherited risk and document loudly. dev-rust recommended this in §E.3. **I push back.** For the brief verbs specifically, persistent re-programming is qualitatively worse than transient impersonation, and the architect should acknowledge this gap explicitly rather than burying it in a doc-comment.

**Architect must weigh in.** If the call is "do-nothing", the plan should at minimum add a §3a labeled "Inherited weakness (escalated for #137)" so future readers do not have to re-discover this from a doc-comment.

### HIGH-2 — Stale-lock recovery + slow legitimate writer ⇒ tmp collision + lost update

**Where:** plan §4 (`LockGuard::acquire`, `LOCK_STALE_AFTER_60S`), §7 ("Coordinator process crashes mid-edit AFTER taking lock").

**Input:** writer A has the lock and is in the middle of `fs::write(&tmp_path, &new_content)`. The write blocks for >60 s — realistic causes on Windows include Windows Defender real-time scanning a tmp file with an unfamiliar extension, OneDrive/iCloud sync interception, a momentary disk hang on a sleepy SATA drive, or an OS pause due to memory pressure.

**Behavior:**
1. T=0: A holds `BRIEF.md.lock`, has read `BRIEF.md`, and is now blocked inside `fs::write(BRIEF.md.tmp, …)`.
2. T=60s: B arrives, polls the lock, finds it; `fs::metadata(lock).modified()` is now > 60s old; `let _ = fs::remove_file(lock)` succeeds; B's `create_new` succeeds → B holds the lock.
3. T=60.001s: B reads `BRIEF.md` (state still pre-A because A hasn't renamed yet).
4. T=60.002s: B calls `fs::write(&tmp_path, …)`. **Same path A is still using.** On Windows, `fs::write` opens with `GENERIC_WRITE | CREATE_ALWAYS`; A's existing handle was opened the same way. Behavior is OS-defined: typical outcome on Windows is `ERROR_SHARING_VIOLATION` for B (A's `truncate(true)` open uses default share modes that don't permit a second writer); on Linux, B's open succeeds but B's CREATE+TRUNCATE detaches A's inode — A is now writing into a "ghost" file, and B has the visible one.
5. Whichever finishes first renames; the other either fails or overwrites.

**Consequence:** silent lost update OR loud-but-confusing error message attributed to "BRIEF.md is locked" when the lock was just released by stale-recovery. In the Linux flavor, A's content is written to a deleted inode and discarded; B's tmp gets renamed; A's `rename()` finds the source file gone or the destination already moved → returns `RenameFailed`. Backup captures the pre-A state, so audit trail is intact, but A's edit is silently dropped or surfaced as a confusing error.

**Fix (one of):**
- **(cheapest, recommended)** Suffix the tmp path with the writer's PID: `tmp_path = wg_root.join(format!("BRIEF.md.tmp.{}", process::id()))`. Eliminates the tmp-collision race entirely (each writer has its own tmp). The post-rename cleanup is unaffected (rename itself unlinks the tmp). Stale `BRIEF.md.tmp.{pid}` files from crashed writers will accumulate; address with a best-effort sweep at lock acquire (delete `BRIEF.md.tmp.*` whose PID is no longer alive). Acceptable to defer the sweep to a follow-up if no one cares about a few stale tmp files in a wg_root.
- **(more thorough)** Add writer-liveness check: the lockfile already contains `pid={pid} ts={rfc3339}` — at stale-recovery time, parse the pid, call `OpenProcess(SYNCHRONIZE, FALSE, pid)` on Windows / `kill(pid, 0)` on Unix to test liveness; only treat as stale if the writer is dead. Rejects this attack at root.
- **(weakest)** Increase `LOCK_STALE_AFTER_60S` to 5–10 minutes. Plausible writes always finish in microseconds, so 60 s is already extremely conservative; bumping further trades crash-recovery latency for fewer false-positive recoveries. Acceptable but doesn't fix the underlying race.

**My recommendation:** per-PID tmp suffix (cheapest, deterministic) + 5-minute stale window. Liveness check is overkill for this verb but is the proper long-term fix.

### HIGH-3 — Leading UTF-8 BOM in `BRIEF.md` defeats frontmatter detection ⇒ duplicate `---` blocks

**Where:** plan §5 `parse_brief`, the `s.starts_with("---\n")` / `s.starts_with("---\r\n")` check.

**Input:** a `BRIEF.md` saved with a UTF-8 BOM (Windows Notepad does this by default; many cross-platform editors do too). The first three bytes are `\xEF\xBB\xBF`, then `---\n`, then frontmatter, then `---\n`, then body.

**Behavior:** `s.starts_with("---\n")` is **false** because the first character of `s` (a Rust `&str`, which holds the BOM as `\u{FEFF}`) is the BOM, not `-`. `parse_brief` returns `has_frontmatter: false, body: <whole input including BOM and existing ---blocks>`. The set-title behavior matrix in §5 ("`parsed.has_frontmatter == false` (no leading `---\n`)") then **prepends a fresh frontmatter block** ahead of the body.

**Consequence:** the resulting file contains `\u{FEFF}` + `---\ntitle: 'NEW'\n---\n` + (original `---\nold-fm\n---\nbody`). Two `---` blocks. The next invocation of `parse_brief` sees the BOM-prefixed `---\n` open and *still* fails `starts_with`, so it again prepends a third block. **Cumulative on every invocation.** Eventually the file is mostly frontmatter chaff. Markdown renderers are typically forgiving but YAML tooling (and any future structured reader of BRIEF.md) gets the wrong title — the **first** block is the new one, the **last** block is the original old one, and YAML libraries vary on which they pick.

**Fix:** at the very start of `parse_brief`, peel off a leading BOM, parse normally, then re-prefix the BOM at render time. This requires a small extension to `ParsedBrief`:

```text
struct ParsedBrief { bom: bool, has_frontmatter: bool, frontmatter: Vec<String>, body: String }

fn parse_brief(s: &str) -> ParsedBrief:
    let (bom, rest) = if s.starts_with('\u{FEFF}') {
        (true, &s['\u{FEFF}'.len_utf8()..])
    } else {
        (false, s)
    };
    let parsed = parse_brief_inner(rest);
    ParsedBrief { bom, ..parsed }

fn render(p: &ParsedBrief) -> String:
    let mut out = String::new();
    if p.bom { out.push('\u{FEFF}'); }
    ...
```

Add `parse_brief_strips_and_re_emits_leading_bom` unit test.

**Architect must weigh in** on whether to (a) preserve+re-emit the BOM, (b) silently strip the BOM (the file becomes BOM-less after first edit), or (c) reject BOM-prefixed BRIEF files at read time. (a) is least surprising for users; (b) is simpler and aligns with Unix conventions; (c) is loudest but might frustrate Windows-native users.

### HIGH-4 — Advisory lock does NOT block non-cooperating external writers; backup captures the right snapshot but user-visible loss is undocumented

**Where:** plan §7 ("Concurrency proof") describes the `BRIEF.md.lock` as "advisory" implicitly (it's a sentinel file, not an OS-level mandatory lock) but the failure-modes table does not enumerate the case where a non-CLI process (user's editor, IDE git auto-save, OneDrive/Dropbox sync, antivirus quarantine-restore) writes to BRIEF.md concurrently with the verb.

**Input:** a coordinator has BRIEF.md open in VS Code and presses Cmd+S during the brief window between the verb's `read_to_string(&brief_path)` (step 2) and `fs::rename(tmp, brief_path)` (step 7).

**Behavior:**
1. T=0: verb reads BRIEF.md (state A).
2. T=10ms: VS Code writes BRIEF.md (state B, includes the user's hand-edits).
3. T=20ms: verb runs `fs::copy(&brief_path, &backup)` → backup captures **state B** (the user's edits). Good.
4. T=21ms: verb writes tmp using `parse_brief(state A) + edit`. Tmp = `edited_A`.
5. T=22ms: verb renames tmp → BRIEF.md. **State B is silently overwritten by `edited_A`.** The user's hand-edits are gone from BRIEF.md.

**Consequence:** the user's edit is lost from BRIEF.md but **is in the backup** (which captures state B at copy time). So the data is recoverable, but:
- The user has zero visibility — the verb prints "BRIEF.md title updated; backup: …" with no warning that an external edit was overwritten.
- Backup proliferation is silent: every verb call produces a `*.bak.md`. With dev-rust's collision-suffix fix (B.2), they all persist. Distinguishing "this backup contains user content I need to recover" from "this backup is just routine" is impossible without manual diff.

**Fix (one of):**
- **(simplest)** Document the limitation explicitly in §7 and in the verb success-message: when backup is created, emit *"backup at <path> retains the prior state of BRIEF.md, including any concurrent unsaved edits"*. Cheap, sets correct expectations.
- **(better)** Before the rename, stat BRIEF.md and compare the size+mtime to the values captured at the read-step. If they differ, abort with `Error: BRIEF.md was modified externally between read and write; aborting. Backup at <path> retains the externally-modified state.` This is still racy (stat-then-rename has a TOCTOU), but it catches the realistic case (editor save events are seconds apart, not microseconds).
- **(thorough but invasive)** Open BRIEF.md with exclusive-share-mode for the duration of the operation, blocking the editor. User-hostile; not recommended for a CLI verb.

**My recommendation:** the (simplest) fix is mandatory; the (better) fix is nice-to-have and worth a follow-up. Don't do (thorough).

### MED-1 — Test U6 wording is too vague to catch CRIT-1

**Where:** plan §9 unit tests, U6 `parse_brief_tolerates_crlf`: *"`"---\r\ntitle: x\r\n---\r\nbody"` → `has_frontmatter:true`, body preserves CRLF"*.

**Input:** the input string above contains no CRLF in its body section ("body" has no trailing newline). "body preserves CRLF" is therefore unsatisfiable in the literal sense and ambiguous as a test assertion.

**Behavior/Consequence:** an implementer reading this might write `assert!(p.has_frontmatter)` and stop there. That assertion **passes even with the CRIT-1 bug present** (because `has_frontmatter` is set to `true` regardless of the consumed offset). The test is now a "passes for wrong reason" — it claims to validate CRLF tolerance but it actually validates nothing about the body slice.

**Fix:** rewrite as `assert_eq!(p.body, "body")` (and add U-CRLF-RoundTrip per CRIT-1).

### MED-2 — Test I15 risks "passes for wrong reason" if not made explicitly concurrent

**Where:** plan §9 integration test I15 `concurrent_writes_dont_corrupt_file`: *"two `execute()` calls in parallel both report success and both edits land"*.

**Input/Behavior:** Rust test runners do not parallelize tests within a single `#[test]` body. If the test simply calls `execute(args1); execute(args2);` sequentially, the lock is never contended; both calls succeed trivially, both edits land trivially. Test passes. **A bug-introducing implementation that ALSO never contends — for example, an implementation where the lock is no-op'd or where `LOCK_TIMEOUT_5S` is misread as 5 ns — would also pass this test sequentially.**

**Fix:** spec the test to spawn two `std::thread::spawn` workers, synchronized with a `std::sync::Barrier::new(2)` so they hit the lock at the same instant. Run a small loop (say, 50 iterations with random ordering of set-title vs append-body) to maximize the probability that the test catches a regression. Concrete addition to the plan:

```rust
#[test]
fn concurrent_writes_dont_corrupt_file() {
    let fixture = ...;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let h1 = { let b = barrier.clone(); std::thread::spawn(move || { b.wait(); execute(set_title_args) }) };
    let h2 = { let b = barrier.clone(); std::thread::spawn(move || { b.wait(); execute(append_body_args) }) };
    let r1 = h1.join().unwrap();
    let r2 = h2.join().unwrap();
    assert_eq!(r1, 0); assert_eq!(r2, 0);
    let final_content = std::fs::read_to_string(&brief_path).unwrap();
    assert!(final_content.contains("title: 'X'"));
    assert!(final_content.contains("appended body line"));
}
```

The barrier is what makes the test honest. Without it, the test name lies.

Same caution applies to U25 (`concurrent_set_title_and_append_body_both_apply`).

### MED-3 — Idempotence short-circuit fails byte-comparison when input is CRLF

**Where:** plan §4 step 5 (idempotence short-circuit `if new_content == existing`).

**Input:** existing BRIEF.md uses CRLF for the frontmatter section (e.g. produced by a Windows editor). User runs `brief-set-title --title <same_value_already_present>`.

**Behavior:** `parse_brief` (after the CRIT-1 fix) returns the frontmatter as a `Vec<String>` of trimmed lines. `render` always emits LF between lines (`out.push('\n')`). The rendered `new_content` is byte-different from the original CRLF-styled `existing`. The short-circuit's `new_content == existing` comparison is **false** even though semantically nothing changed; the verb proceeds to write a backup and rewrite the file.

**Consequence:** every set-title call on a CRLF-frontmatter brief produces a new backup, even when the title is unchanged. Backup proliferation. Storage isn't a real concern but audit trail noise is. Also surprises the user who expects "no-op when value matches" to mean "no backup created".

**Fix:** make the short-circuit semantic, not byte-exact:
```text
if title_value(parsed_old) == title_value(parsed_new):
    return Ok(EditOutcome::NoOp)
```
Where `title_value` is a helper that pulls the raw value of the `title:` line (after YAML unescape) from a `ParsedBrief`. This makes the NoOp work regardless of line-ending style.

### MED-4 — `fs::rename` on Windows has no retry under transient AV/Explorer holds

**Where:** plan §7 acknowledges this as "the realistic case" but the §4 pseudocode does a single `fs::rename` and surfaces the error.

**Input:** Windows Defender or Explorer holds a read handle on BRIEF.md for a few hundred milliseconds (a known Windows behavior, not exotic).

**Behavior:** `fs::rename` returns `Err(io::Error)` with the OS error `ERROR_SHARING_VIOLATION` (32) or `ERROR_ACCESS_DENIED` (5). The verb maps to `RenameFailed`, prints `Error: failed to publish BRIEF.md (rename): …`, exits 1. **Lock is released by Drop.** Backup is on disk.

**Consequence:** UX-poor. The next invocation succeeds because the AV scan completed in the meantime. The user sees a transient error and has to retry manually. Coordinators using this from a PTY (per #107) will see the error injected into their session.

**Fix:** add a 3-attempt retry with 100 ms backoff inside `perform`, between `fs::rename` and the final `RenameFailed`:
```text
for attempt in 0..=2:
    match fs::rename(&tmp_path, &brief_path):
        Ok(_) => return Ok(...);
        Err(e) if e.kind() == PermissionDenied
              || e.raw_os_error() == Some(32)  # ERROR_SHARING_VIOLATION
              || e.raw_os_error() == Some(5):  # ERROR_ACCESS_DENIED
            if attempt < 2 { thread::sleep(Duration::from_millis(100)); continue; }
            return Err(RenameFailed(brief_path, backup_path, e));
        Err(e) => return Err(RenameFailed(brief_path, backup_path, e));
```

### MED-5 — §3 error matrix omits `LockIo` and lock-create-failure cases

**Where:** plan §3 error matrix.

**Input:** lock-file create fails for any reason other than `AlreadyExists` — examples: WG root is read-only, FS is exhausted, parent dir was just deleted by an out-of-band cleanup, the WG path contains chars NTFS rejects, etc.

**Behavior:** dev-rust §H.4 added `LockIo(PathBuf, std::io::Error)` to `BriefOpError` with display `io error acquiring lock at {0}: {1}`. But §3's user-facing error matrix only lists "Lock acquisition timeout (5 s)". The `LockIo` case has no row, so an implementer may forget to wire its `Display` to a stable user-facing string.

**Consequence:** users see whatever `Display` produces (`io error acquiring lock at <path>: <inner>`) which is not in the matrix and not part of the verb's contract. Future tests can't assert against it.

**Fix:** add a row to the §3 error matrix:

| Cause | Exact error string |
|---|---|
| Lock-file create fails for any reason other than `AlreadyExists` (e.g. read-only FS, ENOENT on parent, denied by ACL) | `Error: failed to acquire BRIEF.md lock at <path>: <io::Error>. Aborting; BRIEF.md left unchanged.` |

### MED-6 — ENOSPC mid-`tmp` write leaves stale `BRIEF.md.tmp` litter

**Where:** plan §4 step 7 — `fs::write(&tmp_path, &new_content)` returns `Err` on disk-full, but the plan does not delete the partial tmp before returning `TmpWriteFailed`.

**Input:** disk runs out of space midway through writing the tmp.

**Behavior:** partial tmp file remains on disk; lock is released by Drop; backup is intact; BRIEF.md is unchanged. Plan's §7 footnote *"BRIEF.md.tmp may exist as garbage — next successful run overwrites it via std::fs::write"* is correct in the happy case but incomplete: if **subsequent runs ALSO fail** (the disk stays full), the tmp file accumulates bytes from the most recent attempt and never gets cleaned. Next user-visible problem: a wg_root with a stale `BRIEF.md.tmp` that confuses humans inspecting the directory ("did the last write fail? did it succeed?").

Dev-rust addressed an analogous case for backup partial-cleanup (§C.1). The same logic applies to tmp.

**Fix:** mirror dev-rust's §C.1 approach. On `fs::write(&tmp_path, …)` failure, best-effort delete:
```text
match fs::write(&tmp_path, &new_content):
    Ok(_) => ...
    Err(e) =>
        let _ = fs::remove_file(&tmp_path);   # cleanup
        return Err(TmpWriteFailed(e))
```

### LOW-1 — NUL / control-char bytes in `--title` and `--text` are silently embedded

**Where:** plan §3 arg validation only rejects empty-after-trim. Dev-rust §D.2 adds `\r`/`\n` rejection for `--title`. Neither covers NUL or other control chars.

**Input:** `--title "abc$(printf '\x01\x02')def"` (or via CLI escaping equivalents).

**Behavior:** the YAML escape `replace('\'', "''")` doesn't touch control chars. The single-quoted YAML scalar in the file becomes `title: 'abc\u{1}\u{2}def'` with literal control bytes on disk. Round-trips through `parse_brief` (because `.lines()` splits only on `\n`). Downstream YAML parsers, terminals, and editors react variously: some truncate at NUL, some render as blanks, some refuse to load the file.

**Consequence:** silent file content that the user did not intend, hard to debug because the bytes are invisible.

**Fix:** at the verb boundary (alongside dev-rust's `\r`/`\n` reject for `--title`), reject any character with `c.is_control() && c != '\t'`. Apply to `--title`. For `--text` (`brief-append-body`), allow `\n`/`\r`/`\t` (legitimate body content) but reject other control chars.

### LOW-2 — System clock running backward (NTP correction) breaks backup-name ordering

**Where:** plan §B.2 (with dev-rust's collision-suffix fix) addresses *same-second* collision but not *clock-rewind* collision.

**Input:** a wall-clock NTP correction moves the clock backward by 30+ s mid-session. Two sequential brief edits straddle the correction.

**Behavior:** edit 1 produces `BRIEF.20260503-013000.bak.md`. NTP rewinds. Edit 2 produces `BRIEF.20260503-012945.bak.md` — **earlier** than edit 1 by filename ordering. The collision-suffix loop is no help (no name collision; the names just sort wrong).

**Consequence:** the backup directory's filename-sorted order no longer matches the chronological order. Audit-trail timeline is broken. Issue #137 says "timestamped backup" but doesn't strictly require monotonic ordering, so this is technically within spec — but a coordinator inspecting backups by sort order will be misled.

**Fix:** acceptable as-is per spec. If the architect cares: change the timestamp source to a monotonic counter (`Instant::now()` produces an opaque monotonic `Instant` but isn't directly formattable; use a per-process atomic counter or a hash-of-pid+seq). Not recommended for v1 — the cost outweighs the benefit. Just pin the limitation in a `// NOTE:` comment in `brief_ops.rs`.

### LOW-3 — Frontmatter line-endings always re-rendered as LF, even when input was CRLF

**Where:** plan §5 `render` always emits `out.push('\n')` between frontmatter lines.

**Input:** existing BRIEF.md frontmatter uses CRLF.

**Behavior:** after edit, frontmatter is rewritten with LF. Body line-endings are preserved (byte-for-byte slice via `consumed`). Result: a mixed-line-ending file with LF in the frontmatter and CRLF in the body.

**Consequence:** mixed-line-ending files are tolerated by most tools but considered a code smell by linters. Not corruption, just style drift. After CRIT-1 is fixed, this is the only remaining line-ending oddity.

**Fix:** detect the dominant line-ending style at parse time (look at the opening line) and emit it consistently in `render`. Add to `ParsedBrief`:
```text
struct ParsedBrief { line_ending: &'static str /* "\n" or "\r\n" */, ... }
```
Render uses `parsed.line_ending` instead of literal `\n`. Acceptable to defer to a follow-up if the architect prefers minimal diff for #137.

### NIT-1 — §3 error-matrix completeness (subsumed by MED-5)

The same fix as MED-5 also closes this NIT — listed separately because dev-rust's §H.4 added the variant without back-propagating to the user-facing matrix; the matrix is the contract.

### NIT-2 — §H.6 success-message inconsistency (already in dev-rust round-2 #6)

Listed for completeness; my position is **AGREE** (see round-2 calls below).

### NIT-3 — No integration test for read-only-attributed BRIEF.md

**Where:** plan §9 integration tests.

**Input:** Windows administrator marks BRIEF.md read-only via `attrib +R`, then a coordinator invokes the verb.

**Behavior:** `fs::write(tmp_path, …)` succeeds (tmp is a different file). `fs::rename(tmp_path, brief_path)` fails with `ERROR_ACCESS_DENIED` because the destination is read-only. Verb returns `RenameFailed`.

**Consequence:** correct behavior, but no test pins it.

**Fix:** add integration test `set_title_aborts_on_readonly_brief_md_with_clean_state`. Mark BRIEF.md read-only via `std::fs::set_permissions(&brief, perms)` (cross-platform via `std::os::windows::fs::OpenOptionsExt` + ATTRIBUTE_READONLY on Windows; `mode 0o444` on Unix). Assert exit 1, exact error string, lock cleaned up, tmp cleaned up.

### NIT-4 — Plan §10's PTY-prompt template uses literal `<…>` placeholders

**Where:** plan §10's #107 forward-looking note shows:
```
"<BinaryPath>" brief-set-title --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --title "<the title text>"
```

**Issue:** the literal `<>` inside backticks is fine for documentation but if the #107 PTY template uses these markers verbatim (and not a real placeholder convention like `{BinaryPath}` or similar), the agent might pass them through to the shell, where `<` is a redirect operator. cmd.exe/PowerShell would error.

**Consequence:** out-of-scope for this plan, but the note in §10 is the only artifact and could mislead the #107 implementer. Recommend annotating the snippet: *"angle brackets are placeholders; the prompt template will substitute concrete values before injection."*

---

### My calls on dev-rust's six round-2 items

1. **§B.2 — Backup-timestamp collision (collision-suffix loop):** **AGREE.** The 1-second resolution is unsafe; collision-suffix loop is the right call and matches the existing `phone/messaging.rs:208` pattern. Accept the proposed `BackupExhausted` variant and U29.

2. **§C.1 — Backup partial-file cleanup on `fs::copy` failure:** **AGREE.** dev-rust is correct that the stdlib makes no guarantee about partial-file cleanup. The one-line `let _ = remove_file(&bp)` on the error path is correctness-critical and free.

3. **§D.2 — Reject `\n`/`\r` in `--title`:** **AGREE.** Silent YAML breakage is the worst kind of bug. One-line check at the verb boundary; new error string in §3 matrix. Also see my LOW-1 — extend the rejection to *all* control chars in `--title`, and to non-`\n\r\t` control chars in `--text`.

4. **§D.1 — Frontmatter parser tolerance for trailing whitespace on `---`:** **AGREE, with refinement.** dev-rust's `line.trim() == "---"` is correct. **Add HIGH-3's BOM peeling** to the same edit — both are "tolerate user-edited input that doesn't byte-exactly match the expected open/close" and they share a code site. Putting them in the same plan section keeps the parser definition tidy.

5. **§G.1 — `perform_inner` clock-injection seam:** **AGREE.** The plan as written doesn't actually inject a failure; the seam is necessary. dev-rust's pseudocode is sound. Bonus: the same seam supports the per-PID-tmp test and the BOM-roundtrip test.

6. **§H.6 — Per-verb success messages instead of shared template:** **AGREE.** §4's template produces grammatically broken output for append-body. dev-rust's per-verb table is correct. Trivial to apply.

### Items the architect must resolve before code starts

In priority order:

1. **CRIT-1** — accept the `parse_brief` consumed-init fix (Form B preferred) and pin U6 to a strict body assertion. **Blocking.**
2. **HIGH-1** — decide token-trust posture: tighten `validate_cli_token`, bind `(token, root)`, or accept the inherited risk and label it explicitly in the plan.
3. **HIGH-3** — decide BOM handling policy: preserve+re-emit (recommended), strip silently, or reject.
4. **HIGH-2** — decide stale-lock posture: per-PID tmp + 5-min stale window (cheap, recommended) vs liveness check (thorough but new code) vs status quo + bigger window.
5. **HIGH-4** — accept the documentation-only mitigation, or layer in the size+mtime sentinel before rename.
6. **LOW-3** — decide whether to detect+preserve dominant line-ending at parse time (matters more if BRIEF files are user-edited regularly).

Items 1–5 are not "nice to have" — each has a concrete reproducer in this section.

### Items I checked but found clean

- `chrono::Utc::now()` formatting beyond year 9999 — no panic, just wider field; filenames remain sortable. ✓
- `agent_fqn_from_path` with malformed `--root` (no project before `.ac-new`) — falls through to `agent_name_from_path`, returns a non-coordinator-shaped FQN, gate fails closed. ✓ (matches plan §F.2.)
- `agent_fqn_from_path` with UNC `\\?\` prefix — `replace('\\', '/')` flattens it; `rposition` on `.ac-new` still finds the right anchor (regression test `agent_fqn_from_path_handles_unc_prefix` at `teams.rs:741` confirms). ✓
- Multi-byte UTF-8 (café / emoji) in title/body — `parse_brief`'s `s[consumed..]` slices on byte offset. After CRIT-1 is fixed, all offsets land on `\n` byte boundaries which are always char boundaries. No panic. ✓
- `LockGuard::Drop` runs on panic-unwind — Rust's drop-on-unwind is mandatory; the lock is released even if a downstream `?` returns Err or a panic occurs. ✓
- "Two writers race in stale-lock removal" (dev-rust §B.1) — kernel `create_new` mutex is correct; only one wins. ✓
- Cross-WG impersonation via `extract_wg_team` (without HIGH-1's amplification) — the project-strict path in `is_coordinator` (`teams.rs:438-446`) closes simple unqualified-name attacks. The remaining hole is HIGH-1.

— grinch, round 1.

---

## Round 2 — Architect Resolution

> Author: architect (round 2). All decisions below are reflected in the canonical §1–§12 above; this section is the *audit trail* — what changed, what was rejected, and the reasoning for each blocking call. A third reader can use this to challenge any individual trade-off without re-reading the original review sections.

### Summary

- **Issues addressed:** 1 CRIT, 4 HIGH, 6 MED, 3 LOW, 4 NIT (plus dev-rust's 6 round-2 consensus items).
- **Open after this round:** **0 CRIT, 0 HIGH** (HIGH-1 is *accepted-with-explicit-escalation* in §3a — see ruling below for why this is the correct disposition for #137 scope, not a buried risk).
- **Plan length growth:** roughly +400 lines (mostly defensive-code pseudocode, error-matrix rows, and the new §3a). The implementation surface area is about three additional code paths (collision-suffix loop, sentinel check, AV retry) plus four small struct-field additions (`bom`, `line_ending`, `pre_sentinel`, per-PID tmp).

---

### Ruling: CRIT-1 — `parse_brief` byte-offset bug for CRLF

**Decision: Form B (pull opening line out of `split_inclusive` iterator first).**

**Why Form B over Form A:** Form A (`let opening_len = if s.starts_with("---\r\n") { 5 } else { 4 };`) is a hard-coded special-case that bakes "the opening is either `---\n` or `---\r\n`" into the parser. With D.1's trim-tolerance for `--- \n`, `\t---\n`, etc., the opening can be any of several byte lengths — Form A would need another branch for every variant. Form B reads the **actual** byte length of whatever `split_inclusive('\n')` produced as the first line, so it stays correct as the open-marker tolerance widens. One source of truth, no special cases.

**Test pinning:** U6 is rewritten to `assert_eq!(p.body, "body")` — strict body equality. The original wording "body preserves CRLF" passes for the wrong reason (MED-1). Plus a new U32 (`set_title_round_trip_preserves_crlf_no_extra_blank_line`) for end-to-end byte-exact regression.

**Status: implemented in §5 pseudocode, U6 rewritten, U32 added.**

---

### Ruling: HIGH-1 — Token-to-`--root` binding NOT enforced

**Decision: Accept inherited risk for #137 scope, escalate explicitly via new §3a, recommend follow-up issue.**

**Reasoning:**

1. The fix grinch prefers (registry-bound `validate_cli_token`) is **architectural** — it requires designing a session-scoped credentials registry that doesn't exist today. That is its own plan, with its own design questions (where does the registry live? on-disk under `~/.agentscommander/` or in-memory in the AC backend? per-project or per-instance? how does it survive AC restarts?). Pulling that into #137 would balloon the issue and delay BRIEF-CLI delivery indefinitely.

2. The role explicitly forbids "architectural changes (new crates, module restructuring) without strong justification." Adding a credentials registry crosses that bar — it's not a small refactor, and the design space is non-trivial.

3. **However**, grinch is correct that brief-verbs are **qualitatively worse** than the same hole in `send`/`close-session` because BRIEF mutations re-program every agent in the WG durably. Hiding this in a doc-comment is genuinely insufficient. The right disposition is to **document loudly in the canonical plan** (so a future auditor reads about it before they read the code) **and recommend a follow-up issue** that closes the hole for `send`/`close-session`/brief simultaneously.

4. Operational mitigation in the meantime: the success-path log line includes `sender=` and `wg=` (§12), so a victim team or audit tool can grep `[brief]` log entries with `wg=` mismatching the caller's home workgroup. Detect-after-the-fact, not prevent — but combined with the collision-resistant timestamped backups (B.2), a wronged team can roll back to the most recent legitimate state.

**What was added:** new §3a labeled *"Inherited weakness (escalated for #137)"* with the full attack walkthrough, what protects us today (the GOLDEN RULE confines fake-path synthesis), why the brief blast radius is qualitatively worse than `send`/`close-session`, why the fix is out-of-scope for #137, and the operational mitigation.

**Recommended follow-up issue (suggested title):** *"Bind CLI tokens to issued sessions to prevent UUID-mint forgery (closes hole shared by send / close-session / brief verbs)"*. The follow-up plan should make `validate_cli_token` consult an authoritative session registry, bind `(token, root)` at issuance time, and apply uniformly to all four verbs. Tech-lead to file.

**Status: §3a written; follow-up recommendation embedded in §3a and in this section.**

---

### Ruling: HIGH-2 — Stale-lock recovery + slow legitimate writer ⇒ tmp collision

**Decision: per-PID tmp suffix (`BRIEF.md.tmp.<pid>`) + extend stale-lock window from 60 s to 5 minutes (`LOCK_STALE_AFTER_5M`). Liveness check deferred to a follow-up.**

**Reasoning:**

1. Per-PID tmp eliminates the **tmp-collision race** deterministically — A and B can each call `fs::write` on their own tmp paths without interfering. This is the most likely failure mode (the one grinch's reproducer triggers).

2. The 5-minute stale window makes the **rename-overwrite race** (A's eventual rename overwriting B's just-completed rename) extremely rare in practice. A `fs::write` blocking 5+ minutes requires a confluence of unusual events (Defender scanning a tmp file >5 min, OneDrive sync hanging, sleepy-disk pause). The race exists, but its window is tiny.

3. Writer-liveness check (parse pid → `OpenProcess`/`kill(pid,0)`) **does** close the rename-race at the root. The Windows side is callable today via the existing `windows-sys` dep (`src-tauri/Cargo.toml:34-35` already declares `Win32_System_Threading`, which exposes `OpenProcess`); the Unix side adds `libc::kill(pid, 0)` (also already a transitive dep). So the cost is **not** a new dependency — it is an extra unsafe-FFI block + a small Windows/Unix `cfg`-split + new error paths to plumb the "writer is alive, fall back to lock-timeout" branch through `LockGuard::acquire`. For a race window this narrow (writer must block >5 minutes inside `fs::write`), the cost-benefit doesn't justify v1 inclusion against the role's "minimal blast radius" bias. The follow-up note at the bottom of this ruling tracks the eventual fix.

4. Backup captures pre-A state, so even if the race triggers, **data is recoverable** from `*.bak.md`. The damage is "B's edit silently dropped" — bad, but not catastrophic and not silent in audit logs.

**What was added:**
- §4 pseudocode: `tmp_path = wg_root.join(format!("BRIEF.md.tmp.{}", std::process::id()))`
- §4 LockGuard: comment block explaining the per-PID rationale and the deferred liveness check
- Constant rename: `LOCK_STALE_AFTER_60S` → `LOCK_STALE_AFTER_5M = Duration::from_secs(300)`
- §7 failure-modes table: new row for the stale-recovery-vs-slow-writer scenario, documenting both the per-PID mitigation and the residual rename-race
- §H.7: clarified that `BRIEF.md.tmp.<pid>` is the operational artifact (not bare `BRIEF.md.tmp`)
- §H.4: `TmpWriteFailed` signature changed to `(PathBuf, std::io::Error)` so the user-facing string names the actual per-PID path
- U21, U22 updated for the new constants/paths

**Liveness-check follow-up:** if user reports of "lost edits" surface in practice, file a follow-up to add `is_writer_alive(pid)` using either `sysinfo` (already a transitive dep of some Tauri crates — verify before adding) or a thin wrapper around `OpenProcess`/`kill`.

**Status: implemented in §4, §7, §H.4, §H.7, U21, U22.**

---

### Ruling: HIGH-3 — Leading UTF-8 BOM defeats frontmatter detection

**Decision: Preserve and re-emit the BOM (option a). Add `bom: bool` to `ParsedBrief`.**

**Reasoning:**

1. **Preserve+re-emit (a)** is the principle-of-least-surprise option for Windows users. Notepad-saved BRIEF files keep their BOM through the verb cycle; the user sees no encoding diff in their git client; their editor doesn't trigger an "encoding changed" warning.

2. **Silent strip (b)** is simpler but creates a one-time encoding diff on first use that shows up as a noisy commit. Users who don't know what a BOM is will be confused by the diff.

3. **Reject (c)** is loudest but most user-hostile — Windows users don't necessarily know how to remove a BOM, and the verb being unable to operate on a Notepad-saved file is a poor first-impression UX.

4. The cost of (a) is one extra `bool` field on `ParsedBrief` and a 3-byte conditional emit in `render`. Negligible.

**What was added:**
- §5 `ParsedBrief`: `bom: bool` field
- §5 `parse_brief`: peel `\u{FEFF}` at top, store on struct
- §5 `render`: re-emit BOM at output start
- U31 (`parse_brief_strips_and_re_emits_leading_bom`) round-trip test
- §12 implementer note pinning the convention

**Status: implemented in §5, U31, §12.**

---

### Ruling: HIGH-4 — Advisory lock doesn't block external writers

**Decision: Both (a) explicit documentation + (b) size+mtime sentinel before rename.**

**Reasoning:**

1. The (simplest) docs-only mitigation grinch flagged as mandatory is in §7's failure-modes table and the §3 error-matrix row. That's the floor.

2. The (better) size+mtime sentinel is a 5-line addition that **catches the realistic case** (editor save events seconds apart, AV scans hundreds of ms). Sub-millisecond TOCTOU remains theoretically open, but the user-realistic timescales are caught. The cost is one `metadata()` call after the read and one before the rename. Negligible.

3. The (thorough) exclusive-share-mode locking grinch already rejected as user-hostile, and I agree — a CLI verb that locks the file out of the user's editor is a poor experience.

4. Combining (a) + (b) gives the user **two layers**: the verb either detects the conflict and aborts with a clear message (b), or — if the conflict slips through the sub-millisecond TOCTOU — the backup captures the externally-modified content (a). Either way, no user data is lost without a recoverable artifact.

**What was added:**
- §4 step 2a: capture `(len, mtime)` after read
- §4 step 7a: re-stat and compare; if changed, abort with `ExternalWrite(backup_path)`
- §4 surrounding prose: "External-writer abort path (HIGH-4 sentinel)" subsection
- §3 error matrix: row for `ExternalWrite`
- §7 failure-modes table: dedicated row for "external writer modifies BRIEF.md between our read and our rename"
- §H.4 `BriefOpError::ExternalWrite(Option<PathBuf>)` variant
- I21 (`set_title_aborts_on_external_modification_between_read_and_rename`)

**Status: implemented in §3, §4, §7, §H.4, I21.**

---

### Accepted suggestions from dev-rust round 1 (six consensus items)

1. **§B.2 — Backup-timestamp collision-suffix loop.** Accepted as proposed. `BriefOpError::BackupExhausted(PathBuf)` added; new error string in §3 matrix; collision loop in §4 pseudocode; U29 added.
2. **§C.1 — Backup partial-file cleanup.** Accepted as proposed. Explicit `let _ = std::fs::remove_file(&bp)` on copy failure; surrounding prose in §4; U30 (`backup_failure_releases_lockfile`) pins the lock-cleanup half.
3. **§D.2 — Reject `\n`/`\r` in `--title`.** Accepted with **expansion** per grinch LOW-1: reject all `c.is_control() && c != '\t'` in `--title`; reject all `c.is_control() && c != '\n' && c != '\r' && c != '\t'` in `--text`. Two error rows added to §3 matrix; validation step added to §3 pseudocode; I17 covers the `--title` rejection.
4. **§D.1 — Frontmatter marker tolerance via `line.trim() == "---"`.** Accepted, paired with HIGH-3 BOM peeling in the same §5 edit. U26 added.
5. **§G.1 — `perform_inner` clock-injection seam.** Accepted as proposed. Production `perform` wraps `perform_inner(_, _, chrono::Utc::now)`; tests pass a deterministic `now` closure. Used by U24 (backup failure injection), U29 (collision test), and U32 (CRLF round-trip).
6. **§H.6 — Per-verb success messages.** Accepted as proposed. Drop the shared template; emit per-verb strings. `apply_append_body` emits no `NoOp` (an append always changes the file).

---

### Accepted MED items (with rationale)

- **MED-1 (U6 vague).** Subsumed by CRIT-1 fix; U6 rewritten to strict `assert_eq!(p.body, "body")`.
- **MED-2 (I15 + U25 need barrier).** Accepted. Both tests now spec `std::sync::Barrier::new(2)` to force same-instant lock contention. Without the barrier, the test name lies (Rust does not parallelise within a single `#[test]`).
- **MED-3 (Idempotence fails byte-comparison on CRLF).** Accepted. Idempotence short-circuit is now **semantic** — compares `title_value_of(parsed_old) == title_value_of(parsed_new)` instead of byte-equality. Documented in §5 idempotence subsection.
- **MED-4 (No `fs::rename` retry on Windows AV/Explorer holds).** Accepted. 3-attempt retry with 100 ms backoff for `ErrorKind::PermissionDenied` / `ERROR_SHARING_VIOLATION` (32) / `ERROR_ACCESS_DENIED` (5). UX win at trivial cost.
- **MED-5 (`LockIo` missing from §3 matrix).** Accepted. `LockIo` row added to §3 matrix; `BriefOpError::LockIo` Display string updated to match the matrix wording.
- **MED-6 (ENOSPC tmp litter).** Accepted. `let _ = std::fs::remove_file(&tmp_path)` on `fs::write` failure, mirroring §C.1's pattern. `BriefOpError::TmpWriteFailed` signature updated to carry the per-PID path.

---

### Accepted LOW / NIT items

- **LOW-1 (control chars).** Folded into the §D.2 expansion above.
- **LOW-3 (preserve dominant line-ending).** Accepted. Cheap and removes a category of follow-up surprises (linter complaints about mixed line endings, MED-3's byte-compare problem, etc.). `ParsedBrief.line_ending: &'static str` field added; `render` uses it for frontmatter delimiters; body content is preserved byte-for-byte regardless. U33 pins the behavior.
- **NIT-1.** Subsumed by MED-5.
- **NIT-2.** Subsumed by §H.6.
- **NIT-3 (read-only BRIEF test).** Accepted. I20 added.
- **NIT-4 (PTY template angle-bracket placeholders).** Accepted. §10 now has an explicit "Note for the #107 implementer" paragraph clarifying the placeholder convention and reminding them about the single-line/control-char rejection at the verb boundary.

---

### Rejected suggestions (with reasoning)

- **LOW-2 (clock-rewind breaks backup-name ordering).** **Rejected for #137; accepted as a `// NOTE:` comment.** Issue #137 says "timestamped backup" and does not require monotonic ordering. Switching to a monotonic counter (or hashing pid+seq) introduces complexity for a rare-and-recoverable problem (a coordinator inspecting backups by sort order will see two consecutive entries that look chronologically wrong, but the on-disk content is correct and can be discriminated by reading the files). Adding a `// NOTE:` comment in `brief_ops.rs` near the timestamp formatting documents the limitation for future maintainers (per §12 implementer notes).

- **HIGH-2's "writer-liveness check" option.** **Rejected for v1; deferred to a follow-up.** The proper fix would parse the pid from the lockfile and call `OpenProcess`/`kill(pid,0)` to test liveness before declaring a lock stale. On Windows this requires either the `windows` crate (a substantial new dependency) or raw FFI (clumsy). For a race that requires `fs::write` to block >5 minutes, the cost-benefit doesn't justify v1 inclusion. If lost-edit reports surface in practice, the follow-up plan should add it. Documented in the §4 LockGuard comment block.

- **HIGH-1's "tighten `validate_cli_token`" option.** **Out of scope for #137 (see ruling above);** belongs in a separate plan that benefits all four CLI verbs simultaneously.

- **HIGH-4's "exclusive-share-mode locking" option.** **Rejected as user-hostile** — agree with grinch. A CLI verb that prevents the user from saving in their editor is a poor experience and would surface as a different bug class entirely.

---

### Items where further work was identified but punted (with reasoning)

- **Per-PID tmp sweep on lock acquire.** Currently, a writer that crashes mid-`fs::write` leaves a `BRIEF.md.tmp.<pid>` file behind. The next writer with a different PID will not overwrite it (different filename). This is "litter, not a bug." A follow-up could sweep `BRIEF.md.tmp.*` files at lock-acquire and delete those whose suffix is a non-alive PID. Acceptable to defer per "minimal blast radius."

- **Rename-retry test on Windows.** MED-4's retry loop is in the code path but lacks an explicit test, since reliably triggering ERROR_SHARING_VIOLATION in CI requires holding a file handle from another process. Documented in the §9 mapping table; can be added if Windows CI is wired up later.

- **HIGH-1 follow-up issue title.** I drafted a suggested title in §3a (*"Bind CLI tokens to issued sessions to prevent UUID-mint forgery"*). Tech-lead to file the actual issue.

---

### Files updated in this round (canonical sections)

- **§3** — Error matrix expanded: `+5 rows` (LockIo, BackupExhausted, control-char ×2, ExternalWrite); rename row clarified.
- **§3** — Auth-flow pseudocode: control-char rejection step added (4b).
- **§3a** — NEW: Inherited weakness section documenting HIGH-1 escalation.
- **§4** — File-touch pseudocode: per-PID tmp, sentinel snapshot/recheck, collision-suffix backup loop, partial-file cleanup, semantic idempotence, AV rename retry. New "External-writer abort path (HIGH-4 sentinel)" subsection. LockGuard comment block updated.
- **§5** — `ParsedBrief` extended (`bom`, `line_ending`); parse Form B + BOM peel + trim() comparisons; render BOM re-emit + line-ending preservation; idempotence becomes semantic.
- **§7** — Failure-modes table: stale-lock-vs-slow-writer row added; external-writer row added; backup-collision row added; tmp-write-failure row added; lock-window updated to 5 min.
- **§9** — Test plan: U6 rewritten strict; U21/U22 updated; U25/I15 require Barrier; new U26-U33 + I17-I21 added; mapping table updated.
- **§10** — #107 forward-looking note: angle-bracket placeholder clarification.
- **§12** — Implementer notes: 14 new bullets covering all the above.
- **§H.4** — `BriefOpError` enum: `ExternalWrite` variant added, `TmpWriteFailed` signature updated, `LockIo` Display updated to match matrix.
- **§H.7** — Per-PID tmp clarification + crashed-writer litter note.

— architect, round 2.

---

## Dev-Rust Round 2 Review

> Reviewer: dev-rust (round 2). All architect resolutions reviewed against `feature/137-brief-cli-verb` HEAD. Verdict: **green-light to grinch round-2** — six NIT-level findings below, none blocking.

### Position on architect's round-2 rulings

| Decision | Position | Note |
|---|---|---|
| **CRIT-1** — Form B parser | **Accept** | Form B verified clean against LF / CRLF / BOM+CRLF / leading-or-trailing whitespace on `---` markers / empty input / BOM-only / unclosed frontmatter. Body slice is byte-exact for the regression. U6 strict + U32 byte-exact is the right pinning. |
| **HIGH-1** — accept inherited risk + §3a + follow-up | **Accept** | All file:line anchors in §3a verified. The "GOLDEN RULE confines fake-path synthesis" mitigation is correctly qualified with "(necessary but not sufficient)" — it is prompt-level (`config/session_context.rs:478`), not binary-level, and the qualifier honors this. The follow-up issue title in §3a is well-framed for `send` / `close-session` / brief-* simultaneously. |
| **HIGH-2** — per-PID tmp + 5-min stale + defer liveness | **Accept** with one note (B below) | §H.4 / §7 / §H.7 / U21 / U22 are consistent. Deferral-conclusion is defensible; the stated rationale has one factual error (windows-sys is already a dep with `Win32_System_Threading`). |
| **HIGH-3** — preserve+re-emit BOM | **Accept** | Round-trip verified for BOM/CRLF/LF combinations across set-title, append-body, and new-file paths. New-file initialization correctly defaults `bom: false`. Body slice never contains the BOM (peeled before `consumed` arithmetic). U31 sufficient. |
| **HIGH-4** — sentinel + docs | **Accept** with two notes (C, D below) | The realistic case (editor saves seconds apart, AV scans hundreds of ms) is caught. Two minor refinements suggested for FAT32 mtime granularity and `unwrap_or(UNIX_EPOCH)` asymmetry. |
| **MED-1** — U6 strict | **Accept** | Subsumed by CRIT-1. |
| **MED-2** — Barrier in U25 / I15 | **Accept** | Correct; without `Barrier::new(2)`, the test passes for the wrong reason in a single-threaded `#[test]` body. |
| **MED-3** — semantic idempotence | **Accept** | Traced `title_value_of` for canonical `'…'` / bare scalar / double-quoted / quote-escaped values. Conservative direction is harmless audit-trail noise; unsafe direction is impossible because the parsed-after-edit form is always canonical single-quoted. |
| **MED-4** — Windows rename retry | **Accept** | Retry on `PermissionDenied` / OS errors 32 / 5 with 100 ms backoff is appropriate. |
| **MED-5** — `LockIo` matrix row | **Accept** | §3 matrix row at line 75 + §H.4 Display string aligned. |
| **MED-6** — tmp-write partial cleanup | **Accept** | Mirrors §C.1 pattern. |
| **LOW-1** (control chars in `--title` / `--text`) | **Accept** | Folded into D.2 expansion; verb-boundary check covers the silent-byte class. |
| **LOW-2** (clock-rewind, comment-only) | **Accept** | Rare scenario; recoverable by reading backup contents. NOTE comment in `brief_ops.rs` near `format!("%Y%m%d-%H%M%S")` is sufficient. |
| **LOW-3** (line-ending preservation) | **Accept** | `line_ending: &'static str` field + render uses it for frontmatter delimiters; body is preserved byte-for-byte (with the documented caveat covered in finding E). |
| **NIT-1** — subsumed by MED-5 | **Accept** | |
| **NIT-2** — subsumed by §H.6 | **Accept** | |
| **NIT-3** — read-only test (I20) | **Accept** | |
| **NIT-4** — angle-bracket placeholder clarification in §10 | **Accept** | |
| Items punted (per-PID tmp sweep, rename-retry test, follow-up issue title) | **Accept** | All have clear triggers for follow-up. |

### New round-2 findings (NIT level — none blocking)

#### A. §7 line 540 has a leftover "60 s" reference (NIT, doc-only)

§7's opening paragraph at `_plans/137-brief-cli-verb.md:540` still says:

> "Stale-lock recovery (60 s) keeps a crashed coordinator from permanently blocking writes."

Every other §7 reference uses 5 min: line 554 ("Next caller within 5 min gets `LockTimeout`"), line 562 ("stale-lock recovery handles after 5 min"), line 569 ("the 5 min stale-lock window"). Suggested fix: replace "60 s" with "5 min" at line 540 to keep §7 internally consistent. Doc-only; no test impact.

#### B. HIGH-2 deferral rationale partially contradicts the codebase (NIT, wording)

The HIGH-2 ruling at lines 1663-1664 states:

> "Writer-liveness check ... on Windows it requires either the `windows` crate (not currently a dependency) or raw FFI — both cross the role's 'no new crates without strong justification' bar."

But `src-tauri/Cargo.toml:34-35` already declares:

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_System_Console", "Win32_Foundation", "Win32_System_Threading"] }
```

`Win32_System_Threading` includes `OpenProcess`. So a Windows liveness check is already callable without a new dep or new feature. The architect's later note at line 1677 acknowledges `OpenProcess`/`kill` as an option — that contradicts the rationale at 1664.

The deferral itself is **still defensible** on code-cost grounds (extra unsafe-FFI block, additional error paths, and the rename-overwrite race genuinely is rare with the 5-min window). I'm OK either way. Flagging so the architect can either tighten the rationale ("extra unsafe code for a rare race") OR reconsider implementing in v1 (~10 lines of `windows-sys::Win32::System::Threading::OpenProcess` + Unix-side `libc::kill`).

#### C. HIGH-4 sentinel: FAT32 mtime granularity extends TOCTOU (NIT)

`std::fs::Metadata::modified()` on FAT32 has 2-second granularity. Two writes inside the same 2-second bucket that don't change file size will produce `(len, mtime)` snapshots that compare equal — sentinel does not fire, external write is silently overwritten.

In practice BRIEF.md lives in `.ac-new/` on the user's project drive (NTFS / EXT4 / APFS, all sub-second). FAT32 is unusual (USB stick, very old SD card). Worth a single-line NOTE in the §4 "External-writer abort path (HIGH-4 sentinel)" subsection: "On FAT32 (2 s mtime granularity), the realistic-detection window expands to 2 seconds; not blocking for typical AC layouts."

#### D. HIGH-4 sentinel: `unwrap_or(UNIX_EPOCH)` asymmetry can produce a false-positive (NIT)

§4 step 2a uses `m.modified().unwrap_or(UNIX_EPOCH)`; step 7a uses the same fallback at line 286. If `modified()` succeeds at step 2a (real timestamp captured) but fails at step 7a (transient FS error — rare on local drives, possible on SMB/NFS), the post-snapshot becomes UNIX_EPOCH while the pre-snapshot was a real timestamp. The comparison fails → `ExternalWrite` is emitted as a false positive. Cost: user sees an error pointing at the backup, retries.

Refinement: capture both as `Option<SystemTime>` and skip the mtime equality check when either side is `None`; always compare `len`. ~3 lines of pseudocode adjustment in §4 step 7a. I'm OK either way; flagging for the implementer's awareness.

#### E. Append-body strips the body's trailing line-ending (NIT, test gap)

`parsed.body.trim_end()` at §5 matrix row 510 strips trailing `\r\n` from the existing body's last line; the new last line uses `\n` per the format!. Implicitly covered by §5 line 514's mixed-line-ending note, but no unit test pins the specific behavior. U17 (`apply_append_body_does_not_touch_frontmatter`) verifies frontmatter byte-equality but no analog exists for body line-endings.

Suggested addition (U34): `apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss` — input body `"Line1\r\nLine2\r\n"` + text `"NewLine"` → result body `"Line1\r\nLine2\n\nNewLine\n"` (Line1's CRLF preserved, Line2's trailing CRLF replaced by `\n\n` separator, NewLine ends with LF). Pins the documented trade-off and prevents accidental "fixes" that regress to all-LF body re-rendering.

Optional — the §5 line 514 prose covers it; a regression test makes the trade-off harder to undo unintentionally.

#### F. Sentinel snapshot timing (NIT, doc clarification)

The `(len, mtime)` snapshot in §4 step 2a is taken AFTER the read in step 2. Therefore an external write that lands between the read and the metadata call is reflected in the captured snapshot, not detected at step 7a. This is one component of the architect's acknowledged "sub-millisecond TOCTOU remains theoretically open" at §4 line 281, but the specific window is worth pinning to discourage future implementers from "tightening" the sentinel by moving the snapshot BEFORE the read (which would introduce a different, larger TOCTOU — write between snapshot and read is unbounded).

Suggested clarification in §4 line 281: append "(specifically, the read→metadata window of ~µs)".

### Items I checked and found clean (not flagged)

- **§5 Form B + D.1 + HIGH-3 BOM peeling interaction** — traced for LF / CRLF / BOM+CRLF / leading-whitespace-on-marker / unclosed-FM / empty-input / BOM-only-input — all consistent. The body slice is byte-exact post-BOM for every combination.
- **§5 idempotence semantic comparison** — traced `title_value_of` for canonical `'…'` / bare / double-quoted / quote-escaped values. Conservative direction harmless; unsafe direction impossible.
- **§4 LockGuard race in stale-lock removal** — kernel `create_new` is the mutex; only one writer wins. (Re-confirmed from Round 1 §B.1.)
- **§4 per-PID tmp prevents tmp-collision** — A and B have distinct tmp paths so concurrent `fs::write` calls don't interfere. The remaining rename-overwrite race is correctly documented and deferred.
- **§3 error matrix consistency with §H.4** — all variants (LockTimeout, LockIo, ReadFailed, BackupFailed, BackupExhausted, TmpWriteFailed, ExternalWrite, RenameFailed) match the matrix rows. Wording aligned per MED-5.
- **§3a anchors** — `validate_cli_token` (`cli/mod.rs:87-97`, with `Uuid::parse_str` at line 88), `agent_fqn_from_path` (`config/teams.rs:62`), `workgroup_root` (`phone/messaging.rs:54`), `is_coordinator` (`config/teams.rs:403`), `default_context` GOLDEN RULE (`config/session_context.rs:478`) — all verified. `agent_fqn_from_path` and `workgroup_root` confirmed pure path operations (no filesystem touch); the §3a attack walkthrough is technically accurate.
- **§9 test plan coverage** — U6 strict / U21 5-min stale / U22 per-PID-tmp / U25 Barrier / U26 trim-tolerance / U27 Unicode / U28 indentation / U29 collision / U30 lock-cleanup / U31 BOM / U32 CRLF round-trip / U33 line-ending preservation / I15 Barrier / I17 control-chars / I20 read-only / I21 ExternalWrite — all line up with the rulings. No tests are coupled to the old 60 s value.
- **`windows-sys` dep already covers `Win32_Foundation` + `Win32_System_Threading` + `Win32_System_Console`** — a future liveness-check follow-up does NOT need a new dep on Windows. (Cross-reference for finding B.)

### Verdict

**Green-light to grinch round-2.** The architect's resolutions are sound; CRIT-1 and HIGH-1..4 are all properly addressed. The six findings above are NIT-level: A is a doc-only stale reference, B is a wording inconsistency, C–F are minor refinements an implementer can apply during coding without architect re-review. None block grinch's round-2 review.

— dev-rust, round 2.

---

## Grinch Round 2 Findings

> Reviewer: dev-rust-grinch (adversarial), round 2. Read against `feature/137-brief-cli-verb` HEAD; verified architect's round-2 resolutions section-by-section against the canonical §1–§12 and against the actual `cli/mod.rs`, `config/teams.rs`, `phone/messaging.rs`, and `config/session_context.rs` source. Tried to break each round-2 fix; one MED bug remains in the canonical pseudocode and a handful of NIT-level cosmetics. None of the round-1 CRIT/HIGH items regressed. Severity counts below.

### Severity counts

- **CRIT:** 0
- **HIGH:** 0
- **MED:** 1
- **NIT:** 7

### Position on architect's round-2 rulings (one line each)

| Ruling | Position | Note |
|---|---|---|
| CRIT-1 — Form B parser | **Accept** | Traced against LF / CRLF / BOM+CRLF / leading-or-trailing whitespace on `---` markers / empty / BOM-only / "---" with no newline / opening-only with no closer. Body slice is byte-exact in every case; `consumed` is always the actual `opening.len()` so D.1 trim-tolerance widens the open-marker variants without re-introducing CRIT-1. The `_ => return ParsedBrief { ... body: s.to_string() }` arm uses `match` instead of `.expect()` — no panic path on empty input. ✓ |
| HIGH-1 — accept inherited risk + §3a + follow-up | **Accept** with NIT-4 (below) on §3a wording | The attack walkthrough in §3a is technically accurate against the current `agent_fqn_from_path` (`teams.rs:62`, pure path op confirmed) and `workgroup_root` (`messaging.rs:54`, pure path op confirmed). The "GOLDEN RULE confines fake-path synthesis (necessary but not sufficient)" qualifier is *too generous* to the GOLDEN RULE — the real bound is "extant `wg-N-*` directories on disk", which the GOLDEN RULE does not produce. See NIT-4. The follow-up-issue framing is correct and well-scoped across `send` / `close-session` / brief verbs. |
| HIGH-2 — per-PID tmp + 5-min stale + defer liveness | **Accept** | No new race introduced by the longer window. PID reuse on Windows is handled cleanly because `std::fs::write` truncates an existing tmp on subsequent runs (the same-PID litter case self-heals). The rename-overwrite race remains documented and bounded; backups capture pre-A state per §C.1. The deferral of liveness check is *defensible*, but the architect's stated rationale at §"Round 2 — Architect Resolution" is factually wrong (windows-sys IS already a dep — see NIT-B in dev-rust round 2, and my position below). |
| HIGH-3 — preserve+re-emit BOM | **Accept** | Round-trip verified across all six rewrite paths: set-title-with-FM-and-title, set-title-with-FM-no-title, set-title-no-FM, set-title-on-missing-file (correctly defaults `bom:false`), append-body-on-existing, append-body-on-missing-file. BOM-only file and BOM+body-no-FM file both behave correctly (the "no frontmatter" matrix row preserves `bom`). The peel-then-arithmetic order in `parse_brief` ensures `consumed` never includes BOM bytes. ✓ |
| HIGH-4 — sentinel + docs | **Accept** with NIT-1 (below) on the delete case | Realistic editor-save case is caught. The dev-rust NIT-D (asymmetric `unwrap_or(UNIX_EPOCH)`) is a real false-positive surface and I agree with the refinement. There is one additional gap not flagged by dev-rust: `Err(_) => ()` on the post-snapshot `metadata()` call silently undoes external deletes (rename to a vanished destination *creates* the destination on both Windows and Unix). See NIT-1. |
| MED-1 (U6 strict) | **Accept** | |
| MED-2 (Barrier in U25 / I15) | **Accept** | Without the barrier the test name lies — Rust does not parallelize within a single `#[test]`. |
| MED-3 (semantic idempotence) | **Accept** | Conservative-direction-harmless / unsafe-direction-impossible analysis is correct. One wording slip in §4 step 4-5 noted as NIT-7. |
| MED-4 (Windows rename retry) | **Accept** | But — the retry-exhausted path does not clean up the per-PID tmp file, contradicting I20. See **MED-1 below.** |
| MED-5 (`LockIo` matrix row) | **Accept** | |
| MED-6 (tmp-write partial cleanup) | **Accept** | |
| LOW-1 (control chars in --title / --text) | **Accept** | |
| LOW-2 (clock-rewind, comment-only) | **Accept** | |
| LOW-3 (line-ending preservation) | **Accept** | |
| NIT-1, NIT-2, NIT-3 (read-only test), NIT-4 (PTY angle-brackets) | **Accept** | |
| Rejected: LOW-2 (monotonic counter), HIGH-2 liveness, HIGH-1 token-tighten, HIGH-4 exclusive-share | **Accept all rejections** | Each rejection rationale is sound; deferrals are scoped correctly. |
| Punted: per-PID tmp sweep, rename-retry test on Windows CI, follow-up issue title | **Accept** | |

### Position on dev-rust's six round-2 NITs

- **NIT-A (60s leftover at line 540):** **AGREE.** Trivial doc-consistency fix; replace "60 s" with "5 min" to match every other §7 reference and the `LOCK_STALE_AFTER_5M` constant.
- **NIT-B (windows-sys already a dep with Win32_System_Threading):** **AGREE on the factual correction; no change in conclusion.** Dev-rust is correct that `src-tauri/Cargo.toml` already declares `windows-sys` with `Win32_System_Threading` (verified). The architect's deferral *rationale* should be tightened to "extra unsafe FFI block + Unix-side `libc::kill` wrapper for a rare race, against the role's bias for minimal blast radius." The deferral *conclusion* (don't add liveness check in v1) is still defensible on those grounds — the rename-overwrite race genuinely is rare with a 5-minute window. I'm not asking for the liveness check to be added in v1; I'm asking the rationale text to stop claiming a non-existent dependency cost.
- **NIT-C (FAT32 mtime granularity):** **AGREE.** Single-line NOTE in §4's "External-writer abort path" subsection is sufficient. Realistic AC layouts use NTFS / EXT4 / APFS; the FAT32 case (USB stick, old SD) is a documented limit, not a v1 blocker.
- **NIT-D (asymmetric `unwrap_or(UNIX_EPOCH)`):** **AGREE.** Dev-rust's refinement (capture both as `Option<SystemTime>`, skip mtime equality when either side is `None`, always compare `len`) is correct. False-positives on transient `modified()` failures (rare on local NTFS, possible on SMB / NFS) get the user a confusing `ExternalWrite` error pointing at a backup that doesn't actually contain external content. The 3-line pseudocode adjustment is cheap and removes a category of false-positives.
- **NIT-E (append-body trailing-line-ending test gap):** **AGREE.** Adding U34 (`apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss`) pins the documented mixed-line-ending trade-off. Prevents a future contributor from "fixing" the body to be all-LF and silently regressing the byte-for-byte body-preservation guarantee.
- **NIT-F (sentinel snapshot timing clarification):** **AGREE.** Append "(specifically, the read→metadata window of ~µs)" to §4 line 281. The current "sub-millisecond TOCTOU remains theoretically open" is correct but doesn't tell a future implementer *which window* — and a well-meaning "tightening" that moves the snapshot BEFORE the read would introduce an unbounded write-between-snapshot-and-read window. Pinning the existing window is regression-protection.

### New round-2 findings

#### MED-1 — Tmp file is NOT cleaned up on rename failure (contradicts I20 + NIT-3 intent)

**Where:** §4 step 7b (`_plans/137-brief-cli-verb.md:291-300`).

**Trace:** the rename-retry loop has two `return Err(RenameFailed(e, backup_path.clone()))` arms (line 299 for retry-exhausted, line 300 for non-retryable). Neither calls `let _ = std::fs::remove_file(&tmp_path)` before returning. The lock file IS removed by `LockGuard::Drop`, the backup IS on disk, BRIEF.md IS unchanged — but `wg_root/BRIEF.md.tmp.<pid>` remains as litter.

**Why this matters:** test I20 (`set_title_aborts_on_readonly_brief_md_with_clean_state`, line 679) explicitly asserts *"lock file removed, no `BRIEF.md.tmp.*` litter."* The mapping table at line 700 reinforces this: *"read-only BRIEF.md fails cleanly with no litter (NIT-3) → I20."* With the current pseudocode, I20 fails: the rename-retry exhausts, the verb returns `RenameFailed`, and the per-PID tmp file is left on disk.

The plan establishes a clear pattern of best-effort cleanup on graceful-failure paths:
- §4 line 277: tmp-write failure → `let _ = remove_file(&tmp_path)` (MED-6).
- §4 line 287: sentinel-fired → `let _ = remove_file(&tmp_path)` ("tidy").
- §4 line 268 (in §C.1 prose): backup-copy failure → `let _ = remove_file(&bp)`.

The rename-failure path is the only graceful-failure path missing the same cleanup. §H.7 explicitly distinguishes "crashed-writer litter" (acceptable, deferred) from clean operational artifacts (must be absent post-call) — rename-failure is the *clean* case and should match the cleanup pattern.

**Consequence on disk for a coordinator hitting this path** (example: BRIEF.md marked read-only by Windows admin policy or anti-tamper tooling):
- Lock removed ✓
- BRIEF.md unchanged ✓
- Backup at `BRIEF.{ts}.bak.md` ✓
- **`BRIEF.md.tmp.<pid>` left behind ✗** — confuses humans inspecting the wg dir, accumulates if the read-only state persists across multiple invocations, and triggers a false "did the last write fail mid-stream?" diagnostic for anyone looking at the directory listing.

**Fix (1-2 lines per arm):** add `let _ = std::fs::remove_file(&tmp_path);` immediately before each `return Err(RenameFailed(...))` in the rename retry loop. Mirrors the §C.1 / MED-6 pattern. No new error path, no test infrastructure needed beyond what I20 already specifies.

**Disposition:** the implementer can apply this fix during coding without architect re-review — the plan's *intent* (NIT-3 "fails cleanly with no litter") is unambiguous; only the pseudocode missed the parallel update. Flagging so dev-rust does NOT inadvertently "fix" by removing the no-litter assertion from I20.

---

#### NIT-1 — HIGH-4 sentinel silently undoes external deletes

**Where:** §4 step 7a, line 289 (`Err(_) => ()   # file vanished — let rename surface the real error`).

**Trace:** if the post-snapshot `metadata(&brief_path)` call returns `Err(NotFound)` (an external process deleted BRIEF.md between our read and our sentinel-recheck), the code currently falls through to the rename. `std::fs::rename(&tmp_path, &brief_path)` does *not* require the destination to exist — on Windows (`MoveFileExW(MOVEFILE_REPLACE_EXISTING)`) and Unix, rename creates the destination if absent. So our verb silently re-creates the file with our edited content, undoing the external delete.

**Why the comment is misleading:** *"let rename surface the real error"* assumes rename will error when destination is gone. It won't — rename to a vanished destination is a normal create operation. Other `metadata()` errors (transient FS issues) might cause rename to also fail, but `NotFound` specifically is silently un-deleted.

**Whether this is a regression vs pre-HIGH-4:** strictly no — the pre-sentinel code also would have silently un-deleted. But the plan now claims (in §3 error matrix line 80, §4 "External-writer abort path", §7 failure-modes table) that the sentinel detects "external writers." A delete IS an external write semantically, and a user who reads §3a or §7 may reasonably expect deletion to be detected.

**Two reasonable fixes:**
- **(minimal)** Treat `Err(NotFound)` specifically as `ExternalWrite`: pattern-match `e.kind() == ErrorKind::NotFound`, abort with `ExternalWrite(backup_path.clone())`. Fix the comment to say the *other* `metadata` errors fall through. ~3 lines of pseudocode.
- **(documentation-only)** Update §4 / §7 / §3 error matrix to say "external *modification* (not deletion) detected." Cheap, sets correct expectations. Doesn't fix the un-delete behavior.

I prefer the minimal fix — it costs 3 lines and removes a surprising semantic. The user who deletes BRIEF.md gets `ExternalWrite` pointing at the backup that captures their pre-delete content. Recovery story is identical to the modify case.

**Disposition:** NIT, not blocking. Implementer's choice during coding.

---

#### NIT-2 — `ExternalWrite` and `RenameFailed` Display interpolate `Option<PathBuf>` with `{N:?}` → `Some("path")` / `None`

**Where:** §H.4, lines 1148-1152.

```rust
#[error("BRIEF.md was modified externally between read and write; aborting. Backup at {0:?} retains the externally-modified state.")]
ExternalWrite(Option<PathBuf>),
#[error("failed to publish BRIEF.md (rename): {0}. Backup at {1:?} retains the prior state.")]
RenameFailed(std::io::Error, Option<PathBuf>),
```

**Behavior:** `{0:?}` / `{1:?}` is Debug formatting on `Option<PathBuf>`. So:
- `Some(p)` → `Some("/path/to/BRIEF.20260101-000000.bak.md")` — quotes around the path AND a `Some(...)` wrapper.
- `None` → `None`.

**Mismatch with §3 error matrix:** the matrix rows for both errors use `<path>` as the placeholder (lines 80-81), implying a clean path string in the user-facing message. The Display impls produce the Debug form, which is shell-toolchain-ugly and confusing for end users.

Concrete example for `ExternalWrite` with backup at `C:\proj\.ac-new\wg-1-team\BRIEF.20260101-000000.bak.md`:

> Actual:    `Error: BRIEF.md was modified externally between read and write; aborting. Backup at Some("C:\\proj\\.ac-new\\wg-1-team\\BRIEF.20260101-000000.bak.md") retains the externally-modified state.`
> Expected:  `Error: BRIEF.md was modified externally between read and write; aborting. Backup at C:\proj\.ac-new\wg-1-team\BRIEF.20260101-000000.bak.md retains the externally-modified state.`

For `RenameFailed` with `backup_path == None` (the "brand-new BRIEF.md, rename fails" path — possible if the wg dir has weird permissions): the message ends with `Backup at None retains the prior state.` — semantically wrong (there's no prior state to retain).

**Why dev-rust missed this:** dev-rust round 2's review verified the variants exist and the matrix has rows, but didn't trace the actual interpolation output for `Option`. `{:?}` on `Option<T>` is a `<T as Debug>` wrap, not a `<T as Display>` unwrap.

**Fix options:**
- **(simplest)** Change `ExternalWrite` to `ExternalWrite(PathBuf)` (not Option) — invariant-enforced because in practice it's always `Some` (sentinel is only set when `file_existed`, and `file_existed → backup_path == Some(_)`). Display becomes `Backup at {0}`.
- **(robust)** Custom `impl Display for BriefOpError` instead of derived `#[error(...)]` — handles the Option case uniformly: `backup.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<no prior file>".into())`. Costs ~15 lines but matches the matrix wording exactly.

For `RenameFailed`, the backup-is-None case is "brand-new file, rename failed" — the natural string is "no backup (BRIEF.md did not exist before)." Worth handling explicitly in either fix.

**Disposition:** NIT, cosmetic but user-facing. Implementer can apply during coding.

---

#### NIT-3 — `TmpWriteFailed` Display interpolates full PathBuf vs matrix's filename-only placeholder

**Where:** §H.4 line 1147 (`TmpWriteFailed(PathBuf, std::io::Error)`) vs §3 matrix line 79.

`TmpWriteFailed`'s `#[error]` template uses `{0}` for the full PathBuf, so a typical message is:

> `Error: failed to write C:\proj\.ac-new\wg-1-team\BRIEF.md.tmp.12345: No space left on device. Aborting; BRIEF.md left unchanged.`

The §3 error matrix row 79 shows the placeholder as `BRIEF.md.tmp.<pid>` (filename only):

> `Error: failed to write BRIEF.md.tmp.<pid>: <io::Error>. Aborting; BRIEF.md left unchanged.`

**Cosmetic mismatch.** The full path is arguably MORE useful for diagnosis (the user immediately knows where to look). I prefer the full path. Recommend updating the matrix wording to `<absolute-tmp-path>` to match the Display, rather than reducing the Display.

**Disposition:** NIT, doc/Display alignment.

---

#### NIT-4 — §3a "GOLDEN RULE confines fake-path synthesis (necessary but not sufficient)" overstates the GOLDEN RULE's role

**Where:** §3a "What protects us today" subsection (lines 166-170).

**The claim:** *"The GOLDEN RULE in `default_context()` confines an agent's writes to its own replica root. So a malicious agent cannot synthesise a believable on-disk path: the fake `--root` it passes to the verb has to either point at a real sibling-coordinator's replica (which exists on disk because AC laid it out) or at a fabricated path (which doesn't exist on disk, so `wg_root.join("BRIEF.md")` writes a stray file in a non-WG location). The first case is the real attack; the second case is harmless."*

**Why this is too generous to the GOLDEN RULE:**

The GOLDEN RULE (verified at `config/session_context.rs:478`, generated by `default_context()`) is a **prompt-level instruction** to the *agent* (the LLM running inside the session). It says "you may only modify files in {allowed places}." It does NOT constrain what the *binary* does when invoked with arbitrary `--root`.

A malicious agent that crafts `<bin> brief-set-title --token <fresh-uuid> --root "C:/proj/.ac-new/wg-7-tech-lead-team/__agent_tech-lead/anywhere"` is making a **tool invocation** (Bash), not a write. The GOLDEN RULE has no clause about "you may not invoke the binary with `--root` pointing outside your replica" — and even if it did, a malicious agent that ignores the rule is exactly the threat we're modeling.

The actual bound on the attack surface is **independent of the GOLDEN RULE**:
- `agent_fqn_from_path` is a pure string operation (verified `teams.rs:62`) — accepts any path-shaped string.
- `workgroup_root` is a pure string operation (verified `messaging.rs:54`) — walks ancestors of the string.
- The verb writes to `<wg_root>/BRIEF.md`. Success requires `<wg_root>` to exist on disk. **That's the bound.** The set of writable target paths is "extant `wg-N-*` directories on disk" — exactly the set of sibling workgroups, which is what the attack wants. `list-peers` enumerates this set directly.

So the GOLDEN RULE provides ZERO defense against the actual attack. It bounds well-behaved agents (who would not forge `--root` because the rule says not to), but the attack we worry about is precisely a malicious agent that ignores the rule.

**Suggested rewording for §3a:**

> *"What protects us today (necessary but not sufficient)*
>
> *The attack requires a `--root` whose ancestors include a real `wg-N-<team>` directory on disk — otherwise `wg_root.join("BRIEF.md")` writes to a non-existent parent and `fs::write` fails. So the attack surface is bounded to **extant sibling WG dirs** (i.e. workgroups AC has already laid out under the same project). `list-peers` enumerates this set directly.*
>
> *The GOLDEN RULE in `default_context()` (`config/session_context.rs:478`) tells well-behaved agents to confine writes to their own replica — but it is a prompt-level instruction to the LLM, not a binary-level enforcement. A malicious agent that ignores the GOLDEN RULE can freely pass any `--root` string to the binary; `agent_fqn_from_path` and `workgroup_root` are pure path operations that never check ownership. The GOLDEN RULE bounds **honest agent behavior**, not **the attack surface**."*

The `(necessary but not sufficient)` qualifier as currently written implies the GOLDEN RULE is doing some defensive work. It isn't — it's orthogonal to the attack. Worth tightening so a future auditor doesn't misread §3a as "the GOLDEN RULE provides some defense in depth."

**Disposition:** NIT, wording-only. The risk acceptance for #137 is sound and the follow-up-issue framing is correct; only the "what protects us today" framing needs a small tighten.

---

#### NIT-5 — Duplicate `title:` lines in frontmatter cause set-title to replace only the first

**Where:** §5 set-title behavior matrix line 481 ("Replace the FIRST line whose `trim_start()` starts with `title:`").

**Scenario:** a hand-edited BRIEF.md frontmatter with two `title:` lines (rare but possible — user mistake, merge conflict resolution gone wrong, etc.). Apply `set-title "new"`:
- First `title: old` line is replaced with `title: 'new'`.
- Second `title: old2` line is preserved verbatim.
- Result frontmatter has two title lines: `title: 'new'` and `title: old2`.

**YAML semantics:** YAML parsers vary on duplicate keys — strict mode rejects, lenient mode picks (typically) the last. So a downstream YAML reader of BRIEF.md may see `title: old2` (lenient last-wins) instead of the new title the user just set.

**Idempotence interaction:** `title_value_of` finds the FIRST title line (per the implementation in §5 line 489). So a subsequent `set-title "new"` would idempotence-skip (first title already matches), leaving the duplicate `title: old2` permanently in the file.

**Disposition:** NIT, edge case (most BRIEF.md files have exactly one title line). Not blocking. A defensive option for the implementer: warn (via `log::warn!`) when the frontmatter contains multiple `title:` lines, and consider adding a comment in the implementation noting the FIRST-line semantics. Don't change the "replace FIRST" rule — changing to "replace ALL" would surprise users who deliberately have multiple title-shaped lines (e.g., `title: x` in a YAML literal block).

---

#### NIT-6 — U21 test description is misleading about how to fake mtime in std-only Rust

**Where:** §9 unit-test U21 (line 640).

**Quote:** *"pre-create a lockfile with mtime 6 min ago; call `acquire` with `stale_after=Duration::from_secs(300)` ... Use a shorter `stale_after` in the test if convenient (the constant is configurable per-call)."*

**Problem:** "pre-create a lockfile with mtime 6 min ago" requires either:
- The `filetime` crate (NOT a current dep, NEW dep — forbidden by §12 "Do NOT add `filetime` to `Cargo.toml`" — well, it doesn't list filetime explicitly but the spirit of "no new deps" applies).
- `std::os::unix::fs::PermissionsExt` + libc `utime()` on Unix and `SetFileTime` via `windows-sys` FFI on Windows — clumsy, requires platform-conditional code in tests.

The "Use a shorter `stale_after` in the test if convenient" qualifier makes the easier path possible (e.g., `stale_after = Duration::from_millis(10)` + `thread::sleep(Duration::from_millis(20))` after creating the lock, then call `acquire`). But the qualifier is *secondary* to the "pre-create with mtime 6 min ago" instruction, which an implementer might take literally.

**Recommended rewording:** invert the priority — *"Test approach: pre-create the lockfile via `OpenOptions::new().create_new(true).write(true).open(...)`, sleep briefly (e.g., 20ms), then call `acquire` with a small `stale_after` (e.g., `Duration::from_millis(10)`). Asserts that the stale lock is removed (warn log emitted) and the second acquire succeeds. The production constant is `LOCK_STALE_AFTER_5M = Duration::from_secs(300)`; the test uses a smaller value because std-only Rust cannot easily fake file mtimes without an FFI call."*

**Disposition:** NIT, test-doc clarity. The implementer with std-only knowledge will figure it out, but the current wording invites a wasted hour.

---

#### NIT-7 — §4 step 4-5 wording inconsistency: `apply_edit` returns `new_content` but step 5 references `parsed_after_edit`

**Where:** §4 lines 235-244.

```text
# ─── 4. Apply edit (per-op; see §5 + §6) ──────────────────────────────────
let new_content = apply_edit(parsed, op)?

# ─── 5. Idempotence short-circuit (set-title only; see §H.6 + MED-3) ──────
#        ...
if op.is_set_title() && title_value(&parsed_after_edit) == title_value_of(&existing_parsed):
    return Ok(EditOutcome::NoOp)
```

`apply_edit` returns `new_content` (a `String`), but the idempotence check at step 5 references `parsed_after_edit` (a `ParsedBrief`). These don't match. §5 line 494 then shows the *correct* check using `new_parsed`, also a `ParsedBrief`. So the §4 pseudocode is internally inconsistent and the correct intent is in §5.

**Implementer impact:** the implementer might write `apply_edit -> String` and then have to re-parse to do the idempotence check, OR write `apply_edit -> ParsedBrief` and render separately. The clean design is the second: `apply_edit -> ParsedBrief`, then `render(&new_parsed) -> String` happens after the idempotence skip-check. This avoids re-parsing.

**Recommended clarification in §4 step 4:**

```text
# ─── 4. Apply edit (per-op; see §5 + §6) — returns the post-edit ParsedBrief ─
let new_parsed = apply_edit(parsed, op)?

# ─── 5. Idempotence short-circuit (set-title only) ──────
if op.is_set_title() && title_value_of(&new_parsed) == title_value_of(&existing_parsed):
    return Ok(EditOutcome::NoOp)

# ─── 5b. Render to string for write ──────
let new_content = render(&new_parsed)
```

This makes the data-flow explicit: `parse → apply_edit → idempotence check → render → write`.

**Disposition:** NIT, pseudocode clarity. The implementer with attention will reconcile §4 vs §5; the implementer in a hurry might double-parse.

---

### Items I checked and found clean (not flagged)

- **CRIT-1 Form B parser interactions** with D.1 trim-tolerance, HIGH-3 BOM peeling, empty input, BOM-only input, "---"-with-no-newline, opening-only, leading-LF — all paths produce byte-exact body slices and don't panic.
- **HIGH-2 PID reuse on Windows** — `std::fs::write` truncates an existing same-PID tmp on subsequent runs (default `OpenOptions::write(true).create(true).truncate(true)`); same-PID litter self-heals.
- **HIGH-3 BOM round-trip across all six rewrite paths** (set-title × {has-FM-with-title, has-FM-no-title, no-FM} × file-exists, append-body × file-exists, NoOp skip, brand-new file) — `bom: bool` correctly carried; new files default `bom:false`; `consumed` arithmetic happens after BOM peel so no BOM bytes leak into body.
- **§4 lock release on every error path** — `_lock` is a named binding (not `let _ =`), `LockGuard::Drop` fires on every `?`-return and every explicit `return Err(...)`. Confirmed against `BackupFailed`, `BackupExhausted`, `TmpWriteFailed`, `ExternalWrite`, `RenameFailed`.
- **§B.2 collision-suffix loop semantics** — `OpenOptions::create_new(true)` is the kernel-mutex; same-second concurrent callers get distinct numbered suffixes; lock serialization keeps the loop bounded; 99 retries is gross overkill but consistent with `phone/messaging.rs:208`.
- **§C.1 backup partial-cleanup** — `let _ = remove_file(&bp)` after `fs::copy` failure is correct; the create_new'd 0-byte file gets cleaned regardless of whether copy wrote anything.
- **MED-6 tmp-write partial-cleanup** — same pattern, correct.
- **§3a anchors** — `validate_cli_token` (`cli/mod.rs:59-98`, UUID parse at line 88), `agent_fqn_from_path` (`teams.rs:62`, pure path op verified), `workgroup_root` (`messaging.rs:54`, pure path op verified), `is_coordinator` (`teams.rs:403`, AR2-strict verified), `default_context` GOLDEN RULE (`session_context.rs:478` — generates the prompt verified). Attack walkthrough is technically accurate.
- **§3 error matrix vs §H.4 BriefOpError variants** — every variant has a matrix row, every row has a variant. Wording on `LockIo` aligned per MED-5.
- **§9 test plan barrier-vs-no-barrier (U25 / I15)** — both correctly require `std::sync::Barrier::new(2)` per MED-2; absent the barrier, the test would pass for the wrong reason.
- **U6 strict body equality** — pins CRIT-1 fix without ambiguity.
- **U22 per-PID tmp cleanup assertion** — pins HIGH-2 cleanup; the test correctly checks no `BRIEF.md.tmp.*` files remain after a *successful* call (the rename consumes the tmp by atomically moving it).
- **U31 BOM round-trip** — pins HIGH-3 with sufficient assertions.
- **U32 CRLF round-trip byte-exact** — pins CRIT-1 end-to-end.
- **U33 line-ending preservation** — pins LOW-3.
- **I15 + Barrier** — pins MED-2.
- **I17 control-char rejection** — pins LOW-1 + D.2.
- **`extract_yaml_single_quoted` fallback for non-canonical inputs** — conservative-direction-harmless / unsafe-direction-impossible analysis is correct (parsed-after-edit is always canonical single-quoted, so a NoOp false-positive is impossible).
- **Idempotence check ordering** — runs BEFORE backup, so a NoOp creates no backup file. Matches the `EditOutcome::NoOp` success-message ("BRIEF.md unchanged ...").
- **Rejected items rationale** — LOW-2 monotonic counter, HIGH-2 liveness check, HIGH-1 token-tightening, HIGH-4 exclusive-share locking — each rejection is correctly justified for #137 scope.

### Verdict

**APPROVE for implementation, with 1 MED to be fixed during coding (no architect re-review needed).**

- 0 CRIT.
- 0 HIGH.
- 1 MED (tmp-cleanup-on-rename-failure — direct read of pseudocode vs I20; 1-2 line fix in §4 step 7b).
- 7 NIT (semantic-gap on external delete, two cosmetic Display-format issues, one §3a wording issue, one duplicate-title edge case, one test-description clarity issue, one §4 pseudocode wording inconsistency).

The architect's round-2 resolutions for CRIT-1 and HIGH-1..4 all close the original failure modes; no regressions, no new races introduced. The MED finding is a self-contained pseudocode oversight (the rename-failure path missed the same `let _ = remove_file(&tmp_path)` cleanup that every other graceful-failure path in §4 already has) and dev-rust will hit it the moment they run I20 — flagging it loudly here so the fix is "add cleanup to the pseudocode" rather than "drop the assertion from the test." All seven NITs are implementer-discretion polish.

This is round-2 consensus from my side; tech-lead can move to Step 6.

— grinch, round 2.

---

## Round 3 — Architect Verdict

> Author: architect (round 3). Reviewed dev-rust round 2 (6 NITs A–F) and grinch round 2 (1 MED + 7 NITs). Both reviewers explicitly green-lit move-to-implementation; no item required arbitration of a divergence (where dev-rust and grinch overlap, they agree; grinch's seven additional findings are NITs dev-rust did not catch, not items dev-rust pushed back on). Round-1 caught CRIT/HIGH; round-2 caught a typo + cosmetics. Convergence pattern is healthy.

### Verdict

**`READY_FOR_IMPLEMENTATION`**

Where the change was small enough to remove ambiguity for the implementer, I patched canonical §1–§12 directly in this round (12 surgical edits — see disposition table below). The remainder are folded into implementer discretion with explicit guidance below.

Tech-lead can dispatch dev-rust for Step 6.

### Per-finding disposition

| ID | Source | Severity | Disposition | Plan delta |
|---|---|---|---|---|
| MED-1 | grinch | MED | **PATCHED §4 step 7b** | Added `let _ = std::fs::remove_file(&tmp_path)` before each `return Err(RenameFailed(...))` arm in the rename-retry loop. Mirrors §C.1 / MED-6. I20's "no `BRIEF.md.tmp.*` litter" assertion now matches the pseudocode. |
| NIT-A | dev-rust | NIT | **PATCHED §7 line 540** | "Stale-lock recovery (60 s)" → "(5 min — `LOCK_STALE_AFTER_5M`, raised from 60 s per HIGH-2)". Doc-only consistency with the rest of §7 and the constant name. |
| NIT-B | dev-rust | NIT | **PATCHED Round-2 audit-trail HIGH-2 ruling** | Tightened the deferral rationale: removed the factually-wrong "`windows` crate (not currently a dependency)" claim (windows-sys is already declared at `src-tauri/Cargo.toml:34-35` with `Win32_System_Threading`). New rationale grounds the deferral in "extra unsafe-FFI block + Windows/Unix `cfg`-split + new error paths + minimal-blast-radius bias", which is the actual reason the deferral is defensible. **Conclusion (defer to follow-up) unchanged.** |
| NIT-C | dev-rust | NIT | **PATCHED §4 sentinel prose** | Added the FAT32 mtime-granularity note inline in the "External-writer abort path (HIGH-4 sentinel)" subsection — documents the 2-second granularity edge case for FAT32 and confirms the realistic AC layout (NTFS / EXT4 / APFS) is sub-second. |
| NIT-D | dev-rust + grinch (both) | NIT | **PATCHED §4 step 2a + 7a** | `pre_sentinel: Option<(u64, SystemTime)>` → `Option<(u64, Option<SystemTime>)>`. Step 7a captures `now_meta.modified().ok()` and only compares mtimes when both sides are `Some`; `len` is always compared. Removes the false-positive surface where transient `modified()` failures (rare on local NTFS, possible on SMB/NFS) made the asymmetric `unwrap_or(UNIX_EPOCH)` produce a spurious `ExternalWrite`. |
| NIT-E | dev-rust + grinch (both) | NIT | **PATCHED §9 (added U34)** | `apply_append_body_preserves_internal_body_line_endings_and_documents_trailing_loss` pins the §5 row-510 mixed-line-ending trade-off. Prevents a future contributor from silently regressing the body-preservation guarantee to all-LF re-rendering. |
| NIT-F | dev-rust + grinch (both) | NIT | **PATCHED §4 step 7a comment + sentinel prose** | Pinned the read→metadata window of ~µs explicitly, with a "do NOT tighten by moving the snapshot before the read" warning. Discourages a well-meaning future "fix" that would introduce a worse, unbounded write-between-snapshot-and-read window. |
| NIT-1 | grinch | NIT | **PATCHED §4 step 7a** | `Err(NotFound)` from the post-snapshot `metadata()` call is now matched specifically and treated as `ExternalWrite(bp)` with tmp cleanup. Without this branch, rename to a vanished destination silently re-creates the file (rename-create is a normal operation on Windows `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` and Unix), undoing the external delete. Recovery story is identical to the modify case (backup captures pre-delete content). Grinch's "minimal fix" path adopted. |
| NIT-2 | grinch | NIT | **PATCHED §H.4** | `ExternalWrite(Option<PathBuf>)` → `ExternalWrite(PathBuf)` (always-Some by invariant: sentinel-fired ⇒ `file_existed` ⇒ `backup_path: Some(_)`). Derived `#[error]` template uses `{0}` cleanly. `RenameFailed(io::Error, Option<PathBuf>)` keeps the Option (brand-new-file path has no prior backup) but adopts a custom `impl Display` block that handles both arms with clean wording instead of `{1:?}` Debug-formatting. Removes the `Some("...")` / `None` user-facing leakage flagged by grinch. |
| NIT-3 | grinch | NIT | **PATCHED §3 matrix line 79** | TmpWriteFailed placeholder `BRIEF.md.tmp.<pid>` → `<absolute-tmp-path>` to match the Display impl which interpolates the full PathBuf. Grinch's "prefer the full path for diagnosis" position adopted; matrix wording is the side that moved. |
| NIT-4 | grinch | NIT | **PATCHED §3a "What protects us today"** | Rewrote the subsection to (a) anchor the attack-surface bound on "extant `wg-N-*` directories on disk" (the actual bound, set by AC layout), and (b) explicitly state that the GOLDEN RULE is a prompt-level instruction to the LLM and provides ZERO defense against a malicious agent that ignores it. Grinch's exact framing adopted (modulo light editorial tightening). The risk-acceptance and follow-up-issue framing in the rest of §3a is unchanged — only the "what protects us" framing needed correcting so a future auditor doesn't misread §3a as "the GOLDEN RULE provides defense in depth." |
| NIT-5 | grinch | NIT | **PATCHED §5** | Added a "Duplicate `title:` lines" subsection under the set-title behavior matrix. Specifies a `log::warn!` when more than one frontmatter line is title-shaped, keeps the "replace FIRST" rule (changing to "replace ALL" would surprise legitimate users with title-shaped lines inside YAML literal blocks), and notes that `title_value_of` also reads the FIRST line so idempotence still works. |
| NIT-6 | grinch | NIT | **PATCHED §9 U21** | Inverted the priority: the test approach now leads with the std-only path (`OpenOptions::create_new` + ~20 ms sleep + `Duration::from_millis(10)` `stale_after`) and explicitly documents *why* (no `filetime` dep, no FFI). Production constant `LOCK_STALE_AFTER_5M = 300s` is documented as the prod value. Removes the "pre-create with mtime 6 min ago" wording that invited a wasted hour for the implementer. |
| NIT-7 | grinch | NIT | **PATCHED §4 step 4-5** | `apply_edit` now returns `ParsedBrief` (was `String`); idempotence check uses `title_value_of(&new_parsed) == title_value_of(&parsed)`; explicit `let new_content = render(&new_parsed)` step 5b. Data flow `parse → apply_edit → idempotence check → render → write` is now internally consistent between §4 and §5 line 494. Implementer no longer has the "double-parse vs return-ParsedBrief" decision. |

### Items where dev-rust and grinch positions diverged

**None.** Where both reviewers flagged the same issue (NIT-D, NIT-E, NIT-F), they agreed on the disposition. Grinch's seven additional findings (MED-1 + NIT-1 through NIT-7) are items dev-rust did not catch in their pass, not items dev-rust pushed back on. Both reviewers' verdict lines explicitly green-light implementation.

### Items where the architect made a judgment call (not strictly imposed by the reviewers)

- **NIT-1 (sentinel external-delete).** Grinch offered two fixes: minimal (3-line pseudocode patch to treat `NotFound` as `ExternalWrite`) or documentation-only ("external *modification* (not deletion) detected"). I adopted the minimal fix because the user-facing semantic is "we detect when BRIEF.md was modified externally during the operation"; a delete is a more aggressive form of modification, the recovery story is identical (backup captures pre-delete content), and the cost is 3 lines. Documentation-only would have left a surprising semantic in the verb's behavior.

- **NIT-2 (Display Option formatting).** Grinch offered "simplest" (`ExternalWrite(PathBuf)` invariant-Some + custom Display for `RenameFailed`) or "robust" (custom Display for both via a single `impl`). I adopted "simplest" for `ExternalWrite` because the invariant *is* genuinely always-Some (sentinel fires ⇒ `file_existed` ⇒ `backup_path == Some(_)`) and reflecting that in the type is more honest than carrying an Option that compiles to a runtime-impossible `None` arm. For `RenameFailed`, the brand-new-file `None` case is real, so the custom-Display path is unavoidable there.

- **NIT-3 (TmpWriteFailed matrix vs Display).** Grinch suggested moving the matrix (full path is more useful for diagnosis); I agreed. The alternative — reducing the Display to filename-only — would lose useful diagnostic information for the user who has to find the failed tmp file in a crowded `.ac-new/` tree.

### Items deliberately NOT folded back into the canonical sections (implementer discretion)

None. Every round-2 finding is reflected somewhere in §1–§12 after this round, in either pseudocode, prose, error-matrix, test plan, or implementer notes. The implementer reads §1–§12 as the authoritative spec; this Round 3 section is the audit trail explaining *why* each round-2 finding produced (or did not produce) a specific edit.

### Files updated in this round (canonical sections)

- **§3** — Matrix row 79: `BRIEF.md.tmp.<pid>` placeholder → `<absolute-tmp-path>` (NIT-3).
- **§3a** — "What protects us today" subsection rewritten as "What bounds the attack surface today" + "Why the GOLDEN RULE is **not** part of this bound" + "Net attack surface" (NIT-4).
- **§4 step 2a** — `pre_sentinel` now `Option<(u64, Option<SystemTime>)>`; uses `.modified().ok()` (NIT-D).
- **§4 step 4-5** — `apply_edit` returns `ParsedBrief`; explicit `render()` at step 5b (NIT-7).
- **§4 step 7a** — sentinel branch handles `Err(NotFound)` as `ExternalWrite`; mtime comparison only when both `Some`; tightened TOCTOU window comment (NIT-1, NIT-D, NIT-F).
- **§4 step 7b** — `let _ = std::fs::remove_file(&tmp_path)` before each `return Err(RenameFailed(...))` arm (MED-1).
- **§4 sentinel prose** — FAT32 mtime-granularity note + read→metadata window pin (NIT-C, NIT-F).
- **§5 set-title prose** — "Duplicate `title:` lines" subsection with `log::warn!` instruction (NIT-5).
- **§7 line 540** — "60 s" → "5 min" (NIT-A).
- **§9 U21** — Test-approach rewritten to lead with std-only path (NIT-6).
- **§9 U34** — New test pinning the append-body line-ending trade-off (NIT-E).
- **§H.4** — `ExternalWrite(PathBuf)` (was `Option<PathBuf>`); custom `impl Display for BriefOpError::RenameFailed` block; surrounding prose updated (NIT-2).
- **Round 2 — Architect Resolution (HIGH-2 ruling)** — Deferral rationale rewritten to ground the defer in "extra unsafe FFI + cfg-split + minimal-blast-radius bias" instead of the factually-wrong "windows crate not a dep" claim (NIT-B).

### Estimated implementation impact of round-3 patches

- ~12 lines of new pseudocode in §4 (step 7a NotFound branch + Option-pair sentinel comparison + step 7b cleanup).
- 1 new struct-field cardinality change (`pre_sentinel`'s mtime field becomes `Option<SystemTime>` rather than always-`SystemTime`).
- 1 enum-variant signature change in `BriefOpError` (`ExternalWrite(PathBuf)` not Option).
- 1 new custom `impl Display` block for `BriefOpError::RenameFailed`.
- 1 new test (U34); 1 rewritten test description (U21).
- 1 new `log::warn!` site in `apply_set_title`.
- All other patches are doc/prose tightening with no code-surface impact.

The round-3 patches do NOT introduce new modules, new crates, or new architectural surfaces. They tighten the existing surface against the issues round-2 found.

— architect, round 3.
