//! Engine notification sink.
//!
//! rollforward fires [`EngineNotificationListener`] callbacks *inside* the
//! engine while it holds internal locks. Doing REST calls there (to register a
//! conflict copy or set a row color) would risk re-entrancy and blocking, so
//! this listener only *records* events into shared buffers. The sync driver in
//! `main` drains them after `engine.sync` returns and performs the side effects
//! (writing the conflict copy, flagging the item) with a plain async client.

use rollforward::types::EngineNotificationListener;
use std::sync::Mutex;

/// A conflict the engine asked the host to duplicate ("keep both").
#[derive(Debug, Clone)]
pub struct ConflictCopy {
    /// The engine file id (`"{item_id}/{relative_path}"`).
    pub file_id: String,
    /// Suggested conflict-marked name from the engine.
    pub suggested_name: String,
}

/// Records engine notifications for the driver to act on post-sync.
#[derive(Debug, Default)]
pub struct RecordingListener {
    /// File ids whose merged content changed during the sync.
    updated: Mutex<Vec<String>>,
    /// Conflict-copy requests raised during the sync.
    conflicts: Mutex<Vec<ConflictCopy>>,
}

impl RecordingListener {
    /// Take (and clear) the recorded conflict-copy requests.
    pub fn take_conflicts(&self) -> Vec<ConflictCopy> {
        std::mem::take(&mut self.conflicts.lock().expect("conflicts lock poisoned"))
    }

    /// Take (and clear) the recorded content-updated file ids.
    pub fn take_updated(&self) -> Vec<String> {
        std::mem::take(&mut self.updated.lock().expect("updated lock poisoned"))
    }
}

impl EngineNotificationListener for RecordingListener {
    fn on_file_content_updated(&self, file_id: String) {
        self.updated.lock().expect("updated lock poisoned").push(file_id);
    }

    fn on_conflict_copy_requested(&self, file_id: String, suggested_name: String) {
        self.conflicts
            .lock()
            .expect("conflicts lock poisoned")
            .push(ConflictCopy { file_id, suggested_name });
    }
}
