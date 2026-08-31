//! eframe application — a steam_dark control panel wrapping the Arlecchino toolkit.
//!
//! Heavy commands (decompile / emulate / setup) are run by invoking the standalone
//! `arlecchino` CLI as a subprocess and streaming its output into the log pane, so the
//! CLI stays the single source of truth.  The light `normalize` action calls the
//! library function directly.  All user-facing text is routed through `i18n::t`.

use crate::i18n::{self, t, tf, Lang, LangPref};
use crate::{launcher, normalize, uninstall};
use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Decompile,
    Emulate,
    Setup,
    Settings,
    About,
}

pub struct ArlApp {
    tab: Tab,
    // decompile
    jar: String,
    out: String,
    deobf: bool,
    per_method: bool,
    no_autofix: bool,
    no_roundtrip: bool,
    no_rename: bool,
    norm_dir: String,
    // emulate
    emu_jar: String,
    width: String,
    height: String,
    scale: String,
    // setup
    setup_no_freej2me: bool,
    // settings
    lang_pref: LangPref,
    // runtime
    log: Arc<Mutex<String>>,
    running: Arc<AtomicBool>,
    status_key: Arc<Mutex<&'static str>>,
    cli: PathBuf,
    confirm_uninstall: bool,
    pub quit: Arc<AtomicBool>,
}

impl ArlApp {
    pub fn new(cc: &eframe::CreationContext<'_>, quit: Arc<AtomicBool>) -> Self {
        super::theme::apply(&cc.egui_ctx);
        ArlApp {
            tab: Tab::Decompile,
            jar: String::new(),
            out: "arlecchino_out".into(),
            deobf: false,
            per_method: false,
            no_autofix: false,
            no_roundtrip: false,
            no_rename: false,
            norm_dir: String::new(),
            emu_jar: String::new(),
            width: "176".into(),
            height: "220".into(),
            scale: "2".into(),
            setup_no_freej2me: false,
            lang_pref: i18n::load_pref(),
            log: Arc::new(Mutex::new(String::new())),
            running: Arc::new(AtomicBool::new(false)),
            status_key: Arc::new(Mutex::new("status_idle")),
            cli: resolve_cli_binary(),
            confirm_uninstall: false,
            quit,
        }
    }

    fn append_log(&self, s: &str) {
        if let Ok(mut l) = self.log.lock() {
            l.push_str(s);
            if !s.ends_with('\n') {
                l.push('\n');
            }
        }
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Run the standalone CLI with `args`, streaming stdout+stderr into the log.
    fn run_cli(&self, args: Vec<String>, ctx: &egui::Context) {
        if self.is_running() {
            self.append_log(t("busy_wait"));
            return;
        }
        let log = self.log.clone();
        let running = self.running.clone();
        let status = self.status_key.clone();
        let cli = self.cli.clone();
        let ctx = ctx.clone();
        let lang_env = i18n::lang().code();
        running.store(true, Ordering::Relaxed);
        if let Ok(mut st) = status.lock() {
            *st = "status_running";
        }
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            use std::process::{Command, Stdio};
            let mut child = match Command::new(&cli)
                .args(&args)
                .env("ARLECCHINO_LANG", lang_env)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    if let Ok(mut l) = log.lock() {
                        l.push_str(&format!("launch error: {e}\n"));
                    }
                    running.store(false, Ordering::Relaxed);
                    if let Ok(mut s) = status.lock() {
                        *s = "status_failed";
                    }
                    ctx.request_repaint();
                    return;
                }
            };
            let so = child.stdout.take();
            let se = child.stderr.take();
            let l1 = log.clone();
            let c1 = ctx.clone();
            let t1 = so.map(|so| {
                std::thread::spawn(move || {
                    for line in BufReader::new(so).lines().map_while(Result::ok) {
                        if let Ok(mut l) = l1.lock() {
                            l.push_str(&line);
                            l.push('\n');
                        }
                        c1.request_repaint();
                    }
                })
            });
            let l2 = log.clone();
            let c2 = ctx.clone();
            let t2 = se.map(|se| {
                std::thread::spawn(move || {
                    for line in BufReader::new(se).lines().map_while(Result::ok) {
                        if let Ok(mut l) = l2.lock() {
                            l.push_str(&line);
                            l.push('\n');
                        }
                        c2.request_repaint();
                    }
                })
            });
            if let Some(t) = t1 {
                let _ = t.join();
            }
            if let Some(t) = t2 {
                let _ = t.join();
            }
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            running.store(false, Ordering::Relaxed);
            if let Ok(mut s) = status.lock() {
                *s = if ok { "status_done" } else { "status_failed" };
            }
            ctx.request_repaint();
        });
    }

    fn do_normalize(&self, ctx: &egui::Context) {
        if self.norm_dir.trim().is_empty() {
            self.append_log(t("pick_jar_first"));
            return;
        }
        let dir = PathBuf::from(self.norm_dir.clone());
        let moved = normalize::normalize_jadx(&dir, true);
        self.append_log(&format!("normalize: moved {moved} file(s) out of defpackage in {}", dir.display()));
        ctx.request_repaint();
    }

    fn add_launcher(&self, ctx: &egui::Context) {
        let home = launcher::home_dir();
        let exec = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("arlecchino-gui"));
        match launcher::install(&home, &exec) {
            Ok(p) => self.append_log(&tf("launcher_added", &[&p.desktop.display().to_string()])),
            Err(e) => self.append_log(&format!("launcher error: {e}")),
        }
        ctx.request_repaint();
    }

    fn do_uninstall(&self, ctx: &egui::Context) {
        let home = uninstall::home_dir();
        let layout = uninstall::Layout::from_home(&home);
        let plan = uninstall::plan(&layout, false);
        match uninstall::execute(&plan, &uninstall::RealExecutor) {
            Ok(n) => self.append_log(&tf("uninstall_done", &[&n.to_string()])),
            Err(e) => self.append_log(&format!("uninstall error: {e}")),
        }
        ctx.request_repaint();
    }
}

