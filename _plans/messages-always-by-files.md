# Plan — Inter-Agent Messaging by Files

**Branch**: `feature/messages-always-by-files`
**Repo**: `repo-AgentsCommander`
**Date**: 2026-04-18
**Requirement source**: `__agent_tech-lead/_scratch/requirement-messages-by-files.md`

---

## 1. Summary

Replace inline `--message`/`--message-file` payloads in the `send` CLI with a
file-based delivery: sender writes a Markdown file under
`<workgroup-root>/messaging/`, then calls `send --send <filename>`. The PTY
injection carries only a short notification (≤500 chars) pointing to the file
on disk. The recipient reads the file via filesystem, bypassing PTY truncation.

No legacy coexistence: `--message` AND `--message-file` are both removed. Only
`--send` carries payload; `--command` is untouched.

---

## 2. Design decisions (resolving open questions)

| # | Question | Decision |
|---|---|---|
| Q1 | Atomic write+send combo? | **No.** Two-step (write file, then `send --send <file>`) is acceptable and simpler. |
| Q2 | File-write failure surfacing? | **Caller's problem.** CLI only validates the file exists. If missing, exit 1 with a clear error. |
| Q3 | `list-inbox` / `read-message` helpers? | **Skip.** The `Read` tool suffices. Don't grow the CLI surface unnecessarily. |
| Q4 | Concurrent same-second collisions? | **`OpenOptions::create_new(true)`** gives atomic allocation; on `EEXIST` retry with `.1`, `.2`, … up to `.99`. Architect-side helper used by both sender and any internal writer. |
| Q5 | Absolute path vs. filename in PTY notification? | **Absolute path.** Recipient Reads directly — no need to walk up to find workgroup root. |
| — | Timestamp timezone? | **UTC** (`chrono::Utc::now()`). Consistent across DST changes and cross-machine deliveries. |
| — | Legacy `--message-file`? | **Removed.** Its content also flowed through `OutboxMessage.body` → PTY → truncation risk. Same root cause as `--message`. |

### Agent name → short form

Rule (from list-peers name, e.g. `wg-7-dev-team/architect` → `wg7-architect`):

1. Split on `/` into `(prefix, suffix)`. If suffix missing, short form = sanitized full name.
2. If `prefix` matches regex `^wg-(\d+)-.*$` → short prefix = `wg{N}`. Otherwise short prefix = sanitized(prefix).
3. Sanitize(x) = lowercase, ASCII only, replace any non `[a-z0-9]` with `-`, collapse `--+` → `-`, trim leading/trailing `-`.
4. Final short = `{short_prefix}-{sanitize(suffix)}`.

Examples:
- `wg-7-dev-team/architect` → `wg7-architect`
- `wg-12-foo-bar/dev-rust` → `wg12-dev-rust`
- `repos/my-project` → `repos-my-project`
- `Agents/Shipper` → `agents-shipper`

### Workgroup root resolution

Rule (from `--root` value):

1. Canonicalize `--root`, split into components.
2. Walk up from the agent root. The **first parent directory** whose basename matches `^wg-\d+-.*$` is the workgroup root.
3. If no such ancestor exists → error: "messaging requires workgroup-scoped root (wg-N-*); no workgroup ancestor found for '<root>'". Fail fast; no silent fallback to user-wide messaging dir.

For the current workgroup example:
- Root: `C:\...\wg-7-dev-team\__agent_architect`
- Workgroup root: `C:\...\wg-7-dev-team`
- Messaging dir: `C:\...\wg-7-dev-team\messaging\`

### Filename pattern

```
YYYYMMDD-HHMMSS-{from_short}-to-{to_short}-{slug}[.N].md
```

- `YYYYMMDD-HHMMSS` UTC.
- `slug`: sender-provided, ≤50 chars. Sanitize same way as short-form suffixes. Empty after sanitization → error.
- `.N` suffix only on collision; integer starting at 1 up to 99; beyond 99 → error.
- Extension: `.md`.

Example: `20260418-143052-wg7-tech-lead-to-wg7-architect-messaging-redesign.md`.

### Notification payload (written to `OutboxMessage.body` by the CLI)

```
Nuevo mensaje: <absolute-path>. Lee este archivo.
```

After mailbox wrap: `[Message from {from}] Nuevo mensaje: <abs>. Lee este archivo.\n(To reply, write your response to <wg-root>/messaging/<new-file>.md, then run: "<bin>" send --token <your_token> --root "<your_root>" --to "{from}" --send <new-file> --mode wake)\n\r`. Fits under 500-char target for typical workgroup paths (< 320 chars body + reply hint).

---

## 3. Files to CREATE

### 3.1 `src-tauri/src/phone/messaging.rs` (new module)

Public API:

```rust
/// Resolve the workgroup root by walking up from agent_root.
/// Error if no ancestor matches the `wg-<N>-*` pattern.
pub fn workgroup_root(agent_root: &Path) -> Result<PathBuf, MessagingError>;

/// Resolve the messaging directory for a workgroup root (creates if missing).
pub fn messaging_dir(wg_root: &Path) -> Result<PathBuf, MessagingError>;

/// Convert an agent name (e.g. "wg-7-dev-team/architect") to short form
/// ("wg7-architect"). See rule in plan §2.
pub fn agent_short_name(full_name: &str) -> String;

/// Sanitize a slug: lowercase, ASCII, kebab-case, ≤ MAX_SLUG_LEN.
/// Returns Err if result is empty.
pub fn sanitize_slug(slug: &str) -> Result<String, MessagingError>;

/// Build the target filename (without path). Does NOT allocate a collision suffix.
pub fn build_filename(ts: DateTime<Utc>, from_short: &str, to_short: &str, slug: &str) -> String;

/// Given a prospective filename, atomically allocate a non-colliding path in
/// messaging_dir using `.1`, `.2`, … suffixes via OpenOptions::create_new.
/// Returns (absolute_path, file_handle). Caller writes content through the handle.
pub fn create_message_file(messaging_dir: &Path, base_filename: &str) -> Result<(PathBuf, File), MessagingError>;

/// Validate that `filename` exists in messaging_dir and is not a traversal.
/// Returns the absolute path on success.
pub fn resolve_existing_message(messaging_dir: &Path, filename: &str) -> Result<PathBuf, MessagingError>;

