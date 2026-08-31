//! System tray (StatusNotifierItem) via ksni — feature `gui` + `tray`.
//!
//! A tray icon with a small menu: status line, Open panel, Quit.  "Open panel" raises
//! and focuses the eframe window through the shared `egui::Context`; "Quit" sets a
//! shared flag the window checks each frame.  DBus errors must not crash the GUI — the
//! tray is optional.

use crate::i18n::{t, tf};
use eframe::egui;
use ksni::menu::{MenuItem, StandardItem};
use ksni::ToolTip;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// Themed icon used when the app icon has not been installed into the user's icon
/// theme (i.e. "Add to application menu" was never pressed).
const FALLBACK_ICON_NAME: &str = "applications-development";

/// Prefer our own icon once it exists under `hicolor`; otherwise fall back to a generic
/// themed one, because a StatusNotifierItem with an unresolvable name shows nothing.
fn tray_icon_name() -> String {
    let home = crate::launcher::home_dir();
    if crate::launcher::icon_paths(&home).iter().any(|p| p.exists()) {
        crate::launcher::ICON_NAME.to_string()
    } else {
        FALLBACK_ICON_NAME.to_string()
    }
}

pub struct ArlTray {
    pub quit: Arc<AtomicBool>,
    pub ctx: egui::Context,
}

impl ksni::Tray for ArlTray {
    fn id(&self) -> String {
        "arlecchino-gui".into()
    }
    fn title(&self) -> String {
        "Arlecchino".into()
    }
    fn icon_name(&self) -> String {
        tray_icon_name()
    }
    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: tray_icon_name(),
            icon_pixmap: Vec::new(),
            title: "Arlecchino".into(),
            description: tf("tray_status", &[t("status_idle")]),
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: tf("tray_status", &[t("status_idle")]),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: t("tray_open").into(),
                activate: Box::new(|s: &mut Self| {
                    s.ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    s.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    s.ctx.request_repaint();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: t("tray_quit").into(),
                activate: Box::new(|s: &mut Self| {
                    s.quit.store(true, Ordering::Relaxed);
                    s.ctx.request_repaint();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Handle of the running tray, kept so the menu can be rebuilt (e.g. after the user
/// switches the interface language).
static HANDLE: OnceLock<ksni::Handle<ArlTray>> = OnceLock::new();

/// Spawn the tray in a background ksni thread. Failures are swallowed (tray optional).
pub fn spawn(ctx: egui::Context, quit: Arc<AtomicBool>) {
    let tray = ArlTray { quit, ctx };
    let service = ksni::TrayService::new(tray);
    let _ = HANDLE.set(service.handle());
    service.spawn();
}

/// Ask the tray to rebuild its menu and tool-tip from the current language tables.
pub fn refresh() {
    if let Some(h) = HANDLE.get() {
        h.update(|_| {});
    }
}
