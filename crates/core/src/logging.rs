//! Unified logging infrastructure for Localref.
//!
//! This module replaces the previous ad-hoc `EventLog` and `RuntimeLogger` with a
//! single `tracing`-based system. It writes structured JSON Lines to
//! `.localref/logs/localref.jsonl` via a non-blocking background thread and keeps
//! an in-memory ring buffer so the REST API and web UI can surface recent entries
//! without reading the file.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

/// Global ring buffer initialized by [`init`].
static GLOBAL_BUFFER: OnceLock<LogRingBuffer> = OnceLock::new();

/// Append-only writer that rotates the active log before it exceeds its size
/// limit. Backups are named `localref.jsonl.1`, `localref.jsonl.2`, etc.
#[derive(Debug)]
struct SizeLimitedLogWriter {
    /// Active JSONL path.
    path: PathBuf,
    /// Buffered active file; temporarily absent while rotating on Windows.
    file: Option<BufWriter<File>>,
    /// Bytes currently stored in the active file.
    current_len: u64,
    /// Maximum bytes allowed before starting a new active file.
    max_bytes: u64,
    /// Number of numbered backup files to retain.
    backup_count: usize,
}

impl SizeLimitedLogWriter {
    /// Open an append-only writer, rotating an already-full file first.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the directory, file, or rotation cannot be
    /// created or updated.
    #[allow(clippy::single_call_fn)]
    fn new(
        path: PathBuf,
        max_bytes: u64,
        backup_count: usize,
    ) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if path.metadata().is_ok_and(|metadata| metadata.len() >= max_bytes) {
            rotate_files(&path, backup_count)?;
        }
        let file = File::options().create(true).append(true).open(&path)?;
        let current_len = file.metadata()?.len();
        Ok(Self {
            path,
            file: Some(BufWriter::new(file)),
            current_len,
            max_bytes,
            backup_count,
        })
    }

    /// Flush and close the active file, rotate backups, and open a fresh file.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when flushing, renaming, or reopening fails.
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        rotate_files(&self.path, self.backup_count)?;
        let file =
            File::options().create(true).append(true).open(&self.path)?;
        self.file = Some(BufWriter::new(file));
        self.current_len = 0;
        Ok(())
    }
}

impl Write for SizeLimitedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let incoming = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self.current_len > 0
            && self.current_len.saturating_add(incoming) > self.max_bytes
        {
            self.rotate()?;
        }
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is not open"))?
            .write_all(bytes)?;
        self.current_len = self.current_len.saturating_add(incoming);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file is not open"))?
            .flush()
    }
}

/// Shift numbered backups and move the active path to backup number one.
///
/// # Errors
///
/// Returns an I/O error when a retained file cannot be removed or renamed.
fn rotate_files(path: &Path, backup_count: usize) -> io::Result<()> {
    if backup_count == 0 {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }
    let oldest = rotated_path(path, backup_count);
    if oldest.exists() {
        fs::remove_file(oldest)?;
    }
    for index in (1..backup_count).rev() {
        let source = rotated_path(path, index);
        if source.exists() {
            fs::rename(source, rotated_path(path, index + 1))?;
        }
    }
    if path.exists() {
        fs::rename(path, rotated_path(path, 1))?;
    }
    Ok(())
}

/// Return the numbered backup path for `path` and `index`.
fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}

/// Return a reference to the global ring buffer, if initialized.
pub fn global_buffer() -> Option<&'static LogRingBuffer> {
    GLOBAL_BUFFER.get()
}

/// One log entry returned by the ring buffer and REST API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LogEntry {
    /// Monotonic entry id.
    pub id: u64,
    /// RFC 3339 timestamp with millisecond precision.
    pub ts: String,
    /// Log level: TRACE, DEBUG, INFO, WARN, or ERROR.
    pub level: String,
    /// Module path that emitted the record.
    pub target: String,
    /// Human-readable log message.
    pub message: String,
    /// Optional stable event kind identifier (e.g. "`import_started`").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_kind: Option<String>,
    /// Optional related item identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Optional library-relative path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Thread-safe fixed-capacity ring buffer of recent [`LogEntry`] values.
