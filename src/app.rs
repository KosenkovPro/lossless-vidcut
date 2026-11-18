//! The application shell: tabs, toolbars, status bar and settings.

use std::path::{Path, PathBuf};

use egui::{Align2, Color32, Layout, RichText, Vec2};

use crate::ffmpeg_tools::Tools;
use crate::jobs::{JobManager, Status};
use crate::joiner::Joiner;
use crate::processor;
use crate::settings::Settings;
use crate::splitter::Splitter;
use crate::toast::Toasts;

/// The available tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Cut a file into segments.
    Splitter,
    /// Glue several files together.
    Joiner,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Splitter => "✂  Splitter",
            Tab::Joiner => "⧉  Joiner",
        }
    }
}

/// The application state.
pub struct App {
    ctx: egui::Context,
    settings: Settings,
    tools: Option<Tools>,
    tab: Tab,
    splitter: Splitter,
    joiner: Joiner,
    jobs: JobManager,
    toasts: Toasts,
    show_settings: bool,
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

        let mut splitter = Splitter::new(ctx.clone());
        splitter
            .player
            .set_tools(tools.clone().unwrap_or_else(dummy_tools));
        splitter.snap = settings.snap_to_keyframes;
        let mut joiner = Joiner::default();
        joiner.set_tools(tools.clone());

        let encoder = match tools.as_ref() {
            Some(tools) => processor::detect_video_encoder(&tools.ffmpeg),
            None => processor::VideoEncoder::fallback(),
        };
        splitter.encoder = encoder.clone();
        joiner.encoder = encoder;

