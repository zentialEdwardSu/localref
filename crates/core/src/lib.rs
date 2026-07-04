//! Core Localref import pipeline and daemon command queue.
//!
//! The core crate orchestrates writes but delegates concrete filesystem
//! operations to `platformfs`. Connector imports, metadata writes, and scans
//! record daemon events and acquire filesystem locks before mutating durable
//! library state.

#![warn(unreachable_pub)]
#![deny(clippy::correctness)]
#![deny(clippy::single_call_fn)]
#![deny(clippy::complexity)]
#![warn(clippy::pedantic)]
#![warn(clippy::useless_attribute)]
#![warn(clippy::redundant_pub_crate)]
#![warn(clippy::excessive_precision)]
#![warn(clippy::missing_docs_in_private_items)]

pub mod error;
pub mod logging;
pub mod model;

// `config` is self-contained (std + serde) and useful to lightweight consumers
// such as plugins that need to resolve the library root without the daemon.
pub mod config;
#[cfg(feature = "server")]
pub mod lock;
#[cfg(feature = "server")]
pub mod manifest;
#[cfg(feature = "server")]
pub mod platformfs;
#[cfg(feature = "server")]
pub mod plugin_state;
#[cfg(feature = "server")]
pub mod rest;
#[cfg(feature = "server")]
pub mod rest_files;
#[cfg(feature = "server")]
pub mod rules;
#[cfg(feature = "server")]
pub mod scan;
#[cfg(feature = "server")]
pub mod schedule;
#[cfg(feature = "server")]
pub mod storage;
// `types` is needed by the ungated `model` module (e.g. `CategoryPath` in
// `Metadata::validate`), so it must build without the `server` feature.
pub mod types;

#[cfg(feature = "server")]
use std::borrow::Borrow;
#[cfg(feature = "server")]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::str::FromStr;
#[cfg(feature = "server")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "server")]
use crate::error::{LocalrefError, Result};
#[cfg(feature = "server")]
use crate::model::{
    Creator, ItemFilesDocument, LogKind, Metadata, MetadataDocument,
    MetadataFile, MetadataFiles, MetadataImport, MetadataState, MetadataTags,
};
#[cfg(feature = "server")]
use crate::manifest::CatManifest;
#[cfg(feature = "server")]
use crate::platformfs::{LibraryFs, sanitize_ntfs_component};
#[cfg(feature = "server")]
use crate::rules::RuleSet;
#[cfg(feature = "server")]
use crate::scan::{AllEntryKind, CatEntryKind, scan_library};
#[cfg(feature = "server")]
use crate::storage::{
    CategorySummary, ItemDocument, SearchHit, StorageDb, path_from_scan_target,
    scan_cat_memberships,
};
#[cfg(feature = "server")]
use crate::types::{
    CategoryPath, ConnectorAttachment, ConnectorImport, ImportOutcome, ItemId,
};
#[cfg(feature = "server")]
use lock::LockManager;
#[cfg(feature = "server")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "server")]
use tokio::sync::broadcast;

/// Category assigned to connector imports that match no classification rule.
#[cfg(feature = "server")]
const UNMATCHED_CATEGORY: &str = "unmatched";

/// Import pipeline rooted at one Localref library.
#[cfg(feature = "server")]
#[derive(Clone, Debug)]
pub struct ImportPipeline {
    /// Stored fs.
    fs: LibraryFs,
    /// Stored locks.
    locks: LockManager,
}

/// Daemon task kinds executed by the core task queue.
#[cfg(feature = "server")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonTask {
    /// Rebuild query storage from `All/`.
    ScanAll,
    /// Import one connector item into `All/`.
    ImportConnector {
        /// Display title of the imported item.
        title: String,
    },
    /// Save a connector attachment into an existing item directory.
    SaveConnectorAttachment {
        /// Attachment filename.
        filename: String,
    },
    /// Patch an existing metadata file.
    PatchMetadata {
        /// Item id being patched.
        item_id: String,
    },
    /// Create metadata for an existing directory under `All/`.
    ImportAllDirectory {
        /// Library-relative or absolute directory path.
        path: String,
    },
    /// Import one explicit file into `All/`.
    ImportFile {
        /// File path to import.
        path: String,
    },
    /// Add one explicit file to an existing item directory.
    AddItemFile {
        /// Item id receiving the file.
        item_id: String,
        /// Source file path.
        path: String,
    },
    /// Delete one indexed item directory.
    DeleteItem {
        /// Item id being deleted.
        item_id: String,
    },
    /// Create an empty category directory.
    CreateCategory {
        /// Category path.
        category: CategoryPath,
    },
    /// Normalize a real directory found under `Cat/`.
    NormalizeCatDirectory {
        /// Library-relative or absolute Cat directory path.
        path: String,
    },
    /// Add one item to a category.
    AddCategory {
        /// Item id.
        item_id: String,
        /// Category path.
        category: CategoryPath,
    },
    /// Remove one item from a category.
    RemoveCategory {
        /// Item id.
        item_id: String,
        /// Category path.
        category: CategoryPath,
    },
    /// Rename a category directory.
    RenameCategory {
        /// Source category.
        from: CategoryPath,
        /// Destination category.
        to: CategoryPath,
    },
    /// Merge a category into another category.
    MergeCategory {
        /// Source category.
        from: CategoryPath,
        /// Destination category.
        to: CategoryPath,
    },
}

/// State of one daemon task.
#[cfg(feature = "server")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonTaskState {
    /// Task has been accepted by the queue.
    Queued,
    /// Task is currently running.
    Running,
    /// Task completed successfully.
    Completed,
    /// Task failed.
    Failed,
}

/// Daemon pause mode.
#[cfg(feature = "server")]
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PauseMode {
    /// Pause all daemon work except status and resume.
    All,
    /// Pause filesystem and database writes.
    Writes,
    /// Pause filesystem watcher processing.
    Watcher,
    /// Pause query database and index updates.
    Indexing,
}

/// Record returned by daemon task APIs.
#[cfg(feature = "server")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DaemonTaskRecord {
    /// Monotonic in-memory task id.
    pub id: u64,
    /// Task kind.
    pub task: DaemonTask,
    /// Current task state.
    pub state: DaemonTaskState,
    /// Human-readable task message.
    pub message: Option<String>,
    /// Number of indexed items for scan tasks.
    pub indexed_items: Option<usize>,
}

/// Current daemon queue status.
#[cfg(feature = "server")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DaemonStatus {
    /// Whether a task is currently running.
    pub running: bool,
    /// Number of queued tasks.
    pub queued_tasks: usize,
    /// Recent task records.
    pub recent_tasks: Vec<DaemonTaskRecord>,
    /// Active pause modes.
    pub paused_modes: Vec<PauseMode>,
}

/// A library mutation that completed successfully, published to hook
/// subscribers so the host can spawn plugins bound to the matching event.
#[cfg(feature = "server")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DaemonEvent {
    /// A new item was imported and indexed.
    ItemImported {
        /// Imported item id.
        item_id: String,
    },
    /// An indexed item directory was deleted.
    ItemDeleted {
        /// Deleted item id.
        item_id: String,
    },
    /// An item's metadata was patched.
    MetadataPatched {
        /// Patched item id.
        item_id: String,
    },
    /// A category was created, renamed, merged, or (un)assigned to items.
    CategoryChanged {
        /// Affected item id(s), when the change targeted specific items.
        item_id: Option<String>,
        /// Affected category path, when known.
        category: Option<String>,
    },
    /// A full library scan finished.
    ScanCompleted {
        /// Number of indexed items after the scan.
        indexed_items: usize,
    },
    /// A file was attached to an existing item.
    ItemFileAdded {
        /// Item that received the file.
        item_id: String,
    },
    /// Automatic-classification rules were replaced.
    RulesChanged,
    /// The runtime schedule set changed (registered or removed).
    SchedulesChanged,
    /// A daemon pause mode was enabled.
    DaemonPaused,
    /// A daemon pause mode was disabled.
    DaemonResumed,
}

#[cfg(feature = "server")]
impl DaemonEvent {
    /// Stable `snake_case` wire name passed to plugins as `hook <event>`.
    #[must_use]
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::ItemImported { .. } => "item_imported",
            Self::ItemDeleted { .. } => "item_deleted",
            Self::MetadataPatched { .. } => "metadata_patched",
            Self::CategoryChanged { .. } => "category_changed",
            Self::ScanCompleted { .. } => "scan_completed",
            Self::ItemFileAdded { .. } => "item_file_added",
            Self::RulesChanged => "rules_changed",
            Self::SchedulesChanged => "schedules_changed",
            Self::DaemonPaused => "daemon_paused",
            Self::DaemonResumed => "daemon_resumed",
        }
    }
}

/// Core daemon facade used by user-facing APIs.
#[cfg(feature = "server")]
#[derive(Clone)]
pub struct LocalrefDaemon {
    /// Stored storage.
    storage: StorageDb,
    /// Stored library root.
    library_root: PathBuf,
    /// Stored queue.
    queue: Arc<Mutex<TaskQueueState>>,
    /// Completion-event publisher; subscribers drive plugin hooks.
    event_tx: broadcast::Sender<DaemonEvent>,
}

#[cfg(feature = "server")]
#[derive(Debug)]
/// Internal representation for task queue state.
struct TaskQueueState {
    /// Stored next id.
    next_id: u64,
    /// Stored running.
    running: bool,
    /// Stored queued.
    queued: Vec<DaemonTaskRecord>,
    /// Stored history.
    history: Vec<DaemonTaskRecord>,
    /// Stored paused modes.
    paused_modes: BTreeSet<PauseMode>,
}

#[cfg(feature = "server")]
impl LocalrefDaemon {
    /// Create a daemon facade backed by query storage.
    #[must_use]
    pub fn new(storage: StorageDb) -> Self {
        let library_root = storage.library_root().to_path_buf();
        let (event_tx, _) = broadcast::channel(256);
        Self {
            library_root,
            storage,
            queue: Arc::new(Mutex::new(TaskQueueState {
                next_id: 1,
                running: false,
                queued: Vec::new(),
                history: Vec::new(),
                paused_modes: BTreeSet::new(),
            })),
            event_tx,
        }
    }

