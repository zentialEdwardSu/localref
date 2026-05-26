//! Core Localref import pipeline and daemon command queue.
//!
//! The core crate orchestrates writes but delegates concrete filesystem
//! operations to `platformfs`. Connector imports, metadata writes, and scans
//! record daemon events and acquire filesystem locks before mutating durable
//! library state.

pub mod config;
pub mod error;
mod event_log;
mod lock;
pub mod model;
mod pending;
pub mod platformfs;
pub mod rest;
mod rest_files;
pub mod rules;
pub mod scan;
pub mod storage;
pub mod types;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{LocalrefError, Result};
use crate::model::{
    Creator, Event, EventKind, Metadata, MetadataDocument, MetadataFile,
    MetadataFiles, MetadataImport, MetadataState, MetadataTags,
};
use crate::platformfs::{LibraryFs, sanitize_ntfs_component};
use crate::rules::RuleSet;
use crate::scan::{AllEntryKind, CatEntryKind, scan_library};
use crate::storage::{CategorySummary, ItemDocument, SearchHit, StorageDb};
use crate::types::{
    CategoryPath, ConnectorAttachment, ConnectorImport, ImportOutcome, ItemId,
};
pub use event_log::EventLog;
use lock::LockManager;
pub use pending::{
    PendingImportConfirmation, PendingImportSession, PendingImportStore,
};
use serde::{Deserialize, Serialize};

/// Import pipeline rooted at one Localref library.
#[derive(Clone, Debug)]
pub struct ImportPipeline {
    fs: LibraryFs,
    events: EventLog,
    locks: LockManager,
}

/// Daemon task kinds executed by the core task queue.
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
#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize,
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

/// Core daemon facade used by user-facing APIs.
#[derive(Clone)]
pub struct LocalrefDaemon {
    storage: StorageDb,
    library_root: PathBuf,
    events: EventLog,
    pending: PendingImportStore,
    queue: Arc<Mutex<TaskQueueState>>,
}

#[derive(Debug)]
struct TaskQueueState {
    next_id: u64,
    running: bool,
    queued: Vec<DaemonTaskRecord>,
    history: Vec<DaemonTaskRecord>,
    paused_modes: BTreeSet<PauseMode>,
}

impl LocalrefDaemon {
    /// Create a daemon facade backed by query storage.
    pub fn new(storage: StorageDb) -> Self {
        let library_root = storage.library_root().to_path_buf();
        Self {
            events: EventLog::new(&library_root),
            library_root,
            storage,
            pending: PendingImportStore::default(),
            queue: Arc::new(Mutex::new(TaskQueueState {
                next_id: 1,
                running: false,
                queued: Vec::new(),
                history: Vec::new(),
                paused_modes: BTreeSet::new(),
            })),
        }
    }

