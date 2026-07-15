//! Shared runtime-health state used by the CLI host and the desktop FFI host.

use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeHealthState {
    Healthy,
    Degraded,
    Fatal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHealthSnapshot {
    pub state: RuntimeHealthState,
    pub component: String,
    pub message: String,
    pub occurrence_count: u64,
    pub generation: u64,
}

impl Default for RuntimeHealthSnapshot {
    fn default() -> Self {
        Self {
            state: RuntimeHealthState::Healthy,
            component: "runtime".to_string(),
            message: "Localref runtime is healthy".to_string(),
            occurrence_count: 0,
            generation: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeHealthTracker {
    inner: Arc<Mutex<RuntimeHealthSnapshot>>,
}

impl RuntimeHealthTracker {
    #[must_use]
    pub fn snapshot(&self) -> RuntimeHealthSnapshot {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn degraded(
        &self,
        component: &str,
        message: impl Into<String>,
        occurrence_count: u64,
    ) {
        self.update(
            RuntimeHealthState::Degraded,
            component,
            message,
            occurrence_count,
        );
    }

    pub fn fatal(
        &self,
        component: &str,
        message: impl Into<String>,
        occurrence_count: u64,
    ) {
        self.update(
            RuntimeHealthState::Fatal,
            component,
            message,
            occurrence_count,
        );
    }

    fn update(
        &self,
        state: RuntimeHealthState,
        component: &str,
        message: impl Into<String>,
        occurrence_count: u64,
    ) {
        let mut health = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if health.state == RuntimeHealthState::Fatal
            && state != RuntimeHealthState::Fatal
        {
            return;
        }
        health.state = state;
        health.component = component.to_string();
        health.message = message.into();
        health.occurrence_count = occurrence_count;
        health.generation = health.generation.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeHealthState, RuntimeHealthTracker};

    #[test]
    fn fatal_health_is_sticky() {
        let tracker = RuntimeHealthTracker::default();
        tracker.degraded("hooks", "panic", 1);
        tracker.fatal("server", "stopped", 2);
        tracker.degraded("cron", "panic", 3);

        let health = tracker.snapshot();
        assert_eq!(health.state, RuntimeHealthState::Fatal);
        assert_eq!(health.component, "server");
        assert_eq!(health.occurrence_count, 2);
        assert_eq!(health.generation, 2);
    }
}
