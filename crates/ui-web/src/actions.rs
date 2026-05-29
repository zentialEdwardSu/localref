//! Form action handlers for the server-rendered Localref UI.

use std::path::{Path, PathBuf};

use localref_core::types::CategoryPath;
use localref_core::{LocalrefDaemon, PauseMode};
use serde::Deserialize;

/// Form payload posted by UI controls.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct UiAction {
    pub(crate) action: String,
    pub(crate) return_to: String,
    pub(crate) item_id: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) mode: Option<String>,
    pub(crate) expected_revision: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) item_type: Option<String>,
    pub(crate) authors: Option<String>,
    pub(crate) doi: Option<String>,
    pub(crate) venue: Option<String>,
    pub(crate) year: Option<String>,
    pub(crate) language: Option<String>,
    pub(crate) uri: Option<String>,
    pub(crate) abstract_note: Option<String>,
    pub(crate) file_path: Option<String>,
    pub(crate) rules_text: Option<String>,
}

/// Execute one posted UI action.
pub(crate) fn run_action(
    daemon: &LocalrefDaemon,
    form: &UiAction,
) -> Result<(), String> {
    match form.action.as_str() {
        "scan" => daemon.scan_all().map(|_| ()).map_err(to_string),
        "pause" => {
            daemon.pause(pause_mode(form.mode.as_deref())?);
            Ok(())
        }
        "resume" => {
            daemon.resume(pause_mode(form.mode.as_deref())?);
            Ok(())
        }
        "create_category" => create_category(daemon, form),
        "add_category" => add_category(daemon, form),
        "remove_category" => remove_category(daemon, form),
        "open_folder" => open_folder(daemon, form),
        "open_file" => open_file(daemon, form),
        "add_file" => add_file(daemon, form),
        "import_file" => import_file(daemon, form),
        "save_metadata" => save_metadata(daemon, form),
        "save_rules" => save_rules(daemon, form),
        _ => Ok(()),
    }
}

fn create_category(
    daemon: &LocalrefDaemon,
    form: &UiAction,
) -> Result<(), String> {
    let Some(category) = form.category.as_deref().and_then(CategoryPath::new)
    else {
        return Ok(());
    };
    daemon.create_category(category).map(|_| ()).map_err(to_string)
}

fn add_category(
    daemon: &LocalrefDaemon,
    form: &UiAction,
) -> Result<(), String> {
    let Some(category) = form.category.as_deref().and_then(CategoryPath::new)
    else {
        return Ok(());
    };
    daemon
        .add_items_category(
            &category_target_ids_from_return(&form.return_to),
            category,
        )
        .map(|_| ())
        .map_err(to_string)
}

fn remove_category(
    daemon: &LocalrefDaemon,
    form: &UiAction,
) -> Result<(), String> {
    let Some(category) = form.category.as_deref().and_then(CategoryPath::new)
    else {
        return Ok(());
    };
    daemon
        .remove_items_category(
            &category_target_ids_from_return(&form.return_to),
            category,
        )
        .map(|_| ())
        .map_err(to_string)
}

fn open_folder(
    daemon: &LocalrefDaemon,
    form: &UiAction,
) -> Result<(), String> {
    let Some(item_id) = form.item_id.as_deref() else {
        return Ok(());
    };
    daemon.open_item_folder(item_id).map(|_| ()).map_err(to_string)
}

fn open_file(daemon: &LocalrefDaemon, form: &UiAction) -> Result<(), String> {
    let Some(item_id) = form.item_id.as_deref() else {
        return Ok(());
    };
    let Some(path) = optional_text(form.file_path.as_deref()) else {
        return Ok(());
    };
    daemon
        .open_item_file(item_id, Path::new(&path))
        .map(|_| ())
        .map_err(to_string)
}

fn add_file(daemon: &LocalrefDaemon, form: &UiAction) -> Result<(), String> {
    let Some(item_id) = form.item_id.as_deref() else {
        return Ok(());
    };
    let Some(path) = optional_text(form.file_path.as_deref()) else {
        return Ok(());
    };
    daemon
        .add_file_to_item(item_id, PathBuf::from(path))
        .map(|_| ())
        .map_err(to_string)
}

fn import_file(
    daemon: &LocalrefDaemon,
    form: &UiAction,
) -> Result<(), String> {
    let Some(path) = optional_text(form.file_path.as_deref()) else {
        return Ok(());
    };
    daemon.import_file(PathBuf::from(path)).map(|_| ()).map_err(to_string)
}

fn save_metadata(
    daemon: &LocalrefDaemon,
    form: &UiAction,
) -> Result<(), String> {
    let Some(item_id) = form.item_id.as_deref() else {
        return Ok(());
    };
    let Some(document) = daemon.get_metadata(item_id).map_err(to_string)?
    else {
        return Ok(());
    };
    let mut metadata = document.metadata;
    metadata.title = form.title.clone().unwrap_or_default();
    metadata.item_type =
        form.item_type.clone().unwrap_or_else(|| "document".to_string());
    metadata.doi = optional_text(form.doi.as_deref());
    metadata.venue = optional_text(form.venue.as_deref());
    metadata.language = optional_text(form.language.as_deref());
    metadata.uri = optional_text(form.uri.as_deref());
    metadata.abstract_note = optional_text(form.abstract_note.as_deref());
    metadata.year =
        form.year.as_deref().and_then(|year| year.trim().parse::<i32>().ok());
    crate::state::replace_author_creators(
        &mut metadata,
        crate::state::parse_author_names(form.authors.as_deref()),
    );
    daemon
        .patch_metadata(
            item_id,
            form.expected_revision
                .as_deref()
                .unwrap_or(document.metadata_revision.as_str()),
            metadata,
        )
        .map(|_| ())
        .map_err(to_string)
}

fn save_rules(daemon: &LocalrefDaemon, form: &UiAction) -> Result<(), String> {
    daemon
        .write_rules_text(form.rules_text.as_deref().unwrap_or_default())
        .map_err(to_string)
}

fn pause_mode(value: Option<&str>) -> Result<PauseMode, String> {
    match value.unwrap_or("watcher") {
        "all" => Ok(PauseMode::All),
        "writes" => Ok(PauseMode::Writes),
        "watcher" => Ok(PauseMode::Watcher),
        "indexing" => Ok(PauseMode::Indexing),
        value => Err(format!("unknown pause mode: {value}")),
    }
}

fn category_target_ids_from_return(path: &str) -> Vec<String> {
    let query = path.split('?').nth(1).unwrap_or_default();
    let selected = query
        .split('&')
        .find_map(|part| part.strip_prefix("selected="))
        .and_then(decode_query_value)
        .map(|value| {
            value
                .split(',')
                .filter(|id| !id.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if selected.is_empty() {
        query
            .split('&')
            .find_map(|part| part.strip_prefix("active="))
            .and_then(decode_query_value)
            .filter(|id| !id.is_empty())
            .map(|id| vec![id])
            .unwrap_or_default()
    } else {
        selected
    }
}

fn decode_query_value(value: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let high = input.next().and_then(hex_value)?;
                let low = input.next().and_then(hex_value)?;
                bytes.push((high << 4) | low);
            }
            byte => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn to_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}