    /// Open storage for a library root and create a daemon facade.
    pub fn for_library(library_root: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self::new(StorageDb::open(library_root)?))
    }

    /// Return daemon status and recent task history.
    pub fn status(&self) -> DaemonStatus {
        let queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        DaemonStatus {
            running: queue.running,
            queued_tasks: queue.queued.len(),
            recent_tasks: queue.history.clone(),
            paused_modes: queue.paused_modes.iter().cloned().collect(),
        }
    }

    /// Add one active pause mode.
    pub fn pause(&self, mode: PauseMode) -> DaemonStatus {
        let mut queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        let message = format!("pause mode enabled: {mode:?}");
        queue.paused_modes.insert(mode);
        let _ =
            self.events.append(EventKind::PauseChanged, message, None, None);
        DaemonStatus {
            running: queue.running,
            queued_tasks: queue.queued.len(),
            recent_tasks: queue.history.clone(),
            paused_modes: queue.paused_modes.iter().cloned().collect(),
        }
    }

    /// Remove one active pause mode.
    pub fn resume(&self, mode: PauseMode) -> DaemonStatus {
        let mut queue =
            self.queue.lock().expect("daemon task queue mutex poisoned");
        let message = format!("pause mode disabled: {mode:?}");
        queue.paused_modes.remove(&mode);
        let _ =
            self.events.append(EventKind::PauseChanged, message, None, None);
        DaemonStatus {
            running: queue.running,
            queued_tasks: queue.queued.len(),
            recent_tasks: queue.history.clone(),
            paused_modes: queue.paused_modes.iter().cloned().collect(),
        }
    }

    /// Enqueue and execute a scan task.
    pub fn scan_all(&self) -> Result<DaemonTaskRecord> {
        self.execute_task(DaemonTask::ScanAll)
    }

    /// Enqueue and execute one connector import task.
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

    /// Create a pending connector import that must be confirmed by the user.
    pub fn create_pending_connector_import(
        &self,
        import: ConnectorImport,
    ) -> Result<PendingImportSession> {
        let item_id = connector_item_id(&import)?;
        let metadata = metadata_from_import(&item_id, &import, &[], &[]);
        let categories =
            RuleSet::load(&self.library_root)?.match_metadata(&metadata)?;
        let session = self.pending.create(import, categories);
        self.events.append(
            EventKind::ImportPendingUserConfirmation,
            "connector import pending user confirmation",
            Some(item_id.as_str().to_string()),
            None,
        )?;
        Ok(session)
    }

    /// Return pending imports waiting for user confirmation.
    pub fn pending_imports(&self) -> Vec<PendingImportSession> {
        self.pending.list()
    }

    /// Confirm a pending import and write it to `All/` with selected categories.
    pub fn confirm_pending_import(
        &self,
        id: u64,
        confirmation: PendingImportConfirmation,
    ) -> Result<ImportOutcome> {
        let record = self
            .pending
            .take(id)
            .ok_or(LocalrefError::MissingField("pending import"))?;
        let categories = confirmation
            .categories
            .unwrap_or_else(|| record.session.suggested_categories.clone());
        let mut task = self.enqueue(DaemonTask::ImportConnector {
            title: record.import.item.title.clone(),
        });
        self.mark_running(task.id);
        let result = self
            .ensure_task_allowed(&task.task)
            .and_then(|()| {
                ImportPipeline::new(&self.library_root)
                    .import_connector_item_with_categories(
                        record.import,
                        categories,
                    )
            })
            .and_then(|outcome| {
                self.storage.rebuild_from_all()?;
                task.indexed_items = Some(self.storage.list_items()?.len());
                task.message = Some(format!(
                    "confirmed pending import {}",
                    outcome.item_id.as_str()
                ));
                Ok(outcome)
            });
        match result {
            Ok(outcome) => {
                task.state = DaemonTaskState::Completed;
                self.finish(task);
                Ok(outcome)
            }
            Err(error) => {
                task.state = DaemonTaskState::Failed;
                task.message = Some(error.to_string());
                self.finish(task);
                Err(error)
            }
        }
    }

    /// Cancel a pending import without writing it to `All/`.
    pub fn cancel_pending_import(
        &self,
        id: u64,
    ) -> Result<PendingImportSession> {
        let record = self
            .pending
            .take(id)
            .ok_or(LocalrefError::MissingField("pending import"))?;
        self.events.append(
            EventKind::ImportCancelled,
            "pending import cancelled",
            None,
            None,
        )?;
        Ok(record.session)
    }

    /// Enqueue and execute one late connector attachment save.
    pub fn save_connector_attachment_to_item(
        &self,
        item_dir: &Path,
        attachment: ConnectorAttachment,
    ) -> Result<PathBuf> {
        let mut record = self.enqueue(DaemonTask::SaveConnectorAttachment {
            filename: attachment.filename.clone(),
        });
        self.mark_running(record.id);
        let result = self
            .ensure_task_allowed(&record.task)
            .and_then(|()| {
                ImportPipeline::new(&self.library_root)
                    .save_connector_attachment_to_item(item_dir, &attachment)
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
    pub fn patch_metadata(
        &self,
        item_id: &str,
        expected_revision: &str,
        metadata: Metadata,
    ) -> Result<ItemDocument> {
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
                        &metadata,
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
                ImportPipeline::new(&self.library_root)
                    .normalize_cat_directory(
                        &cat_dir,
                        self.storage.list_items()?,
                    )
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

    /// Create an empty category directory and rebuild category indexes.
    pub fn create_category(
        &self,
        category: CategoryPath,
    ) -> Result<CategorySummary> {
        let record = self.enqueue(DaemonTask::CreateCategory {
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let path = LibraryFs::new(&self.library_root)
                .create_category_dir(&category)?;
            self.events.append(
                EventKind::CategoryCreated,
                format!("category created: {}", category.as_str()),
                None,
                Some(relative_to_root(&self.library_root, &path)),
            )?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, &category)
        });
        self.finish_task_result(record, result)
    }

    /// Add one indexed item to a category.
    pub fn add_item_category(
        &self,
        item_id: &str,
        category: CategoryPath,
    ) -> Result<CategorySummary> {
        let record = self.enqueue(DaemonTask::AddCategory {
            item_id: item_id.to_string(),
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let item = self
                .storage
                .get_item(item_id)?
                .ok_or(LocalrefError::MissingField("item"))?;
            let item_dir = self.library_root.join(&item.object_path);
            let link = LibraryFs::new(&self.library_root)
                .create_category_link(&category, &item_dir)?;
            self.events.append(
                EventKind::CatLinkCreated,
                format!("category link created: {}", category.as_str()),
                Some(item_id.to_string()),
                Some(relative_to_root(&self.library_root, &link)),
            )?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, &category)
        });
        self.finish_task_result(record, result)
    }

    /// Remove one indexed item from a category.
    pub fn remove_item_category(
        &self,
        item_id: &str,
        category: CategoryPath,
    ) -> Result<CategorySummary> {
        let record = self.enqueue(DaemonTask::RemoveCategory {
            item_id: item_id.to_string(),
            category: category.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let item = self
                .storage
                .get_item(item_id)?
                .ok_or(LocalrefError::MissingField("item"))?;
            let item_dir = self.library_root.join(&item.object_path);
            let entry_name = item_dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(LocalrefError::MissingField("item directory name"))?;
            let removed = LibraryFs::new(&self.library_root)
                .remove_category_link(&category, entry_name)?;
            if let Some(path) = removed {
                self.events.append(
                    EventKind::CatLinkDeleted,
                    format!("category link deleted: {}", category.as_str()),
                    Some(item_id.to_string()),
                    Some(relative_to_root(&self.library_root, &path)),
                )?;
            }
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, &category)
        });
        self.finish_task_result(record, result)
    }

    /// Rename a category directory and rebuild category indexes.
    pub fn rename_category(
        &self,
        from: CategoryPath,
        to: CategoryPath,
    ) -> Result<CategorySummary> {
        let record = self.enqueue(DaemonTask::RenameCategory {
            from: from.clone(),
            to: to.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let path = LibraryFs::new(&self.library_root)
                .rename_category(&from, &to)?;
            self.events.append(
                EventKind::CategoryRenamed,
                format!(
                    "category renamed: {} -> {}",
                    from.as_str(),
                    to.as_str()
                ),
                None,
                Some(relative_to_root(&self.library_root, &path)),
            )?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, &to)
        });
        self.finish_task_result(record, result)
    }

    /// Merge one category directory into another.
    pub fn merge_category(
        &self,
        from: CategoryPath,
        to: CategoryPath,
    ) -> Result<CategorySummary> {
        let record = self.enqueue(DaemonTask::MergeCategory {
            from: from.clone(),
            to: to.clone(),
        });
        self.mark_running(record.id);
        let result = self.ensure_task_allowed(&record.task).and_then(|()| {
            let path = LibraryFs::new(&self.library_root)
                .merge_category(&from, &to)?;
            self.events.append(
                EventKind::CategoryMerged,
                format!(
                    "category merged: {} -> {}",
                    from.as_str(),
                    to.as_str()
                ),
                None,
                Some(relative_to_root(&self.library_root, &path)),
            )?;
            self.storage.rebuild_from_all()?;
            category_summary_for(&self.storage, &to)
        });
        self.finish_task_result(record, result)
    }

    /// Return all indexed items.
    pub fn list_items(&self) -> Result<Vec<ItemDocument>> {
        self.storage.list_items()
    }

    /// Return one indexed item by id.
    pub fn get_item(&self, id: &str) -> Result<Option<ItemDocument>> {
        self.storage.get_item(id)
    }

    /// Return the full parsed metadata document for one indexed item.
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
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        self.storage.search(query)
    }

    /// Return categories derived from `Cat/` links.
    pub fn list_categories(&self) -> Result<Vec<CategorySummary>> {
        self.storage.list_categories()
    }

    /// Return daemon events from `.localref/logs/events.jsonl`.
    pub fn events(&self) -> Result<Vec<Event>> {
        self.events.list()
    }

    fn absolute_library_path(&self, path: PathBuf) -> PathBuf {
        if path.is_absolute() { path } else { self.library_root.join(path) }
    }

    fn finish_task_result<T>(
        &self,
        mut record: DaemonTaskRecord,
        result: Result<T>,
    ) -> Result<T> {
        match result {
            Ok(value) => {
                record.state = DaemonTaskState::Completed;
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

    fn execute_task(&self, task: DaemonTask) -> Result<DaemonTaskRecord> {
        let mut record = self.enqueue(task);
        self.mark_running(record.id);

        let result =
            self.ensure_task_allowed(&record.task).and_then(|()| match record
                .task
            {
                DaemonTask::ScanAll => {
                    self.events.append(
                        EventKind::ScanStarted,
                        "scan started",
                        None,
                        None,
                    )?;
                    self.scan_and_normalize(&mut record)?;
                    self.events.append(
                        EventKind::ScanFinished,
                        "scan finished",
                        None,
                        None,
                    )?;
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
            pipeline.normalize_cat_directory(
                &self.library_root.join(&entry.path),
                self.storage.list_items()?,
            )?;
            self.storage.rebuild_from_all()?;
            cat_normalizations += 1;
        }

        let indexed = self.storage.rebuild_from_all()?;
        record.indexed_items = Some(indexed);
        record.message = Some(format!(
            "indexed {indexed} item(s), imported {all_imports} All folder(s), normalized {cat_normalizations} Cat folder(s)"
        ));
        Ok(())
    }

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
                if queue.paused_modes.contains(&PauseMode::Writes) =>
            {
                Err(LocalrefError::Unsupported("writes are paused"))
            }
            DaemonTask::SaveConnectorAttachment { .. }
                if queue.paused_modes.contains(&PauseMode::Writes) =>
            {
                Err(LocalrefError::Unsupported("writes are paused"))
            }
            DaemonTask::PatchMetadata { .. }
                if queue.paused_modes.contains(&PauseMode::Writes) =>
            {
                Err(LocalrefError::Unsupported("writes are paused"))
            }
            DaemonTask::ImportAllDirectory { .. }
            | DaemonTask::ImportFile { .. }
                if queue.paused_modes.contains(&PauseMode::Writes) =>
            {
                Err(LocalrefError::Unsupported("writes are paused"))
            }
            DaemonTask::NormalizeCatDirectory { .. }
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

impl ImportPipeline {
    /// Create an import pipeline for a library root.
    pub fn new(library_root: impl Into<PathBuf>) -> Self {
        let library_root = library_root.into();
        Self {
            fs: LibraryFs::new(&library_root),
            events: EventLog::new(&library_root),
            locks: LockManager::new(&library_root),
        }
    }

    /// Import one Zotero Connector item and its attachments into `All/`.
    pub fn import_connector_item(
        &self,
        import: ConnectorImport,
    ) -> Result<ImportOutcome> {
        let item_id = connector_item_id(&import)?;
        let categories =
            automatic_categories(self.fs.root(), &item_id, &import)?;
        self.import_connector_item_with_categories(import, categories)
    }

    /// Import one connector item and create the supplied category links.
    pub fn import_connector_item_with_categories(
        &self,
        import: ConnectorImport,
        categories: Vec<CategoryPath>,
    ) -> Result<ImportOutcome> {
        if import.item.title.trim().is_empty() {
            return Err(LocalrefError::MissingField("item.title"));
        }

        let item_id = connector_item_id(&import)?;
        let _lock = self
            .locks
            .acquire(item_id.as_str(), "import_connector_item")
            .inspect_err(|_| {
                let _ = self.events.append(
                    EventKind::WriteConflict,
                    "connector import lock conflict",
                    Some(item_id.as_str().to_string()),
                    None,
                );
            })?;

        self.events.append(
            EventKind::ImportStarted,
            format!("connector import started: {}", import.item.title),
            Some(item_id.as_str().to_string()),
            None,
        )?;
        self.fs.ensure_layout()?;
        let item_dir = self.fs.create_unique_item_dir(&import.item.title)?;
        let mut written_files = Vec::new();

        let attachments = attachments_for_import(&import);
        for attachment in &attachments {
            let file_path = write_attachment(&self.fs, &item_dir, attachment)?;
            written_files.push(file_path);
        }

        let metadata_path = item_dir.join("metadata.toml");
        let metadata = metadata_from_import(
            &item_id,
            &import,
            &attachments,
            &written_files,
        );
        let metadata_bytes = metadata.to_toml_string()?.into_bytes();
        self.fs.atomic_write(&metadata_path, &metadata_bytes)?;
        written_files.push(metadata_path);

        self.events.append(
            EventKind::MetadataWritten,
            "metadata written",
            Some(item_id.as_str().to_string()),
            Some(relative_to_root(self.fs.root(), &item_dir)),
        )?;
        self.events.append(
            EventKind::ItemRegistered,
            "item registered",
            Some(item_id.as_str().to_string()),
            Some(relative_to_root(self.fs.root(), &item_dir)),
        )?;
        self.events.append(
            EventKind::ImportFinished,
            "connector import finished",
            Some(item_id.as_str().to_string()),
            Some(relative_to_root(self.fs.root(), &item_dir)),
        )?;

        for category in &categories {
            let link_path =
                self.fs.create_category_link(category, &item_dir)?;
            self.events.append(
                EventKind::CatLinkCreated,
                format!("category link created: {}", category.as_str()),
                Some(item_id.as_str().to_string()),
                Some(relative_to_root(self.fs.root(), &link_path)),
            )?;
        }
        if !categories.is_empty() {
            self.events.append(
                EventKind::AutoClassifiedOnImport,
                format!("matched {} categor(ies)", categories.len()),
                Some(item_id.as_str().to_string()),
                Some(relative_to_root(self.fs.root(), &item_dir)),
            )?;
        }

        Ok(ImportOutcome { item_id, item_dir, written_files, categories })
    }

    /// Save one connector attachment into an existing imported item directory.
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
        self.events.append(
            EventKind::MetadataWritten,
            "late connector attachment saved",
            None,
            Some(relative_to_root(self.fs.root(), item_dir)),
        )?;
        Ok(path)
    }

    /// Write metadata only if the current file revision matches the expected value.
    ///
    /// On mismatch, the daemon candidate is saved as `metadata.daemon.toml` and
    /// the original `metadata.toml` is left untouched.
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
            self.events.append(
                EventKind::WriteConflict,
                "metadata revision conflict",
                Some(metadata.id.clone()),
                Some(relative_to_root(self.fs.root(), item_dir)),
            )?;
            return Err(LocalrefError::Conflict(format!(
                "metadata revision mismatch for {}",
                metadata.id
            )));
        }
        self.fs.atomic_write(&metadata_path, candidate.as_bytes())?;
        self.events.append(
            EventKind::MetadataWritten,
            "metadata written",
            Some(metadata.id.clone()),
            Some(relative_to_root(self.fs.root(), item_dir)),
        )?;
        Ok(())
    }

    /// Create metadata for an unmanaged existing directory under `All/`.
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
        self.events.append(
            EventKind::MetadataCreated,
            "metadata created for All directory",
            Some(item_id.as_str().to_string()),
            Some(relative_to_root(self.fs.root(), item_dir)),
        )?;
        if pdf_candidates(item_dir)?.len() > 1 {
            self.events.append(
                EventKind::MultipleMainPdfCandidates,
                "multiple PDF files found in manual All directory",
                Some(item_id.as_str().to_string()),
                Some(relative_to_root(self.fs.root(), item_dir)),
            )?;
        }
        Ok(ImportOutcome {
            item_id,
            item_dir: item_dir.to_path_buf(),
            written_files: vec![metadata_path],
            categories: Vec::new(),
        })
    }

    /// Import one file into a new `All/` item directory with minimal metadata.
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
        let metadata = metadata_from_imported_file(&item_id, title, &target)?;
        let metadata_path = item_dir.join("metadata.toml");
        let metadata_text = metadata.to_toml_string()?;
        self.fs.atomic_write(&metadata_path, metadata_text.as_bytes())?;
        self.events.append(
            EventKind::MetadataCreated,
            "metadata created for imported file",
            Some(item_id.as_str().to_string()),
            Some(relative_to_root(self.fs.root(), &item_dir)),
        )?;
        Ok(ImportOutcome {
            item_id,
            item_dir,
            written_files: vec![target, metadata_path],
            categories: Vec::new(),
        })
    }

    /// Copy one file into an existing item directory and update metadata files.
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
        self.events.append(
            EventKind::MetadataWritten,
            "item file added",
            None,
            Some(relative_to_root(self.fs.root(), item_dir)),
        )?;
        Ok(target)
    }

    /// Normalize a real directory under `Cat/` into `All/` plus a category link.
    pub fn normalize_cat_directory(
        &self,
        cat_dir: &Path,
        indexed_items: Vec<ItemDocument>,
    ) -> Result<ImportOutcome> {
        self.fs.ensure_layout()?;
        ensure_inside_cat(self.fs.root(), cat_dir)?;
        if !cat_dir.is_dir() {
            return Err(LocalrefError::MissingField("Cat directory"));
        }
        let category = category_from_cat_path(self.fs.root(), cat_dir)?;
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
            indexed_items.iter().find(|item| item.id == metadata.id).cloned()
        });
        let target = if existing.is_some() {
            None
        } else {
            Some(unique_all_dir_for_cat_entry(&self.fs, &entry_name)?)
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
                let metadata =
                    metadata_from_all_directory(&item_id, title, &target)?;
                let metadata_text = metadata.to_toml_string()?;
                let metadata_path = target.join("metadata.toml");
                self.fs
                    .atomic_write(&metadata_path, metadata_text.as_bytes())?;
                self.events.append(
                    EventKind::MetadataCreated,
                    "metadata created for Cat directory",
                    Some(item_id.as_str().to_string()),
                    Some(relative_to_root(self.fs.root(), &target)),
                )?;
                if pdf_candidates(&target)?.len() > 1 {
                    self.events.append(
                        EventKind::MultipleMainPdfCandidates,
                        "multiple PDF files found in manual Cat directory",
                        Some(item_id.as_str().to_string()),
                        Some(relative_to_root(self.fs.root(), &target)),
                    )?;
                }
            }
            target
        };

        let link_path = self.fs.create_category_link_named(
            &category,
            &entry_name,
            &item_dir,
        )?;
        self.events.append(
            EventKind::CatCopyReplacedByLink,
            "Cat real directory normalized",
            Some(item_id.as_str().to_string()),
            Some(relative_to_root(self.fs.root(), &link_path)),
        )?;
        self.events.append(
            EventKind::CatLinkCreated,
            format!("category link created: {}", category.as_str()),
            Some(item_id.as_str().to_string()),
            Some(relative_to_root(self.fs.root(), &link_path)),
        )?;

        Ok(ImportOutcome {
            item_id,
            item_dir,
            written_files: vec![link_path],
            categories: vec![category],
        })
    }

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
        creators: connector_creators(&import.item.raw),
        files: MetadataFiles { main, extra: attachment_files },
        tags: connector_tags(&import.item.raw),
        import: MetadataImport {
            source: "zotero-connector".to_string(),
            session_id: import.item.session_id.clone(),
            imported_at: None,
        },
        state: MetadataState::default(),
        raw_connector,
    }
}

