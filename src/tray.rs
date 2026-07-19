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
use std::sync::Arc;

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
        // fall back to a generic development icon if the app icon is not themed
        "applications-development".into()
    }
    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: "applications-development".into(),
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

/// Spawn the tray in a background ksni thread. Failures are swallowed (tray optional).
pub fn spawn(ctx: egui::Context, quit: Arc<AtomicBool>) {
    let tray = ArlTray { quit, ctx };
    let service = ksni::TrayService::new(tray);
    service.spawn();
}
