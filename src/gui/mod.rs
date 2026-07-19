//! GUI frontend (feature `gui`): a steam_dark eframe control panel over the toolkit.

pub mod app;
pub mod theme;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Launch the GUI. Blocks until the window closes.
pub fn run() -> eframe::Result<()> {
    let quit = Arc::new(AtomicBool::new(false));

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_app_id(crate::launcher::WM_CLASS)
            .with_title(crate::i18n::t("app_title"))
            .with_inner_size([760.0, 640.0])
            .with_min_inner_size([560.0, 460.0]),
        ..Default::default()
    };

    let quit_for_app = quit.clone();
    eframe::run_native(
        crate::launcher::WM_CLASS,
        options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            let app = app::ArlApp::new(cc, quit_for_app.clone());

            // System tray (feature `tray`): optional, must not fail the GUI.
            #[cfg(feature = "tray")]
            crate::tray::spawn(ctx.clone(), quit_for_app.clone());
            let _ = ctx;

            Ok(Box::new(app))
        }),
    )
}