///
/// The buffer is shared between the tracing layer (which pushes) and the REST
/// API (which reads). When full, the oldest entry is evicted.
#[derive(Clone, Debug)]
pub struct LogRingBuffer {
    /// Stored inner.
    inner: Arc<Mutex<LogRingBufferInner>>,
}

#[derive(Debug)]
/// Internal representation for log ring buffer inner.
struct LogRingBufferInner {
    /// Stored entries.
    entries: Vec<LogEntry>,
    /// Stored capacity.
    capacity: usize,
    /// Stored next id.
    next_id: u64,
}

impl LogRingBuffer {
    /// Create a ring buffer that holds at most `capacity` entries.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(LogRingBufferInner {
                entries: Vec::with_capacity(capacity),
                capacity,
                next_id: 1,
            })),
        }
    }

    /// Append one entry, evicting the oldest if the buffer is full.
    /// # Panics
    ///
    /// Panics if an internal invariant is violated.
    pub fn push(&self, entry: LogEntry) {
        let mut inner = self.inner.lock().expect("log ring buffer poisoned");
        if inner.entries.len() >= inner.capacity {
            inner.entries.remove(0);
        }
        inner.entries.push(entry);
    }

    /// Return a snapshot of all buffered entries (oldest first).
    #[must_use]
    /// # Panics
    ///
    /// Panics if an internal invariant is violated.
    pub fn entries(&self) -> Vec<LogEntry> {
        let inner = self.inner.lock().expect("log ring buffer poisoned");
        inner.entries.clone()
    }

    /// Allocate the next monotonic entry id.
    #[must_use]
    /// # Panics
    ///
    /// Panics if an internal invariant is violated.
    pub fn next_id(&self) -> u64 {
        let mut inner = self.inner.lock().expect("log ring buffer poisoned");
        let id = inner.next_id;
        inner.next_id += 1;
        id
    }
}

/// Handle returned by [`init`]. Must be kept alive for the process lifetime.
pub struct LogHandle {
    /// Ring buffer shared with REST and UI layers.
    pub buffer: LogRingBuffer,
    /// Guards the non-blocking appender worker thread.
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Visitor that extracts structured fields from a tracing event.
#[derive(Default)]
struct LogFieldVisitor {
    /// Stored message.
    message: String,
    /// Stored event kind.
    event_kind: Option<String>,
    /// Stored item id.
    item_id: Option<String>,
    /// Stored path.
    path: Option<String>,
    /// Runtime target override (used by [`log_dynamic`] so plugin logs can
    /// carry a per-plugin target the const tracing macro cannot express).
    dyn_target: Option<String>,
}

impl Visit for LogFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let display = format!("{value:?}");
        let cleaned = if display.len() >= 2
            && display.starts_with('"')
            && display.ends_with('"')
        {
            display[1..display.len() - 1].to_string()
        } else {
            display
        };
        match field.name() {
            "message" | "" => self.message = cleaned,
            "event_kind" => self.event_kind = non_empty(cleaned),
            "item_id" => self.item_id = non_empty(cleaned),
            "path" => self.path = non_empty(cleaned),
            "dyn_target" => self.dyn_target = non_empty(cleaned),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "event_kind" => self.event_kind = non_empty(value.to_string()),
            "item_id" => self.item_id = non_empty(value.to_string()),
            "path" => self.path = non_empty(value.to_string()),
            "dyn_target" => self.dyn_target = non_empty(value.to_string()),
            _ => {}
        }
    }
}

/// `Some(value)` unless the string is empty, in which case `None`.
///
/// Optional structured fields are emitted as empty strings by callers that
/// pass a fixed field set (e.g. [`log_dynamic`]); collapse those to absent so
/// they are omitted from the serialized entry.
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