    /// Subscribe to daemon completion events.
    ///
    /// Each successful mutating action publishes a [`DaemonEvent`]; the host
    /// uses this stream to spawn plugins bound to the matching hook event.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.event_tx.subscribe()
    }

    /// Publish one completion event, ignoring the case of no live subscribers.
    fn emit_event(&self, event: DaemonEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Publish [`DaemonEvent::SchedulesChanged`] so the cron scheduler reloads
    /// its schedule set from the current plugin list and runtime calls.
    ///
    /// The host calls this after rediscovering plugins so manifest cron jobs
    /// from newly added plugins register without a restart.
    pub fn notify_schedules_changed(&self) {
        self.emit_event(DaemonEvent::SchedulesChanged);
    }

    /// Open storage for a library root and create a daemon facade.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn for_library(library_root: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self::new(StorageDb::open(library_root)?))
    }

    /// Declare the plugin-contributed `extra` fields that should be indexed.
    ///
    /// Each entry is a `"namespace.key"` string, aggregated by the host from
    /// discovered plugins' manifests. Call this before the daemon is shared and
    /// before the first rebuild; the next [`Self::rebuild_index`] or scan
    /// populates the secondary index for these fields. Values of indexed fields
    /// then participate in [`Self::search`].
    pub fn set_indexed_extra_fields(
        &self,
        fields: BTreeSet<String>,
    ) -> Result<()> {
        self.storage.set_indexed_fields(fields);
        self.storage.rebuild_from_all()?;
        Ok(())
    }

    /// Return the library root this daemon operates on.
    #[must_use]
    pub fn library_root(&self) -> &Path {
        &self.library_root
    }

    /// Return the raw automatic-classification rules text.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn read_rules_text(&self) -> Result<String> {
        let path = self.library_root.join(".localref").join("rules.toml");
        if !path.exists() {
            return Ok(String::new());
        }
        std::fs::read_to_string(&path)
            .map_err(|source| LocalrefError::io(&path, source))
    }

    /// Validate and replace the automatic-classification rules text.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn write_rules_text(&self, text: &str) -> Result<()> {
        RuleSet::parse(text)?;
        let dir = self.library_root.join(".localref");
        std::fs::create_dir_all(&dir)
            .map_err(|source| LocalrefError::io(&dir, source))?;
        let path = dir.join("rules.toml");
        std::fs::write(&path, text)
            .map_err(|source| LocalrefError::io(&path, source))?;
        self.emit_event(DaemonEvent::RulesChanged);
        Ok(())
    }

    /// Return all runtime-registered scheduled plugin calls.
    /// # Errors
    ///
    /// Returns an error when the schedules file cannot be read or parsed.
    pub fn list_schedules(
        &self,
    ) -> Result<Vec<crate::schedule::ScheduledCall>> {
        crate::schedule::load(&self.library_root)
    }

    /// Register a scheduled plugin call, rejecting duplicate ids and invalid
    /// cron expressions, then publish [`DaemonEvent::SchedulesChanged`].
    /// # Errors
    ///
    /// Returns a [`LocalrefError::Rule`] when the id is empty, already exists,
    /// or the cron expression does not parse; or an IO/serialization error when
    /// the schedules file cannot be written.
    pub fn register_schedule(
        &self,
        call: crate::schedule::ScheduledCall,
    ) -> Result<()> {
        if call.id.trim().is_empty() {
            return Err(LocalrefError::Rule(
                "schedule id must not be empty".to_string(),
            ));
        }
        if cron::Schedule::from_str(&call.schedule).is_err() {
            return Err(LocalrefError::Rule(format!(
                "invalid cron expression: {}",
                call.schedule
            )));
        }
        let mut schedules = crate::schedule::load(&self.library_root)?;
        if schedules.iter().any(|existing| existing.id == call.id) {
            return Err(LocalrefError::Rule(format!(
                "schedule id already exists: {}",
                call.id
            )));
        }
        schedules.push(call);
        crate::schedule::save(&self.library_root, &schedules)?;
        self.emit_event(DaemonEvent::SchedulesChanged);
        Ok(())
    }

    /// Remove a scheduled call by id, publishing [`DaemonEvent::SchedulesChanged`]
    /// when a schedule was actually removed.
    ///
    /// Returns `true` when a matching schedule was found and removed.
    /// # Errors
    ///
    /// Returns an error when the schedules file cannot be read or written.
    pub fn remove_schedule(&self, id: &str) -> Result<bool> {
        let mut schedules = crate::schedule::load(&self.library_root)?;
        let before = schedules.len();
        schedules.retain(|existing| existing.id != id);
        let removed = schedules.len() != before;
        if removed {
            crate::schedule::save(&self.library_root, &schedules)?;
            self.emit_event(DaemonEvent::SchedulesChanged);
        }
        Ok(removed)
    }

    /// Return daemon status and recent task history.
    #[must_use]
    /// # Panics
    ///
    /// Panics if an internal invariant is violated.
    pub fn status(&self) -> DaemonStatus {
        let queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        DaemonStatus {
            running: queue.running,
            queued_tasks: queue.queued.len(),
            recent_tasks: queue.history.clone(),
            paused_modes: queue.paused_modes.iter().copied().collect(),
        }
    }

    /// Add one active pause mode.
    /// # Panics
    ///
    /// Panics if an internal invariant is violated.
    pub fn pause(&self, mode: PauseMode) -> DaemonStatus {
        let mut queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        tracing::info!(
            event_kind = LogKind::PauseChanged.as_str(),
            "pause mode enabled: {mode:?}",
        );
        let changed = queue.paused_modes.insert(mode);
        let status = DaemonStatus {
            running: queue.running,
            queued_tasks: queue.queued.len(),
            recent_tasks: queue.history.clone(),
            paused_modes: queue.paused_modes.iter().copied().collect(),
        };
        drop(queue);
        if changed {
            self.emit_event(DaemonEvent::DaemonPaused);
        }
        status
    }

    /// Remove one active pause mode.
    /// # Panics
    ///
    /// Panics if an internal invariant is violated.
    pub fn resume(&self, mode: PauseMode) -> DaemonStatus {
        let mut queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        tracing::info!(
            event_kind = LogKind::PauseChanged.as_str(),
            "pause mode disabled: {mode:?}",
        );
        let changed = queue.paused_modes.remove(&mode);
        let status = DaemonStatus {
            running: queue.running,
            queued_tasks: queue.queued.len(),
            recent_tasks: queue.history.clone(),
            paused_modes: queue.paused_modes.iter().copied().collect(),
        };
        drop(queue);
        if changed {
            self.emit_event(DaemonEvent::DaemonResumed);
        }
        status
    }

    /// Enqueue and execute a scan task.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn scan_all(&self) -> Result<DaemonTaskRecord> {
        self.execute_task(DaemonTask::ScanAll)
    }

    /// Rebuild the query index from `All/*/metadata.toml` without normalizing.
    ///
    /// Unlike [`Self::scan_all`], this only refreshes the redb cache (items and
    /// the extra index) from metadata; it does not create metadata for
    /// unmanaged directories or reconcile `Cat/` junctions. Returns the number
    /// of indexed items.
    ///
    /// # Errors
    ///
    /// Returns an error when the index cannot be rebuilt.
    pub fn rebuild_index(&self) -> Result<usize> {
        self.storage.rebuild_from_all()
    }

    /// Enqueue and execute one connector import task.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn import_connector_item(
        &self,
        import: ConnectorImport,
    ) -> Result<ImportOutcome> {
        let mut record = self.enqueue(DaemonTask::ImportConnector {
            title: import.item.title.clone(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                ImportPipeline::new(&self.library_root)
                    .import_connector_item(import)
            })
            .and_then(|outcome| {
                self.storage.rebuild_from_all()?;
                record.indexed_items = Some(self.storage.list_items()?.len());
                record.message =
                    Some(format!("imported {}", outcome.item_id.as_str()));
                Ok(outcome)
            });

        match result {
            Ok(outcome) => {
                record.state = DaemonTaskState::Completed;
                self.emit_event(DaemonEvent::ItemImported {
                    item_id: outcome.item_id.as_str().to_string(),
                });
                self.finish(record);
                Ok(outcome)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Enqueue and execute one late connector attachment save.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn save_connector_attachment_to_item(
        &self,
        item_dir: &Path,
        attachment: impl Borrow<ConnectorAttachment>,
    ) -> Result<PathBuf> {
        let attachment = attachment.borrow();
        let mut record = self.enqueue(DaemonTask::SaveConnectorAttachment {
            filename: attachment.filename.clone(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                ImportPipeline::new(&self.library_root)
                    .save_connector_attachment_to_item(item_dir, attachment)
            })
            .and_then(|path| {
                self.storage.rebuild_from_all()?;
                record.message =
                    Some(format!("saved attachment {}", path.display()));
                Ok(path)
            });

        match result {
            Ok(path) => {
                record.state = DaemonTaskState::Completed;
                self.finish(record);
                Ok(path)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Patch metadata after validating the expected revision.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn patch_metadata(
        &self,
        item_id: &str,
        expected_revision: &str,
        metadata: impl Borrow<Metadata>,
    ) -> Result<ItemDocument> {
        let metadata = metadata.borrow();
        if metadata.id != item_id {
            return Err(LocalrefError::Unsupported(
                "metadata id cannot be changed",
            ));
        }
        let mut record = self.enqueue(DaemonTask::PatchMetadata {
            item_id: item_id.to_string(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                let item = self
                    .storage
                    .get_item(item_id)?
                    .ok_or(LocalrefError::MissingField("item"))?;
                let item_dir = self.library_root.join(item.object_path);
                ImportPipeline::new(&self.library_root)
                    .write_metadata_if_revision(
                        &item_dir,
                        metadata,
                        expected_revision,
                    )
            })
            .and_then(|()| {
                self.storage.rebuild_from_all()?;
                self.storage
                    .get_item(item_id)?
                    .ok_or(LocalrefError::MissingField("item"))
            });

        match result {
            Ok(item) => {
                record.state = DaemonTaskState::Completed;
                record.message =
                    Some(format!("patched metadata for {item_id}"));
                self.emit_event(DaemonEvent::MetadataPatched {
                    item_id: item_id.to_string(),
                });
                self.finish(record);
                Ok(item)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Set or clear one plugin `extra` value on an item.
    ///
    /// `metadata.toml` is the source of truth; this performs a locked
    /// read-modify-write via [`Metadata::apply_extra_edit`] (preserving the
    /// user's formatting) and rebuilds the query index so any declared-indexed
    /// field becomes searchable. Passing `None` for `value` removes the key.
    /// This is the write path plugins drive through the REST endpoint; it is
    /// not revision-gated because a plugin edits only its own namespace.
    ///
    /// # Errors
    ///
    /// Returns an error when the item is missing or the metadata cannot be
    /// read or written.
    pub fn set_item_extra(
        &self,
        item_id: &str,
        namespace: &str,
        key: &str,
        value: Option<&str>,
    ) -> Result<ItemDocument> {
        let mut record = self.enqueue(DaemonTask::PatchMetadata {
            item_id: item_id.to_string(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let item = self
                .storage
                .get_item(item_id)?
                .ok_or(LocalrefError::MissingField("item"))?;
            let item_dir = self.library_root.join(&item.object_path);
            let locks = LockManager::new(&self.library_root);
            let _lock = locks.acquire(item_id, "set_item_extra")?;
            let metadata_path = item_dir.join("metadata.toml");
            let current = std::fs::read_to_string(&metadata_path)
                .map_err(|source| LocalrefError::io(&metadata_path, source))?;
            let edited = Metadata::apply_extra_edit(
                &current, namespace, key, value,
            )?;
            if edited != current {
                LibraryFs::new(&self.library_root)
                    .atomic_write(&metadata_path, edited.as_bytes())?;
            }
            self.storage.rebuild_from_all()?;
            self.storage
                .get_item(item_id)?
                .ok_or(LocalrefError::MissingField("item"))
        });

        match result {
            Ok(item) => {
                record.state = DaemonTaskState::Completed;
                record.message =
                    Some(format!("set extra {namespace}.{key} for {item_id}"));
                self.emit_event(DaemonEvent::MetadataPatched {
                    item_id: item_id.to_string(),
                });
                self.finish(record);
                Ok(item)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Create minimal metadata for an existing directory under `All/`.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn import_all_directory(
        &self,
        item_dir: impl Into<PathBuf>,
    ) -> Result<ImportOutcome> {
        let item_dir = item_dir.into();
        let item_dir = if item_dir.is_absolute() {
            item_dir
        } else {
            self.library_root.join(item_dir)
        };
        let mut record = self.enqueue(DaemonTask::ImportAllDirectory {
            path: item_dir.display().to_string(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                ImportPipeline::new(&self.library_root)
                    .create_metadata_for_all_directory(&item_dir)
            })
            .and_then(|outcome| {
                self.storage.rebuild_from_all()?;
                record.indexed_items = Some(self.storage.list_items()?.len());
                record.message =
                    Some(format!("imported {}", outcome.item_id.as_str()));
                Ok(outcome)
            });

        match result {
            Ok(outcome) => {
                record.state = DaemonTaskState::Completed;
                self.emit_event(DaemonEvent::ItemImported {
                    item_id: outcome.item_id.as_str().to_string(),
                });
                self.finish(record);
                Ok(outcome)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Import one explicit file by copying it into a new `All/` item directory.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn import_file(
        &self,
        file_path: impl Into<PathBuf>,
    ) -> Result<ImportOutcome> {
        let file_path = self.absolute_library_path(file_path.into());
        let mut record = self.enqueue(DaemonTask::ImportFile {
            path: file_path.display().to_string(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                ImportPipeline::new(&self.library_root).import_file(&file_path)
            })
            .and_then(|outcome| {
                self.storage.rebuild_from_all()?;
                record.indexed_items = Some(self.storage.list_items()?.len());
                record.message =
                    Some(format!("imported {}", outcome.item_id.as_str()));
                Ok(outcome)
            });

        match result {
            Ok(outcome) => {
                record.state = DaemonTaskState::Completed;
                self.emit_event(DaemonEvent::ItemImported {
                    item_id: outcome.item_id.as_str().to_string(),
                });
                self.finish(record);
                Ok(outcome)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Copy one explicit file into an existing indexed item directory.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn add_file_to_item(
        &self,
        item_id: &str,
        file_path: impl Into<PathBuf>,
    ) -> Result<ItemDocument> {
        let file_path = self.absolute_library_path(file_path.into());
        let mut record = self.enqueue(DaemonTask::AddItemFile {
            item_id: item_id.to_string(),
            path: file_path.display().to_string(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                let item = self
                    .storage
                    .get_item(item_id)?
                    .ok_or(LocalrefError::MissingField("item"))?;
                let item_dir = self.library_root.join(&item.object_path);
                ImportPipeline::new(&self.library_root)
                    .add_file_to_item(&item_dir, &file_path)
            })
            .and_then(|_| {
                self.storage.rebuild_from_all()?;
                self.storage
                    .get_item(item_id)?
                    .ok_or(LocalrefError::MissingField("item"))
            });

        match result {
            Ok(item) => {
                record.state = DaemonTaskState::Completed;
                record.message = Some(format!("added file to {item_id}"));
                self.finish(record);
                self.emit_event(DaemonEvent::ItemFileAdded {
                    item_id: item_id.to_string(),
                });
                Ok(item)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Write uploaded bytes into an existing indexed item directory.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn add_uploaded_file_to_item(
        &self,
        item_id: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<ItemDocument> {
        let mut record = self.enqueue(DaemonTask::AddItemFile {
            item_id: item_id.to_string(),
            path: filename.to_string(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                let item = self
                    .storage
                    .get_item(item_id)?
                    .ok_or(LocalrefError::MissingField("item"))?;
                let item_dir = self.library_root.join(&item.object_path);
                ImportPipeline::new(&self.library_root)
                    .add_uploaded_file_to_item(&item_dir, filename, bytes)
            })
            .and_then(|_| {
                self.storage.rebuild_from_all()?;
                self.storage
                    .get_item(item_id)?
                    .ok_or(LocalrefError::MissingField("item"))
            });

        match result {
            Ok(item) => {
                record.state = DaemonTaskState::Completed;
                record.message = Some(format!("uploaded file to {item_id}"));
                self.finish(record);
                self.emit_event(DaemonEvent::ItemFileAdded {
                    item_id: item_id.to_string(),
                });
                Ok(item)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Normalize one real directory under `Cat/`.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn normalize_cat_directory(
        &self,
        cat_dir: impl Into<PathBuf>,
    ) -> Result<ImportOutcome> {
        let cat_dir = self.absolute_library_path(cat_dir.into());
        let mut record = self.enqueue(DaemonTask::NormalizeCatDirectory {
            path: cat_dir.display().to_string(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                self.storage.rebuild_from_all()?;
                let items = self.storage.list_items()?;
                ImportPipeline::new(&self.library_root)
                    .normalize_cat_directory(&cat_dir, &items)
            })
            .and_then(|outcome| {
                self.storage.rebuild_from_all()?;
                record.indexed_items = Some(self.storage.list_items()?.len());
                record.message =
                    Some(format!("normalized {}", outcome.item_id.as_str()));
                Ok(outcome)
            });

        match result {
            Ok(outcome) => {
                record.state = DaemonTaskState::Completed;
                self.finish(record);
                Ok(outcome)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Apply category membership edits to one item's `metadata.toml` in place.
    ///
    /// `metadata.toml` is the source of truth for membership; this performs a
    /// locked read-modify-write using `toml_edit` so the user's formatting and
    /// comments survive. `add` categories are ensured present (and any matching
    /// tombstone cleared); `remove` categories are dropped and tombstoned so
    /// auto-classification will not re-file them. These are daemon-authored
    /// edits, so — unlike `patch_metadata` — they are not revision-gated.
    ///
    /// Returns the item's `All/` directory path for follow-on junction work.
    fn edit_item_categories(
        &self,
        item_id: &str,
        add: &[&str],
        remove: &[&str],
        drop: &[&str],
    ) -> Result<PathBuf> {
        let item = self
            .storage
            .get_item(item_id)?
            .ok_or(LocalrefError::MissingField("item"))?;
        let item_dir = self.library_root.join(&item.object_path);
        let locks = LockManager::new(&self.library_root);
        let _lock = locks.acquire(item_id, "edit_item_categories")?;
        let metadata_path = item_dir.join("metadata.toml");
        let current = std::fs::read_to_string(&metadata_path)
            .map_err(|source| LocalrefError::io(&metadata_path, source))?;
        let edited =
            Metadata::apply_category_edits(&current, add, remove, drop)?;
        if edited != current {
            LibraryFs::new(&self.library_root)
                .atomic_write(&metadata_path, edited.as_bytes())?;
        }
        Ok(item_dir)
    }

    /// Load the category junction manifest.
    fn load_manifest(&self) -> Result<CatManifest> {
        CatManifest::load(&self.library_root)
    }

    /// Create an empty category directory and rebuild category indexes.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn create_category(
        &self,
        category: impl Borrow<CategoryPath>,
    ) -> Result<CategorySummary> {
        let category = category.borrow();
        let record = self.enqueue(DaemonTask::CreateCategory {
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let path = LibraryFs::new(&self.library_root)
                .create_category_dir(category)?;
            tracing::info!(
                event_kind = LogKind::CategoryCreated.as_str(),
                path = relative_to_root(&self.library_root, &path),
                "category created: {}",
                category.as_str(),
            );
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, category)
        });
        self.finish_task_result(record, result)
    }

    /// Add one indexed item to a category.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn add_item_category(
        &self,
        item_id: &str,
        category: impl Borrow<CategoryPath>,
    ) -> Result<CategorySummary> {
        let category = category.borrow();
        let record = self.enqueue(DaemonTask::AddCategory {
            item_id: item_id.to_string(),
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let mut manifest = self.load_manifest()?;
            self.file_item_into_category(item_id, category, &mut manifest)?;
            manifest.save(&self.library_root)?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, category)
        });
        self.finish_task_result(record, result)
    }

    /// File one item into a category: write metadata, project the junction, and
    /// record it in the manifest. Filing into a real category also clears any
    /// `unmatched` membership (metadata, junction, and manifest), since an item
    /// is either `unmatched` or filed under real categories, never both.
    fn file_item_into_category(
        &self,
        item_id: &str,
        category: &CategoryPath,
        manifest: &mut CatManifest,
    ) -> Result<()> {
        let item_dir = self.edit_item_categories(
            item_id,
            &[category.as_str()],
            &[],
            &[],
        )?;
        let fs = LibraryFs::new(&self.library_root);
        let link = fs.create_category_link(category, &item_dir)?;
        manifest.insert(item_id, category.as_str());
        tracing::debug!(
            event_kind = LogKind::CatLinkCreated.as_str(),
            item_id = item_id,
            path = relative_to_root(&self.library_root, &link),
            "category link created: {}",
            category.as_str(),
        );
        if category.as_str() != UNMATCHED_CATEGORY {
            // An item is either `unmatched` or filed under real categories,
            // never both, so clear any `unmatched` membership, junction, and
            // manifest record. Removing a non-existent link is a harmless no-op.
            let unmatched = CategoryPath::new(UNMATCHED_CATEGORY)
                .expect("UNMATCHED_CATEGORY is a valid category path");
            let _ = self.edit_item_categories(
                item_id,
                &[],
                &[],
                &[UNMATCHED_CATEGORY],
            )?;
            let entry_name = item_dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(LocalrefError::MissingField("item directory name"))?;
            fs.remove_category_link(&unmatched, entry_name)?;
            manifest.remove(item_id, unmatched.as_str());
        }
        Ok(())
    }

    /// Add multiple indexed items to one category with one index rebuild.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn add_items_category(
        &self,
        item_ids: &[String],
        category: impl Borrow<CategoryPath>,
    ) -> Result<CategorySummary> {
        let category = category.borrow();
        let record = self.enqueue(DaemonTask::AddCategory {
            item_id: item_ids.join(","),
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let mut manifest = self.load_manifest()?;
            for item_id in item_ids {
                self.file_item_into_category(item_id, category, &mut manifest)?;
            }
            manifest.save(&self.library_root)?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, category)
        });
        self.finish_task_result(record, result)
    }

    /// Remove one indexed item from a category.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn remove_item_category(
        &self,
        item_id: &str,
        category: impl Borrow<CategoryPath>,
    ) -> Result<CategorySummary> {
        let category = category.borrow();
        let record = self.enqueue(DaemonTask::RemoveCategory {
            item_id: item_id.to_string(),
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let mut manifest = self.load_manifest()?;
            self.unfile_item_from_category(item_id, category, &mut manifest)?;
            manifest.save(&self.library_root)?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, category)
        });
        self.finish_task_result(record, result)
    }

    /// Remove one item from a category: drop it from metadata (writing a
    /// tombstone so auto-classification won't re-file it), remove the junction,
    /// and drop the manifest record.
    fn unfile_item_from_category(
        &self,
        item_id: &str,
        category: &CategoryPath,
        manifest: &mut CatManifest,
    ) -> Result<()> {
        let item_dir = self.edit_item_categories(
            item_id,
            &[],
            &[category.as_str()],
            &[],
        )?;
        let entry_name = item_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("item directory name"))?;
        let removed = LibraryFs::new(&self.library_root)
            .remove_category_link(category, entry_name)?;
        manifest.remove(item_id, category.as_str());
        if let Some(path) = removed {
            tracing::debug!(
                event_kind = LogKind::CatLinkDeleted.as_str(),
                item_id = item_id,
                path = relative_to_root(&self.library_root, &path),
                "category link deleted: {}",
                category.as_str(),
            );
        }
        Ok(())
    }

    /// Remove multiple indexed items from one category with one index rebuild.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn remove_items_category(
        &self,
        item_ids: &[String],
        category: impl Borrow<CategoryPath>,
    ) -> Result<CategorySummary> {
        let category = category.borrow();
        let record = self.enqueue(DaemonTask::RemoveCategory {
            item_id: item_ids.join(","),
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let mut manifest = self.load_manifest()?;
            for item_id in item_ids {
                self.unfile_item_from_category(
                    item_id,
                    category,
                    &mut manifest,
                )?;
            }
            manifest.save(&self.library_root)?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, category)
        });
        self.finish_task_result(record, result)
    }

    /// Delete one indexed item directory and its category links.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn delete_item(&self, item_id: &str) -> Result<bool> {
        let record = self
            .enqueue(DaemonTask::DeleteItem { item_id: item_id.to_string() });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let Some(item) = self.storage.get_item(item_id)? else {
                return Ok(false);
            };
            let item_dir = self.library_root.join(&item.object_path);
            ensure_inside_all(&self.library_root, &item_dir)?;
            let entry_name = item_dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(LocalrefError::MissingField("item directory name"))?;
            let fs = LibraryFs::new(&self.library_root);
            let locks = LockManager::new(&self.library_root);
            let _lock = locks.acquire(item_id, "delete_item")?;
            for category in &item.categories {
                let category =
                    CategoryPath::new(category).ok_or_else(|| {
                        LocalrefError::InvalidPathComponent {
                            component: category.clone(),
                            reason: "indexed category path is invalid",
                        }
                    })?;
                if let Some(path) =
                    fs.remove_category_link(&category, entry_name)?
                {
                    tracing::debug!(
                        event_kind = LogKind::CatLinkDeleted.as_str(),
                        item_id = item_id,
                        path = relative_to_root(&self.library_root, &path),
                        "category link deleted: {}",
                        category.as_str(),
                    );
                }
            }
            std::fs::remove_dir_all(&item_dir)
                .map_err(|source| LocalrefError::io(&item_dir, source))?;
            let mut manifest = self.load_manifest()?;
            manifest.remove_item(item_id);
            manifest.save(&self.library_root)?;
            tracing::warn!(
                event_kind = LogKind::ItemDeleted.as_str(),
                item_id = item_id,
                path = relative_to_root(&self.library_root, &item_dir),
                "item deleted",
            );
            self.storage.rebuild_from_all()?;
            Ok(true)
        });
        self.finish_task_result(record, result)
    }

    /// Rename a category directory and rebuild category indexes.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn rename_category(
        &self,
        from: impl Borrow<CategoryPath>,
        to: impl Borrow<CategoryPath>,
    ) -> Result<CategorySummary> {
        let from = from.borrow();
        let to = to.borrow();
        let record = self.enqueue(DaemonTask::RenameCategory {
            from: from.clone(),
            to: to.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let path = LibraryFs::new(&self.library_root)
                .rename_category(from, to)?;
            self.remap_category_prefix(from, to)?;
            tracing::info!(
                event_kind = LogKind::CategoryRenamed.as_str(),
                path = relative_to_root(&self.library_root, &path),
                "category renamed: {} -> {}",
                from.as_str(),
                to.as_str(),
            );
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, to)
        });
        self.finish_task_result(record, result)
    }

    /// Rewrite metadata categories and manifest entries after a category tree
    /// moves from one prefix to another (rename or merge).
    ///
    /// Every item whose metadata lists `from` — either exactly or as a `from/…`
    /// subcategory — has that path rewritten to the corresponding `to` path.
    /// The junction directory itself is moved by the caller; this keeps the
    /// source-of-truth metadata and the manifest consistent with the move.
    fn remap_category_prefix(
        &self,
        from: &CategoryPath,
        to: &CategoryPath,
    ) -> Result<()> {
        let from_str = from.as_str();
        let sub_prefix = format!("{from_str}/");
        let remap = |category: &str| -> Option<String> {
            if category == from_str {
                Some(to.as_str().to_string())
            } else {
                category
                    .strip_prefix(&sub_prefix)
                    .map(|rest| format!("{}/{rest}", to.as_str()))
            }
        };

        let mut manifest = self.load_manifest()?;
        for item in self.storage.list_items()? {
            for category in &item.categories {
                if let Some(new_category) = remap(category) {
                    let _ = self.edit_item_categories(
                        &item.id,
                        &[new_category.as_str()],
                        &[],
                        &[category.as_str()],
                    )?;
                    manifest.remove(&item.id, category);
                    manifest.insert(&item.id, &new_category);
                }
            }
        }
        manifest.save(&self.library_root)?;
        Ok(())
    }

    /// Merge one category directory into another.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn merge_category(
        &self,
        from: impl Borrow<CategoryPath>,
        to: impl Borrow<CategoryPath>,
    ) -> Result<CategorySummary> {
        let from = from.borrow();
        let to = to.borrow();
        let record = self.enqueue(DaemonTask::MergeCategory {
            from: from.clone(),
            to: to.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let path =
                LibraryFs::new(&self.library_root).merge_category(from, to)?;
            self.remap_category_prefix(from, to)?;
            tracing::info!(
                event_kind = LogKind::CategoryMerged.as_str(),
                path = relative_to_root(&self.library_root, &path),
                "category merged: {} -> {}",
                from.as_str(),
                to.as_str(),
            );
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, to)
        });
        self.finish_task_result(record, result)
    }

    /// Return all indexed items.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn list_items(&self) -> Result<Vec<ItemDocument>> {
        self.storage.list_items()
    }

    /// Return filesystem entries under one indexed item directory.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn item_files(
        &self,
        item_id: &str,
    ) -> Result<Option<ItemFilesDocument>> {
        rest_files::item_files(self, item_id)
    }

    /// Open one indexed item directory with the platform file manager.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn open_item_folder(&self, item_id: &str) -> Result<bool> {
        let Some(path) = rest_files::item_folder(self, item_id)? else {
            return Ok(false);
        };
        rest_files::open_system_path(&path)?;
        Ok(true)
    }

    /// Open one item-relative file with the platform default application.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn open_item_file(
        &self,
        item_id: &str,
        relative: &Path,
    ) -> Result<bool> {
        let Some(path) = rest_files::item_file_path(self, item_id, relative)?
        else {
            return Ok(false);
        };
        rest_files::open_system_path(&path)?;
        Ok(true)
    }

    /// Open a plugin directory with the platform file manager.
    ///
    /// The caller passes a directory from a discovered plugin's manifest, so
    /// the path is host-controlled rather than user-supplied. Returns `false`
    /// when the path is not an existing directory.
    /// # Errors
    ///
    /// Returns an error when the platform file manager cannot be launched.
    pub fn open_plugin_folder(&self, dir: &Path) -> Result<bool> {
        if !dir.is_dir() {
            return Ok(false);
        }
        rest_files::open_system_path(dir)?;
        Ok(true)
    }

    /// Return one indexed item by id.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn get_item(&self, id: &str) -> Result<Option<ItemDocument>> {
        self.storage.get_item(id)
    }

    /// Return the full parsed metadata document for one indexed item.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn get_metadata(&self, id: &str) -> Result<Option<MetadataDocument>> {
        let Some(item) = self.storage.get_item(id)? else {
            return Ok(None);
        };
        let metadata_path =
            self.library_root.join(&item.object_path).join("metadata.toml");
        let text = std::fs::read_to_string(&metadata_path)
            .map_err(|source| LocalrefError::io(&metadata_path, source))?;
        let metadata_revision = Metadata::revision_for_text(&text);
        let metadata = Metadata::from_toml_str(&text)?;
        Ok(Some(MetadataDocument {
            item_id: id.to_string(),
            metadata_revision,
            metadata,
        }))
    }

    /// Search indexed item metadata.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        self.storage.search(query)
    }

    /// Return categories derived from `Cat/` links.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn list_categories(&self) -> Result<Vec<CategorySummary>> {
        self.storage.list_categories()
    }

    /// Return recent log entries from the in-memory ring buffer.
    ///
    /// Returns an empty list when the logging system has not been initialized
    /// (e.g. in tests that do not call [`logging::init`]).
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn events(&self) -> Result<Vec<crate::logging::LogEntry>> {
        Ok(crate::logging::global_buffer()
            .map(logging::LogRingBuffer::entries)
            .unwrap_or_default())
    }

    /// Internal helper for absolute library path.
    fn absolute_library_path(&self, path: PathBuf) -> PathBuf {
        if path.is_absolute() { path } else { self.library_root.join(path) }
    }

    /// Internal helper for finish task result.
    fn finish_task_result<T>(
        &self,
        mut record: DaemonTaskRecord,
        result: Result<T>,
    ) -> Result<T> {
        match result {
            Ok(value) => {
                record.state = DaemonTaskState::Completed;
                // Category and delete tasks carry their ids in the task
                // payload, so derive the completion event from it directly.
                match &record.task {
                    DaemonTask::DeleteItem { item_id } => {
                        self.emit_event(DaemonEvent::ItemDeleted {
                            item_id: item_id.clone(),
                        });
                    }
                    DaemonTask::CreateCategory { category }
                    | DaemonTask::RenameCategory { to: category, .. }
                    | DaemonTask::MergeCategory { to: category, .. } => {
                        self.emit_event(DaemonEvent::CategoryChanged {
                            item_id: None,
                            category: Some(category.as_str().to_string()),
                        });
                    }
                    DaemonTask::AddCategory { item_id, category }
                    | DaemonTask::RemoveCategory { item_id, category } => {
                        self.emit_event(DaemonEvent::CategoryChanged {
                            item_id: Some(item_id.clone()),
                            category: Some(category.as_str().to_string()),
                        });
                    }
                    _ => {}
                }
                self.finish(record);
                Ok(value)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Internal helper for execute task.
    fn execute_task(&self, task: DaemonTask) -> Result<DaemonTaskRecord> {
        let mut record = self.enqueue(task);
        self.mark_running(record.id);

        let result =
            self.ensure_task_allowed(&record.task).and_then(|()| match record
                .task
            {
                DaemonTask::ScanAll => {
                    tracing::info!(
                        event_kind = LogKind::ScanStarted.as_str(),
                        "scan started",
                    );
                    self.scan_and_normalize(&mut record)?;
                    tracing::info!(
                        event_kind = LogKind::ScanFinished.as_str(),
                        "scan finished",
                    );
                    Ok(())
                }
                DaemonTask::ImportConnector { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use import_connector_item for import tasks",
                    ))
                }
                DaemonTask::SaveConnectorAttachment { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use save_connector_attachment_to_item for attachment tasks",
                    ))
                }
                DaemonTask::PatchMetadata { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use patch_metadata for metadata patch tasks",
                    ))
                }
                DaemonTask::ImportAllDirectory { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use import_all_directory for manual imports",
                    ))
                }
                DaemonTask::ImportFile { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use import_file for file imports",
                    ))
                }
                DaemonTask::AddItemFile { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use add_file_to_item for item file additions",
                    ))
                }
                DaemonTask::DeleteItem { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use delete_item for item deletion",
                    ))
                }
                DaemonTask::CreateCategory { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use create_category for category creation",
                    ))
                }
                DaemonTask::NormalizeCatDirectory { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use normalize_cat_directory for Cat normalization",
                    ))
                }
                DaemonTask::AddCategory { .. }
                | DaemonTask::RemoveCategory { .. }
                | DaemonTask::RenameCategory { .. }
                | DaemonTask::MergeCategory { .. } => {
                    Err(LocalrefError::Unsupported(
                        "use category command methods for category tasks",
                    ))
                }
            });

        match result {
            Ok(()) => {
                record.state = DaemonTaskState::Completed;
                if matches!(record.task, DaemonTask::ScanAll) {
                    self.emit_event(DaemonEvent::ScanCompleted {
                        indexed_items: record.indexed_items.unwrap_or(0),
                    });
                }
                self.finish(record.clone());
                Ok(record)
            }
            Err(error) => {
                record.state = DaemonTaskState::Failed;
                record.message = Some(error.to_string());
                self.finish(record);
                Err(error)
            }
        }
    }

    /// Import user-created library folders discovered during a scan.
    fn scan_and_normalize(&self, record: &mut DaemonTaskRecord) -> Result<()> {
        let scan = scan_library(&self.library_root)?;
        let pipeline = ImportPipeline::new(&self.library_root);
        let mut all_imports = 0_usize;
        for entry in scan
            .all_entries
            .iter()
            .filter(|entry| entry.kind == AllEntryKind::UnmanagedCandidate)
        {
            pipeline.create_metadata_for_all_directory(
                &self.library_root.join(&entry.path),
            )?;
            all_imports += 1;
        }

        self.storage.rebuild_from_all()?;
        let mut cat_normalizations = 0_usize;
        for entry in scan
            .cat_entries
            .iter()
            .filter(|entry| entry.kind == CatEntryKind::RealDirectoryCandidate)
        {
            let items = self.storage.list_items()?;
            pipeline.normalize_cat_directory(
                &self.library_root.join(&entry.path),
                &items,
            )?;
            self.storage.rebuild_from_all()?;
            cat_normalizations += 1;
        }
        let fs = LibraryFs::new(&self.library_root);
        for entry in scan
            .cat_entries
            .iter()
            .filter(|entry| entry.kind == CatEntryKind::StaleItemLink)
        {
            let category = entry
                .category
                .as_ref()
                .ok_or(LocalrefError::MissingField("Cat category"))?;
            let target_path = entry
                .target_path
                .as_deref()
                .ok_or(LocalrefError::MissingField("stale item target"))?;
            let entry_name = Path::new(&entry.path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(LocalrefError::MissingField("Cat entry name"))?;
            fs.replace_empty_category_dir_with_link(
                category,
                entry_name,
                &path_from_scan_target(&self.library_root, target_path),
            )?;
            cat_normalizations += 1;
        }

        // Rebuild so category reconciliation sees current metadata, then
        // reconcile the Cat/ junctions against metadata + manifest.
        self.storage.rebuild_from_all()?;
        let reconciled = self.reconcile_categories()?;

        let indexed = self.storage.rebuild_from_all()?;
        record.indexed_items = Some(indexed);
        record.message = Some(format!(
            "indexed {indexed} item(s), imported {all_imports} All folder(s), normalized {cat_normalizations} Cat folder(s), reconciled {reconciled} category change(s)"
        ));
        Ok(())
    }

    /// Reconcile `Cat/` junctions against metadata membership and the manifest.
    ///
    /// `metadata.toml` is authoritative; the manifest records which junctions the
    /// daemon has projected. Per `(item, category)` the classification is:
    ///
    /// - metadata has it, junction present → in sync (ensure manifest records it)
    /// - metadata has it, junction absent, manifest recorded it → the user
    ///   deleted the junction: drop the category from metadata and tombstone it
    /// - metadata has it, junction absent, manifest did not record it → not yet
    ///   projected: create the junction and record it
    /// - junction present, metadata lacks it, not tombstoned → a hand-made or
    ///   pre-existing junction: adopt the category into metadata
    /// - junction present, metadata lacks it but tombstoned → honor the removal:
    ///   remove the junction
    ///
    /// Returns the number of membership changes applied.
    fn reconcile_categories(&self) -> Result<usize> {
        let items = self.storage.list_items()?;
        let item_pairs: Vec<(String, String)> = items
            .iter()
            .map(|item| (item.id.clone(), item.object_path.clone()))
            .collect();
        let on_disk = scan_cat_memberships(&self.library_root, &item_pairs)?;
        let mut manifest = self.load_manifest()?;
        let fs = LibraryFs::new(&self.library_root);
        let mut changes = 0_usize;

        // Load tombstones per item so we can honor deliberate removals.
        let mut tombstones: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for item in &items {
            let metadata_path =
                self.library_root.join(&item.object_path).join("metadata.toml");
            if let Ok(text) = std::fs::read_to_string(&metadata_path)
                && let Ok(metadata) = Metadata::from_toml_str(&text)
            {
                let _ = tombstones.insert(
                    item.id.clone(),
                    metadata.state.removed_categories,
                );
            }
        }
        let disk_set: std::collections::BTreeSet<(String, String)> =
            on_disk.iter().cloned().collect();

        // Detect a library relocation. `metadata.toml` and the manifest are
        // plain files that survive a copy / backup / cloud-sync / restore, but
        // the `Cat/` item-link junctions are NTFS reparse points that do NOT —
        // so after a move every membership would look like "manifest recorded
        // it, junction gone", i.e. a deliberate user deletion, and the
        // destructive branch would strip the category from metadata AND
        // tombstone it: silent, permanent loss of all membership on the first
        // scan after a move. The manifest records the absolute root it was
        // projected against; when that differs from the current root the tree
        // was relocated, so we re-project from metadata rather than honoring the
        // missing junctions as deletions (re-projection is recoverable — the
        // user can delete again — a wipe+tombstone is not).
        // A stable key for the current root: canonicalize when possible so
        // equivalent spellings compare equal, falling back to the lossy path.
        let current_root = self
            .library_root
            .canonicalize()
            .unwrap_or_else(|_| self.library_root.clone())
            .to_string_lossy()
            .replace('\\', "/");
        let relocated = manifest
            .projected_root()
            .is_some_and(|projected| projected != current_root);
        if relocated {
            tracing::warn!(
                target: "localref::core",
                "cat-manifest was projected against a different library root; \
                 treating missing junctions as a relocation and re-projecting \
                 from metadata instead of removing category membership",
            );
        }

        for item in &items {
            let entry_name = Path::new(&item.object_path)
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(LocalrefError::MissingField("item directory name"))?;

            // metadata-driven directions
            for category in &item.categories {
                let pair = (item.id.clone(), category.clone());
                let category_path = CategoryPath::new(category.as_str())
                    .ok_or_else(|| LocalrefError::InvalidPathComponent {
                        component: category.clone(),
                        reason: "indexed category path is invalid",
                    })?;
                if disk_set.contains(&pair) {
                    manifest.insert(&item.id, category); // in sync
                } else if manifest.contains(&item.id, category) && !relocated {
                    // user deleted the junction → drop + tombstone
                    let _ = self.edit_item_categories(
                        &item.id,
                        &[],
                        &[category.as_str()],
                        &[],
                    )?;
                    manifest.remove(&item.id, category);
                    changes += 1;
                    tracing::info!(
                        event_kind = LogKind::CatLinkDeleted.as_str(),
                        item_id = item.id,
                        "category dropped after user removed junction: {category}",
                    );
                } else {
                    // Not yet projected, or the whole junction tree was lost to
                    // a relocation → (re)create the junction from metadata.
                    let item_dir = self.library_root.join(&item.object_path);
                    let _ =
                        fs.create_category_link(&category_path, &item_dir)?;
                    manifest.insert(&item.id, category);
                    changes += 1;
                }
            }

            // junction-driven adoption: on disk but not in metadata
            let metadata_categories: std::collections::BTreeSet<&str> =
                item.categories.iter().map(String::as_str).collect();
            let item_tombstones = tombstones.get(&item.id);
            for (disk_id, disk_category) in &on_disk {
                if disk_id != &item.id
                    || metadata_categories.contains(disk_category.as_str())
                {
                    continue;
                }
                let category_path = CategoryPath::new(disk_category.as_str())
                    .ok_or_else(|| LocalrefError::InvalidPathComponent {
                        component: disk_category.clone(),
                        reason: "Cat junction category is invalid",
                    })?;
                let is_suppressed = item_tombstones
                    .is_some_and(|list| list.contains(disk_category));
                if is_suppressed {
                    // honor the removal: remove the stray junction
                    let _ =
                        fs.remove_category_link(&category_path, entry_name)?;
                    manifest.remove(&item.id, disk_category);
                    changes += 1;
                } else {
                    // adopt hand-made / pre-existing junction into metadata
                    let _ = self.edit_item_categories(
                        &item.id,
                        &[disk_category.as_str()],
                        &[],
                        &[],
                    )?;
                    manifest.insert(&item.id, disk_category);
                    changes += 1;
                    tracing::info!(
                        event_kind = LogKind::CatLinkCreated.as_str(),
                        item_id = item.id,
                        "adopted existing junction into metadata: {disk_category}",
                    );
                }
            }
        }

        // Record the root we just (re)projected against, so a later scan can
        // recognise a relocation instead of destroying membership.
        manifest.set_projected_root(current_root);
        manifest.save(&self.library_root)?;
        Ok(changes)
    }

    /// Internal helper for enqueue.
    fn enqueue(&self, task: DaemonTask) -> DaemonTaskRecord {
        let mut queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        let record = DaemonTaskRecord {
            id: queue.next_id,
            task,
            state: DaemonTaskState::Queued,
            message: None,
            indexed_items: None,
        };
        queue.next_id += 1;
        queue.queued.push(record.clone());
        queue.history.push(record.clone());
        record
    }

    /// Internal helper for ensure task allowed.
    fn ensure_task_allowed(&self, task: &DaemonTask) -> Result<()> {
        let queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        if queue.paused_modes.contains(&PauseMode::All) {
            return Err(LocalrefError::Unsupported("daemon is paused"));
        }

        match task {
            DaemonTask::ScanAll
                if queue.paused_modes.contains(&PauseMode::Indexing)
                    || queue.paused_modes.contains(&PauseMode::Writes) =>
            {
                Err(LocalrefError::Unsupported("indexing is paused"))
            }
            DaemonTask::ImportConnector { .. }
            | DaemonTask::SaveConnectorAttachment { .. }
            | DaemonTask::PatchMetadata { .. }
            | DaemonTask::ImportAllDirectory { .. }
            | DaemonTask::ImportFile { .. }
            | DaemonTask::AddItemFile { .. }
            | DaemonTask::DeleteItem { .. }
            | DaemonTask::NormalizeCatDirectory { .. }
            | DaemonTask::CreateCategory { .. }
            | DaemonTask::AddCategory { .. }
            | DaemonTask::RemoveCategory { .. }
            | DaemonTask::RenameCategory { .. }
            | DaemonTask::MergeCategory { .. }
                if queue.paused_modes.contains(&PauseMode::Writes) =>
            {
                Err(LocalrefError::Unsupported("writes are paused"))
            }
            _ => Ok(()),
        }
    }

    /// Internal helper for mark running.
    fn mark_running(&self, id: u64) {
        let mut queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        queue.running = true;
        if let Some(record) =
            queue.history.iter_mut().find(|record| record.id == id)
        {
            record.state = DaemonTaskState::Running;
        }
    }

    /// Internal helper for finish.
    fn finish(&self, record: DaemonTaskRecord) {
        let mut queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        queue.running = false;
        queue.queued.retain(|queued| queued.id != record.id);
        if let Some(existing) =
            queue.history.iter_mut().find(|existing| existing.id == record.id)
        {
            *existing = record;
        } else {
            queue.history.push(record);
        }
    }
}

