---
name: Kill blocking workgroup-specific standalone exe on deploy
description: When YOUR workgroup's `agentscommander_standalone_wg-<N>.exe` is locked during a deploy copy, kill it yourself without asking. Never touch other workgroups' exes.
type: feedback
---

When a deploy copy fails with `Device or resource busy` because YOUR workgroup's `agentscommander_standalone_wg-<N>.exe` is running, kill that process yourself by PID and proceed with the copy. Do NOT message tech-lead or the user for permission. Do NOT touch another workgroup's running exe — only your own.

**Why:** Tech-lead said on 2026-04-23 (wg-5, 0.7.8 main post-merge build): "if this happens again, kill the process yourself (don't ask me, don't ask the user). Just do it and deploy. The user wants shipping, not process-babysitting." This supersedes the earlier "notify with PID, don't kill" instruction that was in the older build messages and in Role.md.

**How to apply:**
- Scope: ONLY the current workgroup's `agentscommander_standalone_wg-<N>.exe`. Do NOT kill another workgroup's running exe (they are testing their own builds in parallel). The Role.md hard rule still stands for everything else — NEVER kill `agentscommander_mb.exe` (live production instance), NEVER kill anything under `Program Files`.
- Note: the bare `agentscommander_standalone.exe` is now an orphan — never deployed to, never killed.
- Mechanics: `Stop-Process -Id <PID> -Force` via PowerShell, then retry `cp`. Verify with `--help` and version check after.
- In the deploy report, mention the kill as a one-liner ("killed PID N to unblock primary copy") — still be transparent, just don't gate on approval.
- If killing fails (access denied, unusual error) or the process isn't in the expected `0_AC\` location, stop and escalate to tech-lead before taking further action.
