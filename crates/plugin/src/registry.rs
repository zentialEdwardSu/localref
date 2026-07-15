//! Live plugin-invocation registry.
//!
//! Every spawned plugin subprocess registers here for the duration of its run,
//! so the host can (a) list what is currently executing and (b) cancel a run —
//! including killing *all* runs on app shutdown. Registration returns a
//! [`RegistrationGuard`] whose `Drop` removes the entry, so a run deregisters
//! itself on completion, timeout, cancellation, or panic without the caller
//! having to remember to.
//!
//! Cancellation is a one-shot signal: the spawn loop in `invoke` selects on the
//! [`oneshot::Receiver`] returned by [`register`](PluginProcessRegistry::register);
//! [`cancel`](PluginProcessRegistry::cancel) fires the matching sender, the
//! select's cancel arm wins, and the child future is dropped — which kills the
//! OS process because it was spawned with `kill_on_drop(true)`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::oneshot;

/// What kind of plugin entry point produced an invocation, for display.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvocationKind {
    /// A user-triggered `run <action>`.
    Action,
    /// A debounced preview `run <action>` (interactive, not user-committed).
    Preview,
    /// A lifecycle `hook <event>` fired by a daemon event.
    Hook,
    /// A scheduled `cron <job>` (manifest cron or runtime scheduled call).
    Cron,
}

impl InvocationKind {
    /// Stable lowercase label used for logs and the UI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Preview => "preview",
            Self::Hook => "hook",
            Self::Cron => "cron",
        }
    }
}

/// A live invocation held in the registry while its subprocess runs.
struct RunningEntry {
    /// Plugin name.
    plugin: String,
    /// Action / hook event / cron job id being run.
    action: String,
    /// Which entry point produced this run.
    kind: InvocationKind,
    /// Unix-epoch milliseconds when the run was registered.
    started_at_ms: u64,
    /// Fires the cancel arm of the run's `select!`. Dropping it (e.g. on
    /// `cancel`/`cancel_all` removing the entry) also cancels, since the
    /// receiver resolves on sender drop.
    kill_tx: oneshot::Sender<()>,
}

/// A point-in-time snapshot of one live invocation, safe to hand to the UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningInvocation {
    /// Registry id; pass to [`PluginProcessRegistry::cancel`].
    pub id: u64,
    /// Plugin name.
    pub plugin: String,
    /// Action / hook event / cron job id.
    pub action: String,
    /// Which entry point produced this run.
    pub kind: InvocationKind,
    /// Unix-epoch milliseconds when the run started.
    pub started_at_ms: u64,
}

/// Process-wide table of live plugin invocations, shared behind an `Arc`.
///
/// `RunningEntry` holds a `oneshot::Sender` (not `Debug`), so `Debug` is
/// implemented by hand to report just the live count.
#[derive(Default)]
pub struct PluginProcessRegistry {
    /// Monotonic id source; ids are never reused within a process.
    next_id: AtomicU64,
    /// Live invocations keyed by id.
    entries: Mutex<HashMap<u64, RunningEntry>>,
}

impl std::fmt::Debug for PluginProcessRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginProcessRegistry")
            .field("live", &self.lock().len())
            .finish()
    }
}

