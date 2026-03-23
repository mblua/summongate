use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use base64::Engine;
use serde::Serialize;

static AUDIT_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

#[derive(Serialize)]
struct AuditEntry<'a> {
    ts: String,
    event: &'a str,
    session_id: Option<&'a str>,
    channel: &'a str,
    direction: &'a str,
    byte_len: usize,
    text: String,
    bytes_b64: Option<String>,
}

fn file_handle() -> &'static Mutex<Option<File>> {
    AUDIT_FILE.get_or_init(|| {
        let file = crate::config::config_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join("audit-io.jsonl");
            OpenOptions::new().create(true).append(true).open(path).ok()
        });
        Mutex::new(file)
    })
}

pub fn init() {
    let _ = file_handle();
}

fn write_entry(entry: &AuditEntry<'_>) {
    let Ok(mut guard) = file_handle().lock() else {
        return;
    };
    let Some(file) = guard.as_mut() else {
        return;
    };
    let Ok(line) = serde_json::to_string(entry) else {
        return;
    };
    let _ = writeln!(file, "{}", line);
    let _ = file.flush();
}

pub fn log_text(
    event: &'static str,
    session_id: Option<&str>,
    channel: &'static str,
    direction: &'static str,
    text: &str,
) {
    write_entry(&AuditEntry {
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        event,
        session_id,
        channel,
        direction,
        byte_len: text.len(),
        text: text.to_string(),
        bytes_b64: None,
    });
}

pub fn log_bytes(
    event: &'static str,
    session_id: Option<&str>,
    channel: &'static str,
    direction: &'static str,
    data: &[u8],
) {
    write_entry(&AuditEntry {
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        event,
        session_id,
        channel,
        direction,
        byte_len: data.len(),
        text: String::from_utf8_lossy(data).into_owned(),
        bytes_b64: Some(base64::engine::general_purpose::STANDARD.encode(data)),
    });
}
