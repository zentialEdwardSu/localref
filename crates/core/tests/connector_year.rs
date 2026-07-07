//! Connector imports must recover the publication year from Zotero's free-form
//! `date` field. Before this, `metadata_from_import` hardcoded `year: None`, so
//! ~70% of connector imports lost their year. These tests pin the contract that
//! the year written to `metadata.toml` comes from the imported item's raw JSON.

use localref_core::LocalrefDaemon;
use localref_core::types::{ConnectorImport, ConnectorItem};
use serde_json::json;

fn import_with_raw(id: &str, raw: serde_json::Value) -> ConnectorImport {
    ConnectorImport {
        item: ConnectorItem {
            session_id: Some(format!("session-{id}")),
            uri: None,
            connector_item_id: Some(id.to_string()),
            item_type: Some("journalArticle".to_string()),
            title: "Paper".to_string(),
            abstract_note: None,
            doi: None,
            raw,
        },
        attachments: Vec::new(),
    }
}

#[test]
fn import_populates_year_from_free_form_date() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

    let outcome = daemon
        .import_connector_item(import_with_raw(
            "paper-year",
            json!({ "title": "Paper", "date": "March 2021" }),
        ))
        .unwrap();

    let document = daemon
        .get_metadata(outcome.item_id.as_str())
        .unwrap()
        .expect("imported item has metadata");
    assert_eq!(document.metadata.year, Some(2021));
}

#[test]
fn import_leaves_year_none_without_a_date() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();

    let outcome = daemon
        .import_connector_item(import_with_raw(
            "paper-no-year",
            json!({ "title": "Paper" }),
        ))
        .unwrap();

    let document = daemon
        .get_metadata(outcome.item_id.as_str())
        .unwrap()
        .expect("imported item has metadata");
    assert_eq!(document.metadata.year, None);
}
