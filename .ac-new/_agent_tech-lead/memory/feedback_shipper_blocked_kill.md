---
name: Shipper deploy blocked — kill, don't ask
description: When shipper reports a deploy blocked by running exe, authorize shipper to kill the process; don't ask the user to close it.
type: feedback
---

When the shipper reports a deploy blocked because the target exe is in use (e.g. `Device or resource busy`, running PID cited), **instruct shipper to kill the process and redeploy**. Do NOT ask the user to manually close it.

**Why:** User explicitly said "LA PRÓXIMA VEZ... MATALO" (2026-04-23) after a second round-trip of waiting for them to close the standalone during the #70/#77 ship. The user wants shipping, not process-babysitting. The default shipper Role.md says "notify, don't kill" — this project overrides that.

**How to apply:** In the FIRST message to shipper for any deploy task, include the standing authorization: "if blocked by running target exe, kill the process yourself and proceed — don't ask me, don't ask the user." If shipper still surfaces a blocker ping anyway, respond with "kill it" immediately — no additional user round-trip.

**Scope:** AgentsCommander project (wg-5-dev-team). May generalize to other projects, but check the user's expectations first before assuming.
