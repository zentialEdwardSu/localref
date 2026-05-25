//! Native iced tray host for Localref.
//!
//! The tray icon remains a lightweight command source. Menu events are bridged
//! into the iced desktop app through a standard channel that the UI polls via an
//! iced subscription, keeping window ownership inside `iced::daemon`.

use std::sync::mpsc;

use crate::tray::{TrayAction, TrayController, TrayMenuItem};
use crate::ui::desktop_app::{DesktopLaunchOptions, DesktopSignal};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Run the native tray-hosted iced app with explicit startup visibility.
pub fn run_native_tray_with_options(
    controller: TrayController,
    options: DesktopLaunchOptions,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let tray = LocalrefTray::new(&controller.menu_items())
        .map_err(|error| error.to_string())?;
    spawn_menu_thread(sender, tray.ids.clone());
    let result =
        crate::ui::desktop_app::launch_with_client_signals_and_options(
            controller.rest_client(),
            receiver,
            options,
        );
    drop(tray);
    result
}

fn spawn_menu_thread(sender: mpsc::Sender<DesktopSignal>, ids: TrayMenuIds) {
    std::thread::Builder::new()
        .name("localref-tray-menu".to_string())
        .spawn(move || {
            while let Ok(event) = MenuEvent::receiver().recv() {
                let signal = ids.signal_for(event.id());
                if sender.send(signal).is_err() {
                    break;
                }
            }
        })
        .expect("failed to start Localref tray menu thread");
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
    fn signal_for(&self, id: &MenuId) -> DesktopSignal {
        self.ids
            .iter()
            .find(|entry| id == &entry.id)
            .map(|entry| signal_for_action(entry.action))
            .unwrap_or(DesktopSignal::Refresh)
    }
}

fn signal_for_action(action: TrayAction) -> DesktopSignal {
    match action {
        TrayAction::OpenSimpleUi => DesktopSignal::Open,
        TrayAction::RunScan => DesktopSignal::Scan,
        TrayAction::PauseWatcher => DesktopSignal::PauseWatcher,
        TrayAction::PauseWrites => DesktopSignal::PauseWrites,
        TrayAction::ResumeWatcher => DesktopSignal::ResumeWatcher,
        TrayAction::ResumeWrites => DesktopSignal::ResumeWrites,
        TrayAction::RefreshStatus => DesktopSignal::Refresh,
        TrayAction::Quit => DesktopSignal::Quit,
    }
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
