# Shipper build report — Issue #199 (wg-1)

**Status:** SUCCESS — build, deploy, and verification all green.

## Inputs (matches request)

| Field | Expected | Observed | OK |
|---|---|---|---|
| Repo | `…/wg-1-dev-team/repo-AgentsCommander` | same | yes |
| Branch | `feature/191-cli-project-open-create` | same | yes |
| HEAD | `1144f074813b3a6ceea702b2a519dee361ff1db7` | same | yes |
| Version (`package.json`) | `0.8.17` | `0.8.17` | yes |
| Version (`src-tauri/tauri.conf.json`) | `0.8.17` | `0.8.17` | yes |
| Version (`src-tauri/Cargo.toml`) | `0.8.17` | `0.8.17` | yes |
| Review verdict (`_plans/199-grinch-implementation-review.md`) | `APPROVED` | `APPROVED` (line 14 + line 152) | yes |

Working tree at build time: only `M package-lock.json` (pre-existing dirt acknowledged in the request) and the untracked review file `_plans/199-grinch-implementation-review.md`. Nothing touched by Shipper.

## Build

- Command: `npx tauri build` (run from the wg-1 repo root)
- Background task id: `be6g2w93v` — exit code 0
- Frontend (Vite): built in 751 ms, single dynamic-import warning + chunk-size warning (existing, non-blocking).
- Backend: `Finished release profile [optimized] target(s) in 51.49s`.
- Rust warnings: 2 dead-code warnings only (`extract_brief_first_line`, `read_brief_capped` in `src/commands/ac_discovery.rs`). Pre-existing, non-blocking.
- Bundles also produced (not deployed by Shipper, build output only):
  - `…/bundle/msi/Agents Commander New_0.8.17_x64_en-US.msi`
  - `…/bundle/nsis/Agents Commander New_0.8.17_x64-setup.exe`
- Build log: `repo-AgentsCommander/build-199.log`

## Binary validation

| Item | Bytes | Notes |
|---|---|---|
| Reference `agentscommander_mb.exe` | 23,178,752 | live production reference |
| Built `agentscommander-new.exe` | 23,220,224 | +41,472 B vs reference |

New binary ≥ reference → frontend successfully embedded.

`VersionInfo` of the built exe:
- `FileVersion` = `0.8.17`
- `ProductVersion` = `0.8.17`
- `ProductName` = `Agents Commander New`

## Deploy

- Source: `…/repo-AgentsCommander/src-tauri/target/release/agentscommander-new.exe`
- Destination: `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone_wg-1.exe`
- Pre-deploy process check for `agentscommander_standalone_wg-1*`: `NO_WG1_PROCESS` (nothing to kill).
- Copy completed: 23,220,224 bytes at the destination.
- Bare `agentscommander_standalone.exe`: NOT touched.
- Other workgroup exe files (`agentscommander_standalone_wg-2.exe`, etc.): NOT touched.

## Post-deploy verification

- `agentscommander_standalone_wg-1.exe --help` → exit 0, full CLI help printed, including the new `open-project` and `new-project` subcommands from #191.
- `--version` / `-V` are not implemented at the CLI surface (consistent with help output that shows no version flag); runtime version verified instead via Win32 file metadata above (`FileVersion=0.8.17`).

## Constraints honored

- No git commits, branches, pushes, merges.
- No source-code modifications.
- No changes to `package-lock.json` (the pre-existing worktree dirt was left as-is).
- Deploy target was only the wg-1-specific path.
- `agentscommander_mb.exe` (live production) was not touched and not killed.

## Result

The wg-1 standalone binary at `C:\Users\maria\0_mmb\0_AC\agentscommander_standalone_wg-1.exe` is now built from commit `1144f074` of `feature/191-cli-project-open-create` at version `0.8.17`, and is ready to be exercised by tech-lead / QA against issue #199.
