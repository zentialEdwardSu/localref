//! Lightweight tray entry support.
//!
//! This crate owns the desktop tray command boundary. It calls the user-facing
//! REST API and can request that the single `localref` process show its egui
//! simple UI, but it does not access the Localref library filesystem directly.

use localref_config::LocalrefConfig;
use ui::{DashboardSnapshot, RestClient};

#[cfg(feature = "native")]
pub mod native;

/// Tray-visible daemon state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrayStatus {
    /// REST API responded and no attention state is active.
    Running,
    /// The daemon has a queued or running task.
    Busy,
    /// One or more daemon pause modes are active.
    Paused(Vec<String>),
    /// There are pending imports or events that may need user attention.
    PendingUserAction,
    /// REST API could not be reached.
    Error(String),
}

/// Command exposed by the tray menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrayAction {
    /// Refresh status from the REST API.
    RefreshStatus,
    /// Open the Dioxus simple UI.
    OpenSimpleUi,
    /// Ask the daemon to rebuild the query cache.
    RunScan,
    /// Pause watcher-driven work.
    PauseWatcher,
    /// Pause write operations.
    PauseWrites,
    /// Resume watcher-driven work.
    ResumeWatcher,
    /// Resume write operations.
    ResumeWrites,
    /// Quit the tray process.
    Quit,
}

/// One tray menu item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrayMenuItem {
    /// User-visible menu label.
    pub label: &'static str,
    /// Command invoked by the item.
    pub action: TrayAction,
}

/// Thin controller used by tray shells.
#[derive(Clone, Debug)]
pub struct TrayController {
    client: RestClient,
}

impl TrayController {
    /// Create a tray controller using a REST client.
    pub fn new(client: RestClient) -> Self {
        Self { client }
    }

    /// Create a tray controller from an already loaded Localref configuration.
    pub fn from_config(config: &LocalrefConfig) -> Self {
        Self::new(RestClient::from_config(config))
    }

    /// Create a tray controller by loading the Localref configuration file.
    pub fn from_config_file() -> Result<Self, String> {
        LocalrefConfig::load().map(|config| Self::from_config(&config))
    }

    /// Return a clone of the REST client used by this controller.
    pub fn rest_client(&self) -> RestClient {
        self.client.clone()
    }

    /// Return the tray menu model.
    pub fn menu_items(&self) -> Vec<TrayMenuItem> {
        vec![
            TrayMenuItem {
                label: "Open Simple UI",
                action: TrayAction::OpenSimpleUi,
            },
            TrayMenuItem { label: "Run Scan", action: TrayAction::RunScan },
            TrayMenuItem {
                label: "Pause Watcher",
                action: TrayAction::PauseWatcher,
            },
            TrayMenuItem {
                label: "Pause Writes",
                action: TrayAction::PauseWrites,
            },
            TrayMenuItem {
                label: "Resume Watcher",
                action: TrayAction::ResumeWatcher,
            },
            TrayMenuItem {
                label: "Resume Writes",
                action: TrayAction::ResumeWrites,
            },
            TrayMenuItem {
                label: "Refresh Status",
                action: TrayAction::RefreshStatus,
            },
            TrayMenuItem { label: "Quit", action: TrayAction::Quit },
        ]
    }

    /// Return the current tray status.
    pub fn status(&self) -> TrayStatus {
        let daemon_status = match self.client.daemon_status() {
            Ok(status) => status,
            Err(message) => return TrayStatus::Error(message),
        };
        if !daemon_status.paused_modes.is_empty() {
            return TrayStatus::Paused(daemon_status.paused_modes);
        }
        if daemon_status.running || daemon_status.queued_tasks > 0 {
            return TrayStatus::Busy;
        }
        match self.client.dashboard_snapshot() {
            Ok(snapshot) if snapshot.pending_count > 0 => {
                TrayStatus::PendingUserAction
            }
            Ok(_) => TrayStatus::Running,
            Err(message) => TrayStatus::Error(message),
        }
    }

    /// Execute one tray action.
    pub fn run_action(
        &self,
        action: TrayAction,
    ) -> Result<TrayCommandResult, String> {
        match action {
            TrayAction::RefreshStatus => {
                Ok(TrayCommandResult::Status(self.status()))
            }
            TrayAction::OpenSimpleUi => Ok(TrayCommandResult::UiRequested),
            TrayAction::RunScan => {
                let snapshot = self.scan()?;
                Ok(TrayCommandResult::Snapshot(snapshot))
            }
            TrayAction::PauseWatcher => {
                self.client.pause("watcher")?;
                Ok(TrayCommandResult::Status(self.status()))
            }
            TrayAction::PauseWrites => {
                self.client.pause("writes")?;
                Ok(TrayCommandResult::Status(self.status()))
            }
            TrayAction::ResumeWatcher => {
                self.client.resume("watcher")?;
                Ok(TrayCommandResult::Status(self.status()))
            }
            TrayAction::ResumeWrites => {
                self.client.resume("writes")?;
                Ok(TrayCommandResult::Status(self.status()))
            }
            TrayAction::Quit => Ok(TrayCommandResult::Quit),
        }
    }

    /// Trigger a daemon scan from the tray.
    pub fn scan(&self) -> Result<DashboardSnapshot, String> {
        self.client.scan()?;
        self.client.dashboard_snapshot()
    }
}

/// Result of one tray command.
#[derive(Clone, Debug, PartialEq)]
pub enum TrayCommandResult {
    /// The command returned status.
    Status(TrayStatus),
    /// The command returned dashboard counts.
    Snapshot(DashboardSnapshot),
    /// The command produced a user-visible message.
    Message(String),
    /// The single `localref` process should show its in-process simple UI.
    UiRequested,
    /// The tray process should quit.
    Quit,
}

/// Return a compact user-visible status label.
pub fn status_label(status: &TrayStatus) -> String {
    match status {
        TrayStatus::Running => "Localref: running".to_string(),
        TrayStatus::Busy => "Localref: busy".to_string(),
        TrayStatus::Paused(modes) => {
            format!("Localref: paused ({})", modes.join(", "))
        }
        TrayStatus::PendingUserAction => {
            "Localref: pending user action".to_string()
        }
        TrayStatus::Error(message) => format!("Localref: error: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_model_keeps_complex_management_in_simple_ui() {
        let controller =
            TrayController::new(RestClient::new("http://127.0.0.1:1"));
        let menu = controller.menu_items();
        let actions = menu.iter().map(|item| item.action).collect::<Vec<_>>();
        let labels = menu.iter().map(|item| item.label).collect::<Vec<_>>();

        assert!(actions.contains(&TrayAction::OpenSimpleUi));
        assert!(actions.contains(&TrayAction::RunScan));
        assert!(actions.contains(&TrayAction::PauseWrites));
        assert!(actions.contains(&TrayAction::Quit));
        assert!(
            labels.iter().all(|label| !label.contains("Category")
                && !label.contains("Metadata"))
        );
    }

    #[test]
    fn error_status_carries_message() {
        let controller =
            TrayController::new(RestClient::new("http://127.0.0.1:1"));

        assert!(matches!(controller.status(), TrayStatus::Error(_)));
    }

    #[test]
    fn status_labels_are_stable() {
        assert_eq!(status_label(&TrayStatus::Running), "Localref: running");
        assert_eq!(
            status_label(&TrayStatus::Paused(vec!["writes".to_string()])),
            "Localref: paused (writes)"
        );
    }
}
