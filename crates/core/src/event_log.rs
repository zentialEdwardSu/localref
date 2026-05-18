//! JSONL event log support for the daemon.
//!
//! The event log is a filesystem fact independent of the rebuildable query
//! database. Every daemon write path can append records here before or after it
//! mutates `All/`, `Cat/`, metadata, or the query cache.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use error::{LocalrefError, Result};
use model::{Event, EventKind};

/// Append-only event log stored under `.localref/logs/events.jsonl`.
#[derive(Clone, Debug)]
pub struct EventLog {
    path: PathBuf,
    mutex: Arc<Mutex<()>>,
}

impl EventLog {
    /// Create an event log helper for one library root.
    pub fn new(library_root: impl Into<PathBuf>) -> Self {
        Self {
            path: library_root
                .into()
                .join(".localref")
                .join("logs")
                .join("events.jsonl"),
            mutex: Arc::new(Mutex::new(())),
        }
    }

    /// Append one event and flush the event log file.
    pub fn append(
        &self,
        kind: EventKind,
        message: impl Into<String>,
        item_id: Option<String>,
        path: Option<String>,
    ) -> Result<Event> {
        let _guard = self.mutex.lock().expect("event log mutex poisoned");
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| LocalrefError::io(parent, source))?;
        }

        let event = Event::new(
            next_event_id(&self.path)?,
            kind,
            message,
            item_id,
            path,
        );
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| LocalrefError::io(&self.path, source))?;
        let line = serde_json::to_string(&event)?;
        file.write_all(line.as_bytes())
            .map_err(|source| LocalrefError::io(&self.path, source))?;
        file.write_all(b"\n")
            .map_err(|source| LocalrefError::io(&self.path, source))?;
        file.sync_all()
            .map_err(|source| LocalrefError::io(&self.path, source))?;
        Ok(event)
    }

    /// Read all event records from the log.
    pub fn list(&self) -> Result<Vec<Event>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path)
            .map_err(|source| LocalrefError::io(&self.path, source))?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line =
                line.map_err(|source| LocalrefError::io(&self.path, source))?;
            if !line.trim().is_empty() {
                events.push(serde_json::from_str(&line)?);
            }
        }
        Ok(events)
    }
}

fn next_event_id(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(1);
    }
    Ok(EventLog { path: path.to_path_buf(), mutex: Arc::new(Mutex::new(())) }
        .list()?
        .last()
        .map_or(1, |event| event.id + 1))
}
