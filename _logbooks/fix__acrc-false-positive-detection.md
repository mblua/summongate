# Bug Fix: %%ACRC%% marker false positive detection

## Problem Statement
The `# === Session Credentials ===` block fires repeatedly and uncontrollably. The agent never outputs the `%%ACRC%%` marker, yet credentials keep being injected into the PTY.

## Root Cause
The PTY read loop in `pty/manager.rs` uses `scan_text.contains("%%ACRC%%")` to detect the marker. This fires on ANY occurrence of the string in the PTY output — including Claude Code's own rendered text (tool call parameters, prose, code references).

When the agent mentions `%%ACRC%%` in its output (e.g., searching for it, explaining it, or referencing it in code), Claude Code renders that text in the terminal. The PTY captures it, detects the marker, and injects credentials. The injection appears as a new message, the agent responds, and if the response mentions `%%ACRC%%` again, a feedback loop forms.

## Evidence
Transcript `20260331_122917_c4a3b0b9.log` contains 14 occurrences of `%%ACRC%%`:
- Tool call parameters: `Search(pattern: "ACRC|%%ACRC%%", ...)`
- Agent prose: `"la detección del marker %%ACRC%% en el PTY read loop"`
- None were intentional marker outputs by the agent

## Fix
Changed detection from `.contains("%%ACRC%%")` to line-based matching:
```rust
let has_standalone_marker = scan_text.lines().any(|line| line.trim() == "%%ACRC%%");
```

This only triggers when `%%ACRC%%` appears as a standalone line (with optional whitespace), matching the documented usage pattern: agents output the marker on its own line. When it appears embedded in prose or code, it's ignored.

Cross-buffer detection still works because the `acrc_tail` preserves newlines from the previous chunk.

## File Changed
- `src-tauri/src/pty/manager.rs` (line 176): `.contains()` → `.lines().any(|line| line.trim() == ...)`
