//! Native iced tray host for Localref.
//!
//! The tray icon remains a lightweight command source. Menu events are bridged
//! into the iced desktop app through a standard channel that the UI polls via an
//! iced subscription, keeping window ownership inside `iced::daemon`.

use std::sync::mpsc;

use crate::TrayController;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use ui::desktop_app::{DesktopLaunchOptions, DesktopSignal};

/// Run the native tray-hosted iced app until the user chooses Quit.
pub fn run_native_tray(controller: TrayController) -> Result<(), String> {
    run_native_tray_with_options(controller, DesktopLaunchOptions::visible())
}

/// Run the native tray-hosted iced app and ignore legacy UI callbacks.
pub fn run_native_tray_with_ui(
    controller: TrayController,
    _show_ui: impl FnMut() -> Result<(), String> + 'static,
) -> Result<(), String> {
    run_native_tray_with_options(controller, DesktopLaunchOptions::visible())
}

/// Run the native tray-hosted iced app with explicit startup visibility.
pub fn run_native_tray_with_options(
    controller: TrayController,
    options: DesktopLaunchOptions,
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let tray = LocalrefTray::new().map_err(|error| error.to_string())?;
    spawn_menu_thread(sender, tray.ids.clone());
    let result = ui::desktop_app::launch_with_client_signals_and_options(
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
    fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let menu = Menu::new();
        let open = MenuItem::new("Open Simple UI", true, None);
        let scan = MenuItem::new("Run Scan", true, None);
        let pause_watcher = MenuItem::new("Pause Watcher", true, None);
        let pause_writes = MenuItem::new("Pause Writes", true, None);
        let resume_watcher = MenuItem::new("Resume Watcher", true, None);
        let resume_writes = MenuItem::new("Resume Writes", true, None);
        let refresh = MenuItem::new("Refresh Status", true, None);
        let quit = MenuItem::new("Quit", true, None);
        for item in [
            &open,
            &scan,
            &pause_watcher,
            &pause_writes,
            &resume_watcher,
            &resume_writes,
            &refresh,
            &quit,
        ] {
            menu.append(item)?;
        }

        let icon = TrayIconBuilder::new()
            .with_tooltip("Localref")
            .with_icon(localref_icon()?)
            .with_menu(Box::new(menu))
            .build()?;
        Ok(Self {
            _icon: icon,
            ids: TrayMenuIds {
                open: open.id().clone(),
                scan: scan.id().clone(),
                pause_watcher: pause_watcher.id().clone(),
                pause_writes: pause_writes.id().clone(),
                resume_watcher: resume_watcher.id().clone(),
                resume_writes: resume_writes.id().clone(),
                refresh: refresh.id().clone(),
                quit: quit.id().clone(),
            },
        })
    }
}

#[derive(Clone)]
struct TrayMenuIds {
    open: MenuId,
    scan: MenuId,
    pause_watcher: MenuId,
    pause_writes: MenuId,
    resume_watcher: MenuId,
    resume_writes: MenuId,
    refresh: MenuId,
    quit: MenuId,
}

impl TrayMenuIds {
    fn signal_for(&self, id: &MenuId) -> DesktopSignal {
        if id == &self.open {
            DesktopSignal::Open
        } else if id == &self.scan {
            DesktopSignal::Scan
        } else if id == &self.pause_watcher {
            DesktopSignal::PauseWatcher
        } else if id == &self.pause_writes {
            DesktopSignal::PauseWrites
        } else if id == &self.resume_watcher {
            DesktopSignal::ResumeWatcher
        } else if id == &self.resume_writes {
            DesktopSignal::ResumeWrites
        } else if id == &self.refresh {
            DesktopSignal::Refresh
        } else if id == &self.quit {
            DesktopSignal::Quit
        } else {
            DesktopSignal::Refresh
        }
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