#[cfg(feature = "server")]
impl ImportPipeline {
    /// Create an import pipeline for a library root.
    pub fn new(library_root: impl Into<PathBuf>) -> Self {
        let library_root = library_root.into();
        Self {
            fs: LibraryFs::new(&library_root),
            locks: LockManager::new(&library_root),
        }
    }

    /// Import one Zotero Connector item and its attachments into `All/`.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn import_connector_item(
        &self,
        import: impl Borrow<ConnectorImport>,
    ) -> Result<ImportOutcome> {
        let import = import.borrow();
        let item_id = connector_item_id(import)?;
        let metadata = metadata_from_import(&item_id, import, &[], &[]);
        let categories =
            RuleSet::load(self.fs.root())?.match_metadata(&metadata)?;
        self.import_connector_item_with_categories(import, categories)
    }

    /// Import one connector item and create the supplied category links.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    /// # Panics
    ///
    /// Panics if `UNMATCHED_CATEGORY` is not a valid category path. This is a
    /// compile-time-fixed constant, so the panic is unreachable in practice.
    pub fn import_connector_item_with_categories(
        &self,
        import: impl Borrow<ConnectorImport>,
        categories: Vec<CategoryPath>,
    ) -> Result<ImportOutcome> {
        let import = import.borrow();
        if import.item.title.trim().is_empty() {
            return Err(LocalrefError::MissingField("item.title"));
        }

        let item_id = connector_item_id(import)?;
        let _lock = self
            .locks
            .acquire(item_id.as_str(), "import_connector_item")
            .inspect_err(|_| {
                tracing::warn!(
                    event_kind = LogKind::WriteConflict.as_str(),
                    item_id = item_id.as_str(),
                    "connector import lock conflict",
                );
            })?;

        tracing::info!(
            event_kind = LogKind::ImportStarted.as_str(),
            item_id = item_id.as_str(),
            "connector import started: {}",
            import.item.title,
        );
        self.fs.ensure_layout()?;
        let item_dir = self.fs.create_unique_item_dir(&import.item.title)?;
        let mut written_files = Vec::new();

        let attachments = import.attachments_with_webpage_source();
        for attachment in &attachments {
            let file_path = write_attachment(&self.fs, &item_dir, attachment)?;
            written_files.push(file_path);
        }

        // No rule matched: fall back to the `unmatched` category so the item
        // is always reachable from Cat/ and never orphaned.
        let categories = if categories.is_empty() {
            vec![CategoryPath::new(UNMATCHED_CATEGORY).expect(
                "UNMATCHED_CATEGORY must be a valid category path",
            )]
        } else {
            categories
        };

        let metadata_path = item_dir.join("metadata.toml");
        let mut metadata = metadata_from_import(
            &item_id,
            import,
            &attachments,
            &written_files,
        );
        // metadata.toml is the source of truth for membership; record the
        // resolved categories there before projecting the Cat/ junctions.
        metadata.categories =
            categories.iter().map(|c| c.as_str().to_string()).collect();
        let metadata_bytes = metadata.to_toml_string()?.into_bytes();
        self.fs.atomic_write(&metadata_path, &metadata_bytes)?;
        written_files.push(metadata_path);

        tracing::debug!(
            event_kind = LogKind::MetadataWritten.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), &item_dir),
            "metadata written",
        );
        tracing::info!(
            event_kind = LogKind::ItemRegistered.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), &item_dir),
            "item registered",
        );
        tracing::info!(
            event_kind = LogKind::ImportFinished.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), &item_dir),
            "connector import finished",
        );

        for category in &categories {
            let link_path =
                self.fs.create_category_link(category, &item_dir)?;
            tracing::debug!(
                event_kind = LogKind::CatLinkCreated.as_str(),
                item_id = item_id.as_str(),
                path = relative_to_root(self.fs.root(), &link_path),
                "category link created: {}",
                category.as_str(),
            );
        }
        tracing::info!(
            event_kind = LogKind::AutoClassifiedOnImport.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), &item_dir),
            "matched {} categor(ies)",
            categories.len(),
        );

        Ok(ImportOutcome { item_id, item_dir, written_files, categories })
    }

    /// Save one connector attachment into an existing imported item directory.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn save_connector_attachment_to_item(
        &self,
        item_dir: &std::path::Path,
        attachment: &ConnectorAttachment,
    ) -> Result<PathBuf> {
        let _lock = self.locks.acquire(
            relative_to_root(self.fs.root(), item_dir),
            "save_connector_attachment_to_item",
        )?;
        let path = write_attachment(&self.fs, item_dir, attachment)?;
        self.append_attachment_to_metadata(item_dir, attachment, &path)?;
        tracing::debug!(
            event_kind = LogKind::MetadataWritten.as_str(),
            path = relative_to_root(self.fs.root(), item_dir),
            "late connector attachment saved",
        );
        Ok(path)
    }

    /// Write metadata only if the current file revision matches the expected value.
    ///
    /// On mismatch, the daemon candidate is saved as `metadata.daemon.toml` and
    /// the original `metadata.toml` is left untouched.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn write_metadata_if_revision(
        &self,
        item_dir: &Path,
        metadata: &Metadata,
        expected_revision: &str,
    ) -> Result<()> {
        let _lock = self.locks.acquire(
            relative_to_root(self.fs.root(), item_dir),
            "write_metadata_if_revision",
        )?;
        let metadata_path = item_dir.join("metadata.toml");
        let current = std::fs::read_to_string(&metadata_path)
            .map_err(|source| LocalrefError::io(&metadata_path, source))?;
        let current_revision = Metadata::revision_for_text(&current);
        let candidate = metadata.to_toml_string()?;
        if current_revision != expected_revision {
            let candidate_path = item_dir.join("metadata.daemon.toml");
            self.fs.atomic_write(&candidate_path, candidate.as_bytes())?;
            tracing::warn!(
                event_kind = LogKind::WriteConflict.as_str(),
                item_id = metadata.id,
                path = relative_to_root(self.fs.root(), item_dir),
                "metadata revision conflict",
            );
            return Err(LocalrefError::Conflict(format!(
                "metadata revision mismatch for {}",
                metadata.id
            )));
        }
        self.fs.atomic_write(&metadata_path, candidate.as_bytes())?;
        tracing::debug!(
            event_kind = LogKind::MetadataWritten.as_str(),
            item_id = metadata.id,
            path = relative_to_root(self.fs.root(), item_dir),
            "metadata written",
        );
        Ok(())
    }

    /// Create metadata for an unmanaged existing directory under `All/`.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn create_metadata_for_all_directory(
        &self,
        item_dir: &Path,
    ) -> Result<ImportOutcome> {
        self.fs.ensure_layout()?;
        ensure_inside_all(self.fs.root(), item_dir)?;
        if !item_dir.is_dir() {
            return Err(LocalrefError::MissingField("All item directory"));
        }
        let metadata_path = item_dir.join("metadata.toml");
        if metadata_path.exists() {
            return Err(LocalrefError::Conflict(format!(
                "metadata already exists at {}",
                metadata_path.display()
            )));
        }
        let title = item_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("item directory name"))?
            .to_string();
        let item_id = manual_item_id(&title)?;
        let _lock = self
            .locks
            .acquire(item_id.as_str(), "create_metadata_for_all_directory")?;
        let metadata =
            metadata_from_all_directory(&item_id, &title, item_dir)?;
        let metadata_text = metadata.to_toml_string()?;
        self.fs.atomic_write(&metadata_path, metadata_text.as_bytes())?;
        tracing::info!(
            event_kind = LogKind::MetadataCreated.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), item_dir),
            "metadata created for All directory",
        );
        if pdf_candidates(item_dir)?.len() > 1 {
            tracing::warn!(
                event_kind = LogKind::MultipleMainPdfCandidates.as_str(),
                item_id = item_id.as_str(),
                path = relative_to_root(self.fs.root(), item_dir),
                "multiple PDF files found in manual All directory",
            );
        }
        Ok(ImportOutcome {
            item_id,
            item_dir: item_dir.to_path_buf(),
            written_files: vec![metadata_path],
            categories: Vec::new(),
        })
    }

    /// Import one file into a new `All/` item directory with minimal metadata.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn import_file(&self, file_path: &Path) -> Result<ImportOutcome> {
        self.fs.ensure_layout()?;
        if !file_path.is_file() {
            return Err(LocalrefError::MissingField("import file"));
        }
        let stem = file_path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("file stem"))?;
        let item_dir = self.fs.create_unique_item_dir(stem)?;
        let filename = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("file name"))?;
        let filename = sanitize_ntfs_component(filename)?;
        let target = item_dir.join(filename);
        std::fs::copy(file_path, &target)
            .map_err(|source| LocalrefError::io(&target, source))?;
        let title = item_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("item directory name"))?;
        let item_id = manual_item_id(title)?;
        let _lock = self.locks.acquire(item_id.as_str(), "import_file")?;
        let imported_filename = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("file name"))?
            .to_string();
        let metadata = Metadata {
            id: item_id.as_str().to_string(),
            item_type: "document".to_string(),
            title: title.to_string(),
            abstract_note: None,
            doi: None,
            uri: None,
            year: None,
            venue: None,
            language: None,
            creators: Vec::new(),
            categories: Vec::new(),
            files: MetadataFiles {
                main: Some(imported_filename.clone()),
                extra: vec![MetadataFile {
                    path: imported_filename,
                    kind: "attachment".to_string(),
                    mime_type: mime_type_for_path(&target),
                }],
            },
            tags: MetadataTags::default(),
            import: MetadataImport {
                source: "manual-file".to_string(),
                session_id: None,
                imported_at: None,
            },
            state: MetadataState::default(),
            raw_connector: BTreeMap::default(),
            extra: BTreeMap::default(),
        };
        let metadata_path = item_dir.join("metadata.toml");
        let metadata_text = metadata.to_toml_string()?;
        self.fs.atomic_write(&metadata_path, metadata_text.as_bytes())?;
        tracing::info!(
            event_kind = LogKind::MetadataCreated.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), &item_dir),
            "metadata created for imported file",
        );
        Ok(ImportOutcome {
            item_id,
            item_dir,
            written_files: vec![target, metadata_path],
            categories: Vec::new(),
        })
    }

    /// Copy one file into an existing item directory and update metadata files.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn add_file_to_item(
        &self,
        item_dir: &Path,
        file_path: &Path,
    ) -> Result<PathBuf> {
        self.fs.ensure_layout()?;
        ensure_inside_all(self.fs.root(), item_dir)?;
        if !file_path.is_file() {
            return Err(LocalrefError::MissingField("item file"));
        }
        let _lock = self.locks.acquire(
            relative_to_root(self.fs.root(), item_dir),
            "add_file_to_item",
        )?;
        let filename = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("file name"))?;
        let target = unique_item_file_path(
            item_dir,
            &sanitize_ntfs_component(filename)?,
        );
        std::fs::copy(file_path, &target)
            .map_err(|source| LocalrefError::io(&target, source))?;
        self.append_file_to_metadata(item_dir, &target)?;
        tracing::debug!(
            event_kind = LogKind::MetadataWritten.as_str(),
            path = relative_to_root(self.fs.root(), item_dir),
            "item file added",
        );
        Ok(target)
    }

    /// Write uploaded file bytes into an existing item directory.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn add_uploaded_file_to_item(
        &self,
        item_dir: &Path,
        filename: &str,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        self.fs.ensure_layout()?;
        ensure_inside_all(self.fs.root(), item_dir)?;
        let _lock = self.locks.acquire(
            relative_to_root(self.fs.root(), item_dir),
            "add_uploaded_file_to_item",
        )?;
        let target = unique_item_file_path(
            item_dir,
            &sanitize_ntfs_component(filename)?,
        );
        self.fs.atomic_write(&target, bytes)?;
        self.append_file_to_metadata(item_dir, &target)?;
        tracing::debug!(
            event_kind = LogKind::MetadataWritten.as_str(),
            path = relative_to_root(self.fs.root(), item_dir),
            "uploaded item file added",
        );
        Ok(target)
    }

    /// Normalize a real directory under `Cat/` into `All/` plus a category link.
    /// # Panics
    ///
    /// Panics if an internal invariant is violated.
    /// # Errors
    ///
    /// Returns an error when the operation cannot be completed.
    pub fn normalize_cat_directory(
        &self,
        cat_dir: &Path,
        items: &[ItemDocument],
    ) -> Result<ImportOutcome> {
        self.fs.ensure_layout()?;
        let category = self.fs.category_for_real_directory(cat_dir)?;
        if !cat_dir.is_dir() {
            return Err(LocalrefError::MissingField("Cat directory"));
        }
        let entry_name = cat_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(LocalrefError::MissingField("Cat entry name"))?
            .to_string();
        let metadata_path = cat_dir.join("metadata.toml");
        let metadata = if metadata_path.exists() {
            let metadata_text = std::fs::read_to_string(&metadata_path)
                .map_err(|source| LocalrefError::io(&metadata_path, source))?;
            Some(Metadata::from_toml_str(&metadata_text)?)
        } else {
            None
        };
        let existing = metadata.as_ref().and_then(|metadata| {
            items.iter().find(|item| item.id == metadata.id).cloned()
        });
        let target = if existing.is_some() {
            None
        } else {
            Some(self.fs.unique_all_item_path(&entry_name)?)
        };
        let item_id = if let Some(metadata) = &metadata {
            ItemId::new(metadata.id.clone())
                .ok_or(LocalrefError::MissingField("metadata.id"))?
        } else {
            let target_name = target
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .ok_or(LocalrefError::MissingField(
                    "All item directory name",
                ))?;
            manual_item_id(target_name)?
        };
        let _lock =
            self.locks.acquire(item_id.as_str(), "normalize_cat_directory")?;
        let item_dir = if let Some(existing) = existing {
            let item_dir = self.fs.root().join(existing.object_path);
            std::fs::remove_dir_all(cat_dir)
                .map_err(|source| LocalrefError::io(cat_dir, source))?;
            item_dir
        } else {
            let target = target.expect("new Cat normalization target exists");
            std::fs::rename(cat_dir, &target)
                .map_err(|source| LocalrefError::io(&target, source))?;
            if metadata.is_none() {
                let title =
                    target.file_name().and_then(|name| name.to_str()).ok_or(
                        LocalrefError::MissingField("All item directory name"),
                    )?;
                self.write_directory_metadata(&item_id, title, &target)?;
            }
            target
        };
        let link_path = self.fs.create_category_link_named(
            &category,
            &entry_name,
            &item_dir,
        )?;
        // metadata.toml is the source of truth for membership; record the
        // category there so a rebuild reflects it without the junction.
        let item_metadata_path = item_dir.join("metadata.toml");
        let current = std::fs::read_to_string(&item_metadata_path)
            .map_err(|source| LocalrefError::io(&item_metadata_path, source))?;
        let edited = Metadata::apply_category_edits(
            &current,
            &[category.as_str()],
            &[],
            &[],
        )?;
        if edited != current {
            self.fs.atomic_write(&item_metadata_path, edited.as_bytes())?;
        }
        tracing::info!(
            event_kind = LogKind::CatCopyReplacedByLink.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), &link_path),
            "Cat real directory normalized",
        );
        tracing::debug!(
            event_kind = LogKind::CatLinkCreated.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), &link_path),
            "category link created: {}",
            category.as_str(),
        );
        Ok(ImportOutcome {
            item_id,
            item_dir,
            written_files: vec![link_path],
            categories: vec![category],
        })
    }

    /// Create metadata for a manually imported `All/` directory.
    ///
    /// # Errors
    ///
    /// Returns an error when directory inspection, serialization, or writing
    /// fails.
    pub fn write_directory_metadata(
        &self,
        item_id: &ItemId,
        title: &str,
        target: &Path,
    ) -> Result<()> {
        let metadata = metadata_from_all_directory(item_id, title, target)?;
        let metadata_text = metadata.to_toml_string()?;
        let metadata_path = target.join("metadata.toml");
        self.fs.atomic_write(&metadata_path, metadata_text.as_bytes())?;
        tracing::info!(
            event_kind = LogKind::MetadataCreated.as_str(),
            item_id = item_id.as_str(),
            path = relative_to_root(self.fs.root(), target),
            "metadata created for Cat directory",
        );
        if pdf_candidates(target)?.len() > 1 {
            tracing::warn!(
                event_kind = LogKind::MultipleMainPdfCandidates.as_str(),
                item_id = item_id.as_str(),
                path = relative_to_root(self.fs.root(), target),
                "multiple PDF files found in manual Cat directory",
            );
        }
        Ok(())
    }

    /// Internal helper for append attachment to metadata.
    fn append_attachment_to_metadata(
        &self,
        item_dir: &std::path::Path,
        attachment: &ConnectorAttachment,
        path: &std::path::Path,
    ) -> Result<()> {
        let metadata_path = item_dir.join("metadata.toml");
        if !metadata_path.exists() {
            return Ok(());
        }

        let metadata_text = std::fs::read_to_string(&metadata_path)
            .map_err(|source| LocalrefError::io(&metadata_path, source))?;
        let mut metadata = Metadata::from_toml_str(&metadata_text)?;
        let Some(filename) = path.file_name().and_then(|name| name.to_str())
        else {
            return Ok(());
        };

        if metadata.files.main.is_none() {
            metadata.files.main = Some(filename.to_string());
        }
        if !metadata.files.extra.iter().any(|file| file.path == filename) {
            metadata.files.extra.push(MetadataFile {
                path: filename.to_string(),
                kind: "attachment".to_string(),
                mime_type: attachment.mime_type.clone(),
            });
        }

        let metadata_bytes = metadata.to_toml_string()?.into_bytes();
        self.fs.atomic_write(&metadata_path, &metadata_bytes)?;
        Ok(())
    }

    /// Internal helper for append file to metadata.
    fn append_file_to_metadata(
        &self,
        item_dir: &std::path::Path,
        path: &std::path::Path,
    ) -> Result<()> {
        let metadata_path = item_dir.join("metadata.toml");
        if !metadata_path.exists() {
            return Ok(());
        }

        let metadata_text = std::fs::read_to_string(&metadata_path)
            .map_err(|source| LocalrefError::io(&metadata_path, source))?;
        let mut metadata = Metadata::from_toml_str(&metadata_text)?;
        let Some(filename) = path.file_name().and_then(|name| name.to_str())
        else {
            return Ok(());
        };

        if metadata.files.main.is_none() {
            metadata.files.main = Some(filename.to_string());
        } else if !metadata
            .files
            .extra
            .iter()
            .any(|file| file.path == filename)
        {
            metadata.files.extra.push(MetadataFile {
                path: filename.to_string(),
                kind: "attachment".to_string(),
                mime_type: None,
            });
        }

        let metadata_bytes = metadata.to_toml_string()?.into_bytes();
        self.fs.atomic_write(&metadata_path, &metadata_bytes)?;
        Ok(())
    }
}