/// Custom tracing layer that writes JSONL to a writer and appends to the ring
/// buffer.
struct LogLayer<W: Write + Send + Sync + 'static> {
    /// Stored writer.
    writer: Arc<Mutex<W>>,
    /// Stored buffer.
    buffer: LogRingBuffer,
}

impl<S, W> Layer<S> for LogLayer<W>
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span>,
    W: Write + Send + Sync + 'static,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();

        // Extract structured fields from the event.
        let mut visitor = LogFieldVisitor::default();
        event.record(&mut visitor);

        let id = self.buffer.next_id();
        let duration = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();
        let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX);
        let time_of_day = secs % 86_400;
        let hours = time_of_day / 3_600;
        let minutes = (time_of_day % 3_600) / 60;
        let seconds = time_of_day % 60;
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = u32::try_from(z - era * 146_097).unwrap_or_default();
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let mut year = i64::from(yoe) + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        year += i64::from(month <= 2);
        let ts = format!(
            "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z"
        );
        let level = metadata.level().to_string();
        let target = visitor
            .dyn_target
            .clone()
            .unwrap_or_else(|| metadata.target().to_string());

        let entry = LogEntry {
            id,
            ts,
            level,
            target,
            message: visitor.message,
            event_kind: visitor.event_kind,
            item_id: visitor.item_id,
            path: visitor.path,
        };

        // Write JSON line to the file.
        if let Ok(mut json_line) = serde_json::to_string(&entry) {
            json_line.push('\n');
            let mut writer =
                self.writer.lock().expect("log writer mutex poisoned");
            let _ = writer.write_all(json_line.as_bytes());
            let _ = writer.flush();
        }

        // Push to the in-memory ring buffer for API access.
        self.buffer.push(entry);
    }
}

/// Initialize the unified tracing-based logging system.
///
/// Must be called once at process start, before any `tracing` macros fire.
/// Writes structured JSONL to `<library_root>/.localref/logs/localref.jsonl`
/// via a non-blocking background thread, and keeps a ring buffer of recent
/// entries for REST / UI access.
///
/// In debug builds the stderr layer is always enabled regardless of `quiet`.
/// In release builds, `quiet` suppresses the human-readable stderr output.
///
/// The default filter level is `info`. Set the `LOCALREF_LOG` environment
/// variable to override (e.g. `LOCALREF_LOG=debug` or
/// `LOCALREF_LOG=warn,localref_core=debug`).
/// On-disk file size and backup retention come from the `[logging]` section of
/// the Localref configuration file.
///
/// Returns `None` when a global subscriber is already installed in this
/// process (e.g. a second `start_daemon` call in the same test binary); the
/// existing ring buffer keeps serving `events()`, so this is not an error.
pub fn init(
    library_root: impl Into<PathBuf>,
    quiet: bool,
    max_file_bytes: u64,
    backup_count: u32,
) -> Option<LogHandle> {
    // Idempotent: the tracing subscriber is a process-global singleton and
    // installing it twice panics. If it is already set, reuse it.
    if GLOBAL_BUFFER.get().is_some() {
        return None;
    }

    let library_root: PathBuf = library_root.into();
    let log_dir = library_root.join(".localref").join("logs");

    // Non-blocking, size-limited file appender (dedicated background thread).
    let file_appender = SizeLimitedLogWriter::new(
        log_dir.join("localref.jsonl"),
        max_file_bytes,
        usize::try_from(backup_count).ok()?,
    )
    .ok()?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    // In-memory ring buffer holding the last 2000 entries.
    let buffer = LogRingBuffer::new(2000);

    // Store in the global OnceLock for access from daemon and REST.
    let _ = GLOBAL_BUFFER.set(buffer.clone());

    // Our custom JSONL file + ring buffer layer.
    let jsonl_layer = LogLayer {
        writer: Arc::new(Mutex::new(non_blocking)),
        buffer: buffer.clone(),
    };

    // Default to INFO, overridable via LOCALREF_LOG env var.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Debug builds always get stderr output. Release builds respect `quiet`.
    let stderr_enabled = cfg!(debug_assertions) || !quiet;

    if stderr_enabled {
        let stderr_layer = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_level(true)
            .compact();

        if tracing_subscriber::registry()
            .with(filter)
            .with(jsonl_layer)
            .with(stderr_layer)
            .try_init()
            .is_err()
        {
            return None;
        }
    } else if tracing_subscriber::registry()
        .with(filter)
        .with(jsonl_layer)
        .try_init()
        .is_err()
    {
        return None;
    }

    Some(LogHandle { buffer, _guard: guard })
}

