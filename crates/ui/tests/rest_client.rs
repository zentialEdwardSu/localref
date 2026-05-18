//! Intent tests for the desktop REST client boundary.
//!
//! These tests use a one-shot local HTTP server so the public client methods
//! can be verified without reaching a real daemon or opening a desktop window.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};

use localref_config::LocalrefConfig;
use model::{
    Metadata, MetadataFiles, MetadataImport, MetadataState, MetadataTags,
};
use serde_json::json;
use ui::RestClient;

#[test]
fn patch_metadata_sends_revision_so_core_can_reject_external_edit_conflicts() {
    let (endpoint, request) = serve_once(item_document_json());
    let client = RestClient::new(endpoint);

    client
        .patch_metadata(
            "lr:test:item/one",
            "rev-before-edit",
            sample_metadata("lr:test:item/one"),
        )
        .unwrap();

    let request = request.join().unwrap();
    assert!(
        request.starts_with(
            "PATCH /api/items/lr:test:item%2Fone/metadata HTTP/1.1"
        )
    );
    assert!(request.contains("\"expected_revision\":\"rev-before-edit\""));
    assert!(request.contains("\"abstract\":\"Intent matters\""));
}

#[test]
fn delete_category_preserves_hierarchy_so_cat_paths_stay_authoritative() {
    let (endpoint, request) = serve_once(
        json!({"path": "Wireless/RIS 2026", "item_ids": []}).to_string(),
    );
    let client = RestClient::new(endpoint);

    client.remove_item_category("lr:test item", "Wireless/RIS 2026").unwrap();

    let request = request.join().unwrap();
    assert!(request.starts_with(
        "DELETE /api/items/lr:test%20item/categories/Wireless/RIS%202026 HTTP/1.1"
    ));
}

#[test]
fn client_uses_rest_endpoint_from_config_file() {
    let (endpoint, request) = serve_once("[]".to_string());
    let config_path = temp_config_path("ui-rest-endpoint.toml");
    std::fs::write(
        &config_path,
        format!("[rest]\nendpoint = \"{endpoint}\"\n"),
    )
    .unwrap();
    let config = LocalrefConfig::load_from_path(&config_path).unwrap();
    let client = RestClient::from_config(&config);

    client.list_items().unwrap();

    let request = request.join().unwrap();
    assert!(request.starts_with("GET /api/items HTTP/1.1"));

    std::fs::remove_file(config_path).unwrap();
}

fn serve_once(response_body: String) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 512];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            buffer.extend_from_slice(&chunk[..read]);
            if request_is_complete(&buffer) {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8(buffer).unwrap()
    });
    (endpoint, handle)
}

fn request_is_complete(buffer: &[u8]) -> bool {
    let request = String::from_utf8_lossy(buffer);
    let Some((head, body)) = request.split_once("\r\n\r\n") else {
        return false;
    };
    let content_length = head
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    body.len() >= content_length
}

fn sample_metadata(id: &str) -> Metadata {
    Metadata {
        id: id.to_string(),
        item_type: "journalArticle".to_string(),
        title: "A paper".to_string(),
        abstract_note: Some("Intent matters".to_string()),
        doi: Some("10.1000/localref".to_string()),
        uri: None,
        year: Some(2026),
        venue: Some("Localref Tests".to_string()),
        language: Some("en".to_string()),
        creators: Vec::new(),
        files: MetadataFiles::default(),
        tags: MetadataTags::default(),
        import: MetadataImport::default(),
        state: MetadataState::default(),
        raw_connector: Default::default(),
    }
}

fn item_document_json() -> String {
    json!({
        "id": "lr:test:item/one",
        "object_path": "All/A paper",
        "metadata_revision": "rev-after-edit",
        "title": "A paper",
        "abstract_note": "Intent matters",
        "item_type": "journalArticle",
        "doi": "10.1000/localref",
        "uri": null,
        "main_file": null,
        "extra_files": [],
        "tags": [],
        "venue": "Localref Tests",
        "year": 2026,
        "categories": []
    })
    .to_string()
}

fn temp_config_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("localref-ui-{}-{name}", std::process::id()))
}
