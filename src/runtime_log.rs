//! Structured runtime logging for Localref executables.
//!
//! Domain events remain in core's event log. This module records process-level
//! facts such as startup, listener addresses, tray commands, and connector
//! traffic under `.localref/logs/runtime.jsonl`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Append-only JSONL runtime logger rooted in one Localref library.
#[derive(Clone, Debug)]
pub struct RuntimeLogger {
    path: PathBuf,
    mutex: Arc<Mutex<()>>,
}

#[derive(Serialize)]
struct RuntimeLogRecord<'a> {
    ts_unix_ms: u128,
    level: &'a str,
    target: &'a str,
    message: &'a str,
}

impl RuntimeLogger {
    /// Create a runtime logger for one configured library root.
    pub fn new(library_root: impl Into<PathBuf>) -> Self {
        Self {
            path: library_root
                .into()
                .join(".localref")
                .join("logs")
                .join("runtime.jsonl"),
            mutex: Arc::new(Mutex::new(())),
        }
    }

    /// Return the log file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append an informational runtime record.
    pub fn info(&self, target: &'static str, message: impl AsRef<str>) {
        self.write("info", target, message.as_ref());
    }

    /// Append a warning runtime record.
    pub fn warn(&self, target: &'static str, message: impl AsRef<str>) {
        self.write("warn", target, message.as_ref());
    }

    /// Append an error runtime record.
    pub fn error(&self, target: &'static str, message: impl AsRef<str>) {
        self.write("error", target, message.as_ref());
    }

    fn write(&self, level: &'static str, target: &'static str, message: &str) {
        if let Err(error) = self.try_write(level, target, message) {
            eprintln!("localref runtime log write failed: {error}");
        }
    }

    fn try_write(
        &self,
        level: &'static str,
        target: &'static str,
        message: &str,
    ) -> Result<(), String> {
        let _guard = self.mutex.lock().expect("runtime log mutex poisoned");
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let record = RuntimeLogRecord {
            ts_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_millis(),
            level,
            target,
            message,
        };
        let line =
            serde_json::to_string(&record).map_err(|e| e.to_string())?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| format!("{}: {error}", self.path.display()))?;
        file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())
    }
}
