---
name: Kill blocking agentscommander_standalone.exe on deploy
description: When the primary or wg-N standalone exe is locked during a deploy copy, kill it yourself without asking tech-lead or the user.
type: feedback
---

When a deploy copy fails with `Device or resource busy` because `agentscommander_standalone.exe` (or the `_wg-N.exe` variant) is running, kill that process yourself by PID and proceed with the copy. Do NOT message tech-lead or the user for permission.

**Why:** Tech-lead said on 2026-04-23 (wg-5, 0.7.8 main post-merge build): "if this happens again, kill the process yourself (don't ask me, don't ask the user). Just do it and deploy. The user wants shipping, not process-babysitting." This supersedes the earlier "notify with PID, don't kill" instruction that was in the older build messages and in Role.md.

**How to apply:**
- Scope: only the `agentscommander_standalone*` process family. The Role.md hard rule still stands for everything else — NEVER kill `agentscommander_mb.exe` (live production instance), NEVER kill anything under `Program Files`.
- Mechanics: `Stop-Process -Id <PID> -Force` via PowerShell, then retry `cp`. Verify with `--help` and version check after.
- In the deploy report, mention the kill as a one-liner ("killed PID N to unblock primary copy") — still be transparent, just don't gate on approval.
- If killing fails (access denied, unusual error) or the process isn't in the expected `0_AC\` location, stop and escalate to tech-lead before taking further action.
