//! Native tray host for the browser-served Localref UI.
//!
//! The tray owns only the menu icon and command dispatch. User-facing UI opens
//! in the configured browser endpoint, while daemon work continues through the
//! local REST API.

use crate::runtime_log::RuntimeLogger;
use crate::tray::{
    TrayAction, TrayController, TrayMenuItem, TrayNotification,
    TrayNotificationKind, notification_for_command,
};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const LOCALREF_ICON_ASSET: &str = "assets/favicon.ico";

enum TrayUserEvent {
    Menu(MenuEvent),
}

/// Run the native tray loop until the Quit menu action is selected.
pub fn run_native_tray(
    controller: TrayController,
    logger: RuntimeLogger,
) -> Result<(), String> {
    let notifier = NativeTrayNotifier::new(logger.clone());
    if let Err(error) = native_win32::register_app_notifications_with_icon(
        &localref_icon_path(),
    ) {
        logger.warn("tray-notification", error.to_string());
    }
    let event_loop =
        EventLoopBuilder::<TrayUserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = proxy.send_event(TrayUserEvent::Menu(event));
    }));
    let tray = LocalrefTray::new(&controller.menu_items())
        .map_err(|error| error.to_string())?;
    logger.info("tray", "native tray host started");

    let mut tray = Some(tray);
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        let Event::UserEvent(TrayUserEvent::Menu(event)) = event else {
            return;
        };
        let Some(active_tray) = tray.as_ref() else {
            *control_flow = ControlFlow::Exit;
            return;
        };
        let action = active_tray.ids.action_for(event.id());
        if !handle_tray_action(action, &controller, &notifier) {
            tray.take();
            if let Err(error) = native_win32::unregister_app_notifications() {
                logger.warn("tray-notification", error.to_string());
            }
            *control_flow = ControlFlow::Exit;
        }
    })
}

fn handle_tray_action(
    action: TrayAction,
    controller: &TrayController,
    notifier: &NativeTrayNotifier,
) -> bool {
    match action {
        TrayAction::OpenSimpleUi => {
            // open the SSR UI in default broswer
            let result =
                native_win32::open_uri(controller.rest_client().endpoint())
                    .map(|()| crate::tray::TrayCommandResult::UiRequested)
                    .map_err(|error| error.to_string());
            notifier.notify(&notification_for_command(action, &result));
            true
        }
        TrayAction::Quit => {
            let result = Ok(crate::tray::TrayCommandResult::Quit);
            notifier.notify(&notification_for_command(action, &result));
            false
        }
        _ => {
            let result = controller.run_action(action);
            notifier.notify(&notification_for_command(action, &result));
            true
        }
    }
}

#[derive(Clone)]
struct NativeTrayNotifier {
    logger: RuntimeLogger,
}

impl NativeTrayNotifier {
    /// Create a notifier that records every notification attempt.
    fn new(logger: RuntimeLogger) -> Self {
        Self { logger }
    }

    /// Deliver one tray notification through the Windows App SDK native layer.
    fn notify(&self, notification: &TrayNotification) {
        let message = format!("{}: {}", notification.title, notification.body);
        match notification.kind {
            TrayNotificationKind::Error => {
                self.logger.error("tray-notification", &message)
            }
            _ => self.logger.info("tray-notification", &message),
        }
        let kind = match notification.kind {
            TrayNotificationKind::Info => native_win32::NotificationKind::Info,
            TrayNotificationKind::Success => {
                native_win32::NotificationKind::Success
            }
            TrayNotificationKind::Error => {
                native_win32::NotificationKind::Error
            }
        };
        if let Err(error) = native_win32::show_app_notification(
            &notification.title,
            &notification.body,
            kind,
        ) {
            self.logger.warn("tray-notification", error.to_string());
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

/// Return the shared Localref icon asset path.
pub fn localref_icon_path() -> std::path::PathBuf {
    resolve_localref_icon_path(
        std::env::current_exe().ok().as_deref(),
        &workspace_icon_path(),
    )
}

/// Load the shared Localref tray icon asset.
pub fn localref_icon() -> Result<Icon, String> {
    #[cfg(windows)]
    {
        return Icon::from_path(localref_icon_path(), Some((32, 32)))
            .map_err(|error| error.to_string());
    }

    #[cfg(not(windows))]
    {
        let size = 32_u32;
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let border =
                    x == 0 || y == 0 || x == size - 1 || y == size - 1;
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
}

fn resolve_localref_icon_path(
    current_exe: Option<&std::path::Path>,
    fallback: &std::path::Path,
) -> std::path::PathBuf {
    if let Some(exe_dir) = current_exe.and_then(std::path::Path::parent) {
        let packaged_icon = exe_dir.join(LOCALREF_ICON_ASSET);
        if packaged_icon.is_file() {
            return packaged_icon;
        }
    }
    fallback.to_path_buf()
}

fn workspace_icon_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(LOCALREF_ICON_ASSET)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_icon_asset_exists() {
        assert!(localref_icon_path().is_file());
    }

    #[test]
    fn packaged_icon_next_to_exe_takes_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let exe_dir = temp.path().join("Localref");
        let assets = exe_dir.join("assets");
        std::fs::create_dir_all(&assets).unwrap();
        let icon = assets.join("favicon.ico");
        std::fs::write(&icon, [0, 0, 1, 0]).unwrap();
        let fallback = temp.path().join("fallback.ico");

        let resolved = resolve_localref_icon_path(
            Some(&exe_dir.join("localref.exe")),
            &fallback,
        );

        assert_eq!(resolved, icon);
    }

    #[test]
    fn missing_packaged_icon_uses_workspace_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("Localref").join("localref.exe");
        let fallback = temp.path().join("assets").join("favicon.ico");

        let resolved = resolve_localref_icon_path(Some(&exe), &fallback);

        assert_eq!(resolved, fallback);
    }

    #[test]
    fn shared_icon_has_valid_dimensions() {
        assert!(localref_icon().is_ok());
    }
}
