use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use uuid::Uuid;

// ── Types (kept for caller API) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub enum InjectReason {
    InitPrompt,
    TokenRefresh,
    MessageDelivery,
    TelegramInput,
    EnterKeystroke,
}

impl InjectReason {
    fn label(&self) -> &'static str {
        match self {
            Self::InitPrompt => "init_prompt",
            Self::TokenRefresh => "token_refresh",
            Self::MessageDelivery => "message_delivery",
            Self::TelegramInput => "telegram_input",
            Self::EnterKeystroke => "enter",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarkerKind {
    Busy,
    Idle,
}

/// Speaker context passed to the filter so it can apply speaker-specific rules.
#[derive(Debug, Clone, PartialEq)]
pub enum Speaker {
    User,
    Agent,
    Inject,
    Marker,
}

// ── TranscriptWriter ────────────────────────────────────────────────

struct SessionTranscript {
    raw_writer: BufWriter<File>,
    filtered_writer: BufWriter<File>,
}

#[derive(Clone)]
pub struct TranscriptWriter {
    inner: Arc<Mutex<HashMap<Uuid, SessionTranscript>>>,
}

impl TranscriptWriter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a session. Opens both raw and filtered log files at
    /// `{cwd}/.agentscommander/transcripts/YYYYMMDD_HHMMSS.log` and
    /// `{cwd}/.agentscommander/transcripts/YYYYMMDD_HHMMSS_filtered.log`.
    pub fn register_session(&self, session_id: Uuid, cwd: &str) {
        let dir = PathBuf::from(cwd)
            .join(".agentscommander")
            .join("transcripts");
        if let Err(e) = fs::create_dir_all(&dir) {
            log::warn!("[transcript] Failed to create transcripts dir for {}: {}", session_id, e);
            return;
        }
        let now = Utc::now();
        let filename = now.format("%Y%m%d_%H%M%S").to_string();
        let raw_path = dir.join(format!("{}.log", filename));
        let filtered_path = dir.join(format!("{}_filtered.log", filename));

        let raw_file = match OpenOptions::new().create(true).append(true).open(&raw_path) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("[transcript] Failed to open raw log for {}: {}", session_id, e);
                return;
            }
        };
        let filtered_file = match OpenOptions::new().create(true).append(true).open(&filtered_path) {
            Ok(f) => f,
            Err(e) => {
                log::warn!("[transcript] Failed to open filtered log for {}: {}", session_id, e);
                return;
            }
        };

        let header_ts = now.format("%Y-%m-%d %H:%M:%S UTC").to_string();
        let header = format!(
            "# Transcript — {}\n# session: {}\n# cwd: {}\n#",
            header_ts, session_id, cwd
        );

        let mut raw_writer = BufWriter::with_capacity(8192, raw_file);
        let mut filtered_writer = BufWriter::with_capacity(8192, filtered_file);
        let _ = writeln!(raw_writer, "{}", header);
        let _ = writeln!(filtered_writer, "{}", header);

        self.inner.lock().unwrap().insert(session_id, SessionTranscript {
            raw_writer,
            filtered_writer,
        });
        log::info!("[transcript] Recording session {} to {}", &session_id.to_string()[..8], raw_path.display());
    }

    /// Write a line to the raw log and, if the filter passes, to the filtered log.
    fn write_line(&self, session_id: Uuid, speaker: Speaker, line: &str, text: &str) {
        let mut map = self.inner.lock().unwrap();
        if let Some(session) = map.get_mut(&session_id) {
            // Always write raw
            let _ = writeln!(session.raw_writer, "{}", line);

            // Write filtered only if claude_pty_filter passes
            if let Some(filtered_text) = claude_pty_filter(speaker, text) {
                let filtered_line = line.replace(text, &filtered_text);
                let _ = writeln!(session.filtered_writer, "{}", filtered_line);
            }
        }
    }

    /// Write a line to both logs unconditionally (for markers, headers).
    fn write_line_both(&self, session_id: Uuid, line: &str) {
        let mut map = self.inner.lock().unwrap();
        if let Some(session) = map.get_mut(&session_id) {
            let _ = writeln!(session.raw_writer, "{}", line);
            let _ = writeln!(session.filtered_writer, "{}", line);
        }
    }

    fn ts() -> String {
        Utc::now().format("%H:%M:%S").to_string()
    }

    pub fn flush_session(&self, session_id: Uuid) {
        let mut map = self.inner.lock().unwrap();
        if let Some(session) = map.get_mut(&session_id) {
            let _ = session.raw_writer.flush();
            let _ = session.filtered_writer.flush();
        }
    }

    pub fn close_session(&self, session_id: Uuid) {
        let mut map = self.inner.lock().unwrap();
        if let Some(mut session) = map.remove(&session_id) {
            let _ = session.raw_writer.flush();
            let _ = session.filtered_writer.flush();
        }
    }

    // ── Public recording API ────────────────────────────────────────

    pub fn record_keyboard(&self, session_id: Uuid, data: &[u8]) {
        let text = String::from_utf8_lossy(data);
        let line = format!("[{}] USER: {}", Self::ts(), text);
        self.write_line(session_id, Speaker::User, &line, &text);
    }

    pub fn record_inject(
        &self,
        session_id: Uuid,
        data: &[u8],
        reason: InjectReason,
        sender: Option<String>,
        _submit: bool,
    ) {
        let text = String::from_utf8_lossy(data);
        let tag = match &sender {
            Some(s) => format!("INJECT({}, from=\"{}\")", reason.label(), s),
            None => format!("INJECT({})", reason.label()),
        };
        let line = format!("[{}] {}: {}", Self::ts(), tag, text);
        self.write_line(session_id, Speaker::Inject, &line, &text);
    }

    pub fn record_output(&self, session_id: Uuid, data: &[u8]) {
        let text = String::from_utf8_lossy(data);
        let line = format!("[{}] AGENT: {}", Self::ts(), text);
        self.write_line(session_id, Speaker::Agent, &line, &text);
    }

    pub fn record_marker(&self, session_id: Uuid, kind: MarkerKind) {
        let label = match kind {
            MarkerKind::Busy => "busy",
            MarkerKind::Idle => "idle",
        };
        self.write_line_both(session_id, &format!("[{}] -- {} --", Self::ts(), label));
        self.flush_session(session_id);
    }
}

// ── Filter ──────────────────────────────────────────────────────────

/// Filters PTY text for the _filtered.log file.
/// Returns Some(cleaned_text) to write, or None to skip the line entirely.
///
/// This function is the single place where all filtering logic accumulates.
/// Add rules here as needed.
fn claude_pty_filter(speaker: Speaker, raw_text: &str) -> Option<String> {
    // Step 1: Strip ANSI escape codes
    let bytes = strip_ansi_escapes::strip(raw_text);
    let text = String::from_utf8_lossy(&bytes);

    // Step 2: Trim whitespace
    let trimmed = text.trim();

    // Step 3: Skip empty lines
    if trimmed.is_empty() {
        return None;
    }

    // Step 4: Speaker-specific filters
    match speaker {
        Speaker::Agent => {
            // Skip spinner/animation lines (e.g. "Percolating…", "Thinking…", "⠋ Loading")
            // These are short, repetitive TUI elements
            // TODO: expand pattern list as we discover more TUI noise
            None.or_else(|| {
                let clean = trimmed.to_string();
                Some(clean)
            })
        }
        _ => Some(trimmed.to_string()),
    }
}