/// Emit a log record with a runtime-chosen target and level.
///
/// `tracing`'s macros require a const `target:` and a fixed level, but plugin
/// logs need a per-plugin target (`localref::plugin::<name>`) and a level
/// chosen at runtime. This shim emits under the const `localref::plugin`
/// target while carrying the real target in the `dyn_target` field, which the
/// [`LogLayer`] uses to override the recorded entry's target. The level is
/// selected via a fixed match; callers are expected to cap it (plugins may not
/// emit above `WARN`).
pub fn log_dynamic(
    target: &str,
    level: tracing::Level,
    message: &str,
    event_kind: Option<&str>,
    item_id: Option<&str>,
    path: Option<&str>,
) {
    let event_kind = event_kind.unwrap_or_default();
    let item_id = item_id.unwrap_or_default();
    let path = path.unwrap_or_default();
    match level {
        tracing::Level::WARN => tracing::warn!(
            target: "localref::plugin",
            dyn_target = target,
            event_kind = event_kind,
            item_id = item_id,
            path = path,
            "{message}",
        ),
        tracing::Level::INFO => tracing::info!(
            target: "localref::plugin",
            dyn_target = target,
            event_kind = event_kind,
            item_id = item_id,
            path = path,
            "{message}",
        ),
        tracing::Level::DEBUG => tracing::debug!(
            target: "localref::plugin",
            dyn_target = target,
            event_kind = event_kind,
            item_id = item_id,
            path = path,
            "{message}",
        ),
        // TRACE and any unexpected level fall through to TRACE.
        _ => tracing::trace!(
            target: "localref::plugin",
            dyn_target = target,
            event_kind = event_kind,
            item_id = item_id,
            path = path,
            "{message}",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_debug_quotes(value: &str) -> String {
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            value[1..value.len() - 1].to_string()
        } else {
            value.to_string()
        }
    }

    fn ts_rfc3339(ts: SystemTime) -> String {
        let duration =
            ts.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();
        let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX);
        let time_of_day = secs % 86_400;
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = u32::try_from(z - era * 146_097).unwrap_or_default();
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let mut year = i64::from(yoe) + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        year += i64::from(month <= 2);
        let hours = time_of_day / 3_600;
        let minutes = (time_of_day % 3_600) / 60;
        let seconds = time_of_day % 60;
        format!(
            "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z"
        )
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        let buffer = LogRingBuffer::new(3);
        for i in 1..=5 {
            buffer.push(LogEntry {
                id: i,
                ts: String::new(),
                level: "INFO".to_string(),
                target: "test".to_string(),
                message: format!("entry {i}"),
                event_kind: None,
                item_id: None,
                path: None,
            });
        }
        let entries = buffer.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, 3);
        assert_eq!(entries[2].id, 5);
    }

    #[test]
    fn size_limited_writer_rotates_and_bounds_retained_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("localref.jsonl");
        let mut writer =
            SizeLimitedLogWriter::new(path.clone(), 10, 2).unwrap();

        writer.write_all(b"first\n").unwrap();
        writer.write_all(b"second\n").unwrap();
        writer.write_all(b"third\n").unwrap();
        writer.flush().unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "third\n");
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 1)).unwrap(),
            "second\n"
        );
        assert_eq!(
            fs::read_to_string(rotated_path(&path, 2)).unwrap(),
            "first\n"
        );
        assert!(!rotated_path(&path, 3).exists());
    }

    #[test]
    fn ring_buffer_next_id_is_monotonic() {
        let buffer = LogRingBuffer::new(10);
        assert_eq!(buffer.next_id(), 1);
        assert_eq!(buffer.next_id(), 2);
        assert_eq!(buffer.next_id(), 3);
    }

    #[test]
    fn log_entry_json_omits_optional_fields() {
        let entry = LogEntry {
            id: 1,
            ts: "2026-06-03T00:00:00.000Z".to_string(),
            level: "INFO".to_string(),
            target: "test".to_string(),
            message: "hello".to_string(),
            event_kind: None,
            item_id: None,
            path: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"message\":\"hello\""));
        assert!(!json.contains("event_kind"));
        assert!(!json.contains("item_id"));
        assert!(!json.contains("path"));
    }

    #[test]
    fn log_entry_json_includes_optional_fields_when_present() {
        let entry = LogEntry {
            id: 2,
            ts: "2026-06-03T00:00:00.000Z".to_string(),
            level: "WARN".to_string(),
            target: "test".to_string(),
            message: "conflict".to_string(),
            event_kind: Some("write_conflict".to_string()),
            item_id: Some("lr:zotero:abc".to_string()),
            path: Some("All/Paper".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("event_kind"));
        assert!(json.contains("write_conflict"));
        assert!(json.contains("item_id"));
        assert!(json.contains("path"));
    }

    #[test]
    fn strip_debug_quotes_handles_strings_and_numbers() {
        assert_eq!(strip_debug_quotes("\"hello\""), "hello");
        assert_eq!(strip_debug_quotes("42"), "42");
        assert_eq!(strip_debug_quotes("\"a\""), "a");
        assert_eq!(strip_debug_quotes(""), "");
    }

    #[test]
    fn ts_rfc3339_format_is_well_formed() {
        let ts = ts_rfc3339(SystemTime::UNIX_EPOCH);
        assert_eq!(ts, "1970-01-01T00:00:00.000Z");
        let next_day = ts_rfc3339(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_hours(24),
        );
        assert_eq!(next_day, "1970-01-02T00:00:00.000Z");
    }

    #[test]
    fn log_dynamic_overrides_target_and_keeps_fields() {
        let buffer = LogRingBuffer::new(10);
        let layer = LogLayer {
            writer: Arc::new(Mutex::new(Vec::<u8>::new())),
            buffer: buffer.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            super::log_dynamic(
                "localref::plugin::bibtexer",
                tracing::Level::INFO,
                "exported 3 items",
                Some("plugin_action"),
                Some("lr:zotero:abc"),
                None,
            );
        });

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.target, "localref::plugin::bibtexer");
        assert_eq!(entry.level, "INFO");
        assert_eq!(entry.message, "exported 3 items");
        assert_eq!(entry.event_kind.as_deref(), Some("plugin_action"));
        assert_eq!(entry.item_id.as_deref(), Some("lr:zotero:abc"));
        // Absent optional field stays absent (not an empty string).
        assert!(entry.path.is_none());
    }

    #[test]
    fn log_dynamic_warn_level_is_recorded() {
        let buffer = LogRingBuffer::new(10);
        let layer = LogLayer {
            writer: Arc::new(Mutex::new(Vec::<u8>::new())),
            buffer: buffer.clone(),
        };
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            super::log_dynamic(
                "localref::plugin::hooklog",
                tracing::Level::WARN,
                "something looked off",
                None,
                None,
                None,
            );
        });

        let entries = buffer.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, "WARN");
        assert_eq!(entries[0].target, "localref::plugin::hooklog");
    }
}
