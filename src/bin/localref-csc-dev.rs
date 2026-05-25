//! Manual dynamic-test server for the Zotero Connector-compatible API.
//!
//! This binary is intentionally dev-only. The production application entry is
//! the root `localref` binary; this server exists so Zotero Connector traffic
//! can be inspected without starting the whole app.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use csc::{
    ConnectorEvent, ConnectorImportRequest, ConnectorImportSink,
    serve as serve_csc,
};
use localref_core::config::LocalrefConfig;
use localref_core::LocalrefDaemon;
use serde_json::Value;
use localref_core::types::{
    ConnectorAttachment, ConnectorImport, ConnectorItem, ImportOutcome,
};

/// Import sink that prints connector traffic and writes through core.
struct LoggingImportSink {
    daemon: LocalrefDaemon,
    sessions: Mutex<Vec<PendingImport>>,
}

#[derive(Debug)]
struct PendingImport {
    session_id: Option<String>,
    items: Vec<ConnectorItem>,
    attachments: Vec<ConnectorAttachment>,
    outcome: Option<ImportOutcome>,
}

impl LoggingImportSink {
    /// Create a logging sink for a library root.
    fn new(library_root: PathBuf) -> Self {
        Self {
            daemon: LocalrefDaemon::for_library(library_root)
                .expect("library root must be writable"),
            sessions: Mutex::new(Vec::new()),
        }
    }

    /// Try to import every buffered session that has metadata.
    fn try_import_locked(
        &self,
        sessions: &mut [PendingImport],
    ) -> Result<(), String> {
        for session in
            sessions.iter_mut().filter(|session| session.outcome.is_none())
        {
            let Some(item) = session.items.first().cloned() else {
                continue;
            };
            let outcome = self
                .daemon
                .import_connector_item(ConnectorImport {
                    item,
                    attachments: session.attachments.clone(),
                })
                .map_err(|error| error.to_string())?;
            println!("saved Localref item: {}", outcome.item_dir.display());
            for file in &outcome.written_files {
                println!("  wrote: {}", file.display());
            }
            session.outcome = Some(outcome);
        }
        Ok(())
    }
}

impl ConnectorImportSink for LoggingImportSink {
    /// Print and buffer one normalized `saveItems` request.
    fn accept_import(
        &self,
        request: ConnectorImportRequest,
    ) -> Result<(), String> {
        println!("--- connector import ---");
        print_optional("session", request.session_id.as_deref());
        print_optional("uri", request.uri.as_deref());
        println!("items: {}", request.items.len());
        println!("normalized items: {}", request.normalized_items.len());

        for (index, item) in request.items.iter().enumerate() {
            println!("  item #{index}");
            print_json_field("    id", item, &["id", "itemID", "key"]);
            print_json_field("    type", item, &["itemType", "type"]);
            print_json_field("    title", item, &["title"]);
            print_json_field(
                "    abstract",
                item,
                &["abstractNote", "abstract"],
            );
            print_json_field("    doi", item, &["DOI", "doi"]);
            print_json_field("    url", item, &["url", "uri"]);
            print_creators(item);
            print_attachments(item);
        }

        let mut sessions =
            self.sessions.lock().expect("connector sessions mutex poisoned");
        sessions.push(PendingImport {
            session_id: request.session_id,
            items: request.normalized_items,
            attachments: Vec::new(),
            outcome: None,
        });
        self.try_import_locked(&mut sessions)
    }

    /// Print and store one uploaded attachment.
    fn accept_attachment(
        &self,
        attachment: ConnectorAttachment,
    ) -> Result<(), String> {
        println!("--- connector attachment accepted ---");
        print_optional("session", attachment.session_id.as_deref());
        print_optional("parent item", attachment.parent_item_id.as_deref());
        print_optional("title", attachment.title.as_deref());
        println!("filename: {}", attachment.filename);
        print_optional("mime", attachment.mime_type.as_deref());
        println!(
            "bytes accepted: {} ({})",
            format_bytes(attachment.bytes.len()),
            attachment.bytes.len()
        );
        if let Some(metadata) = &attachment.raw_metadata {
            println!("raw attachment metadata:");
            print_pretty_json(metadata);
        }

        let mut sessions =
            self.sessions.lock().expect("connector sessions mutex poisoned");
        let session_index = sessions
            .iter()
            .position(|session| session.session_id == attachment.session_id)
            .or_else(|| sessions.len().checked_sub(1));
        let Some(session_index) = session_index else {
            return Err(
                "attachment arrived before any saveItems request".to_string()
            );
        };
        let session = &mut sessions[session_index];
        if let Some(outcome) = &session.outcome {
            let path = self
                .daemon
                .save_connector_attachment_to_item(
                    &outcome.item_dir,
                    attachment,
                )
                .map_err(|error| error.to_string())?;
            println!("saved late attachment: {}", path.display());
        } else {
            session.attachments.push(attachment);
            self.try_import_locked(&mut sessions)?;
        }
        Ok(())
    }

