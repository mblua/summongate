---
name: Implementation Workflow adapts to task scope (lite vs full path)
description: Don't dispatch architect+consensus for trivial UI tweaks or pattern-mirroring changes. Use Step 0 triage in Role.md to pick lite path (dev→grinch→shipper) or full path (architect→dev→grinch→consensus→...).
type: feedback
---

The Implementation Workflow has TWO paths. Pick at Step 0 BEFORE clearing agents.

**Lite path** (skip architect + consensus rounds): the change follows an existing precedent in the codebase, single-file/single-component, no new abstractions/dependencies/schema changes, the "how" is mechanical once the "what" is decided. Lite sequence: Step 1 → Step 6 (dev) → 6b → 7 (grinch) → 8 (shipper) → 9 → 10.

**Full path** (architect + consensus rounds): architectural decision, cross-cutting/multi-component, schema/API/protocol/persistence change, non-obvious approach (3 reasonable implementations exist), or user explicitly asks for architect.

**Why:** User pushed back on dispatching architect for a trivial UI addition (right-click context menu on the sidebar "Workgroups" header — a near-verbatim copy of the existing "Agents" header context menu pattern in `ProjectPanel.tsx`). The full pipeline burns tokens and turnaround time when there's no architectural decision to make and the implementation pattern already lives in the codebase. User said: "no hace falta llamar al Arquitecto para TODO. El workflow tiene que adaptarse a lo que parece ser la tarea." (2026-04-30)

**How to apply:** At Step 0, run a sanity check against the lite-path criteria. If ALL four hold (precedent exists, single-file/component, no new abstractions, mechanical implementation), pick lite. Otherwise full. When uncertain, escalate to full — cost of an unneeded architect dispatch is small; cost of skipping architect on something architectural is rework. Always tell the user which path you picked and why (one line) at task start so they can override. Codified in Role.md §Implementation Workflow Step 0 (2026-04-30).
