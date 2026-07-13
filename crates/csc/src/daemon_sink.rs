//! Connector sink that forwards to the core import pipeline.
//!
//! This module assembles Zotero connector items together with their
//! late-arriving attachment uploads into buffered sessions, then forwards each
//! completed session to the `localref-core` import pipeline. It is the
//! production [`crate::ConnectorImportSink`] implementation that bridges the
//! connector HTTP surface to an open [`localref_core::LocalrefDaemon`].

use std::sync::Mutex;

use localref_core::LocalrefDaemon;
use localref_core::types::{
    ConnectorAttachment, ConnectorImport, ConnectorItem, ImportOutcome,
};

use crate::{ConnectorEvent, ConnectorImportRequest, ConnectorImportSink};

/// Connector sink that buffers connector data and forwards it to core.
///
/// Connector saves arrive as a top-level item followed by zero or more
/// attachment uploads that reference the same save session. This sink keeps a
/// per-session buffer so item metadata and its attachments are imported
/// together once the metadata is available.
pub struct DaemonConnectorSink {
    /// Open daemon facade used to run imports.
    daemon: LocalrefDaemon,
    /// Buffered connector save sessions awaiting completion.
    sessions: Mutex<Vec<ConnectorSession>>,
}

/// Buffered connector save session.
#[derive(Debug)]
struct ConnectorSession {
    /// Zotero save session identifier, if the connector supplied one.
    session_id: Option<String>,
    /// Normalized connector items received for this session.
    items: Vec<ConnectorItem>,
    /// Attachments received for this session.
    attachments: Vec<ConnectorAttachment>,
    /// Import outcome once the session has been written to the library.
    outcome: Option<ImportOutcome>,
}

impl DaemonConnectorSink {
    /// Create a connector sink backed by an open daemon.
    #[must_use]
    pub fn new(daemon: LocalrefDaemon) -> Self {
        Self { daemon, sessions: Mutex::new(Vec::new()) }
    }

    /// Try to import every buffered session that has metadata.
    fn try_import_locked(
        &self,
        sessions: &mut [ConnectorSession],
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
            tracing::info!(
                target: "localref::csc_import",
                "saved Localref item: {}",
                outcome.item_dir.display(),
            );
            for file in &outcome.written_files {
                tracing::info!(
                    target: "localref::csc_import",
                    "wrote {}",
                    file.display(),
                );
            }
            session.outcome = Some(outcome);
        }
        Ok(())
    }
}

impl ConnectorImportSink for DaemonConnectorSink {
    fn accept_import(
        &self,
        request: ConnectorImportRequest,
    ) -> Result<(), String> {
        tracing::info!(
            target: "localref::csc_import",
            "connector import: {} item(s)",
            request.items.len(),
        );
        let mut sessions =
            self.sessions.lock().expect("connector sessions mutex poisoned");
        if let Some(session) = sessions.iter_mut().find(|session| {
            session.session_id == request.session_id
                && session.outcome.is_none()
        }) {
            session.items = request.normalized_items;
        } else {
            sessions.push(ConnectorSession {
                session_id: request.session_id,
                items: request.normalized_items,
                attachments: Vec::new(),
                outcome: None,
            });
        }
        self.try_import_locked(&mut sessions)?;
        prune_completed_sessions(&mut sessions);
        Ok(())
    }

