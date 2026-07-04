//! UniFFI DTOs and `From<core::…>` conversions for the Avalonia boundary.
//!
//! `localref-core` stays free of any UniFFI dependency, so every type that
//! crosses to C# is redeclared here as a `uniffi::Record`/`uniffi::Enum` and
//! converted field-for-field. The awkward payloads follow the plan: `PathBuf`
//! crosses as `String`, `serde_json::Value` as a JSON `String`, `BTreeMap` as a
//! `HashMap`, and validated newtypes (`ItemId`, `CategoryPath`) as `String`
//! validated inside the wrapper.

use std::collections::HashMap;

use localref_core::logging::LogEntry as CoreLogEntry;
use localref_core::model::{
    CategorySummary as CoreCategorySummary, Creator as CoreCreator,
    ItemDocument as CoreItemDocument, ItemFileEntry as CoreItemFileEntry,
    ItemFilesDocument as CoreItemFilesDocument, Metadata as CoreMetadata,
    MetadataDocument as CoreMetadataDocument,
    MetadataFile as CoreMetadataFile, MetadataFiles as CoreMetadataFiles,
    MetadataImport as CoreMetadataImport, MetadataState as CoreMetadataState,
    MetadataTags as CoreMetadataTags, ScheduledCall as CoreScheduledCall,
    SearchHit as CoreSearchHit,
};
use localref_core::{
    DaemonEvent as CoreDaemonEvent, DaemonStatus as CoreDaemonStatus,
    PauseMode as CorePauseMode,
};
use localref_plugin::manifest::{
    FieldKind as CoreFieldKind, PluginUiSpec as CorePluginUiSpec,
    PreviewSpec as CorePreviewSpec, UiAction as CoreUiAction,
    UiDataRequirement as CoreUiDataRequirement, UiDisplay as CoreUiDisplay,
    UiField as CoreUiField, UiMount as CoreUiMount, UiPage as CoreUiPage,
    UiTarget as CoreUiTarget,
};

/// One indexed library item, as shown in the item list and detail views.
#[derive(Debug, uniffi::Record)]
pub struct ItemDocument {
    pub id: String,
    pub object_path: String,
    pub metadata_revision: String,
    pub title: String,
    pub authors: Vec<String>,
    pub abstract_note: Option<String>,
    pub item_type: String,
    pub doi: Option<String>,
    pub uri: Option<String>,
    pub main_file: Option<String>,
    pub extra_files: Vec<String>,
    pub tags: Vec<String>,
    pub venue: Option<String>,
    pub year: Option<i32>,
    pub categories: Vec<String>,
    /// Plugin-owned per-item data, keyed by namespace then field key.
    pub extra: HashMap<String, HashMap<String, String>>,
}

impl From<CoreItemDocument> for ItemDocument {
    fn from(value: CoreItemDocument) -> Self {
        Self {
            id: value.id,
            object_path: value.object_path,
            metadata_revision: value.metadata_revision,
            title: value.title,
            authors: value.authors,
            abstract_note: value.abstract_note,
            item_type: value.item_type,
            doi: value.doi,
            uri: value.uri,
            main_file: value.main_file,
            extra_files: value.extra_files,
            tags: value.tags,
            venue: value.venue,
            year: value.year,
            categories: value.categories,
            extra: nested_map_to_ffi(value.extra),
        }
    }
}

/// Convert a core nested `BTreeMap` to the FFI `HashMap` shape.
fn nested_map_to_ffi(
    value: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, String>,
    >,
) -> HashMap<String, HashMap<String, String>> {
    value
        .into_iter()
        .map(|(namespace, fields)| {
            (namespace, fields.into_iter().collect())
        })
        .collect()
}

/// Convert an FFI nested `HashMap` back to the core `BTreeMap` shape.
fn nested_map_from_ffi(
    value: HashMap<String, HashMap<String, String>>,
) -> std::collections::BTreeMap<
    String,
    std::collections::BTreeMap<String, String>,