#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error("no workgroup ancestor found for '{0}'")] NoWorkgroup(String),
    #[error("slug is empty after sanitization")] EmptySlug,
    #[error("filename '{0}' contains path separators or traversal")] InvalidFilename(String),
    #[error("message file not found: {0}")] FileNotFound(String),
    #[error("collision suffix exhausted (99 retries) for {0}")] CollisionExhausted(String),
    #[error("io: {0}")] Io(#[from] std::io::Error),
}
```

Constants:
- `WG_PATTERN: &str = r"^wg-(\d+)-.*$"` (compile via `regex` crate OR hand-rolled parser — see §8 Dependencies).
- `MAX_SLUG_LEN: usize = 50`.
- `MAX_COLLISION_SUFFIX: u32 = 99`.
- `MESSAGING_DIR_NAME: &str = "messaging"`.

Implementation notes:
- `workgroup_root`: iterate `agent_root.ancestors()`, match basename against pattern (hand-rolled: starts with `wg-`, then parse digits until `-`, then any tail).
- `create_message_file`: try base, then `<stem>.1.md`, `<stem>.2.md`, … using `OpenOptions::new().write(true).create_new(true).open(&path)`. On `ErrorKind::AlreadyExists` retry.
- `resolve_existing_message`: reject if `filename` contains `/`, `\`, or `..`. Reject if not ending in `.md`. Reject if `canonicalize(messaging_dir.join(filename)).parent() != canonicalize(messaging_dir)` (prevents symlink escape).

### 3.2 `src-tauri/src/phone/mod.rs`

Add `pub mod messaging;` next to existing declarations (line 1-3 currently: `pub mod mailbox; pub mod manager; pub mod types;`).

**Exact edit** — insert before `pub mod types;`:
```rust
pub mod messaging;
```

---

## 4. Files to MODIFY

### 4.1 `src-tauri/src/cli/send.rs`

**Lines 18-20** (in `#[command(after_help = "…")]`) — replace the `QUOTING:` paragraph with a short `FILE-BASED MESSAGING:` paragraph:

```text
FILE-BASED MESSAGING: --send <filename> delivers the file at \
<workgroup-root>/messaging/<filename> to the recipient. The PTY only carries a \
short notification pointing to the absolute path; the recipient reads the file \
via filesystem, bypassing PTY truncation. Sender MUST write the file before \
invoking this command. Filename must match the standard pattern and exist in \
the messaging directory.
```

**Lines 30-37** (struct fields `message` and `message_file`) — delete both fields. Replace with:

```rust
/// Filename (not path) of a message file that already exists in
/// <workgroup-root>/messaging/. Sender writes the file before calling send.
/// Cannot be combined with --command.
#[arg(long)]
pub send: Option<String>,
```

**Line 49** (doc-comment on `command` field) — change "Cannot be combined with --message" to "Cannot be combined with --send".

**Lines 137-154** (message body resolution + validation block) — replace the `--message-file`/`--message` priority logic with:

```rust
// Resolve message body from --send (file-based) or --command
let message_body = if let Some(ref filename) = args.send {
    // Resolve workgroup root from --root
    let agent_root_path = std::path::Path::new(&root);
    let wg_root = match crate::phone::messaging::workgroup_root(agent_root_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    let msg_dir = wg_root.join(crate::phone::messaging::MESSAGING_DIR_NAME);
    let abs = match crate::phone::messaging::resolve_existing_message(&msg_dir, filename) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return 1;
        }
    };
    format!("Nuevo mensaje: {}. Lee este archivo.", abs.display())
} else {
    String::new()
};

// Require at least --send or --command
if message_body.is_empty() && args.command.is_none() {
    eprintln!("Error: --send or --command is required");
    return 1;
}
```

