---
name: Always merge to main via PR with --admin (single-dev period)
description: When user authorizes "merge to main", path is ALWAYS gh pr create + gh pr merge --admin --merge --delete-branch. The --admin flag is the documented default while no second human reviewer exists.
type: feedback
---

When the user authorizes a feature to land in `main` ("mandalo a main", "merge to main", "ship it", "send to origin/main", "push a main origin", etc.), the path is **always** via a GitHub Pull Request, and the merge command default uses `--admin`:

1. Push the feature branch (already done during normal workflow).
2. `gh pr create --title "..." --body "...Closes #<issue>..."` — title and body reference the issue number from the branch name.
3. `gh pr merge --admin --merge --delete-branch` — `--admin` is default. `--merge` preserves commit chain (override with `--squash` or `--rebase` if user asks).
4. Post-merge cleanup per Role.md Rule 7 (delete local feature branch; remote was already dropped by `--delete-branch`).

**Never** do `git checkout main && git merge feature/... && git push origin main`. The PR is the audit trail; direct-push to `main` removes the audit surface entirely. Admin-merging *through a PR* does not — the PR commits, diff, conversation, and `Closes #<n>` linkage all stay intact, and the "Bypassed rule violations" log entry is the expected signal that this PR was self-merged because no other reviewer was available.

**Why this rule exists**:
- 2026-04-27 incident: direct-pushed feature #86 to `main`, bypassing the "Changes must be made through a pull request" Repository Ruleset. User correction: "deberías siempre ir por PR, y el branch ir por el número del branch."
- 2026-04-28 follow-up: while landing the `.gitattributes` LF fix (#89 / PR #90), the `gh pr merge` was blocked because of `required_approving_review_count: 1` on the legacy Branch Protection. User decided to **align both protection layers to require 1 review** (Repository Ruleset patched to 1 to match) so the configuration is ready for when other contributors join, AND **document `--admin` as the default merge flag for the single-dev period**. Rationale: the bypass log entry is intentional metadata ("self-merged, no peer reviewer available"), not a violation. The day a second reviewer is around, drop `--admin` and let the standard review-gated flow run.

**How to apply**:
- Step 8 of the Implementation Workflow (after user gives the green light) → `gh pr create` then `gh pr merge --admin --merge --delete-branch`.
- PR title must reference the issue number, e.g. `chore(gitattributes): enforce LF for .toml/.json/.rs (#89)`.
- Body must include `Closes #<n>` so the issue auto-closes on merge.

**Transition signal**: When a second human reviewer (or peer agent under a real human identity) is consistently available and approves PRs, **drop `--admin`** and update this memory + Role.md Rule 11 to reflect that. The `--admin` default is a function of single-dev state, not of preference.

**Edge cases**:
- `validate-branch-name` check failure → do NOT bypass with `--admin`. Fix the branch name root cause per Role.md Step 1. `--admin` does not skip required status checks anyway.
- User explicitly asks for a literal `git push origin main` as a one-off → only then skip the PR. Default is always PR + admin-merge.
