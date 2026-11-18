//! The splitter tab: preview, cut points, segments and export.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use anyhow::Result;
use egui::{Color32, Layout, RichText, Sense, Vec2};

use crate::ffmpeg_tools::Tools;
use crate::jobs::JobManager;
use crate::loader::{self, LoadedMedia};
use crate::player::Player;
use crate::probe::{self, MediaInfo};
use crate::processor::{self, EncodeMode};
use crate::settings::Settings;
use crate::timeline::{self, Segment, TimelineState, TimelineView};
use crate::toast::Toasts;
use crate::util::{format_bytes, format_fps, format_timecode, format_timecode_ms};

/// What an export produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportMode {
    /// One file per selected segment.
    Separate,
    /// One file that contains all selected segments.
    Joined,
}

/// The state of the splitter tab.
pub struct Splitter {
    /// Path of the loaded file.
    pub path: Option<PathBuf>,
    /// Metadata of the loaded file.
    pub info: Option<MediaInfo>,
    /// Timestamps of the key frames.
    pub keyframes: Vec<f64>,
    /// Normalised peak amplitudes of the audio track.
    pub waveform: Vec<f32>,
    /// The segments the user selected.
    pub segments: Vec<Segment>,
    /// Start of the segment that is being marked.
    pub in_point: Option<f64>,
    /// End of the segment that is being marked.
    pub out_point: Option<f64>,
    /// Snap the cut points to key frames.
    pub snap: bool,
    /// What the export produces.
    pub export_mode: ExportMode,
    /// How the export is encoded.
    pub encode_mode: EncodeMode,
    /// The video encoder that is used for precise cuts.
    pub encoder: processor::VideoEncoder,
    /// Directory the exported files are written to.
    pub output_dir: Option<PathBuf>,
    /// Zoom and scroll state of the timeline.
    pub timeline: TimelineState,
    /// The preview player.
    pub player: Player,
    load_rx: Option<Receiver<Result<LoadedMedia>>>,
    loading: bool,
}