    /// Print connector diagnostic events such as snapshot calls.
    fn accept_event(&self, event: ConnectorEvent) -> Result<(), String> {
        println!("--- connector event ---");
        print_pretty_json(&event);
        Ok(())
    }
}

/// Print a JSON value in a stable pretty format for manual inspection.
fn print_pretty_json<T>(value: &T)
where
    T: serde::Serialize,
{
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .expect("manual-test output value should serialize")
    );
}

/// Print a label only when the value exists.
fn print_optional(label: &str, value: Option<&str>) {
    if let Some(value) = value {
        println!("{label}: {value}");
    }
}

/// Print the first string-like field found under any candidate key.
fn print_json_field(label: &str, value: &Value, keys: &[&str]) {
    if let Some(text) = first_string(value, keys) {
        println!("{label}: {text}");
    }
}

/// Return the first displayable value found under any candidate key.
fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match value.get(*key) {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        Some(Value::Bool(boolean)) => Some(boolean.to_string()),
        _ => None,
    })
}

/// Print Zotero creator summaries from a translated item payload.
fn print_creators(item: &Value) {
    let Some(creators) = item.get("creators").and_then(Value::as_array) else {
        return;
    };

    println!("    creators: {}", creators.len());
    for creator in creators.iter().take(8) {
        let role = first_string(creator, &["creatorType", "role"])
            .unwrap_or_else(|| "creator".to_string());
        let name = first_string(creator, &["name"]).unwrap_or_else(|| {
            let first = first_string(creator, &["firstName", "given"])
                .unwrap_or_default();
            let last = first_string(creator, &["lastName", "family"])
                .unwrap_or_default();
            format!("{first} {last}").trim().to_string()
        });
        if !name.is_empty() {
            println!("      - {role}: {name}");
        }
    }
    if creators.len() > 8 {
        println!("      ... {} more", creators.len() - 8);
    }
}

/// Print attachment summaries embedded in a Zotero item payload.
fn print_attachments(item: &Value) {
    let Some(attachments) = item.get("attachments").and_then(Value::as_array)
    else {
        return;
    };

    println!("    attachments: {}", attachments.len());
    for attachment in attachments {
        let title = first_string(attachment, &["title", "filename", "name"])
            .unwrap_or_else(|| "<untitled attachment>".to_string());
        println!("      - {title}");
        print_json_field(
            "        mime",
            attachment,
            &["mimeType", "contentType"],
        );
        print_json_field("        url", attachment, &["url"]);
        print_json_field("        path", attachment, &["path"]);
    }
}

/// Format byte counts for human-readable terminal logs.
fn format_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;

    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.2} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// Start the Localref connector compatibility development server.
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let config =
        LocalrefConfig::load().expect("failed to load Localref configuration");
    let addr = config.csc_addr();
    let library_root = PathBuf::from(config.library_root());

    println!("localref CSC dev server listening on http://{addr}");
    println!("config: {}", config.source_path().display());
    println!("library: {}", library_root.display());
    println!("GET  http://{addr}/connector/ping");
    println!("POST http://{addr}/connector/ping");
    println!("POST http://{addr}/connector/saveItems");
    println!("POST http://{addr}/connector/saveAttachment");

    serve_csc(addr, Arc::new(LoggingImportSink::new(library_root))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_string_reads_string_number_and_bool_fields() {
        let value = json!({
            "title": "A Test Paper",
            "year": 2026,
            "openAccess": true
        });

        assert_eq!(
            first_string(&value, &["missing", "title"]).as_deref(),
            Some("A Test Paper")
        );
        assert_eq!(first_string(&value, &["year"]).as_deref(), Some("2026"));
        assert_eq!(
            first_string(&value, &["openAccess"]).as_deref(),
            Some("true")
        );
    }

    #[test]
    fn format_bytes_uses_human_readable_units() {
        assert_eq!(format_bytes(42), "42 B");
        assert_eq!(format_bytes(2048), "2.00 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024), "3.00 MiB");
    }
}
