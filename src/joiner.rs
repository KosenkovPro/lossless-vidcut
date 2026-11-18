//! The joiner tab: add files, reorder them and glue them together.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use anyhow::Result;
use egui::{Align, Color32, Layout, RichText, Sense, Vec2};

use crate::ffmpeg_tools::Tools;
use crate::jobs::JobManager;
use crate::loader;
use crate::probe::MediaInfo;
use crate::processor::{self, EncodeMode};
use crate::toast::Toasts;
use crate::util::{format_bytes, format_fps, format_timecode};

/// The state of a single file in the list.
#[derive(Debug, Clone)]
pub enum ItemState {
    /// The metadata is still being read.
    Loading,
    /// The metadata is available.
    Ready,
    /// The file could not be read.
    Failed(String),
}

/// A file that will be joined.
#[derive(Debug, Clone)]
pub struct JoinItem {
    /// Stable identifier of the item.
    pub id: u64,
    /// Path of the file.
    pub path: PathBuf,
    /// Metadata of the file, once it has been read.
    pub info: Option<MediaInfo>,
    /// The state of the item.
    pub state: ItemState,
}

/// A pending metadata request.
struct PendingLoad {
    id: u64,
    receiver: Receiver<Result<MediaInfo>>,
}

/// The state of the joiner tab.
pub struct Joiner {
    /// The files that will be joined.
    pub items: Vec<JoinItem>,
    /// How the result is encoded.
    pub encode_mode: EncodeMode,
    /// The video encoder that is used for precise joins.
    pub encoder: processor::VideoEncoder,
    /// The file the result is written to.
    pub output: Option<PathBuf>,
    next_id: u64,
    pending: Vec<PendingLoad>,
    tools: Option<Tools>,
    warned: bool,
}

impl Default for Joiner {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            encode_mode: EncodeMode::Lossless,
            encoder: processor::VideoEncoder::fallback(),
            output: None,
            next_id: 1,
            pending: Vec::new(),
            tools: None,
            warned: false,
        }
    }
}

impl Joiner {
    /// Provide the FFmpeg tools that are used to probe the files.
    pub fn set_tools(&mut self, tools: Option<Tools>) {
        self.tools = tools;
    }

    /// Add a file to the list.
    pub fn add(&mut self, path: &Path, tools: &Tools) {
        self.tools = Some(tools.clone());
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(JoinItem {
            id,
            path: path.to_path_buf(),
            info: None,
            state: ItemState::Loading,
        });
        let receiver = loader::probe_async(tools.clone(), path.to_path_buf());
        self.pending.push(PendingLoad { id, receiver });
    }

    /// Collect the metadata of the files that finished loading.
    pub fn poll_loads(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let mut finished: Vec<(u64, Result<MediaInfo>)> = Vec::new();
        self.pending
            .retain_mut(|pending| match pending.receiver.try_recv() {
                Ok(result) => {
                    finished.push((pending.id, result));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished.push((
                        pending.id,
                        Err(anyhow::anyhow!("the probe stopped unexpectedly")),
                    ));
                    false
                }
            });

        for (id, result) in finished {
            let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
                continue;
            };
            match result {
                Ok(info) => {
                    item.info = Some(info);
                    item.state = ItemState::Ready;
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    item.state = ItemState::Failed(message.clone());
                    toasts.error(
                        ctx,
                        format!("Could not read {}: {message}", file_name(&item.path)),
                    );
                }
            }
        }