> {
    value
        .into_iter()
        .map(|(namespace, fields)| {
            (namespace, fields.into_iter().collect())
        })
        .collect()
}

/// A filesystem entry inside an item directory.
#[derive(Debug, uniffi::Record)]
pub struct ItemFileEntry {
    pub path: String,
    pub kind: String,
    pub bytes: Option<u64>,
}

impl From<CoreItemFileEntry> for ItemFileEntry {
    fn from(value: CoreItemFileEntry) -> Self {
        Self { path: value.path, kind: value.kind, bytes: value.bytes }
    }
}

/// The files currently present under one item directory.
#[derive(Debug, uniffi::Record)]
pub struct ItemFilesDocument {
    pub item_id: String,
    pub object_path: String,
    pub files: Vec<ItemFileEntry>,
}

impl From<CoreItemFilesDocument> for ItemFilesDocument {
    fn from(value: CoreItemFilesDocument) -> Self {
        Self {
            item_id: value.item_id,
            object_path: value.object_path,
            files: value.files.into_iter().map(Into::into).collect(),
        }
    }
}

/// A creator (author/editor) attached to an item's metadata.
#[derive(Debug, uniffi::Record)]
pub struct Creator {
    pub role: String,
    pub given: Option<String>,
    pub family: Option<String>,
    pub name: Option<String>,
}

impl From<CoreCreator> for Creator {
    fn from(value: CoreCreator) -> Self {
        Self {
            role: value.role,
            given: value.given,
            family: value.family,
            name: value.name,
        }
    }
}

impl From<Creator> for CoreCreator {
    fn from(value: Creator) -> Self {
        Self {
            role: value.role,
            given: value.given,
            family: value.family,
            name: value.name,
        }
    }
}

/// One file recorded in an item's metadata.
#[derive(Debug, uniffi::Record)]
pub struct MetadataFile {
    pub path: String,
    pub kind: String,
    pub mime_type: Option<String>,
}

impl From<CoreMetadataFile> for MetadataFile {
    fn from(value: CoreMetadataFile) -> Self {
        Self { path: value.path, kind: value.kind, mime_type: value.mime_type }
    }
}

impl From<MetadataFile> for CoreMetadataFile {
    fn from(value: MetadataFile) -> Self {
        Self { path: value.path, kind: value.kind, mime_type: value.mime_type }
    }
}

/// Main + extra files recorded in an item's metadata.
#[derive(Debug, uniffi::Record)]
pub struct MetadataFiles {
    pub main: Option<String>,
    pub extra: Vec<MetadataFile>,
}

