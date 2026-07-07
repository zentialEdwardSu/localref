//! `DaemonConnectorSink` buffers a connector save session, then routes the
//! late-arriving attachment upload to the imported item. The connector's normal
//! order is saveItems -> 201 -> saveAttachment, so by the time an attachment
//! arrives the item is already imported and the attachment must find it by
//! session id. A mismatched session id previously stranded a child attachment
//! in a session that was never imported, losing the bytes — the failure these
//! tests guard against.

use csc::{ConnectorImportRequest, ConnectorImportSink, DaemonConnectorSink};
use localref_core::LocalrefDaemon;
use localref_core::types::{ConnectorAttachment, ConnectorItem};
use serde_json::json;

fn import_request(
    session_id: &str,
    item_id: &str,
) -> ConnectorImportRequest {
    ConnectorImportRequest {
        session_id: Some(session_id.to_string()),
        uri: None,
        items: vec![json!({ "title": "Paper", "itemType": "journalArticle" })],
        normalized_items: vec![ConnectorItem {
            session_id: Some(session_id.to_string()),
            uri: None,
            connector_item_id: Some(item_id.to_string()),
            item_type: Some("journalArticle".to_string()),
            title: "Paper".to_string(),
            abstract_note: None,
            doi: None,
            raw: json!({ "title": "Paper" }),
        }],
    }
}

fn pdf_attachment(
    session_id: Option<&str>,
    parent_item_id: Option<&str>,
) -> ConnectorAttachment {
    ConnectorAttachment {
        session_id: session_id.map(str::to_string),
        parent_item_id: parent_item_id.map(str::to_string),
        title: Some("Paper".to_string()),
        filename: "paper.pdf".to_string(),
        mime_type: Some("application/pdf".to_string()),
        bytes: vec![1, 2, 3, 4],
        raw_metadata: None,
    }
}

/// Count `paper.pdf` files written anywhere under the library's `All/` tree.
fn saved_pdf_count(library_root: &std::path::Path) -> usize {
    let all_dir = library_root.join("All");
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(&all_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join("paper.pdf");
        if candidate.exists() {
            count += 1;
        }
    }
    count
}

#[test]
fn late_attachment_with_matching_session_attaches_to_item() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let sink = DaemonConnectorSink::new(daemon);

    sink.accept_import(import_request("s1", "paper-1")).unwrap();
    sink.accept_attachment(pdf_attachment(Some("s1"), Some("paper-1")))
        .unwrap();

    assert_eq!(
        saved_pdf_count(temp.path()),
        1,
        "the attachment must be written into the imported item directory",
    );
}

#[test]
fn late_attachment_with_mismatched_session_is_not_stranded() {
    let temp = tempfile::tempdir().unwrap();
    let daemon = LocalrefDaemon::for_library(temp.path()).unwrap();
    let sink = DaemonConnectorSink::new(daemon);

    // Item imported under session "s1", but the attachment reports a different
    // session id (as happens when the connector's session bookkeeping drifts).
    // Because it names a parent item, it must still land on the most recent
    // session rather than being dropped into a never-imported orphan.
    sink.accept_import(import_request("s1", "paper-1")).unwrap();
    sink.accept_attachment(pdf_attachment(Some("mismatch"), Some("paper-1")))
        .unwrap();

    assert_eq!(
        saved_pdf_count(temp.path()),
        1,
        "a mismatched-session child attachment must not be stranded",
    );
}
