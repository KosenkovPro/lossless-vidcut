//! The application shell: toolbar, panels, status bar and settings.

use std::path::{Path, PathBuf};

use egui::{Align2, Color32, Layout, RichText, Vec2};

use crate::editor::Editor;
use crate::ffmpeg_tools::Tools;
use crate::i18n::{tf, tr, Lang};
use crate::jobs::{JobManager, Status};
use crate::processor;
use crate::settings::Settings;
use crate::toast::Toasts;

/// The application state.
pub struct App {
    ctx: egui::Context,
    settings: Settings,
    tools: Option<Tools>,
    editor: Editor,
    jobs: JobManager,
    toasts: Toasts,
    show_settings: bool,
    show_shortcuts: bool,
    status: String,
    dirty_settings: bool,
}

impl App {
    /// Create the application.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        cc.egui_ctx.set_theme(egui::Theme::Dark);

        let settings = Settings::load();
        let tools = detect_tools(&settings);

        let mut editor = Editor::new(ctx.clone());
        editor.lang = settings.language;
        editor.snap = settings.snap_to_keyframes;
        editor.set_tools(tools.clone());

        let encoder = match tools.as_ref() {
            Some(tools) => processor::detect_video_encoder(&tools.ffmpeg),
            None => processor::VideoEncoder::fallback(),
        };
        editor.encoder = encoder;