impl eframe::App for ArlApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.quit.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Decompile, t("tab_decompile"));
                ui.selectable_value(&mut self.tab, Tab::Emulate, t("tab_emulate"));
                ui.selectable_value(&mut self.tab, Tab::Setup, t("tab_setup"));
                ui.selectable_value(&mut self.tab, Tab::Settings, t("tab_settings"));
                ui.selectable_value(&mut self.tab, Tab::About, t("tab_about"));
                ui.separator();
                ui.menu_button(t("menu_app"), |ui| {
                    if ui.button(t("add_launcher")).clicked() {
                        self.add_launcher(ctx);
                        ui.close_menu();
                    }
                    if ui
                        .button(egui::RichText::new(t("uninstall")).color(super::theme::RED))
                        .clicked()
                    {
                        self.confirm_uninstall = true;
                        ui.close_menu();
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let st = *self.status_key.lock().unwrap();
                    let color = match st {
                        "status_running" => super::theme::AMBER,
                        "status_done" => super::theme::GREEN,
                        "status_failed" => super::theme::RED,
                        _ => super::theme::DIM,
                    };
                    ui.colored_label(color, t(st));
                });
            });
        });

        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .default_height(220.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(t("log_output"));
                    if ui.button(t("clear_log")).clicked() {
                        if let Ok(mut l) = self.log.lock() {
                            l.clear();
                        }
                    }
                });
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let text = self.log.lock().map(|l| l.clone()).unwrap_or_default();
                        ui.add(
                            egui::Label::new(egui::RichText::new(text).monospace())
                                .wrap(),
                        );
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Decompile => self.ui_decompile(ui, ctx),
            Tab::Emulate => self.ui_emulate(ui, ctx),
            Tab::Setup => self.ui_setup(ui, ctx),
            Tab::Settings => self.ui_settings(ui, ctx),
            Tab::About => self.ui_about(ui),
        });

        if self.confirm_uninstall {
            egui::Window::new(t("uninstall_confirm_title"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(t("uninstall_confirm_body"));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(egui::RichText::new(t("confirm_yes")).color(super::theme::RED))
                            .clicked()
                        {
                            self.confirm_uninstall = false;
                            self.do_uninstall(ctx);
                        }
                        if ui.button(t("confirm_no")).clicked() {
                            self.confirm_uninstall = false;
                        }
                    });
                });
        }
    }
}

impl ArlApp {
    fn path_row(&mut self, ui: &mut egui::Ui, label: &str, value_is_jar: bool, field: FieldSel) {
        ui.horizontal(|ui| {
            ui.label(label);
            let buf = self.field_mut(field);
            ui.add(egui::TextEdit::singleline(buf).desired_width(360.0));
            if ui.button(t("browse")).clicked() {
                if let Some(p) = browse_path(!value_is_jar) {
                    *self.field_mut(field) = p;
                }
            }
        });
    }

    fn field_mut(&mut self, f: FieldSel) -> &mut String {
        match f {
            FieldSel::Jar => &mut self.jar,
            FieldSel::Out => &mut self.out,
            FieldSel::Norm => &mut self.norm_dir,
            FieldSel::EmuJar => &mut self.emu_jar,
        }
    }