fn automatic_categories(
    library_root: &Path,
    item_id: &ItemId,
    import: &ConnectorImport,
) -> Result<Vec<CategoryPath>> {
    let metadata = metadata_from_import(item_id, import, &[], &[]);
    RuleSet::load(library_root)?.match_metadata(&metadata)
}

fn connector_tags(raw: &serde_json::Value) -> MetadataTags {
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
    MetadataTags { items }
}

/// Extract Zotero creator records from raw connector item JSON.
fn connector_creators(raw: &serde_json::Value) -> Vec<Creator> {
    raw.get("creators")
        .and_then(serde_json::Value::as_array)
        .map(|creators| {
            creators.iter().filter_map(connector_creator).collect()
        })
        .unwrap_or_default()
}

/// Convert one Zotero creator object into Localref metadata.
fn connector_creator(value: &serde_json::Value) -> Option<Creator> {
    let role = json_string(value, &["creatorType", "role"])
        .unwrap_or_else(|| "author".to_string());
    let given = json_string(value, &["firstName", "given"]);
    let family = json_string(value, &["lastName", "family"]);
    let name = json_string(value, &["name"]);
    if given.is_none() && family.is_none() && name.is_none() {
        None
    } else {
        Some(Creator { role, given, family, name })
    }
}

/// Return the first non-empty string field from a JSON object.
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
        files: MetadataFiles { main, extra },
        tags: MetadataTags::default(),
        import: MetadataImport {
            source: "manual-all-directory".to_string(),
            session_id: None,
            imported_at: None,
        },
        state: MetadataState::default(),
        raw_connector: Default::default(),
    })
}

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