    fn accept_attachment(
        &self,
        attachment: ConnectorAttachment,
    ) -> Result<(), String> {
        tracing::info!(
            target: "localref::csc_attachment",
            "connector attachment: {} bytes, file {}",
            attachment.bytes.len(),
            attachment.filename,
        );
        let mut sessions =
            self.sessions.lock().expect("connector sessions mutex poisoned");
        let session_index = sessions
            .iter()
            .position(|session| session.session_id == attachment.session_id)
            .or_else(|| {
                // No session matched by id. Under the connector's normal
                // saveItems -> saveAttachment ordering the attachment belongs
                // to the most recently created session, so fall back to it when
                // the upload either carries no session id or is known to belong
                // to a parent item. Without this, a mismatched session id would
                // strand a child attachment in a session that is never imported
                // (see the orphan branch below), losing the bytes.
                let can_fall_back = attachment.session_id.is_none()
                    || attachment.parent_item_id.is_some();
                can_fall_back
                    .then(|| sessions.len().checked_sub(1))
                    .flatten()
            });
        let Some(session_index) = session_index else {
            if attachment.parent_item_id.is_none() {
                // Build a top-level import for a standalone attachment that
                // belongs to no buffered session.
                let title = attachment
                    .title
                    .clone()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| attachment.filename.clone());
                let uri =
                    attachment.raw_metadata.as_ref().and_then(|metadata| {
                        metadata
                            .get("url")
                            .or_else(|| metadata.get("uri"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    });
                let item = ConnectorItem {
                    session_id: attachment.session_id.clone(),
                    uri,
                    connector_item_id: None,
                    item_type: Some("attachment".to_string()),
                    title,
                    abstract_note: None,
                    doi: None,
                    raw: attachment.raw_metadata.clone().unwrap_or_else(
                        || serde_json::json!({ "title": attachment.filename }),
                    ),
                };
                let standalone =
                    ConnectorImport { item, attachments: vec![attachment] };
                let outcome = self
                    .daemon
                    .import_connector_item(standalone)
                    .map_err(|error| error.to_string())?;
                tracing::info!(
                    target: "localref::csc_attachment",
                    "saved standalone attachment: {}",
                    outcome.item_dir.display(),
                );
                return Ok(());
            }
            sessions.push(ConnectorSession {
                session_id: attachment.session_id.clone(),
                items: Vec::new(),
                attachments: vec![attachment],
                outcome: None,
            });
            return Ok(());
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
            tracing::info!(
                target: "localref::csc_attachment",
                "saved late attachment: {}",
                path.display(),
            );
        } else {
            session.attachments.push(attachment);
            self.try_import_locked(&mut sessions)?;
        }
        prune_completed_sessions(&mut sessions);
        Ok(())
    }

    fn accept_event(&self, event: ConnectorEvent) -> Result<(), String> {
        let serialized = serde_json::to_string(&event)
            .map_err(|error| error.to_string())?;
        tracing::info!(
            target: "localref::csc_event",
            "{serialized}",
        );
        Ok(())
    }

    fn category_paths(&self) -> Result<Vec<String>, String> {
        self.daemon
            .list_categories()
            .map(|categories| {
                categories.into_iter().map(|category| category.path).collect()
            })
            .map_err(|error| error.to_string())
    }
}

/// Maximum number of already-imported sessions retained for late attachments.
///
/// A connector save can upload several attachments after its item is imported,
/// so a completed session must stay around to receive them. But the session Vec
/// was never cleared, so it grew unbounded and each new save pushed the "most
/// recent session" fallback further from any given attachment's real parent.
/// Retaining a bounded window of the most recent completed sessions keeps late
/// attachments working while stopping the Vec from growing without limit.
const MAX_COMPLETED_SESSIONS: usize = 16;

/// Drop the oldest imported sessions once more than [`MAX_COMPLETED_SESSIONS`]
/// have completed, preserving order and every not-yet-imported session.
fn prune_completed_sessions(sessions: &mut Vec<ConnectorSession>) {
    let completed =
        sessions.iter().filter(|session| session.outcome.is_some()).count();
    let Some(mut to_drop) = completed.checked_sub(MAX_COMPLETED_SESSIONS)
    else {
        return;
    };
    if to_drop == 0 {
        return;
    }
    sessions.retain(|session| {
        if to_drop > 0 && session.outcome.is_some() {
            to_drop -= 1;
            false
        } else {
            true
        }
    });
}
