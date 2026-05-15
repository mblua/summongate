# Shipper Memory Index

- [Kill blocking standalone](feedback_kill_blocking_standalone.md) — on deploy lock of YOUR `_wg-<N>.exe`, kill it yourself; never touch other workgroups' exes.
- [Local bump then restore](feedback_shipper_local_bump_then_restore.md) — when the feature branch has no committed `tauri.conf.json` bump, bump locally for the build then `git restore` the file after deploy succeeds.