impl From<CoreMetadataFiles> for MetadataFiles {
    fn from(value: CoreMetadataFiles) -> Self {
        Self {
            main: value.main,
            extra: value.extra.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<MetadataFiles> for CoreMetadataFiles {
    fn from(value: MetadataFiles) -> Self {
        Self {
            main: value.main,
            extra: value.extra.into_iter().map(Into::into).collect(),
        }
    }
}

/// Tag names recorded on an item.
#[derive(Debug, uniffi::Record)]
pub struct MetadataTags {
    pub items: Vec<String>,
}

impl From<CoreMetadataTags> for MetadataTags {
    fn from(value: CoreMetadataTags) -> Self {
        Self { items: value.items }
    }
}

impl From<MetadataTags> for CoreMetadataTags {
    fn from(value: MetadataTags) -> Self {
        Self { items: value.items }
    }
}

/// Import provenance recorded on an item.
#[derive(Debug, uniffi::Record)]
pub struct MetadataImport {
    pub source: String,
    pub session_id: Option<String>,
    pub imported_at: Option<String>,
}

impl From<CoreMetadataImport> for MetadataImport {
    fn from(value: CoreMetadataImport) -> Self {
        Self {
            source: value.source,
            session_id: value.session_id,
            imported_at: value.imported_at,
        }
    }
}

impl From<MetadataImport> for CoreMetadataImport {
    fn from(value: MetadataImport) -> Self {
        Self {
            source: value.source,
            session_id: value.session_id,
            imported_at: value.imported_at,
        }
    }
}

/// Item state flags recorded in metadata.
#[derive(Debug, uniffi::Record)]
pub struct MetadataState {
    pub missing_main_file: bool,
    /// Categories the user explicitly removed (tombstones).
    pub removed_categories: Vec<String>,
}

impl From<CoreMetadataState> for MetadataState {
    fn from(value: CoreMetadataState) -> Self {
        Self {
            missing_main_file: value.missing_main_file,
            removed_categories: value.removed_categories,
        }
    }
}

impl From<MetadataState> for CoreMetadataState {
    fn from(value: MetadataState) -> Self {
        Self {
            missing_main_file: value.missing_main_file,
            removed_categories: value.removed_categories,
        }
    }
}

/// Full item metadata edited in the detail view.
///
/// `raw_connector` is opaque connector passthrough surfaced as a string map.
#[derive(Debug, uniffi::Record)]
pub struct Metadata {
    pub id: String,
    pub item_type: String,
    pub title: String,
    pub abstract_note: Option<String>,
    pub doi: Option<String>,
    pub uri: Option<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub language: Option<String>,
    pub creators: Vec<Creator>,
    pub categories: Vec<String>,
    pub files: MetadataFiles,
    pub tags: MetadataTags,
    pub import: MetadataImport,
    pub state: MetadataState,
    pub raw_connector: HashMap<String, String>,
    /// Plugin-owned per-item data, keyed by namespace then field key.
    pub extra: HashMap<String, HashMap<String, String>>,
}

impl From<CoreMetadata> for Metadata {
    fn from(value: CoreMetadata) -> Self {
        Self {
            id: value.id,
            item_type: value.item_type,
            title: value.title,
            abstract_note: value.abstract_note,
            doi: value.doi,
            uri: value.uri,
            year: value.year,
            venue: value.venue,
            language: value.language,
            creators: value.creators.into_iter().map(Into::into).collect(),
            categories: value.categories,
            files: value.files.into(),
            tags: value.tags.into(),
            import: value.import.into(),
            state: value.state.into(),
            raw_connector: value.raw_connector.into_iter().collect(),
            extra: nested_map_to_ffi(value.extra),
        }
    }
}

impl From<Metadata> for CoreMetadata {
    fn from(value: Metadata) -> Self {
        Self {
            id: value.id,
            item_type: value.item_type,
            title: value.title,
            abstract_note: value.abstract_note,
            doi: value.doi,
            uri: value.uri,
            year: value.year,
            venue: value.venue,
            language: value.language,
            creators: value.creators.into_iter().map(Into::into).collect(),
            categories: value.categories,
            files: value.files.into(),
            tags: value.tags.into(),
            import: value.import.into(),
            state: value.state.into(),
            raw_connector: value.raw_connector.into_iter().collect(),
            extra: nested_map_from_ffi(value.extra),
        }
    }
}

/// Full metadata paired with the revision hash for optimistic concurrency.
#[derive(Debug, uniffi::Record)]
pub struct MetadataDocument {
    pub item_id: String,
    pub metadata_revision: String,
    pub metadata: Metadata,
}

impl From<CoreMetadataDocument> for MetadataDocument {
    fn from(value: CoreMetadataDocument) -> Self {
        Self {
            item_id: value.item_id,
            metadata_revision: value.metadata_revision,
            metadata: value.metadata.into(),
        }
    }
}

/// A search result row.
#[derive(Debug, uniffi::Record)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub object_path: String,
    pub doi: Option<String>,
    pub abstract_note: Option<String>,
}

impl From<CoreSearchHit> for SearchHit {
    fn from(value: CoreSearchHit) -> Self {
        Self {
            id: value.id,
            title: value.title,
            authors: value.authors,
            object_path: value.object_path,
            doi: value.doi,
            abstract_note: value.abstract_note,
        }
    }
}

/// A category and the ids of the items linked under it.
#[derive(Debug, uniffi::Record)]
pub struct CategorySummary {
    pub path: String,
    pub item_ids: Vec<String>,
}

impl From<CoreCategorySummary> for CategorySummary {
    fn from(value: CoreCategorySummary) -> Self {
        Self { path: value.path, item_ids: value.item_ids }
    }
}

/// A runtime-registered scheduled plugin call.
#[derive(Debug, uniffi::Record)]
pub struct ScheduledCall {
    pub id: String,
    pub plugin: String,
    pub action: String,
    pub params: HashMap<String, String>,
    pub schedule: String,
}

impl From<CoreScheduledCall> for ScheduledCall {
    fn from(value: CoreScheduledCall) -> Self {
        Self {
            id: value.id,
            plugin: value.plugin,
            action: value.action,
            params: value.params.into_iter().collect(),
            schedule: value.schedule,
        }
    }
}

impl From<ScheduledCall> for CoreScheduledCall {
    fn from(value: ScheduledCall) -> Self {
        Self {
            id: value.id,
            plugin: value.plugin,
            action: value.action,
            params: value.params.into_iter().collect(),
            schedule: value.schedule,
        }
    }
}

/// One log ring-buffer entry surfaced to the logs pane.
#[derive(Debug, uniffi::Record)]
pub struct LogEntry {
    pub id: u64,
    pub ts: String,
    pub level: String,
    pub target: String,
    pub message: String,
    pub event_kind: Option<String>,
    pub item_id: Option<String>,
    pub path: Option<String>,
}

impl From<CoreLogEntry> for LogEntry {
    fn from(value: CoreLogEntry) -> Self {
        Self {
            id: value.id,
            ts: value.ts,
            level: value.level,
            target: value.target,
            message: value.message,
            event_kind: value.event_kind,
            item_id: value.item_id,
            path: value.path,
        }
    }
}

/// Daemon pause mode, mirrored for FFI.
#[derive(Debug, uniffi::Enum)]
pub enum PauseMode {
    All,
    Writes,
    Watcher,
    Indexing,
}

impl From<PauseMode> for CorePauseMode {
    fn from(value: PauseMode) -> Self {
        match value {
            PauseMode::All => CorePauseMode::All,
            PauseMode::Writes => CorePauseMode::Writes,
            PauseMode::Watcher => CorePauseMode::Watcher,
            PauseMode::Indexing => CorePauseMode::Indexing,
        }
    }
}

impl From<CorePauseMode> for PauseMode {
    fn from(value: CorePauseMode) -> Self {
        match value {
            CorePauseMode::All => PauseMode::All,
            CorePauseMode::Writes => PauseMode::Writes,
            CorePauseMode::Watcher => PauseMode::Watcher,
            CorePauseMode::Indexing => PauseMode::Indexing,
        }
    }
}

/// Current daemon queue status.
///
/// `recent_tasks` are rendered as preformatted debug strings; the UI only shows
/// them as a log-like list, so the structured task enum is not marshalled.
#[derive(Debug, uniffi::Record)]
pub struct DaemonStatus {
    pub running: bool,
    pub queued_tasks: u32,
    pub recent_tasks: Vec<String>,
    pub paused_modes: Vec<PauseMode>,
}

impl From<CoreDaemonStatus> for DaemonStatus {
    fn from(value: CoreDaemonStatus) -> Self {
        Self {
            running: value.running,
            queued_tasks: u32::try_from(value.queued_tasks)
                .unwrap_or(u32::MAX),
            recent_tasks: value
                .recent_tasks
                .iter()
                .map(|record| format!("{record:?}"))
                .collect(),
            paused_modes: value
                .paused_modes
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

/// A completed library mutation pushed to the UI for live refresh.
#[derive(Debug, uniffi::Enum)]
pub enum DaemonEvent {
    ItemImported { item_id: String },
    ItemDeleted { item_id: String },
    MetadataPatched { item_id: String },
    CategoryChanged { item_id: Option<String>, category: Option<String> },
    ScanCompleted { indexed_items: u64 },
    ItemFileAdded { item_id: String },
    RulesChanged,
    SchedulesChanged,
    DaemonPaused,
    DaemonResumed,
}

impl From<CoreDaemonEvent> for DaemonEvent {
    fn from(value: CoreDaemonEvent) -> Self {
        match value {
            CoreDaemonEvent::ItemImported { item_id } => {
                DaemonEvent::ItemImported { item_id }
            }
            CoreDaemonEvent::ItemDeleted { item_id } => {
                DaemonEvent::ItemDeleted { item_id }
            }
            CoreDaemonEvent::MetadataPatched { item_id } => {
                DaemonEvent::MetadataPatched { item_id }
            }
            CoreDaemonEvent::CategoryChanged { item_id, category } => {
                DaemonEvent::CategoryChanged { item_id, category }
            }
            CoreDaemonEvent::ScanCompleted { indexed_items } => {
                DaemonEvent::ScanCompleted {
                    indexed_items: indexed_items as u64,
                }
            }
            CoreDaemonEvent::ItemFileAdded { item_id } => {
                DaemonEvent::ItemFileAdded { item_id }
            }
            CoreDaemonEvent::RulesChanged => DaemonEvent::RulesChanged,
            CoreDaemonEvent::SchedulesChanged => DaemonEvent::SchedulesChanged,
            CoreDaemonEvent::DaemonPaused => DaemonEvent::DaemonPaused,
            CoreDaemonEvent::DaemonResumed => DaemonEvent::DaemonResumed,
        }
    }
}

/// Field control kind the UI renders for a plugin form field.
#[derive(Debug, uniffi::Enum)]
pub enum FieldKind {
    Text,
    Textarea,
    Number,
    Checkbox,
    Select,
    Radio,
}

impl From<CoreFieldKind> for FieldKind {
    fn from(value: CoreFieldKind) -> Self {
        match value {
            CoreFieldKind::Text => FieldKind::Text,
            CoreFieldKind::Textarea => FieldKind::Textarea,
            CoreFieldKind::Number => FieldKind::Number,
            CoreFieldKind::Checkbox => FieldKind::Checkbox,
            CoreFieldKind::Select => FieldKind::Select,
            CoreFieldKind::Radio => FieldKind::Radio,
        }
    }
}

/// Where a plugin action or page mounts in the UI.
#[derive(Debug, uniffi::Enum)]
pub enum UiMount {
    ActionButton,
    ContextMenu,
    DetailTab,
    MetadataPage,
    SelectionPage,
}

impl From<CoreUiMount> for UiMount {
    fn from(value: CoreUiMount) -> Self {
        match value {
            CoreUiMount::ActionButton => UiMount::ActionButton,
            CoreUiMount::ContextMenu => UiMount::ContextMenu,
            CoreUiMount::DetailTab => UiMount::DetailTab,
            CoreUiMount::MetadataPage => UiMount::MetadataPage,
            CoreUiMount::SelectionPage => UiMount::SelectionPage,
        }
    }
}

/// Which item ids the host passes to a spawned plugin action.
#[derive(Debug, uniffi::Enum)]
pub enum UiTarget {
    Selection,
    Active,
    None,
}

/// Host data a plugin page requires; used to infer its contextual surface.
#[derive(Debug, uniffi::Enum)]
pub enum UiDataRequirement {
    Library,
    Selection,
    ActiveItem,
    ItemMetadata,
    ItemFiles,
    ItemCategories,
    ImportedItem,
}

impl From<CoreUiDataRequirement> for UiDataRequirement {
    fn from(value: CoreUiDataRequirement) -> Self {
        match value {
            CoreUiDataRequirement::Library => Self::Library,
            CoreUiDataRequirement::Selection => Self::Selection,
            CoreUiDataRequirement::ActiveItem => Self::ActiveItem,
            CoreUiDataRequirement::ItemMetadata => Self::ItemMetadata,
            CoreUiDataRequirement::ItemFiles => Self::ItemFiles,
            CoreUiDataRequirement::ItemCategories => Self::ItemCategories,
            CoreUiDataRequirement::ImportedItem => Self::ImportedItem,
        }
    }
}

impl From<CoreUiTarget> for UiTarget {
    fn from(value: CoreUiTarget) -> Self {
        match value {
            CoreUiTarget::Selection => UiTarget::Selection,
            CoreUiTarget::Active => UiTarget::Active,
            CoreUiTarget::None => UiTarget::None,
        }
    }
}

/// A plugin action button / context-menu entry.
#[derive(Debug, uniffi::Record)]
pub struct UiAction {
    pub id: String,
    pub label: String,
    pub mount: UiMount,
    pub target: UiTarget,
}

impl From<CoreUiAction> for UiAction {
    fn from(value: CoreUiAction) -> Self {
        Self {
            id: value.id,
            label: value.label,
            mount: value.mount.into(),
            target: value.target.into(),
        }
    }
}

/// A single plugin form field the UI renders natively.
#[derive(Debug, uniffi::Record)]
pub struct UiField {
    pub name: String,
    pub label: String,
    pub kind: FieldKind,
    pub options: Vec<String>,
    pub default: Option<String>,
    pub required: bool,
    pub show_if: Option<String>,
    pub enabled_if: Option<String>,
}

impl From<CoreUiField> for UiField {
    fn from(value: CoreUiField) -> Self {
        Self {
            name: value.name,
            label: value.label,
            kind: value.kind.into(),
            options: value.options,
            default: value.default,
            required: value.required,
            show_if: value.show_if,
            enabled_if: value.enabled_if,
        }
    }
}

/// A live-updating readout on a plugin page (Tier-1 binding target).
#[derive(Debug, uniffi::Record)]
pub struct UiDisplay {
    pub id: String,
    pub text: String,
}

impl From<CoreUiDisplay> for UiDisplay {
    fn from(value: CoreUiDisplay) -> Self {
        Self { id: value.id, text: value.text }
    }
}

/// A debounced live-preview callback declared on a plugin page.
#[derive(Debug, uniffi::Record)]
pub struct PreviewSpec {
    pub action: String,
    pub debounce_ms: u64,
    pub into: String,
}

impl From<CorePreviewSpec> for PreviewSpec {
    fn from(value: CorePreviewSpec) -> Self {
        Self {
            action: value.action,
            debounce_ms: value.debounce_ms,
            into: value.into,
        }
    }
}

/// A plugin form page mounted at a UI slot.
#[derive(Debug, uniffi::Record)]
pub struct UiPage {
    pub id: String,
    pub label: String,
    pub mount: UiMount,
    pub route: String,
    pub action: Option<String>,
    pub target: UiTarget,
    pub requires: Vec<UiDataRequirement>,
    pub preview: Option<PreviewSpec>,
    pub fields: Vec<UiField>,
    pub display: Vec<UiDisplay>,
}

impl From<CoreUiPage> for UiPage {
    fn from(value: CoreUiPage) -> Self {
        Self {
            id: value.id,
            label: value.label,
            mount: value.mount.into(),
            route: value.route,
            action: value.action,
            target: value.target.into(),
            requires: value.requires.into_iter().map(Into::into).collect(),
            preview: value.preview.map(Into::into),
            fields: value.fields.into_iter().map(Into::into).collect(),
            display: value.display.into_iter().map(Into::into).collect(),
        }
    }
}

/// A plugin's declarative UI spec (from `ui.toml`), rendered by the UI.
#[derive(Debug, uniffi::Record)]
pub struct PluginUiSpec {
    pub actions: Vec<UiAction>,
    pub pages: Vec<UiPage>,
}

impl From<CorePluginUiSpec> for PluginUiSpec {
    fn from(value: CorePluginUiSpec) -> Self {
        Self {
            actions: value.actions.into_iter().map(Into::into).collect(),
            pages: value.pages.into_iter().map(Into::into).collect(),
        }
    }
}
