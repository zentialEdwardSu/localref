//! Native iced tray host for Localref.
//!
//! The tray icon remains a lightweight command source. Menu events are bridged
//! into the iced desktop app through a standard channel that the UI polls via an
//! iced subscription, keeping window ownership inside `iced::daemon`.

use std::sync::mpsc;

use crate::runtime_log::RuntimeLogger;
use crate::tray::{
    TrayAction, TrayController, TrayMenuItem, TrayNotification,
    TrayNotificationKind, notification_for_command,
};
use crate::ui::desktop_app::{DesktopLaunchOptions, DesktopSignal};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Run the native tray-hosted iced app with explicit startup visibility.
pub fn run_native_tray_with_options(
    controller: TrayController,
    options: DesktopLaunchOptions,
    logger: RuntimeLogger,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let tray = LocalrefTray::new(&controller.menu_items())
        .map_err(|error| error.to_string())?;
    spawn_menu_thread(
        sender,
        tray.ids.clone(),
        controller.clone(),
        NativeTrayNotifier::new(logger.clone()),
    );
    logger.info("tray", "native tray host started");
    let result =
        crate::ui::desktop_app::launch_with_client_signals_and_options(
            controller.rest_client(),
            receiver,
            options,
        );
    drop(tray);
    result
}

fn spawn_menu_thread(
    sender: mpsc::Sender<DesktopSignal>,
    ids: TrayMenuIds,
    controller: TrayController,
    notifier: NativeTrayNotifier,
) {
    std::thread::Builder::new()
        .name("localref-tray-menu".to_string())
        .spawn(move || {
            while let Ok(event) = MenuEvent::receiver().recv() {
                let action = ids.action_for(event.id());
                if !handle_tray_action(action, &controller, &notifier, &sender)
                {
                    break;
                }
            }
        })
        .expect("failed to start Localref tray menu thread");
}

fn handle_tray_action(
    action: TrayAction,
    controller: &TrayController,
    notifier: &NativeTrayNotifier,
    sender: &mpsc::Sender<DesktopSignal>,
) -> bool {
    match action {
        TrayAction::OpenSimpleUi => sender.send(DesktopSignal::Open).is_ok(),
        TrayAction::Quit => {
            let result = Ok(crate::tray::TrayCommandResult::Quit);
            notifier.notify(&notification_for_command(action, &result));
            sender.send(DesktopSignal::Quit).is_ok()
        }
        _ => {
            let result = controller.run_action(action);
            notifier.notify(&notification_for_command(action, &result));
            sender.send(DesktopSignal::Refresh).is_ok()
        }
    }
}

#[derive(Clone)]
struct NativeTrayNotifier {
    logger: RuntimeLogger,
}

impl NativeTrayNotifier {
    /// Create a notifier that records every tray notification in runtime logs.
    fn new(logger: RuntimeLogger) -> Self {
        Self { logger }
    }

    /// Deliver one tray notification and record the delivery attempt.
    fn notify(&self, notification: &TrayNotification) {
        let message = format!("{}: {}", notification.title, notification.body);
        match notification.kind {
            TrayNotificationKind::Error => {
                self.logger.error("tray-notification", &message)
            }
            _ => self.logger.info("tray-notification", &message),
        }
        #[cfg(windows)]
        if let Err(error) = show_platform_notification(notification) {
            self.logger.warn("tray-notification", error);
        }
    }
}

struct LocalrefTray {
    _icon: TrayIcon,
    ids: TrayMenuIds,
}

impl LocalrefTray {
    fn new(
        menu_items: &[TrayMenuItem],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let menu = Menu::new();
        let mut ids = Vec::with_capacity(menu_items.len());
        for model in menu_items {
            let item = MenuItem::new(model.label, true, None);
            menu.append(&item)?;
            ids.push(TrayMenuId {
                id: item.id().clone(),
                action: model.action,
            });
        }

        let icon = TrayIconBuilder::new()
            .with_tooltip("Localref")
            .with_icon(localref_icon()?)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Self { _icon: icon, ids: TrayMenuIds { ids } })
    }
}

#[derive(Clone)]
struct TrayMenuIds {
    ids: Vec<TrayMenuId>,
}

#[derive(Clone)]
struct TrayMenuId {
    id: MenuId,
    action: TrayAction,
}

impl TrayMenuIds {
    fn action_for(&self, id: &MenuId) -> TrayAction {
        self.ids
            .iter()
            .find(|entry| id == &entry.id)
            .map(|entry| entry.action)
            .unwrap_or(TrayAction::RefreshStatus)
    }
}

#[cfg(windows)]
/// No-op Windows notification hook until a native notifier is added.
fn show_platform_notification(
    _notification: &TrayNotification,
) -> Result<(), String> {
    Ok(())
}

/// Build a small generated Localref tray icon.
pub fn localref_icon() -> Result<Icon, String> {
    let size = 32_u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let border = x == 0 || y == 0 || x == size - 1 || y == size - 1;
            let diagonal = x == y || x + y == size - 1;
            let (r, g, b, a) = if border || diagonal {
                (0x00, 0x2F, 0xA7, 0xFF)
            } else {
                (0xFF, 0xFF, 0xFF, 0xFF)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Icon::from_rgba(rgba, size, size).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_icon_has_valid_dimensions() {
        assert!(localref_icon().is_ok());
    }
}
