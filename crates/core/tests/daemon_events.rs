//! Daemon emits `DaemonEvent`s on action completion so the host can dispatch
//! plugin hooks. These tests assert each mutating action publishes the right
//! event with the real item id / category — the contract the hook dispatcher
//! relies on.

use localref_core::types::{ConnectorImport, ConnectorItem};
use localref_core::{DaemonEvent, LocalrefDaemon, PauseMode, StatusKind};
use serde_json::json;

fn connector_import(id: &str, title: &str) -> ConnectorImport {
    ConnectorImport {
        item: ConnectorItem {
            session_id: Some(format!("session-{id}")),
            uri: None,
            connector_item_id: Some(id.to_string()),
            item_type: Some("journalArticle".to_string()),
            title: title.to_string(),
            abstract_note: None,
            doi: None,
            raw: json!({ "title": title }),
        },
        attachments: Vec::new(),
    }
}

#[test]
fn import_emits_item_imported_with_real_id() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let mut rx = daemon.subscribe();

    let outcome = daemon
        .import_connector_item(connector_import("paper-1", "Paper One"))
        .unwrap();

    assert_eq!(
        rx.try_recv().expect("import emits an event"),
        DaemonEvent::ItemImported {
            item_id: outcome.item_id.as_str().to_string()
        },
    );
}

#[test]
fn delete_emits_item_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let outcome = daemon
        .import_connector_item(connector_import("paper-2", "Paper Two"))
        .unwrap();
    let item_id = outcome.item_id.as_str().to_string();

    let mut rx = daemon.subscribe();
    daemon.delete_item(&item_id).unwrap();

    assert_eq!(
        rx.try_recv().expect("delete emits an event"),
        DaemonEvent::ItemDeleted { item_id },
    );
}

#[test]
fn create_category_emits_category_changed() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let mut rx = daemon.subscribe();

    let category =
        localref_core::types::CategoryPath::new("Inbox/New").unwrap();
    daemon.create_category(&category).unwrap();

    assert_eq!(
        rx.try_recv().expect("create_category emits an event"),
        DaemonEvent::CategoryChanged {
            item_id: None,
            category: Some("Inbox/New".to_string()),
        },
    );
}

#[test]
fn scan_emits_scan_completed() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let mut rx = daemon.subscribe();

    daemon.scan_all().unwrap();

    match rx.try_recv().expect("scan emits an event") {
        DaemonEvent::ScanCompleted { .. } => {}
        other => panic!("expected ScanCompleted, got {other:?}"),
    }
}

#[test]
fn event_name_matches_wire_names() {
    assert_eq!(
        DaemonEvent::ItemImported { item_id: String::new() }.event_name(),
        "item_imported",
    );
    assert_eq!(
        DaemonEvent::ItemDeleted { item_id: String::new() }.event_name(),
        "item_deleted",
    );
    assert_eq!(
        DaemonEvent::MetadataPatched { item_id: String::new() }.event_name(),
        "metadata_patched",
    );
    assert_eq!(
        DaemonEvent::CategoryChanged { item_id: None, category: None }
            .event_name(),
        "category_changed",
    );
    assert_eq!(
        DaemonEvent::ScanCompleted { indexed_items: 0 }.event_name(),
        "scan_completed",
    );
    assert_eq!(
        DaemonEvent::ItemFileAdded { item_id: String::new() }.event_name(),
        "item_file_added",
    );
    assert_eq!(DaemonEvent::RulesChanged.event_name(), "rules_changed");
    assert_eq!(
        DaemonEvent::SchedulesChanged.event_name(),
        "schedules_changed",
    );
    assert_eq!(DaemonEvent::DaemonPaused.event_name(), "daemon_paused");
    assert_eq!(DaemonEvent::DaemonResumed.event_name(), "daemon_resumed");
    assert_eq!(
        DaemonEvent::StatusMessage {
            text: String::new(),
            kind: StatusKind::Info,
        }
        .event_name(),
        "status_message",
    );
}

#[test]
fn emit_status_publishes_status_message() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let mut rx = daemon.subscribe();

    daemon.emit_status("syncing 3/10".to_string(), StatusKind::Success);

    assert_eq!(
        rx.try_recv().expect("emit_status emits an event"),
        DaemonEvent::StatusMessage {
            text: "syncing 3/10".to_string(),
            kind: StatusKind::Success,
        },
    );
}

#[test]
fn rules_and_pause_changes_emit_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let mut rx = daemon.subscribe();

    daemon.write_rules_text("").unwrap();
    assert_eq!(rx.try_recv().unwrap(), DaemonEvent::RulesChanged);

    daemon.pause(PauseMode::Indexing);
    assert_eq!(rx.try_recv().unwrap(), DaemonEvent::DaemonPaused);

    daemon.resume(PauseMode::Indexing);
    assert_eq!(rx.try_recv().unwrap(), DaemonEvent::DaemonResumed);
}
