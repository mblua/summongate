# RTK (Rust Token Killer) - Plugin Implementation Guide

## What is RTK?

RTK is a CLI proxy installed on this machine that compresses command outputs to reduce token consumption. It works by filtering and condensing verbose tool output before it reaches the LLM context window.

- **Repo:** https://github.com/rtk-ai/rtk
- RTK only compresses output from Bash tool calls, not native Claude Code tools (Read, Grep, Glob)
- If RTK has a dedicated filter for a command, it compresses the output. If not, it passes through unchanged. RTK is safe to apply to commands that run through a bash/zsh shell.
- **PowerShell + AC CLI caveat:** RTK is bash-oriented. Do **NOT** prefix `rtk` to AgentsCommander CLI invocations made from a PowerShell session — PowerShell parses `rtk & '<BinaryPath>' ...` as `AmpersandNotAllowed` and aborts before the command runs. On PowerShell, invoke the AC CLI directly: `& '<BinaryPath>' <subcommand> <args>`. This carve-out applies only to commands that PowerShell would already execute via the `&` call operator (notably any quoted-path call, including the AC CLI). Bash-tool commands inside Claude Code are unaffected.

## Setup (two parts, both required)

### Part 1: Hook file (suppresses the warning)

RTK checks for a hook file at `~/.claude/hooks/rtk-rewrite.sh`. Without it, every command prints:
```
[rtk] /!\ No hook installed - run `rtk init -g` for automatic token savings
```

The hook also auto-rewrites Bash commands so Claude does not need to prefix `rtk` manually.

**Install:**
```bash
# Copy from the RTK installation
cp ~/.cargo/git/checkouts/rtk-*/*/hooks/rtk-rewrite.sh ~/.claude/hooks/rtk-rewrite.sh

# Or let RTK do it (Unix only - on Windows, copy manually as above)
rtk init -g --auto-patch
```

**Requires `jq`** - the hook script uses jq to parse and rewrite tool inputs.

### Part 2: PreToolUse hook in settings.json

The hook file alone suppresses the warning, but for Claude Code to actually execute the rewriting, the hook must also be registered in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "bash ~/.claude/rtk-rewrite.sh"
          }
        ]
      }
    ]
  }
}
```

With both parts in place:
- No warning on any `rtk` command
- Claude Code auto-rewrites Bash commands to use `rtk` (no manual prefix needed)
- The CLAUDE.md instruction block is still useful as a fallback/reminder

### Part 3 (optional): CLAUDE.md instruction block

Add to the project's `CLAUDE.md` as a fallback in case the hook is not present:

```markdown
<!-- rtk-instructions -->
## RTK (Token Optimizer)

`rtk` is a CLI proxy installed on this machine that compresses command outputs to reduce tokens.

**Rule:** ALWAYS prefix Bash commands with `rtk`. If RTK has a filter for that command, it compresses the output. If not, it passes through unchanged. Safe for Bash/zsh shells. **Do NOT** apply this to AgentsCommander CLI calls under PowerShell — `rtk & '<BinaryPath>' ...` triggers PowerShell's `AmpersandNotAllowed` parser error. On PowerShell, invoke the AC CLI directly: `& '<BinaryPath>' <subcommand> [args]`.

In command chains with &&, prefix each command:
rtk git add . && rtk git commit -m "msg" && rtk git push

Applies to: git, gh, cargo, npm, pnpm, npx, tsc, vitest, playwright, pytest, docker, kubectl, ls, grep, find, curl, and any other command.

Meta: `rtk gain` to view token savings statistics, `rtk discover` to find missed RTK usage opportunities.
<!-- /rtk-instructions -->
```

## Token Savings Overview

| Category | Commands | Typical Savings |
|----------|----------|-----------------|
| Tests | vitest, playwright, cargo test | 90-99% |
| Build | next, tsc, lint, prettier | 70-87% |
| Git | status, log, diff, add, commit | 59-80% |
| GitHub | gh pr, gh run, gh issue | 26-87% |
| Package Managers | pnpm, npm, npx | 70-90% |
| Files | ls, read, grep, find | 60-75% |
| Infrastructure | docker, kubectl | 85% |
| Network | curl, wget | 65-70% |

Overall average: **60-90% token reduction** on common development operations.

## Notes

- The condensed CLAUDE.md block (~200 tokens) is 85% smaller than the full version from `rtk init` (~1,400 tokens)
- The HTML comments (`<!-- rtk-instructions -->`) serve as markers to easily locate and update the block across repos
- `rtk init --show` reports the current configuration status for the repo
- RTK version 0.31.0+ includes the rate-limited warning fix for Windows (PR #742)