/// Internal helper for metadata from import.
#[cfg(feature = "server")]
fn metadata_from_import(
    item_id: &ItemId,
    import: &ConnectorImport,
    attachments: &[ConnectorAttachment],
    written_files: &[PathBuf],
) -> Metadata {
    let attachment_files: Vec<_> = attachments
        .iter()
        .zip(written_files.iter())
        .filter_map(|(attachment, path)| {
            path.file_name().and_then(|name| name.to_str()).map(|filename| {
                MetadataFile {
                    path: filename.to_string(),
                    kind: "attachment".to_string(),
                    mime_type: attachment.mime_type.clone(),
                }
            })
        })
        .collect();

    let main = attachment_files.first().map(|file| file.path.clone());

    let mut raw_connector = std::collections::BTreeMap::new();
    if let Some(connector_item_id) = &import.item.connector_item_id {
        raw_connector.insert(
            "connector_item_id".to_string(),
            connector_item_id.clone(),
        );
    }
    raw_connector.insert(
        "raw_json".to_string(),
        serde_json::to_string(&import.item.raw)
            .unwrap_or_else(|_| "{}".to_string()),
    );

    Metadata {
        id: item_id.as_str().to_string(),
        item_type: import
            .item
            .item_type
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        title: import.item.title.clone(),
        abstract_note: import.item.abstract_note.clone(),
        doi: import.item.doi.clone(),
        uri: import.item.uri.clone(),
        year: None,
        venue: None,
        language: None,
        creators: import
            .item
            .raw
            .get("creators")
            .and_then(serde_json::Value::as_array)
            .map(|creators| {
                creators
                    .iter()
                    .filter_map(|value| Creator::try_from(value).ok())
                    .collect()
            })
            .unwrap_or_default(),
        categories: Vec::new(),
        files: MetadataFiles { main, extra: attachment_files },
        tags: MetadataTags::from(&import.item.raw),
        import: MetadataImport {
            source: "zotero-connector".to_string(),
            session_id: import.item.session_id.clone(),
            imported_at: None,
        },
        state: MetadataState::default(),
        raw_connector,
        extra: BTreeMap::default(),
    }
}