fn metadata_from_imported_file(
    item_id: &ItemId,
    title: &str,
    file_path: &Path,
) -> Result<Metadata> {
    let filename = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(LocalrefError::MissingField("file name"))?
        .to_string();
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
        files: MetadataFiles {
            main: Some(filename.clone()),
            extra: vec![MetadataFile {
                path: filename,
                kind: "attachment".to_string(),
                mime_type: mime_type_for_path(file_path),
            }],
        },
        tags: MetadataTags::default(),
        import: MetadataImport {
            source: "manual-file".to_string(),
            session_id: None,
            imported_at: None,
        },
        state: MetadataState::default(),
        raw_connector: Default::default(),
    })
}

fn direct_files(item_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(item_dir)
        .map_err(|source| LocalrefError::io(item_dir, source))?
    {
        let entry =
            entry.map_err(|source| LocalrefError::io(item_dir, source))?;
        let path = entry.path();
        if path.is_file()
            && path.file_name().and_then(|name| name.to_str())
                != Some("metadata.toml")
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

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

fn manual_item_id(title: &str) -> Result<ItemId> {
    let component = sanitize_ntfs_component(title)?;
    ItemId::new(format!("lr:manual:{component}"))
        .ok_or(LocalrefError::MissingField("manual item id"))
}

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

fn ensure_inside_cat(root: &Path, cat_dir: &Path) -> Result<()> {
    let cat_root = root
        .join("Cat")
        .canonicalize()
        .map_err(|source| LocalrefError::io(root.join("Cat"), source))?;
    let cat_dir = cat_dir
        .canonicalize()
        .map_err(|source| LocalrefError::io(cat_dir, source))?;
    if cat_dir == cat_root || !cat_dir.starts_with(&cat_root) {
        return Err(LocalrefError::InvalidPathComponent {
            component: cat_dir.display().to_string(),
            reason: "Cat normalization must target a directory under Cat/",
        });
    }
    Ok(())
}

fn category_from_cat_path(
    root: &Path,
    cat_dir: &Path,
) -> Result<CategoryPath> {
    let parent =
        cat_dir.parent().ok_or(LocalrefError::MissingField("Cat category"))?;
    let category = parent
        .strip_prefix(root.join("Cat"))
        .unwrap_or(parent)
        .to_string_lossy()
        .replace('\\', "/");
    CategoryPath::new(category).ok_or(LocalrefError::MissingField("category"))
}

fn unique_all_dir_for_cat_entry(
    fs: &LibraryFs,
    entry_name: &str,
) -> Result<PathBuf> {
    let base = sanitize_ntfs_component(entry_name)?;
    let mut candidate = fs.all_dir().join(&base);
    let mut suffix = 2_u32;
    while candidate.exists() {
        candidate = fs.all_dir().join(format!("{base} ({suffix})"));
        suffix += 1;
    }
    Ok(candidate)
}

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

fn relative_to_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn write_attachment(
    fs: &LibraryFs,
    item_dir: &std::path::Path,
    attachment: &ConnectorAttachment,
) -> Result<PathBuf> {
    let filename = sanitize_ntfs_component(&attachment.filename)?;
    let path = unique_file_path(item_dir, &filename);
    fs.atomic_write(&path, &attachment.bytes)?;
    Ok(path)
}

fn attachments_for_import(
    import: &ConnectorImport,
) -> Vec<ConnectorAttachment> {
    let mut attachments = import.attachments.clone();
    if import.item.item_type.as_deref() == Some("webpage")
        && !attachments.iter().any(|attachment| {
            attachment.filename.eq_ignore_ascii_case("source.url")
        })
        && let Some(uri) = &import.item.uri
    {
        attachments.push(ConnectorAttachment {
            session_id: import.item.session_id.clone(),
            parent_item_id: import.item.connector_item_id.clone(),
            title: Some("Source URL".to_string()),
            filename: "source.url".to_string(),
            mime_type: Some("text/uri-list".to_string()),
            bytes: windows_url_shortcut(&import.item.title, uri).into_bytes(),
            raw_metadata: None,
        });
    }
    attachments
}

fn windows_url_shortcut(title: &str, uri: &str) -> String {
    format!(
        "[InternetShortcut]\r\nURL={uri}\r\nIconIndex=0\r\nHotKey=0\r\nIDList=\r\nWorkingDirectory=\r\n"
    ) + &format!("Comment={title}\r\n")
}

fn unique_file_path(dir: &std::path::Path, filename: &str) -> PathBuf {
    let mut candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }

    let path = std::path::Path::new(filename);
    let stem =
        path.file_stem().and_then(|value| value.to_str()).unwrap_or(filename);
    let extension = path.extension().and_then(|value| value.to_str());

    for suffix in 2_u32.. {
        let name = match extension {
            Some(extension) => format!("{stem} ({suffix}).{extension}"),
            None => format!("{stem} ({suffix})"),
        };
        candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("unbounded suffix loop must return a candidate")
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EventKind;
    use crate::types::{ConnectorAttachment, ConnectorImport, ConnectorItem};
    use serde_json::json;

    #[test]
    fn imports_connector_item_and_attachment_to_all() {
        let temp = tempfile::tempdir().unwrap();
        let pipeline = ImportPipeline::new(temp.path());
        let import = ConnectorImport {
            item: ConnectorItem {
                session_id: Some("session-1".to_string()),
                uri: Some("https://example.test/paper".to_string()),
                connector_item_id: Some("abc123".to_string()),
                item_type: Some("journalArticle".to_string()),
                title: "A Test: Paper?".to_string(),
                abstract_note: Some("A useful abstract.".to_string()),
                doi: Some("10.1234/example".to_string()),
                raw: json!({
                    "title": "A Test: Paper?",
                    "creators": [
                        {
                            "creatorType": "author",
                            "firstName": "Ada",
                            "lastName": "Lovelace"
                        }
                    ]
                }),
            },
            attachments: vec![ConnectorAttachment {
                session_id: Some("session-1".to_string()),
                parent_item_id: Some("abc123".to_string()),
                title: Some("PDF".to_string()),
                filename: "paper?.pdf".to_string(),
                mime_type: Some("application/pdf".to_string()),
                bytes: b"pdf bytes".to_vec(),
                raw_metadata: Some(json!({"title": "paper.pdf"})),
            }],
        };

        let outcome = pipeline.import_connector_item(import).unwrap();

        assert_eq!(outcome.item_id.as_str(), "lr:zotero:abc123");
        assert!(outcome.item_dir.ends_with("A Test_ Paper_"));
        assert_eq!(
            std::fs::read(outcome.item_dir.join("paper_.pdf")).unwrap(),
            b"pdf bytes"
        );
        let metadata =
            std::fs::read_to_string(outcome.item_dir.join("metadata.toml"))
                .unwrap();
        assert!(metadata.contains("zotero-connector"));
        assert!(metadata.contains("A useful abstract."));
        assert!(metadata.contains("Ada"));
        assert!(metadata.contains("Lovelace"));
        assert!(metadata.contains("paper_.pdf"));
        assert!(metadata.contains("raw_json"));
    }

    #[test]
    fn import_metadata_preserves_raw_zotero_item_json() {
        let temp = tempfile::tempdir().unwrap();
        let pipeline = ImportPipeline::new(temp.path());
        let outcome = pipeline
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-case".to_string()),
                    uri: None,
                    connector_item_id: Some("case-1".to_string()),
                    item_type: Some("case".to_string()),
                    title: "Smith v. Jones".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({
                        "itemType": "case",
                        "caseName": "Smith v. Jones",
                        "court": "Example Court"
                    }),
                },
                attachments: Vec::new(),
            })
            .unwrap();

        let metadata =
            std::fs::read_to_string(outcome.item_dir.join("metadata.toml"))
                .unwrap();
        assert!(metadata.contains("raw_json"));
        assert!(metadata.contains("Smith v. Jones"));
        assert!(metadata.contains("Example Court"));
    }

    #[test]
    fn imports_item_without_attachments() {
        let temp = tempfile::tempdir().unwrap();
        let pipeline = ImportPipeline::new(temp.path());
        let outcome = pipeline
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-no-attachment".to_string()),
                    uri: Some(
                        "https://example.test/no-attachment".to_string(),
                    ),
                    connector_item_id: Some("no-attachment".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "No Attachment Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "No Attachment Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();

        assert!(outcome.item_dir.join("metadata.toml").exists());
        assert_eq!(outcome.written_files.len(), 1);
    }

    #[test]
    fn daemon_import_runs_through_queue_and_writes_events() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let outcome = daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-daemon-import".to_string()),
                    uri: Some("https://example.test/daemon".to_string()),
                    connector_item_id: Some("daemon-import".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Daemon Import Paper".to_string(),
                    abstract_note: Some("Queue visible import".to_string()),
                    doi: None,
                    raw: json!({"title": "Daemon Import Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();

        assert!(outcome.item_dir.join("metadata.toml").exists());
        assert_eq!(daemon.search("Queue visible").unwrap().len(), 1);
        assert_eq!(
            daemon.status().recent_tasks[0].task,
            DaemonTask::ImportConnector {
                title: "Daemon Import Paper".to_string()
            }
        );
        let events = daemon.events().unwrap();
        assert!(events.iter().any(|event| {
            event.kind == EventKind::ImportStarted
                && event.item_id.as_deref() == Some("lr:zotero:daemon-import")
        }));
        assert!(
            events.iter().any(|event| event.kind == EventKind::ImportFinished)
        );
    }

    #[test]
    fn connector_import_auto_classifies_with_rules() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".localref")).unwrap();
        std::fs::write(
            temp.path().join(".localref").join("rules.toml"),
            r#"
[[rules]]
name = "near-field"
target = "Wireless/RIS"
query = 'title:/near[- ]field/i OR abstract:channel'
"#,
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        let outcome = daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-auto-cat".to_string()),
                    uri: None,
                    connector_item_id: Some("auto-cat".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Near Field Channel Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Near Field Channel Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();

        assert_eq!(outcome.categories[0].as_str(), "Wireless/RIS");
        assert!(
            temp.path()
                .join("Cat")
                .join("Wireless")
                .join("RIS")
                .join("Near Field Channel Paper")
                .exists()
        );
        let events = daemon.events().unwrap();
        assert!(
            events.iter().any(|event| event.kind == EventKind::CatLinkCreated)
        );
        assert!(
            events
                .iter()
                .any(|event| event.kind == EventKind::AutoClassifiedOnImport)
        );
    }

    #[test]
    fn pending_import_can_be_confirmed_with_selected_categories() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".localref")).unwrap();
        std::fs::write(
            temp.path().join(".localref").join("rules.toml"),
            r#"
[[rules]]
name = "tagged"
target = "Suggested"
query = 'tags:ris'
"#,
        )
        .unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let import = ConnectorImport {
            item: ConnectorItem {
                session_id: Some("session-pending".to_string()),
                uri: None,
                connector_item_id: Some("pending".to_string()),
                item_type: Some("journalArticle".to_string()),
                title: "Pending Paper".to_string(),
                abstract_note: None,
                doi: None,
                raw: json!({
                    "title": "Pending Paper",
                    "tags": [{"tag": "RIS"}]
                }),
            },
            attachments: Vec::new(),
        };

        let session = daemon.create_pending_connector_import(import).unwrap();
        assert_eq!(session.suggested_categories[0].as_str(), "Suggested");
        assert_eq!(daemon.pending_imports().len(), 1);

        let outcome = daemon
            .confirm_pending_import(
                session.id,
                PendingImportConfirmation {
                    categories: Some(vec![
                        CategoryPath::new("User/Selected").unwrap(),
                    ]),
                },
            )
            .unwrap();

        assert_eq!(daemon.pending_imports().len(), 0);
        assert_eq!(outcome.categories[0].as_str(), "User/Selected");
        assert!(
            temp.path()
                .join("Cat")
                .join("User")
                .join("Selected")
                .join("Pending Paper")
                .exists()
        );
    }

    #[test]
    fn daemon_patches_metadata_with_revision() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        let outcome = daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-patch".to_string()),
                    uri: None,
                    connector_item_id: Some("patch".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Patch Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Patch Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        let item = daemon.get_item("lr:zotero:patch").unwrap().unwrap();
        let metadata_text =
            std::fs::read_to_string(outcome.item_dir.join("metadata.toml"))
                .unwrap();
        let mut metadata = Metadata::from_toml_str(&metadata_text).unwrap();
        metadata.title = "Patched Paper".to_string();

        let patched = daemon
            .patch_metadata(
                "lr:zotero:patch",
                &item.metadata_revision,
                metadata,
            )
            .unwrap();

        assert_eq!(patched.title, "Patched Paper");
        assert_ne!(patched.metadata_revision, item.metadata_revision);
        assert_eq!(daemon.search("Patched").unwrap()[0].id, "lr:zotero:patch");
    }

    #[test]
    fn daemon_imports_existing_all_directory_with_single_pdf_main() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Manual Paper");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(item_dir.join("paper.pdf"), b"pdf").unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        let outcome = daemon.import_all_directory("All/Manual Paper").unwrap();

        assert_eq!(outcome.item_id.as_str(), "lr:manual:Manual Paper");
        let metadata =
            std::fs::read_to_string(item_dir.join("metadata.toml")).unwrap();
        assert!(metadata.contains("manual-all-directory"));
        assert!(metadata.contains("main = \"paper.pdf\""));
        assert_eq!(daemon.search("Manual Paper").unwrap().len(), 1);
        assert!(
            daemon
                .events()
                .unwrap()
                .iter()
                .any(|event| event.kind == EventKind::MetadataCreated)
        );
    }

    #[test]
    fn daemon_imports_explicit_file() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.pdf");
        std::fs::write(&source, b"pdf").unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        let outcome = daemon.import_file(&source).unwrap();

        assert_eq!(outcome.item_id.as_str(), "lr:manual:source");
        assert_eq!(
            std::fs::read(outcome.item_dir.join("source.pdf")).unwrap(),
            b"pdf"
        );
        let item = daemon.get_item("lr:manual:source").unwrap().unwrap();
        assert_eq!(item.main_file.as_deref(), Some("source.pdf"));
    }

    #[test]
    fn daemon_normalizes_real_cat_directory_into_all_and_link() {
        let temp = tempfile::tempdir().unwrap();
        let cat_dir = temp.path().join("Cat").join("Wireless").join("Copied");
        std::fs::create_dir_all(&cat_dir).unwrap();
        std::fs::write(
            cat_dir.join("metadata.toml"),
            r#"
id = "lr:manual:Copied"
type = "document"
title = "Copied"
"#,
        )
        .unwrap();
        std::fs::write(cat_dir.join("paper.pdf"), b"pdf").unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        let outcome =
            daemon.normalize_cat_directory("Cat/Wireless/Copied").unwrap();

        assert!(outcome.item_dir.ends_with("All/Copied"));
        assert!(outcome.item_dir.join("metadata.toml").exists());
        assert!(
            temp.path().join("Cat").join("Wireless").join("Copied").exists()
        );
        assert_eq!(
            daemon.search("Wireless").unwrap()[0].id,
            "lr:manual:Copied"
        );
        assert!(
            daemon
                .events()
                .unwrap()
                .iter()
                .any(|event| event.kind == EventKind::CatCopyReplacedByLink)
        );
    }

    #[test]
    fn daemon_adds_removes_renames_and_merges_categories() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
        daemon
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-category".to_string()),
                    uri: None,
                    connector_item_id: Some("category".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Category Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Category Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();

        let added = daemon
            .add_item_category(
                "lr:zotero:category",
                CategoryPath::new("Wireless/RIS").unwrap(),
            )
            .unwrap();
        assert_eq!(added.path, "Wireless/RIS");
        assert_eq!(added.item_ids, vec!["lr:zotero:category"]);

        let renamed = daemon
            .rename_category(
                CategoryPath::new("Wireless/RIS").unwrap(),
                CategoryPath::new("Wireless/NearField").unwrap(),
            )
            .unwrap();
        assert_eq!(renamed.path, "Wireless/NearField");

        let merged = daemon
            .merge_category(
                CategoryPath::new("Wireless/NearField").unwrap(),
                CategoryPath::new("Archive").unwrap(),
            )
            .unwrap();
        assert_eq!(merged.path, "Archive");

        let removed = daemon
            .remove_item_category(
                "lr:zotero:category",
                CategoryPath::new("Archive").unwrap(),
            )
            .unwrap();
        assert!(removed.item_ids.is_empty());
    }

    #[test]
    fn daemon_creates_empty_category_directory() {
        let temp = tempfile::tempdir().unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        let summary = daemon
            .create_category(CategoryPath::new("Inbox/New").unwrap())
            .unwrap();

        assert_eq!(summary.path, "Inbox/New");
        assert!(summary.item_ids.is_empty());
        assert!(temp.path().join("Cat").join("Inbox").join("New").is_dir());
        assert!(
            daemon
                .events()
                .unwrap()
                .iter()
                .any(|event| event.kind == EventKind::CategoryCreated)
        );
    }

    #[test]
    fn import_lock_conflict_returns_error_and_logs_event() {
        let temp = tempfile::tempdir().unwrap();
        let lock_dir = temp.path().join(".localref").join("locks");
        std::fs::create_dir_all(&lock_dir).unwrap();
        std::fs::write(lock_dir.join("lr_zotero_locked.lock"), "busy")
            .unwrap();
        let pipeline = ImportPipeline::new(temp.path());

        let error = pipeline
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-locked".to_string()),
                    uri: None,
                    connector_item_id: Some("locked".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Locked Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Locked Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap_err();

        assert!(matches!(error, LocalrefError::Conflict(_)));
        let events = EventLog::new(temp.path()).list().unwrap();
        assert!(
            events.iter().any(|event| event.kind == EventKind::WriteConflict)
        );
    }

    #[test]
    fn metadata_revision_conflict_preserves_external_file() {
        let temp = tempfile::tempdir().unwrap();
        let pipeline = ImportPipeline::new(temp.path());
        let outcome = pipeline
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-conflict".to_string()),
                    uri: None,
                    connector_item_id: Some("conflict".to_string()),
                    item_type: Some("journalArticle".to_string()),
                    title: "Conflict Paper".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Conflict Paper"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();
        let metadata_path = outcome.item_dir.join("metadata.toml");
        let original = std::fs::read_to_string(&metadata_path).unwrap();
        let original_revision = Metadata::revision_for_text(&original);
        std::fs::write(
            &metadata_path,
            original.replace("Conflict Paper", "Externally Edited Paper"),
        )
        .unwrap();
        let mut candidate = Metadata::from_toml_str(&original).unwrap();
        candidate.title = "Daemon Candidate Paper".to_string();

        let error = pipeline
            .write_metadata_if_revision(
                &outcome.item_dir,
                &candidate,
                &original_revision,
            )
            .unwrap_err();

        assert!(matches!(error, LocalrefError::Conflict(_)));
        let current = std::fs::read_to_string(&metadata_path).unwrap();
        assert!(current.contains("Externally Edited Paper"));
        assert!(
            std::fs::read_to_string(
                outcome.item_dir.join("metadata.daemon.toml")
            )
            .unwrap()
            .contains("Daemon Candidate Paper")
        );
    }

    #[test]
    fn webpage_import_creates_source_url_attachment() {
        let temp = tempfile::tempdir().unwrap();
        let pipeline = ImportPipeline::new(temp.path());
        let outcome = pipeline
            .import_connector_item(ConnectorImport {
                item: ConnectorItem {
                    session_id: Some("session-webpage".to_string()),
                    uri: Some("https://example.test/page".to_string()),
                    connector_item_id: Some("webpage".to_string()),
                    item_type: Some("webpage".to_string()),
                    title: "Example Webpage".to_string(),
                    abstract_note: None,
                    doi: None,
                    raw: json!({"title": "Example Webpage"}),
                },
                attachments: Vec::new(),
            })
            .unwrap();

        let shortcut =
            std::fs::read_to_string(outcome.item_dir.join("source.url"))
                .unwrap();
        assert!(shortcut.contains("[InternetShortcut]"));
        assert!(shortcut.contains("URL=https://example.test/page"));
        let metadata =
            std::fs::read_to_string(outcome.item_dir.join("metadata.toml"))
                .unwrap();
        assert!(metadata.contains("source.url"));
    }

    #[test]
    fn daemon_scan_task_indexes_storage_through_queue() {
        let temp = tempfile::tempdir().unwrap();
        let item_dir = temp.path().join("All").join("Paper One");
        std::fs::create_dir_all(&item_dir).unwrap();
        std::fs::write(
            item_dir.join("metadata.toml"),
            r#"
id = "lr:test:queue"
type = "journalArticle"
title = "Queue Indexed Paper"
abstract = "Queue visible abstract"
"#,
        )
        .unwrap();

        let storage = StorageDb::open(temp.path()).unwrap();
        let daemon = LocalrefDaemon::new(storage);
        let task = daemon.scan_all().unwrap();

        assert_eq!(task.state, DaemonTaskState::Completed);
        assert_eq!(task.indexed_items, Some(1));
        assert_eq!(daemon.status().recent_tasks.len(), 1);
        assert_eq!(daemon.search("visible").unwrap()[0].id, "lr:test:queue");
    }

    #[test]
    fn daemon_scan_imports_unmanaged_cat_item_folder() {
        let temp = tempfile::tempdir().unwrap();
        let cat_dir = temp.path().join("Cat").join("Inbox").join("Copied");
        std::fs::create_dir_all(&cat_dir).unwrap();
        std::fs::write(cat_dir.join("paper.pdf"), b"pdf").unwrap();
        let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

        let task = daemon.scan_all().unwrap();

        assert_eq!(task.state, DaemonTaskState::Completed);
        assert!(
            temp.path().join("All").join("Copied").join("paper.pdf").exists()
        );
        assert!(
            temp.path()
                .join("All")
                .join("Copied")
                .join("metadata.toml")
                .exists()
        );
        assert!(temp.path().join("Cat").join("Inbox").join("Copied").exists());
        let item = daemon.get_item("lr:manual:Copied").unwrap().unwrap();
        assert_eq!(item.categories, vec!["Inbox"]);
        assert!(
            daemon.events().unwrap().iter().any(|event| {
                event.kind == EventKind::CatCopyReplacedByLink
            })
        );
    }

    #[test]
    fn daemon_pause_blocks_scan_tasks() {
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageDb::open(temp.path()).unwrap();
        let daemon = LocalrefDaemon::new(storage);

        let status = daemon.pause(PauseMode::Indexing);
        assert_eq!(status.paused_modes, vec![PauseMode::Indexing]);
        assert!(daemon.scan_all().is_err());

        let status = daemon.resume(PauseMode::Indexing);
        assert!(status.paused_modes.is_empty());
        assert!(daemon.scan_all().is_ok());
    }
}
