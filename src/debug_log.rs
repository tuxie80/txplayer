//! Lightweight thread-safe in-memory Admin / Debug Console ring buffer.
//!
//! Captures timestamped startup, CLI/IPC, audio engine, DSD/DAC negotiation,
//! and error diagnostics so users can inspect or copy a full report via `F12`.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_ENTRIES: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Audio,
    Ipc,
    Warn,
    Error,
}

impl LogLevel {
    pub fn tag(self) -> &'static str {
        match self {
            LogLevel::Info => "INFO",
            LogLevel::Audio => "AUDIO",
            LogLevel::Ipc => "IPC",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: LogLevel,
    pub message: String,
}

fn store() -> &'static Mutex<VecDeque<LogEntry>> {
    static LOGS: OnceLock<Mutex<VecDeque<LogEntry>>> = OnceLock::new();
    LOGS.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_LOG_ENTRIES)))
}

fn format_hms_ms() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let h = (total_secs / 3600) % 24;
    let m = (total_secs / 60) % 60;
    let s = total_secs % 60;
    let ms = dur.subsec_millis();
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

pub fn push(level: LogLevel, message: impl Into<String>) {
    let entry = LogEntry {
        timestamp: format_hms_ms(),
        level,
        message: message.into(),
    };
    if let Ok(mut guard) = store().lock() {
        if guard.len() >= MAX_LOG_ENTRIES {
            guard.pop_front();
        }
        guard.push_back(entry);
    }
}

pub fn info(msg: impl Into<String>) {
    push(LogLevel::Info, msg);
}

pub fn audio(msg: impl Into<String>) {
    push(LogLevel::Audio, msg);
}

pub fn ipc(msg: impl Into<String>) {
    push(LogLevel::Ipc, msg);
}

pub fn warn(msg: impl Into<String>) {
    push(LogLevel::Warn, msg);
}

pub fn error(msg: impl Into<String>) {
    push(LogLevel::Error, msg);
}

pub fn snapshot() -> Vec<LogEntry> {
    store()
        .lock()
        .map(|g| g.iter().cloned().collect())
        .unwrap_or_default()
}

pub fn clear() {
    if let Ok(mut guard) = store().lock() {
        guard.clear();
    }
}