    fn ui_decompile(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading(t("tab_decompile"));
        ui.add_space(6.0);
        self.path_row(ui, t("input_jar"), true, FieldSel::Jar);
        self.path_row(ui, t("output_dir"), false, FieldSel::Out);
        ui.add_space(4.0);
        ui.checkbox(&mut self.deobf, t("opt_deobf"));
        ui.checkbox(&mut self.per_method, t("opt_per_method"));
        ui.checkbox(&mut self.no_autofix, t("opt_no_autofix"));
        ui.checkbox(&mut self.no_roundtrip, t("opt_no_roundtrip"));
        ui.checkbox(&mut self.no_rename, t("opt_no_rename"));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let enabled = !self.is_running() && !self.jar.trim().is_empty();
            if ui
                .add_enabled(enabled, egui::Button::new(t("run_decompile")))
                .clicked()
            {
                let mut args = vec!["decompile".to_string(), self.jar.clone(), "-o".into(), self.out.clone()];
                if self.deobf {
                    args.push("--deobf".into());
                }
                if self.per_method {
                    args.push("--per-method".into());
                }
                if self.no_autofix {
                    args.push("--no-autofix".into());
                }
                if self.no_roundtrip {
                    args.push("--no-roundtrip".into());
                }
                if self.no_rename {
                    args.push("--no-rename-members".into());
                }
                self.run_cli(args, ctx);
            }
        });

        ui.add_space(16.0);
        ui.separator();
        ui.label(t("pick_jadx_tree"));
        self.path_row(ui, t("pick_jadx_tree"), false, FieldSel::Norm);
        if ui.button(t("normalize_only")).clicked() {
            self.do_normalize(ctx);
        }
    }

    fn ui_emulate(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading(t("tab_emulate"));
        ui.add_space(6.0);
        self.path_row(ui, t("emu_jar"), true, FieldSel::EmuJar);
        ui.horizontal(|ui| {
            ui.label(t("emu_width"));
            ui.add(egui::TextEdit::singleline(&mut self.width).desired_width(60.0));
            ui.label(t("emu_height"));
            ui.add(egui::TextEdit::singleline(&mut self.height).desired_width(60.0));
            ui.label(t("emu_scale"));
            ui.add(egui::TextEdit::singleline(&mut self.scale).desired_width(40.0));
        });
        ui.add_space(8.0);
        let enabled = !self.is_running() && !self.emu_jar.trim().is_empty();
        if ui
            .add_enabled(enabled, egui::Button::new(t("run_emulate")))
            .clicked()
        {
            let args = vec![
                "emulate".to_string(),
                self.emu_jar.clone(),
                "-W".into(),
                self.width.clone(),
                "-H".into(),
                self.height.clone(),
                "-s".into(),
                self.scale.clone(),
            ];
            self.run_cli(args, ctx);
        }
    }

    fn ui_setup(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading(t("tab_setup"));
        ui.add_space(6.0);
        ui.label(t("setup_desc"));
        ui.checkbox(&mut self.setup_no_freej2me, t("setup_no_freej2me"));
        ui.add_space(8.0);
        if ui
            .add_enabled(!self.is_running(), egui::Button::new(t("run_setup")))
            .clicked()
        {
            let mut args = vec!["setup".to_string()];
            if self.setup_no_freej2me {
                args.push("--no-freej2me".into());
            }
            self.run_cli(args, ctx);
        }
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading(t("tab_settings"));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(t("settings_language"));
            let mut chosen = self.lang_pref;
            egui::ComboBox::from_id_salt("ui_language")
                .selected_text(pref_label(chosen))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut chosen,
                        LangPref::System,
                        pref_label(LangPref::System),
                    );
                    for l in Lang::ALL {
                        // Language names stay in their own language, never translated.
                        ui.selectable_value(&mut chosen, LangPref::Fixed(*l), l.native_name());
                    }
                });
            if chosen != self.lang_pref {
                self.set_language(chosen, ctx);
            }
        });
        ui.add_space(4.0);
        ui.colored_label(super::theme::DIM, t("settings_language_hint"));
    }

    /// Switch the UI language now and remember the choice for the next start.
    fn set_language(&mut self, pref: LangPref, ctx: &egui::Context) {
        self.lang_pref = pref;
        if let Err(e) = i18n::set_pref(pref) {
            // The switch itself already took effect; only storing it failed.
            self.append_log(&format!("settings error: {e}"));
        }
        // Anything drawn outside the per-frame `t()` calls has to be re-sent.
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(t("app_title").to_string()));
        #[cfg(feature = "tray")]
        crate::tray::refresh();
        ctx.request_repaint();
    }

    fn ui_about(&mut self, ui: &mut egui::Ui) {
        ui.heading(t("app_title"));
        ui.add_space(8.0);
        ui.label(t("about_body"));
        ui.add_space(8.0);
        ui.colored_label(super::theme::DIM, format!("v{}", crate::VERSION));
    }
}

/// Menu label for a language preference: "System" is translated, a concrete language
/// is always shown under its own name.
fn pref_label(p: LangPref) -> &'static str {
    match p {
        LangPref::System => t("settings_language_system"),
        LangPref::Fixed(l) => l.native_name(),
    }
}

#[derive(Clone, Copy)]
enum FieldSel {
    Jar,
    Out,
    Norm,
    EmuJar,
}

/// Locate the standalone `arlecchino` CLI: a sibling of this GUI binary, else PATH.
fn resolve_cli_binary() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("arlecchino");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from("arlecchino")
}

/// Open a native file/dir picker via kdialog or zenity (no extra crate dependency).
fn browse_path(dir: bool) -> Option<String> {
    use std::process::Command;
    for (bin, args) in [
        ("kdialog", if dir { vec!["--getexistingdirectory", "."] } else { vec!["--getopenfilename", "."] }),
        ("zenity", if dir { vec!["--file-selection", "--directory"] } else { vec!["--file-selection"] }),
    ] {
        if let Ok(out) = Command::new(bin).args(&args).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}