        // Warn once as soon as the files turn out to be incompatible.
        if !self.warned && self.items.len() >= 2 && self.incompatibility_reason().is_some() {
            self.warned = true;
            toasts.warning(
                ctx,
                "The selected files do not share the same format. Use “Precise” to re-encode them.",
            );
        }
    }

    /// Remove the item with the given identifier.
    pub fn remove(&mut self, id: u64) {
        self.items.retain(|item| item.id != id);
    }

    /// Move an item to a new position.
    pub fn move_item(&mut self, from: usize, to: usize) {
        if from >= self.items.len() || to >= self.items.len() || from == to {
            return;
        }
        let item = self.items.remove(from);
        self.items.insert(to, item);
    }

    /// The total duration of all files.
    pub fn total_duration(&self) -> f64 {
        self.items
            .iter()
            .filter_map(|item| item.info.as_ref().map(|info| info.duration))
            .sum()
    }

    /// `true` when all files share the same codec, resolution and frame rate.
    pub fn is_compatible(&self) -> bool {
        let mut reference: Option<(String, u32, u32, f64)> = None;
        for item in &self.items {
            let Some(info) = item.info.as_ref() else {
                return false;
            };
            let Some(video) = info.video.as_ref() else {
                return false;
            };
            let key = (
                video.codec.clone(),
                video.width,
                video.height,
                (video.fps * 100.0).round() / 100.0,
            );
            match &reference {
                None => reference = Some(key),
                Some(existing) if *existing == key => {}
                Some(_) => return false,
            }
        }
        reference.is_some()
    }

    /// Describe why the files cannot be joined with a plain stream copy.
    pub fn incompatibility_reason(&self) -> Option<String> {
        if self.items.len() < 2 {
            return None;
        }
        if self.is_compatible() {
            return None;
        }
        let mut codecs = Vec::new();
        let mut resolutions = Vec::new();
        let mut rates = Vec::new();
        for item in &self.items {
            if let Some(video) = item.info.as_ref().and_then(|info| info.video.as_ref()) {
                codecs.push(video.codec.clone());
                resolutions.push(format!("{}×{}", video.width, video.height));
                rates.push(format_fps(video.fps));
            }
        }
        Some(format!(
            "The files differ: codecs [{}], resolutions [{}], frame rates [{}]. A stream copy may fail, use “Precise” to re-encode.",
            codecs.join(", "),
            resolutions.join(", "),
            rates.join(", ")
        ))
    }

    /// Draw the central part of the tab.
    pub fn central(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, toasts: &mut Toasts) {
        self.poll_loads(ctx, toasts);

        ui.horizontal(|ui| {
            if ui.button("➕ Add files…").clicked() {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v"])
                    .pick_files()
                {
                    let tools = self.tools.clone();
                    if let Some(tools) = tools {
                        for file in files {
                            self.add(&file, &tools);
                        }
                    }
                }
            }
            if ui.button("Clear").clicked() {
                self.items.clear();
                self.pending.clear();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(format!("{} file(s)", self.items.len()));
                ui.label(format!("Total {}", format_timecode(self.total_duration())));
            });
        });

        ui.separator();

        if self.items.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new("Add the videos you want to join")
                        .size(18.0)
                        .color(Color32::from_rgb(160, 168, 185)),
                );
                ui.label("Drag and drop files onto this window, or use “Add files…”.");
            });
            return;
        }

        self.draw_list(ui);
    }

    fn draw_list(&mut self, ui: &mut egui::Ui) {
        let row_height = 46.0;
        let width = ui.available_width();
        let mut drag_from: Option<usize> = None;
        let mut drag_to: Option<usize> = None;
        let mut remove: Option<u64> = None;

        let pointer = ui.input(|input| input.pointer.hover_pos());
        let first_top = ui.cursor().min.y;

        for index in 0..self.items.len() {
            let item = self.items[index].clone();
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(width, row_height), Sense::click());

            let background = if index % 2 == 0 {
                Color32::from_rgb(32, 35, 42)
            } else {
                Color32::from_rgb(28, 30, 37)
            };
            ui.painter().rect_filled(rect, 4.0, background);

            // The drag handle.
            let handle_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 6.0, rect.center().y - 9.0),
                Vec2::new(18.0, 18.0),
            );
            let handle = ui.interact(handle_rect, ui.id().with(("handle", index)), Sense::drag());
            ui.painter().text(
                handle_rect.left_top(),
                egui::Align2::LEFT_TOP,
                "≡",
                crate::toast::caption_font(),
                Color32::from_rgb(150, 158, 175),
            );
            if handle.drag_started() {
                drag_from = Some(index);
            }
            if handle.dragged() {
                if let Some(position) = pointer {
                    let offset = (position.y - first_top) / row_height;
                    let target = (offset.round() as i32 - index as i32 + index as i32)
                        .clamp(0, self.items.len() as i32 - 1);
                    drag_to = Some(target as usize);
                }
            }
            if handle.drag_stopped() {
                if let (Some(from), Some(to)) = (drag_from, drag_to) {
                    self.move_item(from, to);
                }
                drag_from = None;
                drag_to = None;
            }

            // The file description.
            let (text_rect, _) = ui.allocate_exact_size(
                Vec2::new(rect.width() - 80.0, row_height - 8.0),
                Sense::hover(),
            );
            let painter = ui.painter();
            painter.text(
                egui::pos2(text_rect.left(), text_rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                file_name(&item.path),
                egui::FontId::proportional(13.0),
                Color32::from_rgb(230, 233, 240),
            );

            let description = match (&item.state, item.info.as_ref()) {
                (ItemState::Loading, _) => "Reading…".to_string(),
                (ItemState::Failed(message), _) => format!("Could not read the file: {message}"),
                (ItemState::Ready, Some(info)) => {
                    let video = info
                        .video
                        .as_ref()
                        .map(|video| {
                            format!(
                                "{} · {}×{} · {}",
                                video.codec,
                                video.width,
                                video.height,
                                format_fps(video.fps)
                            )
                        })
                        .unwrap_or_else(|| "no video track".to_string());
                    format!(
                        "{video} · {} · {}",
                        format_timecode(info.duration),
                        format_bytes(info.size_bytes)
                    )
                }
                (ItemState::Ready, None) => String::new(),
            };
            painter.text(
                egui::pos2(text_rect.left(), text_rect.top() + 20.0),
                egui::Align2::LEFT_TOP,
                description,
                crate::toast::caption_font(),
                Color32::from_rgb(150, 158, 175),
            );

            // The remove button.
            let button_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 30.0, rect.center().y - 10.0),
                Vec2::new(20.0, 20.0),
            );
            let button = ui.interact(button_rect, ui.id().with(("remove", index)), Sense::click());
            painter.text(
                button_rect.left_top(),
                egui::Align2::LEFT_TOP,
                "✕",
                egui::FontId::proportional(13.0),
                Color32::from_rgb(150, 158, 175),
            );
            if button.clicked() {
                remove = Some(item.id);
            }

            if response.clicked() {
                // Clicking the row selects it, which is only used for the
                // visual feedback of a drag operation.
            }
        }

        if let Some(id) = remove {
            self.remove(id);
        }
    }

    /// Draw the side panel with the export options.
    pub fn side_panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        jobs: &mut JobManager,
        toasts: &mut Toasts,
    ) {
        ui.heading("Join");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Encoding");
            egui::ComboBox::from_id_salt("join-encode")
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

        if let Some(reason) = self.incompatibility_reason() {
            ui.colored_label(Color32::from_rgb(240, 180, 80), "⚠");
            ui.label(RichText::new(reason).color(Color32::from_rgb(240, 196, 130)));
            ui.label(
                RichText::new("Switch to “Precise” to bring the files to a common format.")
                    .color(Color32::from_rgb(150, 158, 175)),
            );
        } else if self.items.len() >= 2 {
            ui.colored_label(Color32::from_rgb(96, 200, 120), "✔");
            ui.label("All files share the same format.");
        }

        ui.separator();

        ui.horizontal_wrapped(|ui| {
            ui.label("Output file");
            let label = self
                .output
                .as_ref()
                .map(|path| file_name(path))
                .unwrap_or_else(|| "not chosen".to_string());
            ui.label(label);
            if ui.button("Choose…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose the output file")
                    .add_filter("Video", &["mp4", "mkv", "mov", "webm"])
                    .save_file()
                {
                    self.output = Some(path);
                }
            }
        });

        let ready = self.items.len() >= 2
            && self
                .items
                .iter()
                .all(|item| matches!(item.state, ItemState::Ready))
            && self.output.is_some();

        if ui
            .add_enabled(ready, egui::Button::new("Join files"))
            .clicked()
        {
            self.export(jobs, toasts, ctx);
        }

        if self.items.len() < 2 {
            ui.label(
                RichText::new("Add at least two files.").color(Color32::from_rgb(150, 158, 175)),
            );
        } else if self.output.is_none() {
            ui.label(
                RichText::new("Choose where the result should be written.")
                    .color(Color32::from_rgb(150, 158, 175)),
            );
        }
    }

    fn export(&mut self, jobs: &mut JobManager, toasts: &mut Toasts, ctx: &egui::Context) {
        let Some(output) = self.output.clone() else {
            return;
        };
        let inputs: Vec<PathBuf> = self.items.iter().map(|item| item.path.clone()).collect();
        let duration = self.total_duration();
        let encode = self.encode_mode;
        let encoder = self.encoder.clone();
        let tools = match self.tools() {
            Some(tools) => tools,
            None => {
                toasts.error(ctx, "The FFmpeg tools are not available.");
                return;
            }
        };

        jobs.start(
            format!("Joining {} file(s)", inputs.len()),
            move |cancel, progress| {
                Box::pin(async move {
                    processor::export_join(
                        &tools.ffmpeg,
                        &inputs,
                        &output,
                        encode,
                        &encoder,
                        duration,
                        &progress,
                        &cancel,
                    )
                    .await
                })
            },
        );
        toasts.info(ctx, "Started joining the files.");
    }

    /// The tools that were used to load the files.
    fn tools(&self) -> Option<Tools> {
        self.tools.clone()
    }
}

/// The file name of a path, or an empty string.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}