#[cfg(feature = "server")]
impl From<&serde_json::Value> for MetadataTags {
    fn from(raw: &serde_json::Value) -> Self {
        let items = raw
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .map(|tags| {
                tags.iter()
                    .filter_map(|tag| {
                        tag.as_str().map(str::to_string).or_else(|| {
                            tag.get("tag")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string)
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { items }
    }
}

#[cfg(feature = "server")]
impl TryFrom<&serde_json::Value> for Creator {
    type Error = ();

    fn try_from(value: &serde_json::Value) -> std::result::Result<Self, ()> {
        let role = json_string(value, &["creatorType", "role"])
            .unwrap_or_else(|| "author".to_string());
        let given = json_string(value, &["firstName", "given"]);
        let family = json_string(value, &["lastName", "family"]);
        let name = json_string(value, &["name"]);
        if given.is_none() && family.is_none() && name.is_none() {
            Err(())
        } else {
            Ok(Self { role, given, family, name })
        }
    }
}

/// Return the first non-empty string field from a JSON object.
#[cfg(feature = "server")]
fn json_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    })
}

/// Internal helper for metadata from all directory.
#[cfg(feature = "server")]
fn metadata_from_all_directory(
    item_id: &ItemId,
    title: &str,
    item_dir: &Path,
) -> Result<Metadata> {
    let pdfs = pdf_candidates(item_dir)?;
    let main = if pdfs.len() == 1 {
        pdfs[0].file_name().and_then(|name| name.to_str()).map(str::to_string)
    } else {
        None
    };
    let extra = direct_files(item_dir)?
        .into_iter()
        .filter_map(|path| {
            path.file_name().and_then(|name| name.to_str()).map(|filename| {
                MetadataFile {
                    path: filename.to_string(),
                    kind: "attachment".to_string(),
                    mime_type: mime_type_for_path(&path),
                }
            })
        })
        .collect();

    Ok(Metadata {
        id: item_id.as_str().to_string(),
        item_type: "document".to_string(),
        title: title.to_string(),
        abstract_note: None,
        doi: None,
        uri: None,
        year: None,
        venue: None,
        language: None,
        creators: Vec::new(),
        categories: Vec::new(),
        files: MetadataFiles { main, extra },
        tags: MetadataTags::default(),
        import: MetadataImport {
            source: "manual-all-directory".to_string(),
            session_id: None,
            imported_at: None,
        },
        state: MetadataState::default(),
        raw_connector: BTreeMap::default(),
        extra: BTreeMap::default(),
    })
}

/// Internal helper for unique item file path.
#[cfg(feature = "server")]
fn unique_item_file_path(item_dir: &Path, filename: &str) -> PathBuf {
    let candidate = item_dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem =
        path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("file");
    let extension = path.extension().and_then(|extension| extension.to_str());
    for suffix in 2.. {
        let name = match extension {
            Some(extension) => format!("{stem}-{suffix}.{extension}"),
            None => format!("{stem}-{suffix}"),
        };
        let candidate = item_dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unbounded suffix loop returns before exhausting usize")
}

/// Internal helper for direct files.
#[cfg(feature = "server")]
fn direct_files(item_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = std::fs::read_dir(item_dir)
        .map_err(|source| LocalrefError::io(item_dir, source))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| LocalrefError::io(item_dir, source))
        })
        .filter(|path| match path {
            Ok(path) => {
                path.is_file()
                    && path.file_name().and_then(|name| name.to_str())
                        != Some("metadata.toml")
            }
            Err(_) => true,
        })
        .collect::<Result<Vec<_>>>()?;
    files.sort();
    Ok(files)
}