impl PluginProcessRegistry {
    /// A fresh, empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lock the entry table, recovering from a poisoned mutex rather than
    /// panicking — a stuck lock must never take down the daemon.
    fn lock(&self) -> MutexGuard<'_, HashMap<u64, RunningEntry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Register a new invocation, returning the cancel receiver the run must
    /// select on and an RAII guard that deregisters the run when dropped.
    pub fn register(
        self: &Arc<Self>,
        plugin: &str,
        action: &str,
        kind: InvocationKind,
    ) -> (oneshot::Receiver<()>, RegistrationGuard) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (kill_tx, kill_rx) = oneshot::channel();
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| u64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        self.lock().insert(
            id,
            RunningEntry {
                plugin: plugin.to_owned(),
                action: action.to_owned(),
                kind,
                started_at_ms,
                kill_tx,
            },
        );
        (kill_rx, RegistrationGuard { registry: Arc::clone(self), id })
    }

    /// Snapshot every live invocation, ordered by id (oldest first).
    #[must_use]
    pub fn list(&self) -> Vec<RunningInvocation> {
        let mut out: Vec<RunningInvocation> = self
            .lock()
            .iter()
            .map(|(id, entry)| RunningInvocation {
                id: *id,
                plugin: entry.plugin.clone(),
                action: entry.action.clone(),
                kind: entry.kind,
                started_at_ms: entry.started_at_ms,
            })
            .collect();
        out.sort_by_key(|run| run.id);
        out
    }

    /// Cancel one invocation by id. Returns whether an entry was found.
    ///
    /// Removes the entry and fires its kill signal; the run's `select!` cancel
    /// arm then drops the child, killing the process.
    pub fn cancel(&self, id: u64) -> bool {
        match self.lock().remove(&id) {
            Some(entry) => {
                let _ = entry.kill_tx.send(());
                true
            }
            None => false,
        }
    }

    /// Cancel every live invocation. Called on daemon shutdown so no plugin
    /// child outlives the app (and none can keep an inherited port bound).
    pub fn cancel_all(&self) {
        let drained = std::mem::take(&mut *self.lock());
        for (_, entry) in drained {
            let _ = entry.kill_tx.send(());
        }
    }
}

/// Removes its invocation from the registry when dropped.
///
/// Held by the run for its whole lifetime, so completion, timeout,
/// cancellation, and panic all deregister the entry.
#[must_use = "dropping the guard immediately deregisters the invocation"]
pub struct RegistrationGuard {
    /// Registry to remove from on drop.
    registry: Arc<PluginProcessRegistry>,
    /// Id of the entry this guard owns.
    id: u64,
}

impl Drop for RegistrationGuard {
    fn drop(&mut self) {
        let _ = self.registry.lock().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::{InvocationKind, PluginProcessRegistry};
    use std::sync::Arc;

    #[test]
    fn register_lists_then_guard_drop_deregisters() {
        let registry = Arc::new(PluginProcessRegistry::new());
        let (_rx, guard) =
            registry.register("s3sync", "sync_all", InvocationKind::Action);

        let listed = registry.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].plugin, "s3sync");
        assert_eq!(listed[0].action, "sync_all");
        assert_eq!(listed[0].kind, InvocationKind::Action);

        drop(guard);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn cancel_removes_entry_and_signals_receiver() {
        let registry = Arc::new(PluginProcessRegistry::new());
        let (mut rx, _guard) =
            registry.register("p", "act", InvocationKind::Hook);
        let id = registry.list()[0].id;

        assert!(registry.cancel(id));
        // Entry gone and the receiver observes the fired signal.
        assert!(registry.list().is_empty());
        assert_eq!(rx.try_recv(), Ok(()));
    }

    #[test]
    fn cancel_unknown_id_returns_false() {
        let registry = Arc::new(PluginProcessRegistry::new());
        assert!(!registry.cancel(999));
    }

    #[test]
    fn cancel_all_clears_and_signals_every_entry() {
        let registry = Arc::new(PluginProcessRegistry::new());
        let (mut rx1, _g1) = registry.register("a", "x", InvocationKind::Cron);
        let (mut rx2, _g2) =
            registry.register("b", "y", InvocationKind::Preview);

        registry.cancel_all();
        assert!(registry.list().is_empty());
        assert_eq!(rx1.try_recv(), Ok(()));
        assert_eq!(rx2.try_recv(), Ok(()));
    }

    #[test]
    fn ids_are_monotonic_and_unique() {
        let registry = Arc::new(PluginProcessRegistry::new());
        let (_r1, _g1) = registry.register("a", "x", InvocationKind::Action);
        let (_r2, _g2) = registry.register("a", "x", InvocationKind::Action);
        let ids: Vec<u64> = registry.list().iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0] < ids[1]);
    }
}