(Note: this keeps the short notification in `OutboxMessage.body` so the
MailboxPoller's existing injection path is untouched end-to-end.)

**Line 169** onward (`OutboxMessage { ... body: message_body, ... }`) — unchanged; still uses `message_body` local.

### 4.2 `src-tauri/src/phone/mailbox.rs`

**Lines 866-874** (interactive-session reply-hint template in `inject_into_pty`) — replace the `format!` call with:

```rust
format!(
    concat!(
        "\n[Message from {from}] {body}\n",
        "(To reply, write your response to <wg-root>/messaging/<new-filename>.md, ",
        "then run: \"{bin}\" send --token <your_token> --root \"<your_root>\" ",
        "--to \"{from}\" --send <new-filename> --mode wake)\n\r",
    ),
    from = msg.from,
    body = msg.body,
    bin = bin_path,
)
```

**Lines 971-979** (identical reply-hint template in `inject_followup_after_idle_static`) — apply the same replacement.

**Lines 1721-1722** (token-refresh notice embedded in `inject_fresh_token`) — update the example command in the `# === TOKEN REFRESHED ===` block to use `--send` instead of `--message`:

```rust
"#   \"{exe}\" send --token {token} --root \"{root}\" --to \"<agent_name>\" --send <filename> --mode wake\n\
```

(Filename means a file the agent must first write to `<wg-root>/messaging/`; keep the refreshed-token note terse.)

### 4.3 `src-tauri/src/config/session_context.rs`

**Line 421** (embedded `default_context` template, global AgentsCommanderContext.md injected into every agent) — inside the "Inter-Agent Messaging" / "Send a message to another agent" section, replace the quick-start block and add a one-paragraph explanation:

```markdown
### Send a message to another agent

**MANDATORY**: Before sending any message, resolve the exact agent name via `list-peers`.

Messaging is **file-based** to avoid PTY truncation. Two steps:

1. Write your message to a new file in the workgroup messaging directory. The
   directory lives at `<workgroup-root>/messaging/` (walk up from your root
   until you find the parent `wg-<N>-*` folder). Use a descriptive filename
   following the pattern
   `YYYYMMDD-HHMMSS-<wgN>-<you>-to-<wgN>-<peer>-<slug>.md`.
2. Fire the send:

```
"<YOUR_BINARY_PATH>" send --token <YOUR_TOKEN> --root "<YOUR_ROOT>" --to "<agent_name>" --send <filename> --mode wake
```

Do NOT use `--get-output` — it blocks and is only for non-interactive sessions.
After sending, stay idle and wait for the reply.
```

(Keep the `list-peers` quick-start block as-is; only the message-sending block changes.)

---

## 5. Documentation migration (outside Rust)

All occurrences of `--message`/`--message-file` in user-facing docs inside
`repo-AgentsCommander` must be rewritten. Enumerated paths:

| File | Lines | Action |
|---|---|---|
| `CLAUDE.md` | 46 | Update the `send` example to use `--send <filename>` and add the two-step note (write file first, then send). |
| `README.md` | 198-227 | Remove the `--message` and `--message-file` rows from the flag table. Add `--send` row. Remove the "QUOTING/PowerShell" paragraph and replace with a "File-based messaging" paragraph explaining the workgroup `messaging/` dir, the filename pattern, and the two-step flow. Update the first example to `--send <filename>`. |
| `ROLE_AC_BUILDER.md` | 416 | Update the send example to `--send <filename>` and reference the file-write prerequisite. |

Any other doc not listed here was not found by `grep -r --message src` —
devs must rerun the grep during implementation to catch late additions.

---

## 6. Test plan

### 6.1 Unit tests (new file `src-tauri/src/phone/messaging.rs`, `#[cfg(test)] mod tests`)

- `agent_short_name("wg-7-dev-team/architect") == "wg7-architect"`
- `agent_short_name("wg-12-foo-bar/dev-rust") == "wg12-dev-rust"`
- `agent_short_name("repos/my-project") == "repos-my-project"`
- `agent_short_name("Agents/Shipper") == "agents-shipper"`
- `agent_short_name("solo-name") == "solo-name"` (no slash)
- `sanitize_slug("Messaging Redesign!") == Ok("messaging-redesign")`
- `sanitize_slug("   ---  ") == Err(EmptySlug)`
- `sanitize_slug("a".repeat(100)) == Ok("a".repeat(50))` (truncated)
- `workgroup_root(Path::new("/tmp/wg-7-dev-team/__agent_architect")) == Ok(/tmp/wg-7-dev-team)`
- `workgroup_root(Path::new("/tmp/plain/agent")) == Err(NoWorkgroup)`
- `build_filename(fixed_ts, "wg7-lead", "wg7-arch", "redesign") == "20260418-143052-wg7-lead-to-wg7-arch-redesign.md"`
- `resolve_existing_message` rejects `../etc/passwd`, `foo/bar.md`, `foo`, `foo.txt`.
- `create_message_file` — in a tempdir, allocate base file; second call with the same base filename yields `.1.md`; third yields `.2.md`.

### 6.2 Integration tests

Create a scratch workgroup tree under `std::env::temp_dir()`:
```
tmp/
  wg-9-test/
    __agent_a/
    __agent_b/
    messaging/   (created on demand)
```

Flow test:
1. From `__agent_a`, call `messaging::create_message_file(messaging_dir, base)`.
2. Write content (e.g. 5 KB Lorem).
3. Simulate `send --send <filename>` by invoking the parsing block directly: assert `OutboxMessage.body` equals the expected notification string and contains the absolute path.
4. Assert the file still exists after CLI exit (not consumed by delivery).

Collision test:
- Build the same base filename twice within the same second; assert both resolved paths differ and both files persist.

### 6.3 Manual smoke (dev cycle)

- Launch two agents in the current workgroup (e.g. architect + tech-lead).
- From architect: write a 3 KB message file, `send --send <filename>`. Verify recipient receives the short notification and can `Read` the full file via the absolute path.
- Try `send --send nonexistent.md` → expect "message file not found".
- Try `send --send ../evil.md` → expect "contains path separators or traversal".
- Try `send --send foo.txt` → expect rejection (not `.md`).

---

## 7. Risks & pitfalls

1. **Workgroup root assumption.** The resolver fails for any agent not under a
   `wg-<N>-*` ancestor. Legacy per-project agents (non-WG) cannot send
   file-based messages. Acceptable per spec scope ("current workgroups are all
   `wg-N-dev-team`"), but callers must get a precise error, not a panic.
2. **Absolute path leak in PTY.** The full filesystem path appears in the
   recipient's console. Paths may contain the user's home directory. This is
   already the case for CLAUDE.md hints; no new leak, but worth noting.
3. **Notification length.** Absolute paths on Windows can exceed 260 chars on
   deeply-nested WG trees. Target ≤500 chars for the wrapped payload is still
   comfortable (notification ~320 + reply hint ~180). If a deployment exceeds
   500 chars, the wrap template must prefer the short filename over the
   absolute path. Mitigation: add a debug log in `inject_into_pty` warning if
   the wrapped payload exceeds 480 chars so operators notice.
4. **Delivered outbox retains notification only.** `delivered/<id>.json`
   stores the notification body, not the message content. This is by design
   (truth is in `messaging/`), but it means the existing delivered/ audit
   log is no longer a full record of exchanged content. Call this out in the
   commit message.
5. **Never-purged messaging dir.** Per spec, files are persistent. Long-lived
   workgroups will accumulate files. Out of scope for this change, but flag a
   follow-up issue for retention policy.
6. **Sender must own the filename.** Nothing stops a malicious sender from
   picking a filename that claims `to-wg7-other-agent` while actually sending
   `--to wg7-peer`. The filename is informational; routing uses `--to`. Fine,
   but do not add post-hoc consistency checks — they create confusing errors.
7. **Windows path canonicalization.** `std::fs::canonicalize` on Windows
   returns `\\?\C:\…` paths. Strip the `\\?\` prefix before emitting the
   notification (same pattern used in `session_context.rs:13`). Apply the
   same stripping in `resolve_existing_message` before the comparison.
8. **Concurrency on collision retry.** `create_new` is atomic per-file but the
   retry loop is O(N). Under extreme concurrent bursts (>99 writers in the
   same second) we hit `CollisionExhausted`. Acceptable; surface a clear
   error and let the sender retry with a different slug.
9. **Mailbox payload size metric.** Today `inject_into_pty` does not log
   payload length for non-command messages; with file-based we want length
   visibility. Add an `info!` log of `payload.len()` on injection (already
   present at line 894 as `debug!`; promote to `info!` for this path, or keep
   debug and rely on the new size-check warning above).

---

## 8. Dependencies

No new crates required:
- Regex: avoid `regex` crate for the `wg-<N>-*` match. Use hand-rolled `starts_with("wg-")` + digit parse.
- `chrono` already in `Cargo.toml` (line 14) — use for UTC timestamps.
- `thiserror` already in `Cargo.toml` (line 18) — use for `MessagingError`.
- `std::fs::OpenOptions::create_new` is std; no crate needed.

`phone/mod.rs` needs a single `pub mod messaging;` addition (noted §3.2).

---

## 9. Sequence for the implementing dev

1. Create `src-tauri/src/phone/messaging.rs` with the public API and unit tests in §6.1.
2. Register module in `src-tauri/src/phone/mod.rs`.
3. Run `cargo test -p agentscommander-new phone::messaging` — green.
4. Update `src-tauri/src/cli/send.rs` per §4.1.
5. Update `src-tauri/src/phone/mailbox.rs` per §4.2 (3 call sites).
6. Update `src-tauri/src/config/session_context.rs` per §4.3.
7. `cargo check` and `cargo clippy -- -D warnings`.
8. Run integration smoke per §6.3 in a live session.
9. Migrate docs per §5.
10. Bump version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, and `src/sidebar/components/Titlebar.tsx` (per project versioning rule).
11. Commit, push feature branch, hand off to coordinator.

---

## 10. Explicit NOT-scope

- Do NOT retain `--message` or `--message-file` as deprecated fallbacks.
- Do NOT auto-purge or rotate the `messaging/` directory.
- Do NOT change `--mode`, `--command`, `--get-output`, or any other flag semantics.
- Do NOT modify `repo-AgentsConnection`.
- Do NOT add a new outbox JSON field — keep the notification inside `body`.
- Do NOT add a `list-inbox` or `read-message` CLI subcommand.

---

## 11. dev-rust additions — 2026-04-18

Reviewer: dev-rust. Scope: validate every path/line/symbol, flag gaps, propose concrete enrichments. Not rewriting the plan; adding deltas.

### 11.1 Line-number corrections against current HEAD (verified 2026-04-18)

| Plan ref | Claim | Actual | Action |
|---|---|---|---|
| `send.rs` L18-20 | QUOTING paragraph in `after_help` | ✅ matches exactly | none |
| `send.rs` L30-37 | `message` and `message_file` struct fields | ✅ matches (L30-32 message, L34-37 message_file) | none |
| `send.rs` L49 | doc-comment on `command` field saying "Cannot be combined with --message" | ✅ matches (L48 is the text line, L49 is `#[arg(long)]`) | treat plan "L49" as "the comment block L47-48" |
| `send.rs` L137-154 | message body resolution + validation block | ✅ matches | none |
| `send.rs` L169 | `OutboxMessage { ... body: message_body, ... }` | ⚠️ off-by-7 — actual `OutboxMessage { ... }` begins L176, `body: message_body` is L181. L169 is `let msg_id = Uuid::new_v4().to_string();` | update plan to say "unchanged; body still assigned at L181 (current layout)" |
| `phone/mod.rs` L1-3 | `pub mod mailbox; pub mod manager; pub mod types;` | ✅ matches exactly | none |
| `mailbox.rs` L866-874 | `inject_into_pty` reply hint `format!` | ✅ block is L865-874 (off by 1) | minor, accept |
| `mailbox.rs` L971-979 | `inject_followup_after_idle_static` reply hint | ✅ matches exactly | none |
| `mailbox.rs` L1721-1722 | token-refresh example command inside `inject_fresh_token` | ✅ L1722 has the `--message "..."` line | none |
| `session_context.rs` L421 | default_context messaging block | ✅ matches (L421 is the code-block with `--message`; full block is L412-432) | none |
| `session_context.rs` L13 | UNC strip pattern | ✅ matches (`.trim_start_matches(r"\\?\")`) | none |
| `Cargo.toml` L14/L18 | chrono/thiserror presence | ✅ chrono L14, thiserror L18 | none |
| `CLAUDE.md` L46 | send example with `--message` | ✅ matches | none |
| `README.md` L198-227 | CLI section with `--message`/`--message-file` | ✅ matches (flag table L211-221, QUOTING paragraph L225) | none |
| `ROLE_AC_BUILDER.md` L416 | send example | ✅ matches | none |

`grep -r '--message\|--message-file'` inside `repo-AgentsCommander` found **exactly** the 7 files listed in plan §5 (plus plan + code). No stray occurrences. Doc migration scope is complete.

### 11.2 Hidden bonus — plan §4.2 silently fixes a pre-existing reply-hint bug

Current `mailbox.rs` L868 reply hint is:
```
(To reply, run: "{bin}" send --token <your_token> --to "{from}" --message "your reply" --mode wake)
```

But `send.rs` L91-97 enforces `--root` as **mandatory** (returns 1 if `None`). Any agent that literally copy-pastes today's reply hint hits `Error: --root is required`. Plan's replacement template correctly adds `--root "<your_root>"`. Call this out in the commit message as a bonus fix — it's not obvious from the diff alone.

(Confirmed empirically: the inbound message my session received from `tech-lead` minutes ago used the L868 template and the reply hint omits `--root`.)

### 11.3 Missing guard — `--send` + `--command` mutual exclusion

Plan §4.1 updates the doc-comment on `send` to say "Cannot be combined with --command" but the code block in §4.1 never rejects the combination. Today the `message` path and the `command` path coexist in the OutboxMessage (both fields serialized), but semantically they are exclusive.

**Add** to §4.1, immediately after the `if message_body.is_empty() && args.command.is_none()` block:

```rust
if args.send.is_some() && args.command.is_some() {
    eprintln!("Error: --send and --command are mutually exclusive");
    return 1;
}
```

Mirror symmetrical guard on `--command` doc-comment (current L48: "Cannot be combined with --message" → rewrite to "Cannot be combined with --send").

### 11.4 Constant visibility — `MESSAGING_DIR_NAME` must be `pub`

Plan §3.1 declares the constants inside `phone/messaging.rs`. Plan §4.1 references `crate::phone::messaging::MESSAGING_DIR_NAME` from `cli/send.rs`. For that to compile, declare as `pub const MESSAGING_DIR_NAME: &str = "messaging";`. Same for `MAX_SLUG_LEN` and `MAX_COLLISION_SUFFIX` **only** if external callers need them (likely not — keep them private unless/until referenced externally).

### 11.5 Workgroup-root resolver — remove the canonicalize step OR guard for non-existent paths

Plan §2 step 1 says "Canonicalize `--root`, split into components". Plan §3.1 implementation note says "iterate `agent_root.ancestors()`". These are compatible only if canonicalize succeeds. On Windows, `std::fs::canonicalize` fails when the path doesn't exist — which is fine at runtime (agent root always exists) but **breaks the §6.1 unit tests** that use synthetic paths like `Path::new("/tmp/wg-7-dev-team/__agent_architect")`.

**Fix**: drop the canonicalize step from `workgroup_root`. `Path::ancestors()` is a pure string operation. Canonicalization belongs **only** at the call site that needs to emit an absolute path (CLI's `resolve_existing_message` result, before being formatted into the PTY notification).

Updated rule:
1. (no canonicalize) Iterate `agent_root.ancestors()`.
2. For each ancestor, take `file_name()`, check against `^wg-<digits>-`.
3. First match → return `ancestor.to_path_buf()`.
4. No match → `Err(MessagingError::NoWorkgroup(agent_root.display().to_string()))`.

This keeps unit tests pure (no tempdir needed) and matches the plan's §6.1 test fixture style.

### 11.6 Symlink/traversal comparison — drop the UNC-strip-before-compare idea; canonicalize both sides

Plan §3.1 bullet on `resolve_existing_message` says compare canonicalized parent to canonicalized `messaging_dir`. Plan §7.7 adds "apply the same stripping in `resolve_existing_message` before the comparison". Mixing canonical + stripped forms is error-prone and unnecessary — if both sides are canonicalized the `\\?\` prefix is **identical** on both and equality holds naturally.

**Fix**: only strip `\\?\` at the **single** emission point — when the absolute path is formatted into the PTY notification body inside `send.rs` §4.1:

```rust
let abs_str = abs.to_string_lossy();
let abs_display = abs_str.trim_start_matches(r"\\?\");
format!("Nuevo mensaje: {}. Lee este archivo.", abs_display)
```

The comparison inside `resolve_existing_message` stays pure canonical-vs-canonical.

### 11.7 Reply-hint template — optional enhancement: interpolate recipient's own wg-root

Plan §4.2 replacement keeps `<wg-root>` as a literal placeholder in the injected reply hint. Works, but recipients must walk up to find their wg-root before writing a reply.

The injector already has everything it needs to pre-resolve: `mailbox.rs:1727` (token-refresh block) already reads `session.working_directory`. Same field is available at the reply-hint call sites (`inject_into_pty` has `msg.to` and can look up the session to get `working_directory`). Interpolating reduces the recipient's per-reply boilerplate.

**Recommendation**: compute `recipient_wg_root` once per injection (best-effort; fall back to literal `<wg-root>` if resolution fails) and interpolate into both reply-hint templates. Low-cost polish, high ergonomic payoff.

**Decision needed**: accept the polish or ship minimal-diff? Defaulting to minimal-diff (keep `<wg-root>` as literal) unless tech-lead says otherwise. Flagged as OPEN-1 below.

### 11.8 Payload-length logging — don't promote existing debug; add a dedicated warn

Plan §7.9 proposes "promote debug at L893 to info!, or keep debug and rely on new size-check warning". The `debug!` at L893-899 is generic injection diagnostics. Promoting it to `info!` would noise the log for every message (interactive and not).

**Fix**: leave L893 as `debug!`. Add a separate `log::warn!` **inside** the file-based path in `send.rs` §4.1 when `message_body.len() > 480`, emitted only for the short-notification path:

```rust
if message_body.len() > 480 {
    log::warn!("[send] notification payload length {} exceeds 480-char target (path {})", message_body.len(), abs.display());
}
```

Keeps non-file paths untouched.

### 11.9 UTC timestamp observation (documentation only)

Filenames use UTC per plan §2. When a user manually inspects `messaging/` on a non-UTC host, the filenames' timestamp will not match the local wall clock. Not a bug; worth a single-line note in the README migration (§5) so users aren't confused. Adds no code work.

### 11.10 Clippy / style pre-flight

Reviewed proposed new code for common clippy traps:

- `format!` with only `{}` placeholders — fine under clippy defaults.
- `thiserror::Error` with `#[from] std::io::Error` — OK; no manual `From` impl needed.
- `OpenOptions::new().write(true).create_new(true).open(...)` — standard; no clippy complaint.
- `.ancestors()` — lazy iterator; no allocation unless `to_path_buf()` at return. Fine.
- String allocation in `sanitize_slug` — acceptable given small input (≤50 chars).
- Import paths: `crate::phone::messaging::{...}` inside `cli/send.rs` — module tree confirmed: `main.rs` / `lib.rs` already expose `phone` (check). **Action for implementer**: verify `phone` is declared in `lib.rs` before relying on `crate::phone::messaging`. Trivial check, not in plan.

No blockers surfaced.

### 11.11 Test-plan gap — integration test for CLI wiring

Plan §6.2 asserts `OutboxMessage.body` equals the expected notification. Good, but does **not** exercise the actual `SendArgs → execute(args)` path end-to-end. That's where the mutual-exclusion guard (§11.3), the file-existence validation, and the wg-root resolver **all** converge.

**Add**: in §6.2, a subtest that builds a `SendArgs` with `send: Some("<filename>")`, runs `execute()` pointed at a tempdir outbox via `--outbox`, and asserts the outbox JSON contains the expected notification body. Exercises the full CLI wiring minus the MailboxPoller. Low effort; high coverage.

### 11.12 OPEN items for tech-lead / architect

- **OPEN-1**: Reply-hint interpolation (§11.7) — minimal-diff or pre-resolve? Defaulting to minimal-diff.
- **OPEN-2**: Root/master-token (`is_root=true`) senders without a `wg-<N>-*` ancestor. Plan §7.1 notes legacy non-WG agents can't send file-based. The **root-token case** (e.g. sends originated from the app's own outbox via `is_root` path in `send.rs` L123) wasn't addressed: does `--send` apply, or is the file-based path **skipped** when `is_root=true` and the sender writes directly into the app outbox with a legacy inline body? Need a decision: (a) forbid `--send` for root-token unless an explicit `--messaging-root` flag is passed; or (b) allow root-token senders to use an app-level messaging dir under `config_dir().join("messaging")`. I lean (a) — fail fast, no silent fallback — consistent with plan's "fail fast" ethos in §2.
- **OPEN-3**: Transition protocol. Today a stopgap ("temp-mensajeNNNN.md in agent-root") is in effect during this review. Post-ship, senders switch to `<wg-root>/messaging/YYYYMMDD-...-slug.md`. Do we need a one-off migration note or just cut over at merge? I'd cut over clean: the stopgap files live under each agent root (not in `messaging/`), will not be confused for real messages, and can be deleted manually. No automated migration needed.
- **OPEN-4**: Version bump size. Plan §9 step 10 says "bump version in 3 files". This is a breaking CLI change (remove `--message`, `--message-file`). Per SemVer: minor bump (`0.5.4` → `0.6.0`) rather than patch. Worth confirming with tech-lead before implementation.

### 11.13 Implementation-sequence nit

Plan §9 step 3 runs `cargo test -p agentscommander-new phone::messaging` — package name confirmed from `Cargo.toml` L2 (`name = "agentscommander-new"`). ✅ Command is correct as-written.

### 11.14 Summary of proposed edits above (not yet applied to sections 1-10)

If tech-lead/architect accept the enrichments in §§11.3, 11.4, 11.5, 11.6, 11.8, 11.11 they will need a follow-up pass on §§2, 3.1, 4.1, 6.2, 7.7, 7.9 to integrate. I'm **not** rewriting those sections pre-emptively — that's architect's call.

— dev-rust (review pass, implementation pending tech-lead sign-off on OPEN-1..4)

---

## 12. grinch adversarial review — 2026-04-18

Reviewer: dev-rust-grinch. Mandate: break the plan. All claims below verified against HEAD (send.rs, mailbox.rs L830-980 + L1715-1735, session_context.rs L412-432, phone/mod.rs, lib.rs, Cargo.toml). Dev-rust's §11 line-ref table is accurate as re-verified.

### 12.1 Findings

#### P0-1 — Filename pattern is validated only for traversal, not shape
**What.** Plan §4.1 validates filename (a) is in `messaging/`, (b) ends in `.md`, (c) no `..`/`/`/`\`. It does NOT validate the prescribed pattern `YYYYMMDD-HHMMSS-<wgN>-<from>-to-<wgN>-<to>-<slug>[.N].md`.
**Why.** Requirement §2 mandates the pattern. If any `.md` is accepted, a lazy sender can pick `reply.md` or `dump.md`, overwrite with each send, and destroy the audit trail (messaging/ is supposed to be append-only-by-convention). Also: senders who ignore the pattern break the recipient's ability to reason about timestamps/origin at a glance — the whole point of the encoded filename.
**Fix.** Plan §4.1 must pick a policy: (a) hard-enforce pattern in CLI via regex-like validator (fail with clear error on non-conforming); (b) document pattern as advisory and accept any `.md`. (a) is safer and aligns with the spec; (b) is laissez-faire. Default to (a) unless architect explicitly waives.

#### P0-2 — `resolve_existing_message` doesn't verify the target is a regular file
**What.** Plan §3.1 bullets describe reject on `/`, `\`, `..`, non-`.md`, symlink parent mismatch. It does NOT check `metadata.is_file()`. If a sender creates a **directory** `messaging/foo.md/`, `canonicalize` succeeds, parent match holds, path returned as "valid". Recipient's `Read` then errors with a cryptic `ErrorKind::IsADirectory` / `Os { code: 5 }` on Windows.
**Why.** A sender with shell access can trivially `mkdir messaging/foo.md`, pass `--send foo.md`. CLI accepts; recipient's tool chain dies with a confusing error. Not a security issue, but a correctness/UX hole.
**Fix.** In `resolve_existing_message`, after canonicalize/parent check: `if !abs.metadata()?.is_file() { return Err(MessagingError::InvalidFilename(filename.to_string())); }`. Or add a new variant `NotAFile`.

#### P0-3 — `--send` + `--command` mutex guard still missing from §4.1 body
dev-rust flagged this in §11.3 but the section 4.1 code block in §4.1 as-written has no guard. Elevating: this must land in the implemented code. Doc-comment alone is worthless — clap doesn't enforce "Cannot be combined" semantics on field comments. Without the runtime guard, a `--send foo.md --command clear` call both (a) loads the notification into body AND (b) sets `command=clear`. Current mailbox command path (mailbox.rs L830-845) runs the command THEN injects body as follow-up — so both fire. Semantically exclusive per spec, actually runnable per code. Must be rejected at CLI parse.

### 12.2 Findings — P1

#### P1-1 — Non-wg `is_root` senders have no defined path
**What.** `send.rs` L99-105 computes `is_root` via `validate_cli_token`. At L123 the routing gate is skipped for root tokens. Plan §7.1 waves "legacy per-project agents cannot send file-based" but doesn't enumerate who uses root-token. A `grep` shows **no internal Rust caller** of `send::execute` — all callers are external (users, scripts, app-shell-outs, the telegram bridge admin commands if any). After this change, any root-token invocation with `--root` outside a `wg-<N>-*` ancestor HARD-FAILS at `workgroup_root`.
**Why.** Today root-token callers can `send --message "text" --to "<agent>"` successfully. After merge, ALL of them break. The plan does not list them. We don't know the blast radius.
**Fix.** Before merge, audit: (a) enumerate known root-token callers (UI shell-outs? Telegram bridge? External scripts?). (b) Decide policy: forbid (fail fast, document migration path) OR allow app-level `<config_dir>/messaging/` as fallback for root-token only. I lean forbid (dev-rust OPEN-2 option a) — consistent with plan's "no silent fallback" ethos in §2 — BUT the plan must state this explicitly AND the implementer must attempt the audit. If even one legitimate root-token caller exists that we missed, this ships as a regression.

#### P1-2 — Telegram bridge exposes the notification-only body to remote users
**What.** The telegram bridge forwards PTY output (not OutboxMessage body directly) to Telegram subscribers. When a recipient agent receives a message injection, the `[Message from …] Nuevo mensaje: C:\…\foo.md. Lee este archivo.` line scrolls in its PTY — and gets forwarded. A remote Telegram observer sees an absolute file path to a machine they can't access.
**Why.** Regression for any operator using telegram bridge to passively monitor agents. Not a correctness bug (messaging still works agent-to-agent), but a UX cliff.
**Fix.** Acknowledge in §7 (risks). Optionally, for the telegram forwarder, detect the `Nuevo mensaje: <path>. Lee este archivo.` marker and inline the file content into the forward (bounded by some size cap). Out of scope for this feature but flag as follow-up.

#### P1-3 — `use_markers` (non-interactive `--get-output`) branch at mailbox.rs L855-863 is NOT in §4.2
**What.** Plan §4.2 only replaces L866-874 and L971-979 (interactive reply-hint templates). The `use_markers=true` branch (L855-863) renders `\n[Message from {}] {}\n\r` with body, no reply hint. Body is now the notification. Non-interactive callers programmatically processing body via the AC_RESPONSE marker protocol see a notification, not the payload.
**Why.** `--get-output` callers today expect body == full message. After change, body == "Nuevo mensaje: <path>. Lee este archivo." Callers must do a file-read to get the real content. This is a semantic change the plan didn't flag.
**Fix.** Either (a) plan explicitly states non-interactive consumers must handle the two-step read (notification → file-read) — document in §7 risks; OR (b) for `use_markers` path, inline the file content into the payload (defeats the point of file-based messaging but preserves the API). (a) is cleaner. Pick one.

#### P1-4 — Payload-length policy is warn-only, no clamp
**What.** Plan §7.3 adds a debug `warn!` if wrapped payload > 480. No hard cap. If a deployment has a deeply-nested wg path (e.g. `C:\Users\maria\0_repos\agentscommander\.ac-new\wg-42-multi-team-extended-name\messaging\<140-char-filename>.md`), notification body alone can top 280 chars, and the reply-hint wrap adds ~200 more. Total ≥ 480. Warn logs, payload still ships, PTY truncation risk — the exact issue we're fixing.
**Why.** The whole justification for file-based is "PTY truncates long payloads". If our own notification exceeds the PTY safe zone, we regress. Warn is not a mitigation; it's an alert that the bug already happened.
**Fix.** Add a hard behavior contract: if wrapped payload > `PTY_SAFE_MAX` (conservative: 500), the injector falls back to filename-only (`Nuevo mensaje: <filename>. Lee <wg-root>/messaging/<filename>.`). Recipient walks up to resolve wg-root (same cost as dev-rust's OPEN-1 interpolation, symmetrically). Ensures the notification itself never truncates.

#### P1-5 — `workgroup_root` resolver: dev-rust's §11.5 canonicalize-drop is CORRECT and critical
**What.** Confirming dev-rust's §11.5. Plan §2 step 1 says "Canonicalize `--root`". Plan §3.1 says "iterate `agent_root.ancestors()`". On Windows, `canonicalize` fails if any component doesn't exist (e.g. test fixtures under `/tmp/...` via `Path::new`). Keeping the canonicalize step breaks §6.1 unit tests AND adds a filesystem touch per send.
**Why.** Canonicalize is unnecessary for ancestor-matching (pure string op). Keeping it only serves to couple the plan to filesystem state and break unit tests. No benefit.
**Fix.** Apply dev-rust §11.5 as written. Canonicalize ONLY at PTY notification emission site (where we need the user-friendly absolute path).

### 12.3 Findings — P2

#### P2-1 — Windows short-path (8.3) edge case
If a sender uses a short-path form (e.g. `C:\PROGRA~1\...\messaging\foo.md`) via `--send`, the `filename` arg itself doesn't trigger the traversal check (no `/`, `\`, `..`), but the canonicalize would resolve it. Plan §3.1 takes `filename` not `full_path` — so a filename with no separators can't be a short-path representation. Non-issue, just flagging.

#### P2-2 — Clock skew / NTP adjustment mid-second
If the system clock goes backward between two sends (NTP correction, manual adjust), a later send can produce an EARLIER filename timestamp. `create_new` still serializes via `.N` suffix, so no data loss. Audit order via filename becomes misleading but timestamps inside OutboxMessage.timestamp are independent. Accept. No change needed.

#### P2-3 — Chrono `%Y%m%d-%H%M%S` is infallible for `Utc::now()`
No panic path. Confirmed.

#### P2-4 — Delivered audit log now shows escaped path
`delivered/<id>.json` body field shows `"Nuevo mensaje: C:\\Users\\...\\foo.md. Lee este archivo."`. Double-backslash is JSON-escape artifact. Operator eyeballing the log sees `\\`. Cosmetic. No action.

### 12.4 Votes on dev-rust OPEN items

**OPEN-1 (reply-hint interpolation)** — **INTERPOLATE**. Dev-rust's §11.7 data-availability claim verified: `mailbox.rs:1727` uses `session.working_directory` via the SessionManager lookup at the token-refresh injection. Same pattern applies to `inject_into_pty` (has `session_id`) and `inject_followup_after_idle_static` (has `session_id`). Cost is one extra SessionManager read-lock per injection; trivial. Benefit: every recipient stops having to walk up the ancestor tree by hand on every reply. Compounds across hundreds of messages. Fallback to literal `<wg-root>` on resolution failure is safe. Ship the polish.

**OPEN-2 (root-token senders)** — **FORBID (option a)**. Aligns with §2's "no silent fallback". But blocked by P1-1: plan must first enumerate known root-token callers and document their migration. If the audit reveals legitimate callers, implement the forbid as a **hard error with actionable message** (`"root-token senders must specify --root under a wg-<N>-* ancestor; see <doc-link>"`).

**OPEN-3 (stopgap transition)** — **CLEAN CUTOVER**. Stopgap `.temp-mensajeNNNN.md` lives in agent roots. New system lives in `<wg-root>/messaging/`. Different dirs, no collision. Post-merge, senders flip protocol in their next session. Document in commit message. Manual cleanup of stopgap files is fine. NO automated migration.

**OPEN-4 (SemVer)** — **MINOR (0.5.4 → 0.6.0)**. Removing `--message` AND `--message-file` is breaking CLI contract. Per SemVer 0.x conventions, minor = breaking. Patch undersells. Use 0.6.0. If the architect leans toward 0.5.5 "because no one relies on the interface yet", push back: the whole tech-lead → architect → dev-rust → grinch chain we just ran proves there IS reliance.

### 12.5 Approval status

**CONDITIONAL APPROVED.**

Implementation may proceed ONLY after the plan is revised to address:
- **P0-1** (pattern validation policy decision).
- **P0-2** (is_file check in resolve_existing_message).
- **P0-3** (runtime guard for --send + --command, per dev-rust §11.3).
- **P1-1** (is_root caller audit + policy statement).
- **P1-3** (non-interactive `use_markers` path: inline content or document two-step semantics).
- **P1-4** (hard clamp for payload length, not warn-only).

P1-2 and P1-5 can be merged as "accepted notes" into §7 risks without changing code scope. P2-1..P2-4 are FYI.

Not marked "block" because nothing is architecturally wrong — just gaps at the boundaries. Close them before the dev writes a line of code.

— dev-rust-grinch (adversarial review pass, 2026-04-18)

---

## 13. architect resolution — round 2 (2026-04-18)

§11 and §12 remain intact as review history. This section records decisions on
every P0/P1 finding and votes on OPEN-1..4. §§1-10 are treated as amendable:
**edit markers `[r2]`** cite the resolutions below — implementers follow the
`[r2]` edits when they diverge from the original text.

### 13.1 Root-token caller audit (blocks P1-1 decision)

Grep result (verified 2026-04-18):

- **Internal Rust callers of `send::execute`**: exactly one — `cli/mod.rs:105`,
  the CLI subcommand dispatcher. No other module calls it.
- **Root/master token paths** in `send.rs`: the `is_root` branch (L123,
  L199) only selects the app-level outbox directory; it does not originate
  messages from within the app itself.
- **Other invocations of the `send` subcommand**: external only — user shells,
  custom automation scripts, the UI (`commands/*` never shells out to `send`
  per repo grep), the telegram bridge (only uses the mailbox through its own
  ingest paths, not the `send` CLI).

Blast radius of FORBIDding `--send` from roots without a `wg-<N>-*` ancestor:
external users running root-token `send` calls from outside a workgroup. No
internal regressions.

### 13.2 Resolutions

| Finding | Decision | Reasoning | Plan edit |
|---|---|---|---|
| **P0-1** pattern shape validation | **HARD-ENFORCE** | Advisory = lazy senders overwrite `reply.md` repeatedly, destroying the append-only audit convention (per spec §2). | §3.1 + §4.1 [r2]: add `validate_filename_shape(&str) -> Result<(), MessagingError>` in `messaging.rs`, called from `resolve_existing_message` after existence check. Pattern: `^\d{8}-\d{6}-[a-z0-9-]+-to-[a-z0-9-]+-[a-z0-9-]+(\.\d{1,2})?\.md$`. Also called from `create_message_file` to reject caller-supplied bases that violate shape. |
| **P0-2** `is_file()` check | **ACCEPT** | Trivial; closes the `mkdir foo.md` hole. | §3.1 [r2]: bullet on `resolve_existing_message` adds `if !abs.metadata()?.is_file() { return Err(NotAFile(...)); }`. New variant `MessagingError::NotAFile(String)`. |
| **P0-3** `--send`/`--command` mutex | **ACCEPT** (per §11.3) | Doc-comment alone is worthless; clap has no runtime-exclusion from comments. | §4.1 [r2]: after body resolution, reject `args.send.is_some() && args.command.is_some()` with exit 1. Also rewrite L48 doc-comment of `command` field: "Cannot be combined with `--send`". |
| **P1-1** root-token non-wg senders | **FORBID** | Audit §13.1 confirms no internal caller. No silent fallback per §2 ethos. | §4.1 [r2]: `workgroup_root(&root)` is called for every `--send` regardless of `is_root`. Error text: `"--send requires --root under a wg-<N>-* ancestor; root-token senders must use a workgroup-scoped root. See _plans/messages-always-by-files.md §2."`. §7 [r2] gains a "legacy root-token callers" note pointing at this. No `--messaging-root` escape hatch. |
| **P1-2** telegram path exposure | **NOTE + follow-up issue** | Regression is UX-only; messaging still works agent-to-agent. Out of scope. | §7 [r2]: new risk item. Follow-up: detect `Nuevo mensaje: <path>. Lee este archivo.` marker in telegram forwarder and inline file content (size-capped). Track as a separate issue after merge. |
| **P1-3** `use_markers` two-step semantics | **DOCUMENT, no code change** | Verified mailbox.rs L855-863: payload is `[Message from X] {body}\n(Reply between markers…)`. With body=notification, the recipient agent reads the file via filesystem and replies inside markers. Semantically the two-step works; only surprise is for programmatic `--get-output` consumers that parsed `body` as payload. No such consumer exists in-tree. | §7 [r2]: new risk note. "Non-interactive `--get-output` callers now receive body=notification; the recipient is expected to read the file and compose its reply between markers. Programmatic consumers that scrape `body` as full payload must switch to the two-step read." No code change in §4.2. |
| **P1-4** hard payload clamp | **FAIL-FAST AT SEND** | Silent fallback to filename-only is unsafe for cross-WG (coordinator-to-coordinator) paths where recipient cannot resolve the file from its own wg-root. Reject at CLI build time so sender sees the error, not the recipient. | §3.1 [r2]: new `pub const PTY_SAFE_MAX: usize = 500;` and `pub const PTY_WRAP_OVERHEAD: usize = 280;` (reply-hint + interpolated wg-root + framing, measured). §4.1 [r2]: after constructing `message_body`, if `message_body.len() + PTY_WRAP_OVERHEAD > PTY_SAFE_MAX`, exit 1 with actionable error (`"notification path exceeds PTY-safe length; shorten slug or move workgroup to a shallower path"`). §7.3 [r2]: risk text replaced with this clamp description; warn-only mitigation removed. |
| **P1-5** drop canonicalize in `workgroup_root` | **ACCEPT** (per §11.5) | Pure ancestor-matching is a string op. Canonicalize at emission site only. | §2 [r2]: step 1 reworded to "Walk `agent_root.ancestors()` as a pure path operation (no canonicalize)". §3.1 [r2]: confirms same. §7.7 [r2]: UNC-strip happens only at the **single** emission point in `send.rs` §4.1 when formatting `abs.display()` — strip `\\?\` prefix via `.trim_start_matches(r"\\?\")`. `resolve_existing_message` compares canonical-vs-canonical (no stripping). |
| **P2-1..P2-4** | **FYI / no action** | Per §12.3. | — |

### 13.3 OPEN votes

- **OPEN-1 (reply-hint interpolation)** — **INTERPOLATE** (agree with grinch §12.4).
  Precedent `mailbox.rs:1727` (token-refresh) already reads `session.working_directory` inside an injection. Same pattern applies to `inject_into_pty` and `inject_followup_after_idle_static`: acquire SessionManager read-lock, look up session by `session_id`, pass `session.working_directory` into `messaging::workgroup_root`, fall back to the literal `<wg-root>` on any error (non-WG path, session gone, lock contention). Cost: one extra read-lock per injection — trivial. Benefit: recipients stop walking ancestors by hand on every reply.
  §4.2 [r2] replaces the reply-hint templates so `<wg-root>` is replaced by the interpolated path when available, and stays literal when resolution fails.
- **OPEN-2 (root-token FORBID)** — **RATIFIED** — see P1-1 resolution above.
- **OPEN-3 (clean cutover)** — **RATIFIED**. Stopgap `.temp-mensajeNNNN.md` files in agent roots never collide with `<wg-root>/messaging/*.md`. Document the cutover in the commit message; manual cleanup of stopgap files is acceptable.
- **OPEN-4 (SemVer minor bump)** — **RATIFIED**. Breaking CLI contract = minor bump per 0.x SemVer. §9 step 10 [r2]: bump to `0.6.0`.

### 13.4 Additional integrations

- **§6.1 [r2]**: add unit tests for `validate_filename_shape` (accepts the canonical example; rejects missing fields, wrong extension, bad separator, slug with invalid chars, `.100.md`). Add tests for `is_file` rejection using a tempdir-created sub-directory with a `.md` name.
- **§6.2 [r2]**: add CLI end-to-end subtest per dev-rust §11.11 — build `SendArgs` with `send: Some(...)`, run `execute()` with `--outbox` pointed at a tempdir, assert notification body in the written JSON. Add a second subtest exercising the PTY_SAFE_MAX clamp (ensure exit 1 on overly-long synthetic path).
- **§4.1 [r2]**: the absolute-path formatting goes through the UNC-strip step:
  ```rust
  let abs_str = abs.to_string_lossy();
  let abs_display = abs_str.trim_start_matches(r"\\?\");
  let message_body = format!("Nuevo mensaje: {}. Lee este archivo.", abs_display);
  ```
  (replaces the plain `format!("Nuevo mensaje: {}. Lee este archivo.", abs.display())` in the original §4.1).
- **§3.1 [r2]** — `MESSAGING_DIR_NAME` is `pub` (per §11.4). `PTY_SAFE_MAX` and `PTY_WRAP_OVERHEAD` are `pub` (referenced from `cli/send.rs`). `MAX_SLUG_LEN` and `MAX_COLLISION_SUFFIX` stay private.
- **§7.9 [r2]** — remove the "promote debug to info" proposal per §11.8. Add a dedicated `log::warn!` **only** on the file-based path in `cli/send.rs` when `message_body.len() > 200` (pre-wrap signal, well below the clamp): `log::warn!("[send] notification body length {} is unusually long", message_body.len());`. Non-file paths untouched.
- **§11.2 fix carry-over** — the original `mailbox.rs:868` reply-hint omits `--root`. The §4.2 replacement restores it. Mention this in the commit message as a bonus fix (per §11.2).
- **§5 [r2]** — README migration gains a one-liner explaining UTC timestamps in filenames (per §11.9).

### 13.5 Consolidated final edit list for the dev

Sequence unchanged; deltas only:

1. `phone/messaging.rs` — as §3.1 + [r2] additions: `validate_filename_shape`, `is_file` gate in `resolve_existing_message`, `NotAFile` variant, `pub PTY_SAFE_MAX`, `pub PTY_WRAP_OVERHEAD`, canonicalize-free `workgroup_root`.
2. `phone/mod.rs` — unchanged from §3.2.
3. `cli/send.rs` — as §4.1 + [r2]: `--send`/`--command` mutex runtime guard; `workgroup_root` called for every `--send` (including `is_root`); PTY_SAFE_MAX clamp; UNC-strip; long-body warn.
4. `phone/mailbox.rs` — as §4.2 + [r2]: reply-hint templates interpolate recipient's wg-root (fallback to literal `<wg-root>` on resolution failure) at L866-874 and L971-979; L1721-1722 updated to `--send`.
5. `config/session_context.rs` — as §4.3. No [r2] changes.
6. Docs migration — as §5 + [r2]: README adds the UTC-timestamp note.
7. Tests — as §6 + [r2]: add shape-validator, is_file, CLI end-to-end, PTY_SAFE_MAX clamp.
8. Version bump — `0.5.4` → `0.6.0` in `tauri.conf.json`, `Cargo.toml`, `Titlebar.tsx`.

### 13.6 Status

**Plan consolidated. Ready for dev-rust implementation.**

All grinch P0/P1 findings resolved. All OPEN items decided. No `ESCALATE` items — every question had enough information in-repo to decide. Round 2 closed from architect's side; awaiting tech-lead's green light to hand off to dev-rust (Step 6).

— architect (round 2 resolution, 2026-04-18)
