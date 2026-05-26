//! In-memory pending import sessions.
//!
//! Pending sessions are created when an import requires user category
//! confirmation. The durable source item has not been written to `All/` yet;
//! confirmation consumes the stored connector import and passes selected
//! categories back to the import pipeline.

use std::sync::{Arc, Mutex};

use crate::types::{CategoryPath, ConnectorImport};
use serde::{Deserialize, Serialize};

/// User-visible pending import session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PendingImportSession {
    /// Monotonic pending import id.
    pub id: u64,
    /// Imported item title.
    pub title: String,
    /// Imported item type, when known.
    pub item_type: Option<String>,
    /// Source URI, when known.
    pub uri: Option<String>,
    /// Rule-suggested categories.
    pub suggested_categories: Vec<CategoryPath>,
}

/// Request body used to confirm a pending import.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct PendingImportConfirmation {
    /// Categories selected by the user. When absent, suggested categories are used.
    pub categories: Option<Vec<CategoryPath>>,
}

/// Thread-safe pending import storage.
#[derive(Clone, Default)]
pub struct PendingImportStore {
    inner: Arc<Mutex<PendingState>>,
}

#[derive(Default)]
struct PendingState {
    next_id: u64,
    records: Vec<PendingImportRecord>,
}

pub(crate) struct PendingImportRecord {
    pub session: PendingImportSession,
    pub import: ConnectorImport,
}

impl PendingImportStore {
    /// Create and store one pending import session.
    pub fn create(
        &self,
        import: ConnectorImport,
        suggested_categories: Vec<CategoryPath>,
    ) -> PendingImportSession {
        let mut inner =
            self.inner.lock().expect("pending import mutex poisoned");
        let id = if inner.next_id == 0 { 1 } else { inner.next_id };
        inner.next_id = id + 1;
        let session = PendingImportSession {
            id,
            title: import.item.title.clone(),
            item_type: import.item.item_type.clone(),
            uri: import.item.uri.clone(),
            suggested_categories,
        };
        inner
            .records
            .push(PendingImportRecord { session: session.clone(), import });
        session
    }

    /// Return all pending import sessions.
    pub fn list(&self) -> Vec<PendingImportSession> {
        self.inner
            .lock()
            .expect("pending import mutex poisoned")
            .records
            .iter()
            .map(|record| record.session.clone())
            .collect()
    }

    /// Remove and return a pending import record by id.
    pub(crate) fn take(&self, id: u64) -> Option<PendingImportRecord> {
        let mut inner =
            self.inner.lock().expect("pending import mutex poisoned");
        let index =
            inner.records.iter().position(|record| record.session.id == id)?;
        Some(inner.records.remove(index))
    }
}
