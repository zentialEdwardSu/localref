//! Thread-safe v2 event sink drained after each engine call.

use rollforward::{EngineEvent, EngineEventListenerV2};
use std::sync::Mutex;

/// Thread-safe v2 event queue drained by the async plugin after reconcile.
#[derive(Default)]
pub struct RuntimeEventBuffer {
    events: Mutex<Vec<EngineEvent>>,
}

impl RuntimeEventBuffer {
    pub fn take(&self) -> Vec<EngineEvent> {
        std::mem::take(
            &mut *self.events.lock().expect("v2 events lock poisoned"),
        )
    }
}

impl EngineEventListenerV2 for RuntimeEventBuffer {
    fn on_events(&self, events: Vec<EngineEvent>) {
        self.events.lock().expect("v2 events lock poisoned").extend(events);
    }
}
