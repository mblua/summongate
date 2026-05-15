# FIXES_CODEX.md

Historical note: this document described an April 2026 ACRC/PTY paste investigation that is obsolete after issue #212.

Current contract:

- Agent credentials are delivered only through `AGENTSCOMMANDER_*` environment variables set on the PTY child before spawn.
- Live token refresh for a running child process is unsupported; restart or respawn the session.
- PTY injection remains for normal message delivery and non-credential prompts only.

Do not use this file as implementation guidance for credential transport.