/// Internal helper for pdf candidates.
#[cfg(feature = "server")]
fn pdf_candidates(item_dir: &Path) -> Result<Vec<PathBuf>> {
    Ok(direct_files(item_dir)?
        .into_iter()
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        })
        .collect())
}

/// Internal helper for mime type for path.
#[cfg(feature = "server")]
fn mime_type_for_path(path: &Path) -> Option<String> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("pdf") => {
            Some("application/pdf".to_string())
        }
        Some(extension) if extension.eq_ignore_ascii_case("txt") => {
            Some("text/plain".to_string())
        }
        Some(extension) if extension.eq_ignore_ascii_case("html") => {
            Some("text/html".to_string())
        }
        _ => None,
    }
}

/// Internal helper for manual item id.
#[cfg(feature = "server")]
fn manual_item_id(title: &str) -> Result<ItemId> {
    let component = sanitize_ntfs_component(title)?;
    ItemId::new(format!("lr:manual:{component}"))
        .ok_or(LocalrefError::MissingField("manual item id"))
}

/// Internal helper for ensure inside all.
#[cfg(feature = "server")]
fn ensure_inside_all(root: &Path, item_dir: &Path) -> Result<()> {
    let all_dir = root
        .join("All")
        .canonicalize()
        .map_err(|source| LocalrefError::io(root.join("All"), source))?;
    let item_dir = item_dir
        .canonicalize()
        .map_err(|source| LocalrefError::io(item_dir, source))?;
    if item_dir == all_dir || !item_dir.starts_with(&all_dir) {
        return Err(LocalrefError::InvalidPathComponent {
            component: item_dir.display().to_string(),
            reason: "manual All import must target a directory under All/",
        });
    }
    Ok(())
}

