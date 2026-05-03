//! Process-wide logger initialization shared by the GUI and CLI entry points.
//!
//! Both `lib::run()` (GUI) and `main()` (CLI branch, before `cli::handle_cli`)
//! call [`init_logger`] so every `log::*` invocation reaches a single sink:
//! stderr **and** `<config_dir>/app.log`. Pre-#137-followup the CLI path
//! skipped this and silently dropped every `log::*` call (including the
//! `[brief]` audit lines), undermining plan #137 §3a's HIGH-1 mitigation.
//!
//! Idempotent via a process-wide [`OnceLock`]: calling more than once is a
//! silent no-op. Defensive only — current call sites are mutually exclusive
//! (a single process runs either the GUI path OR the CLI path, never both).
//! Without the guard, a second `env_logger::Builder::init()` would panic via
//! `log::set_logger`'s "called twice" contract.

use std::io::Write;
use std::sync::OnceLock;

static INIT: OnceLock<()> = OnceLock::new();

/// Install the global `log` backend. Safe to call from any entry point and
/// safe to call multiple times.
///
/// Filter precedence (matches `lib::run()` pre-fix):
/// 1. `RUST_LOG` environment variable
/// 2. `settings.json::logLevel`
/// 3. Hardcoded default `"agentscommander=info"`
///
/// Sink: stderr + `<config_dir>/app.log` (append-mode; per-line writes are
/// serialized through a `Mutex` so concurrent log calls within one process
/// do not interleave bytes mid-line).
pub fn init_logger() {
    INIT.get_or_init(init_logger_inner);
}

fn init_logger_inner() {
    let log_file: Option<std::sync::Mutex<std::fs::File>> =
        crate::config::config_dir().and_then(|dir| {
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("app.log");
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok()
                .map(|f| {
                    eprintln!("[log] file logging to {}", path.display());
                    std::sync::Mutex::new(f)
                })
        });
    let log_file = std::sync::Arc::new(log_file);

    // #93 precedence: RUST_LOG env > settings.logLevel > "agentscommander=info" default.
    // - read_log_level_only is read-only and side-effect-free: does NOT trigger
    //   migrations, auto-token-gen, or save_settings, so all log calls inside the
    //   full load_settings() flow re-fire on the post-init SettingsState construction
    //   call and are captured.
    // - from_env(Env::default()) preserves RUST_LOG_STYLE handling (color output).
    // - No floor is applied: if `resolved_filter` is malformed (e.g. user typo in
    //   settings.json::logLevel), parse_filters produces no matching directives for
    //   agentscommander* targets, and all logs from those targets are suppressed.
    //   The user-facing recovery is to fix the typo.
    let resolved_filter = std::env::var("RUST_LOG")
        .ok()
        .or_else(crate::config::settings::read_log_level_only)
        .unwrap_or_else(|| "agentscommander=info".to_string());

    env_logger::Builder::from_env(env_logger::Env::default())
        .parse_filters(&resolved_filter)
        .format({
            let log_file = std::sync::Arc::clone(&log_file);
            move |buf, record| {
                let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
                let line = format!(
                    "{} [{}] {} — {}\n",
                    ts,
                    record.level(),
                    record.target(),
                    record.args()
                );
                buf.write_all(line.as_bytes())?;
                if let Some(ref file_mtx) = *log_file {
                    if let Ok(mut f) = file_mtx.lock() {
                        let _ = f.write_all(line.as_bytes());
                    }
                }
                Ok(())
            }
        })
        .init();
}