impl Splitter {
    /// Create an empty splitter tab.
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            path: None,
            info: None,
            keyframes: Vec::new(),
            waveform: Vec::new(),
            segments: Vec::new(),
            in_point: None,
            out_point: None,
            snap: true,
            export_mode: ExportMode::Separate,
            encode_mode: EncodeMode::Lossless,
            encoder: processor::VideoEncoder::fallback(),
            output_dir: None,
            timeline: TimelineState::default(),
            player: Player::new(ctx),
            load_rx: None,
            loading: false,
        }
    }

    /// `true` while the metadata of a file is being read.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Start loading a media file.
    pub fn open(&mut self, path: &Path, tools: &Tools) {
        self.loading = true;
        self.load_rx = Some(loader::load_async(tools.clone(), path.to_path_buf()));
        self.path = Some(path.to_path_buf());
    }

    /// Adopt a file that was dropped onto the window.
    pub fn adopt(&mut self, path: &Path, tools: &Tools) {
        self.player.close();
        self.segments.clear();
        self.in_point = None;
        self.out_point = None;
        self.keyframes.clear();
        self.waveform.clear();
        self.info = None;
        self.open(path, tools);
    }

    /// Collect the result of a background load.
    pub fn poll_load(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(receiver) = self.load_rx.as_ref() else {
            return;
        };
        let finished = match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err(anyhow::anyhow!("the loader stopped unexpectedly")))
            }
        };
        let Some(result) = finished else {
            return;
        };
        self.load_rx = None;
        self.loading = false;
        match result {
            Ok(loaded) => {
                let LoadedMedia {
                    path,
                    info,
                    keyframes,
                    waveform,
                } = loaded;
                match self.player.open(&path, info.clone()) {
                    Ok(()) => {
                        self.info = Some(info);
                        self.keyframes = keyframes;
                        self.waveform = waveform;
                        self.output_dir = path
                            .parent()
                            .map(Path::to_path_buf)
                            .or_else(|| Some(std::env::temp_dir()));
                        toasts.success(ctx, format!("Loaded {}", file_name(&path)));
                    }
                    Err(error) => {
                        toasts.error(ctx, format!("Could not open the file: {error:#}"));
                    }
                }
            }
            Err(error) => {
                toasts.error(ctx, format!("Could not read the file: {error:#}"));
            }
        }
    }

    /// Handle the keyboard shortcuts of the tab.
    pub fn handle_keys(&mut self, ctx: &egui::Context, settings: &Settings, toasts: &mut Toasts) {
        if !self.player.is_loaded() {
            return;
        }
        // The application has no text fields, so the shortcuts always apply.
        // They are only suppressed while a drop down is open.
        if egui::Popup::is_any_open(ctx) {
            return;
        }
        let step = settings.step_seconds;
        let fast_step = settings.fast_step_seconds;

        let mut toggle_play = false;
        let mut seek_delta = 0.0_f64;
        let mut frame_delta = 0_i64;
        let mut mark_in = false;
        let mut mark_out = false;

        ctx.input_mut(|input| {
            let shift = input.modifiers.shift;
            let none = egui::Modifiers::default();
            if input.consume_key(none, egui::Key::Space) {
                toggle_play = true;
            }
            if input.consume_key(none, egui::Key::ArrowLeft) {
                seek_delta += if shift { -fast_step } else { -step };
            }
            if input.consume_key(none, egui::Key::ArrowRight) {
                seek_delta += if shift { fast_step } else { step };
            }
            if input.consume_key(none, egui::Key::Comma) {
                frame_delta -= 1;
            }
            if input.consume_key(none, egui::Key::Period) {
                frame_delta += 1;
            }
            if input.consume_key(none, egui::Key::I) {
                mark_in = true;
            }
            if input.consume_key(none, egui::Key::O) {
                mark_out = true;
            }
        });

        if toggle_play {
            self.player.toggle_play();
        }
        if seek_delta != 0.0 {
            self.player.nudge(seek_delta);
        }
        if frame_delta != 0 {
            self.player.step_frame(frame_delta);
        }
        if mark_in {
            self.set_in_point(self.player.position());
            if let Some(time) = self.in_point {
                toasts.info(ctx, format!("In point set to {}", format_timecode_ms(time)));
            }
        }
        if mark_out {
            self.set_out_point(self.player.position());
            if let Some(time) = self.out_point {
                toasts.info(
                    ctx,
                    format!("Out point set to {}", format_timecode_ms(time)),
                );
            }
        }
    }

    /// Move the in marker, snapping it to a key frame when requested.
    pub fn set_in_point(&mut self, time: f64) {
        self.in_point = Some(self.snap_time(time));
    }

    /// Move the out marker, snapping it to a key frame when requested.
    pub fn set_out_point(&mut self, time: f64) {
        self.out_point = Some(self.snap_time(time));
    }

    fn snap_time(&self, time: f64) -> f64 {
        if self.snap && !self.keyframes.is_empty() {
            probe::nearest_keyframe(&self.keyframes, time).unwrap_or(time)
        } else {
            time
        }
    }

    /// Add the marked range to the list of segments.
    pub fn add_segment(&mut self) {
        let position = self.player.position();
        let (start, end) = match (self.in_point, self.out_point) {
            (Some(start), Some(end)) => (start.min(end), start.max(end)),
            (Some(start), None) => (start, position.max(start)),
            (None, Some(end)) => (position.min(end), end),
            (None, None) => return,
        };
        if end - start < 0.001 {
            return;
        }
        self.segments.push(Segment::new(start, end));
        self.segments.sort_by(|a, b| {
            a.start
                .partial_cmp(&b.start)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Remove the segment at `index`.
    pub fn remove_segment(&mut self, index: usize) {
        if index < self.segments.len() {
            self.segments.remove(index);
        }
    }

    /// Remove every segment.
    pub fn clear_segments(&mut self) {
        self.segments.clear();
    }

    /// The number of key frames that lie inside the marked range.
    pub fn keyframes_in_range(&self) -> usize {
        let (Some(start), Some(end)) = (self.in_point, self.out_point) else {
            return 0;
        };
        self.keyframes
            .iter()
            .filter(|time| **time >= start && **time <= end)
            .count()
    }

    /// Draw the central part of the tab.
    pub fn central(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        settings: &Settings,
        toasts: &mut Toasts,
    ) {
        self.handle_keys(ctx, settings, toasts);
        self.player.tick();
        self.player.refresh();

        if self.loading {
            ui.centered_and_justified(|ui| {
                ui.spinner();
                ui.label("Reading the file...");
            });
            return;
        }

        if !self.player.is_loaded() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Open a video file to get started")
                        .size(18.0)
                        .color(Color32::from_rgb(160, 168, 185)),
                );
                ui.label("Drag and drop a file onto this window, or use “Open file”.");
            });
            return;
        }

        self.file_header(ui);
        ui.add_space(6.0);
        self.preview(ui);
        ui.add_space(6.0);
        self.transport(ui);
        ui.add_space(6.0);
        self.timeline_area(ui, ctx);
        ui.add_space(4.0);
        self.status_line(ui);
    }

    fn file_header(&self, ui: &mut egui::Ui) {
        let Some(info) = self.info.as_ref() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(file_name(self.path.as_deref().unwrap_or(Path::new("")))).strong(),
            );
            ui.label(format!("{:.2} min", info.duration / 60.0));
            if let Some(video) = info.video.as_ref() {
                ui.label(format!(
                    "{} · {}×{}",
                    video.codec, video.width, video.height
                ));
                ui.label(format_fps(video.fps));
            }
            if let Some(audio) = info.audio.as_ref() {
                ui.label(format!(
                    "{} · {} ch · {} Hz",
                    audio.codec, audio.channels, audio.sample_rate
                ));
            }
            ui.label(format_bytes(info.size_bytes));
        });
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let texture_size = self.player.texture_size();
        let aspect = if texture_size[1] == 0 {
            16.0 / 9.0
        } else {
            texture_size[0] as f32 / texture_size[1] as f32
        };
        // Leave room for the transport controls and the timeline.
        const RESERVED: f32 = 210.0;
        let max_height = (available.y - RESERVED).max(120.0);
        let mut size = Vec2::new(available.x, available.x / aspect);
        if size.y > max_height {
            size.y = max_height;
            size.x = size.y * aspect;
        }

        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, Color32::from_rgb(16, 18, 22));
        if response.clicked() {
            self.player.toggle_play();
        }

        let handle = self.player.texture().clone();
        let uv = egui::epaint::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(handle.id(), rect, uv, Color32::WHITE);

        // Position and duration overlay.
        let position = self.player.position();
        let duration = self.player.duration();
        painter.text(
            egui::pos2(rect.left() + 8.0, rect.bottom() - 16.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "{} / {}",
                format_timecode_ms(position),
                format_timecode(duration)
            ),
            crate::toast::caption_font(),
            Color32::from_rgb(235, 238, 245),
        );
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.player.is_playing();
            if ui
                .button(if playing { "⏸ Pause" } else { "▶ Play" })
                .clicked()
            {
                self.player.toggle_play();
            }

            ui.label("Speed");
            egui::ComboBox::from_id_salt("speed")
                .selected_text(format!("{}×", self.player.speed()))
                .show_ui(ui, |ui| {
                    for speed in crate::player::SPEEDS {
                        let label = format!("{speed}×");
                        ui.selectable_value(&mut self.player.speed(), speed, label);
                    }
                });
            if self.player.speed() != 1.0 && ui.button("Reset").clicked() {
                self.player.set_speed(1.0);
            }

            ui.separator();

            ui.label("🔊");
            let mut volume = self.player.volume();
            if ui
                .add(
                    egui::Slider::new(&mut volume, 0.0..=1.0)
                        .custom_formatter(|value, _| format!("{:.0} %", value * 100.0)),
                )
                .changed()
            {
                self.player.set_volume(volume);
            }

            ui.separator();

            if ui.button("⏮ Start").clicked() {
                self.player.seek(0.0);
            }
            if ui.button("⏭ End").clicked() {
                self.player.seek(self.player.duration());
            }
        });
    }

    fn timeline_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let duration = self.player.duration();
        let position = self.player.position();
        let view = TimelineView {
            duration,
            position,
            keyframes: &self.keyframes,
            waveform: &self.waveform,
            segments: &self.segments,
            in_point: self.in_point,
            out_point: self.out_point,
        };
        let response = timeline::show(ui, &mut self.timeline, &view);
        if let Some(time) = response.seek {
            self.player.seek(time);
        }
        if let Some(time) = response.in_point {
            self.set_in_point(time);
        }
        if let Some(time) = response.out_point {
            self.set_out_point(time);
        }
        // Keep the play head visible while the file is playing.
        let width = ui.available_width();
        self.timeline
            .reveal(self.player.position(), duration, width);

        ui.horizontal(|ui| {
            ui.label("Scroll to zoom, Shift + scroll to pan.");
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Fit").clicked() {
                    let width = ui.available_width();
                    self.timeline.fit(duration, width);
                }
            });
        });
        let _ = ctx;
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "In: {}",
                self.in_point
                    .map(format_timecode_ms)
                    .unwrap_or_else(|| "—".to_string())
            ));
            ui.label(format!(
                "Out: {}",
                self.out_point
                    .map(format_timecode_ms)
                    .unwrap_or_else(|| "—".to_string())
            ));
            ui.label(format!("Key frames: {}", self.keyframes.len()));
            if self.snap {
                ui.label("Snapping to key frames");
                // A stream copy always starts at the key frame that lies
                // before the requested position.
                if let Some(start) = self
                    .in_point
                    .and_then(|time| probe::previous_keyframe(&self.keyframes, time))
                {
                    ui.label(format!(
                        "Lossless cut starts at {}",
                        format_timecode_ms(start)
                    ));
                }
                let inside = self.keyframes_in_range();
                if inside > 0 {
                    ui.label(format!("{inside} key frame(s) inside the range"));
                }
            }
        });
    }

    /// Draw the side panel with the list of segments.
    pub fn side_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        jobs: &mut JobManager,
        toasts: &mut Toasts,
    ) {
        ui.heading("Segments");
        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Set In (I)").clicked() {
                self.set_in_point(self.player.position());
            }
            if ui.button("Set Out (O)").clicked() {
                self.set_out_point(self.player.position());
            }
        });
        ui.horizontal(|ui| {
            if ui.button("➕ Add segment").clicked() {
                self.add_segment();
            }
            if ui.button("Clear").clicked() {
                self.clear_segments();
            }
        });

        ui.separator();

        if self.segments.is_empty() {
            ui.label(
                RichText::new(
                    "No segments yet. Mark an in and an out point, then add the segment.",
                )
                .color(Color32::from_rgb(150, 158, 175)),
            );
        }

        let mut to_remove: Option<usize> = None;
        let position = self.player.position();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, segment) in self.segments.iter().enumerate() {
                ui.push_id(index, |ui| {
                    let active = segment.contains(position);
                    if active {
                        ui.label(
                            RichText::new("▶")
                                .color(Color32::from_rgb(244, 96, 96))
                                .strong(),
                        );
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button("▶").clicked() {
                            self.player.seek(segment.start);
                        }
                        ui.label(format!("#{}", index + 1));
                        ui.label(format!(
                            "{} → {}",
                            format_timecode_ms(segment.start),
                            format_timecode_ms(segment.end)
                        ));
                        ui.label(format!("{:.2}s", segment.duration()));
                        if ui.small_button("✕").clicked() {
                            to_remove = Some(index);
                        }
                    });
                });
            }
        });
        if let Some(index) = to_remove {
            self.remove_segment(index);
        }

        ui.separator();
        ui.heading("Export");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Result");
            egui::ComboBox::from_id_salt("export-mode")
                .selected_text(match self.export_mode {
                    ExportMode::Separate => "Separate files",
                    ExportMode::Joined => "Single file",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.export_mode,
                        ExportMode::Separate,
                        "Separate files",
                    );
                    ui.selectable_value(&mut self.export_mode, ExportMode::Joined, "Single file");
                });
        });

        ui.horizontal(|ui| {
            ui.label("Encoding");
            egui::ComboBox::from_id_salt("encode-mode")
                .selected_text(self.encode_mode.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.encode_mode,
                        EncodeMode::Lossless,
                        EncodeMode::Lossless.label(),
                    );
                    ui.selectable_value(
                        &mut self.encode_mode,
                        EncodeMode::Precise,
                        EncodeMode::Precise.label(),
                    );
                });
        });

        ui.checkbox(&mut self.snap, "Snap to key frames (lossless)");

        ui.horizontal_wrapped(|ui| {
            ui.label("Output folder");
            let label = self
                .output_dir
                .as_ref()
                .map(|path| file_name(path))
                .unwrap_or_else(|| "not chosen".to_string());
            ui.label(label);
            if ui.button("Choose…").clicked() {
                if let Some(folder) = rfd::FileDialog::new()
                    .set_title("Choose the output folder")
                    .pick_folder()
                {
                    self.output_dir = Some(folder);
                }
            }
        });

        let can_export = self.path.is_some() && !self.segments.is_empty();
        let export_button = ui
            .add_enabled(can_export, egui::Button::new("Export"))
            .on_hover_text("Cut the marked segments out of the source file");
        if export_button.clicked() {
            self.export(jobs, toasts, ctx);
        }
        if self.segments.is_empty() {
            ui.label(
                RichText::new("Add at least one segment before exporting.")
                    .color(Color32::from_rgb(150, 158, 175)),
            );
        }
    }

    fn export(&mut self, jobs: &mut JobManager, toasts: &mut Toasts, ctx: &egui::Context) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let Some(info) = self.info.clone() else {
            return;
        };
        let segments: Vec<(f64, f64)> = self
            .segments
            .iter()
            .map(|segment| (segment.start, segment.end))
            .collect();
        if segments.is_empty() {
            return;
        }
        let segment_count = segments.len();

        let tools = self.player.tools().cloned();
        let Some(tools) = tools else {
            toasts.error(ctx, "The FFmpeg tools are not available.");
            return;
        };

        let mode = self.export_mode;
        let encode = self.encode_mode;
        let encoder = self.encoder.clone();
        let directory = self
            .output_dir
            .clone()
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(std::env::temp_dir);
        let stem = path
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "output".to_string());
        let extension = processor::extension_of(&path);

        match mode {
            ExportMode::Separate => {
                jobs.start(
                    format!("Exporting {} segment(s)", segments.len()),
                    move |cancel, progress| {
                        Box::pin(async move {
                            processor::export_segments(
                                &tools.ffmpeg,
                                &path,
                                &segments,
                                &directory,
                                &stem,
                                &extension,
                                encode,
                                &encoder,
                                &progress,
                                &cancel,
                            )
                            .await?;
                            Ok(directory)
                        })
                    },
                );
            }
            ExportMode::Joined => {
                let target = directory.join(format!("{stem}_joined.{extension}"));
                jobs.start(
                    format!("Cutting and joining {} segment(s)", segments.len()),
                    move |cancel, progress| {
                        Box::pin(async move {
                            processor::export_cut_and_join(
                                &tools.ffmpeg,
                                &path,
                                &segments,
                                &target,
                                encode,
                                &encoder,
                                &progress,
                                &cancel,
                            )
                            .await
                        })
                    },
                );
            }
        }

        let duration = info.duration;
        toasts.info(
            ctx,
            format!(
                "Started exporting {segment_count} segment(s) of {}",
                format_timecode(duration)
            ),
        );
    }
}

/// The file name of a path, or an empty string.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}