/// Internal helper for category summary for.
#[cfg(feature = "server")]
fn category_summary_for(
    storage: &StorageDb,
    category: &CategoryPath,
) -> Result<CategorySummary> {
    Ok(storage
        .list_categories()?
        .into_iter()
        .find(|summary| summary.path == category.as_str())
        .unwrap_or(CategorySummary {
            path: category.as_str().to_string(),
            item_ids: Vec::new(),
        }))
}

/// Internal helper for relative to root.
#[cfg(feature = "server")]
fn relative_to_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Internal helper for write attachment.
#[cfg(feature = "server")]
fn write_attachment(
    fs: &LibraryFs,
    item_dir: &std::path::Path,
    attachment: &ConnectorAttachment,
) -> Result<PathBuf> {
    let filename = sanitize_ntfs_component(&attachment.filename)?;
    let mut path = item_dir.join(&filename);
    if path.exists() {
        let file_path = std::path::Path::new(&filename);
        let stem = file_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(&filename);
        let extension = file_path.extension().and_then(|value| value.to_str());
        for suffix in 2_u32.. {
            let name = match extension {
                Some(extension) => {
                    format!("{stem} ({suffix}).{extension}")
                }
                None => format!("{stem} ({suffix})"),
            };
            path = item_dir.join(name);
            if !path.exists() {
                break;
            }
        }
    }
    fs.atomic_write(&path, &attachment.bytes)?;
    Ok(path)
}