        Self {
            ctx,
            settings,
            tools,
            editor,
            jobs: JobManager::default(),
            toasts: Toasts::default(),
            show_settings: false,
            show_shortcuts: false,
            status: String::new(),
            dirty_settings: false,
        }
    }

    /// The language of the user interface.
    fn lang(&self) -> Lang {
        self.settings.language
    }

    /// Open a media file that was handed over on the command line.
    pub fn open_path(&mut self, path: &Path) {
        let Some(tools) = self.tools.clone() else {
            self.toasts.error(
                &self.ctx,
                tr(self.lang(), "The FFmpeg tools are not available."),
            );
            return;
        };
        if path.is_file() {
            self.editor.open_path(path, &tools, true);
            self.status = file_name(path);
        }
    }

    /// Look for the FFmpeg tools again, for example after the user changed
    /// the folder they live in.
    fn update_tools(&mut self) {
        let tools = detect_tools(&self.settings);
        if tools != self.tools {
            self.tools = tools;
            self.editor.set_tools(self.tools.clone());
            self.update_encoder();
        }
    }

    /// Find the video encoder that the installed FFmpeg provides.
    fn update_encoder(&mut self) {
        let encoder = match self.tools.as_ref() {
            Some(tools) => processor::detect_video_encoder(&tools.ffmpeg),
            None => processor::VideoEncoder::fallback(),
        };
        self.editor.encoder = encoder;
    }

    /// Change the language of the user interface.
    fn set_language(&mut self, language: Lang) {
        if self.settings.language == language {
            return;
        }
        self.settings.language = language;
        self.editor.lang = language;
        self.dirty_settings = true;
        if let Err(error) = self.settings.save() {
            self.toasts.error(
                &self.ctx,
                tf(
                    self.lang(),
                    "Could not save: {error}",
                    &[("error", &format!("{error:#}"))],
                ),
            );
        }
    }

    /// Add the files that were dropped onto the window.
    fn handle_dropped_files(&mut self) {
        let dropped: Vec<PathBuf> = self.ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        let Some(tools) = self.tools.clone() else {
            self.toasts.error(
                &self.ctx,
                tr(self.lang(), "The FFmpeg tools are not available."),
            );
            return;
        };
        for path in dropped {
            if !path.is_file() {
                continue;
            }
            self.editor.open_path(&path, &tools, true);
            self.status = file_name(&path);
        }
    }

    /// Handle the shortcuts that belong to the application itself.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if egui::Popup::is_any_open(ctx) {
            return;
        }
        let mut open_file = false;
        let mut export = false;
        let mut settings = false;
        let mut shortcuts = false;

        ctx.input_mut(|input| {
            let ctrl = egui::Modifiers {
                ctrl: true,
                ..egui::Modifiers::default()
            };
            if input.consume_key(ctrl, egui::Key::O) {
                open_file = true;
            }
            if input.consume_key(ctrl, egui::Key::E) {
                export = true;
            }
            if input.consume_key(ctrl, egui::Key::Comma) {
                settings = true;
            }
            if input.consume_key(ctrl, egui::Key::Slash) {
                shortcuts = true;
            }
        });

        if open_file {
            self.open_media_dialog();
        }
        if export {
            let mut toasts = std::mem::take(&mut self.toasts);
            let mut jobs = std::mem::take(&mut self.jobs);
            self.editor.export_now(&mut jobs, &mut toasts, ctx);
            self.jobs = jobs;
            self.toasts = toasts;
        }
        if settings {
            self.show_settings = !self.show_settings;
        }
        if shortcuts {
            self.show_shortcuts = !self.show_shortcuts;
        }
    }

    /// Draw the toolbar.
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button(tr(self.lang(), "Open file…"))
                .on_hover_text(tr(self.lang(), "Open file…"))
                .clicked()
            {
                self.open_media_dialog();
            }

            ui.separator();

            if ui
                .button(format!("✂ {}", tr(self.lang(), "Cut")))
                .on_hover_text(tr(self.lang(), "Split at the play head"))
                .clicked()
            {
                self.split_selected();
            }
            if ui
                .button(tr(self.lang(), "Undo"))
                .on_hover_text(tr(self.lang(), "Undo"))
                .clicked()
                && !self.editor.project.undo()
            {
                self.toasts
                    .info(&self.ctx, tr(self.lang(), "Nothing to undo."));
            }
            if ui
                .button(tr(self.lang(), "Redo"))
                .on_hover_text(tr(self.lang(), "Redo"))
                .clicked()
                && !self.editor.project.redo()
            {
                self.toasts
                    .info(&self.ctx, tr(self.lang(), "Nothing to redo."));
            }

            ui.separator();

            let mut language = self.settings.language;
            egui::ComboBox::from_id_salt("language")
                .selected_text(language.native_name())
                .show_ui(ui, |ui| {
                    for option in Lang::all() {
                        ui.selectable_value(&mut language, option, option.native_name());
                    }
                });
            if language != self.settings.language {
                self.set_language(language);
            }

            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button("⚙")
                    .on_hover_text(tr(self.lang(), "Settings"))
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                if ui
                    .button("⌨")
                    .on_hover_text(tr(self.lang(), "Keyboard shortcuts"))
                    .clicked()
                {
                    self.show_shortcuts = !self.show_shortcuts;
                }
                match self.tools.as_ref() {
                    Some(_) => {
                        ui.colored_label(Color32::from_rgb(96, 200, 120), "●");
                        ui.label(tr(self.lang(), "Ready"));
                    }
                    None => {
                        ui.colored_label(Color32::from_rgb(240, 180, 80), "●");
                        ui.label(tr(self.lang(), "FFmpeg was not found."));
                    }
                }
            });
        });
    }

    /// Cut the clip at the play head.
    fn split_selected(&mut self) {
        self.editor
            .split_selected_with_feedback(&self.ctx, &mut self.toasts);
    }

    /// Open the file dialog and add the chosen files to the project.
    fn open_media_dialog(&mut self) {
        let Some(tools) = self.tools.clone() else {
            return;
        };
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v", "ts"])
            .pick_files()
        {
            for file in files {
                self.editor.open_path(&file, &tools, true);
            }
        }
    }

    /// Show a banner when FFmpeg cannot be found.
    fn missing_tools_banner(&mut self, ui: &mut egui::Ui) -> bool {
        if self.tools.is_some() {
            return false;
        }
        egui::Frame::default()
            .fill(Color32::from_rgb(58, 44, 26))
            .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(240, 180, 80)))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(tr(self.lang(), "FFmpeg was not found.")).strong(),
                    );
                    ui.label(tr(
                        self.lang(),
                        "Install it or point the application at the folder that contains ffmpeg and ffprobe.",
                    ));
                });
                ui.horizontal(|ui| {
                    if ui.button(tr(self.lang(), "Locate FFmpeg…")).clicked() {
                        if let Some(folder) = rfd::FileDialog::new()
                            .set_title("Choose the folder that contains ffmpeg and ffprobe")
                            .pick_folder()
                        {
                            self.settings.ffmpeg_dir = Some(folder);
                            self.dirty_settings = true;
                            self.update_tools();
                            if self.tools.is_some() {
                                self.toasts.success(&self.ctx, tr(self.lang(), "FFmpeg found."));
                            } else {
                                self.toasts.error(
                                    &self.ctx,
                                    tr(
                                        self.lang(),
                                        "The folder does not contain ffmpeg and ffprobe.",
                                    ),
                                );
                            }
                        }
                    }
                    if ui.button(tr(self.lang(), "Retry")).clicked() {
                        self.update_tools();
                    }
                });
            });
        true
    }

    /// Draw the status bar and the list of running jobs.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.editor.project.is_loading() {
                ui.label(tr(self.lang(), "Reading the media file…"));
            } else if !self.status.is_empty() {
                ui.label(&self.status);
            }
            if self.jobs.any_running() {
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    let running = self
                        .jobs
                        .jobs()
                        .iter()
                        .filter(|job| job.is_running())
                        .count();
                    ui.label(tf(
                        self.lang(),
                        "{count} running",
                        &[("count", &running.to_string())],
                    ));
                });
            }
        });

        let mut cancelled = None;
        for job in self.jobs.jobs() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&job.title).strong());
                match &job.status {
                    Status::Running => {
                        ui.add(
                            egui::ProgressBar::new(job.fraction())
                                .desired_width(180.0)
                                .show_percentage(),
                        );
                        let stage = job.stage();
                        if !stage.is_empty() {
                            ui.label(stage);
                        }
                        if ui.small_button(tr(self.lang(), "Cancel")).clicked() {
                            cancelled = Some(job.id);
                        }
                    }
                    Status::Finished(path) => {
                        ui.colored_label(Color32::from_rgb(96, 200, 120), "✔");
                        ui.label(file_name(path));
                    }
                    Status::Failed(message) => {
                        ui.colored_label(Color32::from_rgb(240, 96, 96), "✖");
                        ui.label(message);
                    }
                    Status::Cancelled => {
                        ui.colored_label(Color32::from_rgb(150, 158, 175), "■");
                        ui.label(tr(self.lang(), "cancelled"));
                    }
                }
            });
        }
        if let Some(id) = cancelled {
            if let Some(job) = self.jobs.jobs().iter().find(|job| job.id == id) {
                job.cancel();
            }
        }
    }

    /// Draw the settings window.
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new(tr(self.lang(), "Settings"))
            .open(&mut open)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::new(0.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(4.0);

                ui.label(tr(self.lang(), "FFmpeg location"));
                ui.horizontal_wrapped(|ui| {
                    let text = self
                        .settings
                        .ffmpeg_dir
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| tr(self.lang(), "searched in PATH").to_string());
                    ui.label(text);
                    if ui.button(tr(self.lang(), "Choose…")).clicked() {
                        if let Some(folder) = rfd::FileDialog::new()
                            .set_title("Choose the folder that contains ffmpeg and ffprobe")
                            .pick_folder()
                        {
                            self.settings.ffmpeg_dir = Some(folder);
                            self.dirty_settings = true;
                        }
                    }
                    if ui.button(tr(self.lang(), "Auto detect")).clicked() {
                        self.settings.ffmpeg_dir = None;
                        self.dirty_settings = true;
                    }
                });

                ui.separator();

                ui.label(tr(self.lang(), "Seek step"));
                ui.add(
                    egui::Slider::new(&mut self.settings.step_seconds, 0.1..=10.0)
                        .suffix(" s")
                        .clamping(egui::SliderClamping::Never),
                );
                ui.label(tr(self.lang(), "Fast seek step"));
                ui.add(
                    egui::Slider::new(&mut self.settings.fast_step_seconds, 1.0..=60.0)
                        .suffix(" s")
                        .clamping(egui::SliderClamping::Never),
                );
                let dark_theme_label = tr(self.lang(), "Dark theme");
                ui.checkbox(&mut self.settings.dark_theme, dark_theme_label);
                let snap_label = tr(self.lang(), "Snap to key frames by default");
                ui.checkbox(&mut self.settings.snap_to_keyframes, snap_label);

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label(tr(self.lang(), "Language"));
                    let mut language = self.settings.language;
                    egui::ComboBox::from_id_salt("settings-language")
                        .selected_text(language.native_name())
                        .show_ui(ui, |ui| {
                            for option in Lang::all() {
                                ui.selectable_value(&mut language, option, option.native_name());
                            }
                        });
                    if language != self.settings.language {
                        self.set_language(language);
                    }
                });

                ui.separator();

                ui.label(tr(self.lang(), "Desktop shortcut"));
                ui.horizontal(|ui| {
                    if crate::shortcut::exists() {
                        if ui
                            .button(tr(self.lang(), "Remove desktop shortcut"))
                            .clicked()
                        {
                            match crate::shortcut::remove() {
                                Ok(()) => self.toasts.success(
                                    &self.ctx,
                                    tr(self.lang(), "The desktop shortcut was removed."),
                                ),
                                Err(error) => self.toasts.error(
                                    &self.ctx,
                                    tf(
                                        self.lang(),
                                        "Could not remove the desktop shortcut: {error}",
                                        &[("error", &format!("{error:#}"))],
                                    ),
                                ),
                            }
                        }
                    } else if ui
                        .button(tr(self.lang(), "Create desktop shortcut"))
                        .clicked()
                    {
                        match crate::shortcut::create() {
                            Ok(_) => self.toasts.success(
                                &self.ctx,
                                tr(self.lang(), "The desktop shortcut was created."),
                            ),
                            Err(error) => self.toasts.error(
                                &self.ctx,
                                tf(
                                    self.lang(),
                                    "Could not create the desktop shortcut: {error}",
                                    &[("error", &format!("{error:#}"))],
                                ),
                            ),
                        }
                    }
                });

                ui.separator();

                ui.label(
                    RichText::new(tr(
                        self.lang(),
                        "Press “Save” to store the settings in the configuration folder.",
                    ))
                    .color(Color32::from_rgb(150, 158, 175)),
                );
                ui.horizontal(|ui| {
                    if ui.button(tr(self.lang(), "Save")).clicked() {
                        match self.settings.save() {
                            Ok(()) => self
                                .toasts
                                .success(&self.ctx, tr(self.lang(), "Settings saved.")),
                            Err(error) => self.toasts.error(
                                &self.ctx,
                                tf(
                                    self.lang(),
                                    "Could not save: {error}",
                                    &[("error", &format!("{error:#}"))],
                                ),
                            ),
                        }
                        self.dirty_settings = false;
                        self.update_tools();
                    }
                    if ui.button(tr(self.lang(), "Close")).clicked() {
                        self.show_settings = false;
                    }
                });
            });
        if !open {
            self.show_settings = false;
        }
    }

    /// Draw the window that lists the keyboard shortcuts.
    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let mut open = true;
        egui::Window::new(tr(self.lang(), "Keyboard shortcuts"))
            .open(&mut open)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::new(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Grid::new("shortcuts").striped(true).show(ui, |ui| {
                    for (keys, action) in crate::i18n::SHORTCUTS {
                        ui.label(RichText::new(*keys).strong());
                        ui.label(tr(self.lang(), action));
                        ui.end_row();
                    }
                });
                ui.separator();
                if ui.button(tr(self.lang(), "Close")).clicked() {
                    self.show_shortcuts = false;
                }
            });
        if !open {
            self.show_shortcuts = false;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.ctx = ctx.clone();
        self.jobs.poll();
        self.editor.poll_loads(&ctx, &mut self.toasts);
        self.handle_dropped_files();
        self.handle_keys(&ctx);

        egui::Panel::top("toolbar").show_inside(ui, |ui| self.toolbar(ui));

        egui::Panel::bottom("status").show_inside(ui, |ui| self.status_bar(ui));

        egui::Panel::left("media")
            .resizable(true)
            .default_size(240.0)
            .show_inside(ui, |ui| {
                let mut toasts = std::mem::take(&mut self.toasts);
                self.editor.media_panel(ui, &ctx, &mut toasts);
                self.toasts = toasts;
            });

        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| {
                let mut toasts = std::mem::take(&mut self.toasts);
                let mut jobs = std::mem::take(&mut self.jobs);
                self.editor.inspector(ui, &ctx, &mut jobs, &mut toasts);
                self.jobs = jobs;
                self.toasts = toasts;
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.missing_tools_banner(ui) {
                return;
            }
            let mut toasts = std::mem::take(&mut self.toasts);
            let settings = self.settings.clone();
            self.editor.central(ui, &ctx, &settings, &mut toasts);
            self.toasts = toasts;
        });

        self.settings_window(&ctx);
        self.shortcuts_window(&ctx);
        self.toasts.draw(&ctx);

        if self.dirty_settings {
            self.editor.snap = self.settings.snap_to_keyframes;
            self.editor.lang = self.settings.language;
        }
    }
}

/// Find the FFmpeg tools, honouring the directory the user configured.
fn detect_tools(settings: &Settings) -> Option<Tools> {
    if let Some(directory) = settings.ffmpeg_dir.as_ref() {
        if let Some(tools) = Tools::from_dir(directory) {
            if tools.valid() {
                return Some(tools);
            }
        }
    }
    let tools = portable_tools().or_else(|| Tools::detect().filter(|tools| tools.valid()));
    if let Some(tools) = tools.as_ref() {
        log::info!("using FFmpeg at {}", tools.ffmpeg.display());
    }
    tools
}

/// Look for FFmpeg next to the application binary.
///
/// Portable releases ship the tools either directly beside the executable or
/// in a `ffmpeg` subdirectory, so a copied folder keeps working without any
/// installation and without touching `PATH`.
fn portable_tools() -> Option<Tools> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;

    let candidates = [dir.to_path_buf(), dir.join("ffmpeg")];
    for candidate in candidates {
        if let Some(tools) = Tools::from_dir(&candidate) {
            if tools.valid() {
                return Some(tools);
            }
        }
    }
    None
}

/// The file name of a path, or an empty string.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}
