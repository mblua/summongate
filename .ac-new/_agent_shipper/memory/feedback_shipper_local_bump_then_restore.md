---
name: Shipper local-only version bump (when feature branch has no committed bump)
description: When tech-lead asks shipper to build a feature branch that has no committed tauri.conf.json version bump, do a local-only bump for the build, then `git restore` the file after deploy
type: feedback
---
When the feature branch the shipper is asked to build does **not** include a committed version bump (e.g. branch `feature/brief-panel-title-body` with commit `be1db18 chore(brief): revert out-of-scope version bump to 0.8.9`), the shipper should:

1. Edit `src-tauri/tauri.conf.json` locally to bump the `version` field by patch (e.g. 0.8.9 → 0.8.10) **before** running `npx tauri build`. The bumped value is what gets baked into the exe and shown to the user.
2. Run the build and deploy as usual to `agentscommander_standalone_wg-<N>.exe`.
3. **After** the deploy succeeds and `--help` verification passes, run `git restore src-tauri/tauri.conf.json` from inside the workgroup repo to revert the working-tree change. The deployed exe stays on the bumped version; only the source is restored.
4. Final `git status --short --branch` should match what the tech-lead expects (typically: branch name + only the pre-existing untracked plan file).

**Why:** Two competing requirements meet on tightly-scoped feature branches:
- The user's standing rule that every feature build must bump `tauri.conf.json` so they can visually distinguish it from a stale instance (see `feedback_bump_version_on_builds.md`).
- The tech-lead's scope hygiene — sometimes the version bump is explicitly excluded from the feature commit (or actively reverted, as happened on `feature/brief-panel-title-body`).

The local-bump-then-restore pattern satisfies both: the deployed exe is visibly new (0.8.10 in titlebar) while the source tree the tech-lead inspects before notifying the user remains exactly the committed branch state. Tech-lead asked for this cleanup explicitly on 2026-05-07 after the brief-panel build (delivery `2f610e13`), so the deployed exe at 0.8.10 was kept while source was reverted to 0.8.9.

**How to apply:**
- Detect the trigger: pre-flight `git log -3` shows a recent "revert ... version bump" commit, OR the tech-lead's brief explicitly limits the diff scope to non-version files. In either case, default to local-bump-then-restore rather than committing the bump.
- On a normal main-branch build or when the feature branch already has a committed bump in its diff, do **not** apply this pattern — there's nothing to restore.
- In the result message to tech-lead, be explicit that the bump was working-tree only and not committed, so they don't have to ask.
- If tech-lead asks for the cleanup as a follow-up message (rather than you doing it proactively), do it immediately and reply with the exact `git status --short --branch` they're expecting.