#[cfg(feature = "server")]
impl ConnectorImport {
    /// Return imported attachments, adding a URL shortcut for webpages.
    #[must_use]
    pub fn attachments_with_webpage_source(&self) -> Vec<ConnectorAttachment> {
        let mut attachments = self.attachments.clone();
        if self.item.item_type.as_deref() == Some("webpage")
            && !attachments.iter().any(|attachment| {
                attachment.filename.eq_ignore_ascii_case("source.url")
            })
            && let Some(uri) = &self.item.uri
        {
            attachments.push(ConnectorAttachment {
                session_id: self.item.session_id.clone(),
                parent_item_id: self.item.connector_item_id.clone(),
                title: Some("Source URL".to_string()),
                filename: "source.url".to_string(),
                mime_type: Some("text/uri-list".to_string()),
                bytes: (format!("[InternetShortcut]\r\nURL={uri}\r\nIconIndex=0\r\nHotKey=0\r\nIDList=\r\nWorkingDirectory=\r\n") + &format!("Comment={}", self.item.title)).into_bytes(),
                raw_metadata: None,
            });
        }
        attachments
    }
}

/// Internal helper for connector item id.
#[cfg(feature = "server")]
fn connector_item_id(import: &ConnectorImport) -> Result<ItemId> {
    let source = import
        .item
        .connector_item_id
        .as_deref()
        .or(import.item.session_id.as_deref())
        .ok_or(LocalrefError::MissingField(
            "item.connector_item_id or item.session_id",
        ))?;
    ItemId::new(format!("lr:zotero:{source}"))
        .ok_or(LocalrefError::MissingField("item id"))
}