        Self {
            ctx,
            settings,
            tools,
            tab: Tab::Splitter,
            splitter,
            joiner,
            jobs: JobManager::default(),
            toasts: Toasts::default(),
            show_settings: false,
            status: "Ready".to_string(),
            dirty_settings: false,
        }
    }

    /// Open a media file that was handed over on the command line.
    pub fn open_path(&mut self, path: &Path) {
        let Some(tools) = self.tools.clone() else {
            self.toasts.error(
                &self.ctx,
                "FFmpeg was not found, the file cannot be opened.",
            );
            return;
        };
        if path.is_file() {
            self.splitter.adopt(path, &tools);
            self.status = format!("Opening {}", file_name(path));
        }
    }

    fn update_tools(&mut self) {
        let tools = detect_tools(&self.settings);
        if tools != self.tools {
            self.tools = tools;
            self.splitter
                .player
                .set_tools(self.tools.clone().unwrap_or_else(dummy_tools));
            self.joiner.set_tools(self.tools.clone());
            self.update_encoder();
        }
    }

    /// Find the video encoder that the installed FFmpeg provides.
    fn update_encoder(&mut self) {
        let encoder = match self.tools.as_ref() {
            Some(tools) => processor::detect_video_encoder(&tools.ffmpeg),
            None => processor::VideoEncoder::fallback(),
        };
        self.splitter.encoder = encoder.clone();
        self.joiner.encoder = encoder;
    }

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
                "FFmpeg was not found, so the dropped file cannot be opened.",
            );
            return;
        };
        for path in dropped {
            if !path.is_file() {
                continue;
            }
            match self.tab {
                Tab::Splitter => {
                    self.splitter.adopt(&path, &tools);
                    self.status = format!("Opening {}", file_name(&path));
                }
                Tab::Joiner => {
                    self.joiner.add(&path, &tools);
                    self.status = format!("Added {}", file_name(&path));
                }
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("📂 Open file…")
                .on_hover_text("Open a video file in the splitter")
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v", "ts"])
                    .pick_file()
                {
                    if let Some(tools) = self.tools.clone() {
                        self.splitter.adopt(&path, &tools);
                        self.status = format!("Opening {}", file_name(&path));
                    }
                }
            }
            if ui
                .button("➕ Add files…")
                .on_hover_text("Add files to the joiner")
                .clicked()
            {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v", "ts"])
                    .pick_files()
                {
                    if let Some(tools) = self.tools.clone() {
                        for file in files {
                            self.joiner.add(&file, &tools);
                        }
                    }
                }
            }

            ui.separator();

            for tab in [Tab::Splitter, Tab::Joiner] {
                let selected = self.tab == tab;
                if ui
                    .selectable_label(selected, tab.title())
                    .on_hover_text(if tab == Tab::Splitter {
                        "Cut a file into segments"
                    } else {
                        "Join several files"
                    })
                    .clicked()
                {
                    self.tab = tab;
                }
            }

            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("⚙").on_hover_text("Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                match self.tools.as_ref() {
                    Some(_) => {
                        ui.colored_label(Color32::from_rgb(96, 200, 120), "●");
                        ui.label("ffmpeg ready");
                    }
                    None => {
                        ui.colored_label(Color32::from_rgb(240, 180, 80), "●");
                        ui.label("ffmpeg not found");
                    }
                }
            });
        });
    }

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
                    ui.label(RichText::new("⚠ FFmpeg was not found.").strong());
                    ui.label(
                        "Install it or point the application at the folder that contains ffmpeg and ffprobe.",
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("Locate FFmpeg…").clicked() {
                        if let Some(folder) = rfd::FileDialog::new()
                            .set_title("Choose the folder that contains ffmpeg and ffprobe")
                            .pick_folder()
                        {
                            self.settings.ffmpeg_dir = Some(folder);
                            self.dirty_settings = true;
                            self.update_tools();
                            if self.tools.is_some() {
                                self.toasts.success(&self.ctx, "FFmpeg found.");
                            } else {
                                self.toasts
                                    .error(&self.ctx, "The folder does not contain ffmpeg and ffprobe.");
                            }
                        }
                    }
                    if ui.button("Retry").clicked() {
                        self.update_tools();
                    }
                });
            });
        true
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.splitter.is_loading() {
                ui.label("Reading the media file…");
            } else {
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
                    ui.label(format!("{running} running"));
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
                        if ui.small_button("Cancel").clicked() {
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
                        ui.label("cancelled");
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

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::new(0.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(4.0);

                ui.label("FFmpeg location");
                ui.horizontal_wrapped(|ui| {
                    let text = self
                        .settings
                        .ffmpeg_dir
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "searched in PATH".to_string());
                    ui.label(text);
                    if ui.button("Choose…").clicked() {
                        if let Some(folder) = rfd::FileDialog::new()
                            .set_title("Choose the folder that contains ffmpeg and ffprobe")
                            .pick_folder()
                        {
                            self.settings.ffmpeg_dir = Some(folder);
                            self.dirty_settings = true;
                        }
                    }
                    if ui.button("Auto detect").clicked() {
                        self.settings.ffmpeg_dir = None;
                        self.dirty_settings = true;
                    }
                });

                ui.separator();

                ui.label("Seek step");
                ui.add(
                    egui::Slider::new(&mut self.settings.step_seconds, 0.1..=10.0)
                        .suffix(" s")
                        .clamping(egui::SliderClamping::Never),
                );
                ui.label("Fast seek step");
                ui.add(
                    egui::Slider::new(&mut self.settings.fast_step_seconds, 1.0..=60.0)
                        .suffix(" s")
                        .clamping(egui::SliderClamping::Never),
                );
                ui.checkbox(&mut self.settings.dark_theme, "Dark theme");
                ui.checkbox(
                    &mut self.settings.snap_to_keyframes,
                    "Snap to key frames by default",
                );

                ui.separator();
                ui.label(
                    RichText::new(
                        "Press “Save” to store the settings in the configuration folder.",
                    )
                    .color(Color32::from_rgb(150, 158, 175)),
                );
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        match self.settings.save() {
                            Ok(()) => self.toasts.success(ctx, "Settings saved."),
                            Err(error) => {
                                self.toasts.error(ctx, format!("Could not save: {error:#}"))
                            }
                        }
                        self.dirty_settings = false;
                        self.update_tools();
                    }
                    if ui.button("Close").clicked() {
                        self.show_settings = false;
                    }
                });
            });
        if !open {
            self.show_settings = false;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.ctx = ctx.clone();
        self.jobs.poll();
        self.splitter.poll_load(&ctx, &mut self.toasts);
        self.handle_dropped_files();

        egui::Panel::top("toolbar").show_inside(ui, |ui| self.toolbar(ui));

        egui::Panel::bottom("status").show_inside(ui, |ui| self.status_bar(ui));

        egui::Panel::right("side")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| match self.tab {
                Tab::Splitter => {
                    self.splitter
                        .side_panel(ui, &ctx, &mut self.jobs, &mut self.toasts)
                }
                Tab::Joiner => self
                    .joiner
                    .side_panel(ui, &ctx, &mut self.jobs, &mut self.toasts),
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.missing_tools_banner(ui) {
                return;
            }
            match self.tab {
                Tab::Splitter => self
                    .splitter
                    .central(ui, &ctx, &self.settings, &mut self.toasts),
                Tab::Joiner => self.joiner.central(ui, &ctx, &mut self.toasts),
            }
        });

        self.settings_window(&ctx);
        self.toasts.draw(&ctx);

        if self.dirty_settings {
            self.splitter.snap = self.settings.snap_to_keyframes;
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
    Tools::detect().filter(|tools| tools.valid())
}

/// Placeholder used before the tools have been found.
fn dummy_tools() -> Tools {
    Tools {
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
    }
}

/// The file name of a path, or an empty string.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}
