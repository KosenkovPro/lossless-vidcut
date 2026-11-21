//! The unified editor.
//!
//! The editor combines what used to be two separate tabs. A project holds the
//! media files that were loaded and the clips that were cut out of them. The
//! clips form one timeline: the scissors cut them, the pointer drags them
//! into the order they should be exported in.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use anyhow::Result;
use egui::{Color32, Layout, RichText, Sense, Vec2};

use crate::ffmpeg_tools::Tools;
use crate::i18n::{tf, tr, Lang};
use crate::jobs::JobManager;
use crate::loader::{self, LoadedMedia};
use crate::player::Player;
use crate::processor::{self, EncodeMode, ExportClip};
use crate::project::Project;
use crate::sequence::{self, SequenceItem, SequenceState, SequenceView};
use crate::settings::Settings;
use crate::timeline::{self, Segment, TimelineState, TimelineView};
use crate::toast::Toasts;
use crate::util::{format_bytes, format_fps, format_timecode, format_timecode_ms};

/// A metadata request that is still running.
struct PendingLoad {
    /// The source the request belongs to.
    id: u64,
    /// The channel the result arrives on.
    receiver: Receiver<Result<LoadedMedia>>,
    /// `true` when the whole file should join the timeline once it is ready.
    add_as_clip: bool,
}

/// The editor with the preview, the source timeline and the sequence track.
pub struct Editor {
    /// The media and the clips of the project.
    pub project: Project,
    /// The preview player.
    pub player: Player,
    /// Snap the cut points to key frames.
    pub snap: bool,
    /// How the result is encoded.
    pub encode_mode: EncodeMode,
    /// The video encoder that is used for precise cuts.
    pub encoder: processor::VideoEncoder,
    /// The file the result is written to.
    pub output: Option<PathBuf>,
    /// The position of the play head on the timeline.
    pub playhead: f64,
    /// The language of the user interface.
    pub lang: Lang,
    timeline: TimelineState,
    sequence: SequenceState,
    sequence_width: f32,
    pending: Vec<PendingLoad>,
    tools: Option<Tools>,
    player_source: Option<u64>,
}

impl Editor {
    /// Create an empty editor.
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            project: Project::new(),
            player: Player::new(ctx),
            snap: true,
            encode_mode: EncodeMode::Lossless,
            encoder: processor::VideoEncoder::fallback(),
            output: None,
            playhead: 0.0,
            lang: Lang::English,
            timeline: TimelineState::default(),
            sequence: SequenceState::default(),
            sequence_width: 800.0,
            pending: Vec::new(),
            tools: None,
            player_source: None,
        }
    }

    /// Provide the FFmpeg tools that are used to probe and decode the files.
    pub fn set_tools(&mut self, tools: Option<Tools>) {
        self.tools = tools.clone();
        if let Some(tools) = tools {
            self.player.set_tools(tools);
        }
    }

    /// The FFmpeg tools, if they have been found.
    pub fn tools(&self) -> Option<&Tools> {
        self.tools.as_ref()
    }

    /// Add a file to the project.
    ///
    /// When `add_as_clip` is set, the whole file joins the timeline as soon as
    /// its metadata is known. This is what happens when a file is dropped onto
    /// the window.
    pub fn open_path(&mut self, path: &Path, tools: &Tools, add_as_clip: bool) {
        let id = self.project.add_source(path);
        let receiver = loader::load_async(tools.clone(), path.to_path_buf());
        self.pending.push(PendingLoad {
            id,
            receiver,
            add_as_clip,
        });
    }

    /// Collect the metadata of the files that finished loading.
    pub fn poll_loads(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let mut finished: Vec<(u64, Result<LoadedMedia>, bool)> = Vec::new();
        self.pending
            .retain_mut(|pending| match pending.receiver.try_recv() {
                Ok(result) => {
                    finished.push((pending.id, result, pending.add_as_clip));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished.push((
                        pending.id,
                        Err(anyhow::anyhow!("the loader stopped unexpectedly")),
                        pending.add_as_clip,
                    ));
                    false
                }
            });

        for (id, result, add_as_clip) in finished {
            match result {
                Ok(loaded) => {
                    let LoadedMedia {
                        path,
                        info,
                        keyframes,
                        waveform,
                    } = loaded;
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.project.set_loaded(id, info, keyframes, waveform);

                    if add_as_clip {
                        self.project.add_whole_source(id);
                    }
                    if self.project.selected_source == Some(id) {
                        self.open_player(id, 0.0);
                    }
                    toasts.success(ctx, tf(self.lang, "Loaded {name}", &[("name", &name)]));
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    self.project.set_failed(id, message.clone());
                    toasts.error(
                        ctx,
                        tf(
                            self.lang,
                            "Could not read the file: {error}",
                            &[("error", &message)],
                        ),
                    );
                }
            }
        }
    }

    /// The name of a source, or an empty string.
    fn source_name(&self, id: u64) -> String {
        self.project
            .source(id)
            .map(|source| source.name())
            .unwrap_or_default()
    }

    /// Show a source in the preview and jump to `time`.
    fn open_player(&mut self, source_id: u64, time: f64) {
        let Some(source) = self.project.source(source_id) else {
            return;
        };
        let Some(info) = source.info.clone() else {
            return;
        };
        let path = source.path.clone();
        if self.player_source != Some(source_id) {
            // A new file starts paused, so the playback has to be continued
            // when the preview switches to another file.
            let was_playing = self.player.is_playing();
            if let Err(error) = self.player.open(&path, info) {
                log::warn!("could not open the preview: {error:#}");
                self.player_source = None;
                return;
            }
            self.player_source = Some(source_id);
            if was_playing {
                self.player.set_playing(true);
            }
        }
        self.player.seek(time);
    }

    /// Move the play head along with the player.
    ///
    /// The play head only follows when the player shows the source of the
    /// selected clip, so that a source can also be previewed on its own.
    fn follow_player(&mut self) {
        if !self.player.is_playing() {
            return;
        }
        let Some(clip) = self.project.selected_clip() else {
            return;
        };
        if self.player_source != Some(clip.source_id) {
            return;
        }
        let Some(placement) = self.project.placement_of(clip.id) else {
            return;
        };
        let inner = self.player.position() - clip.start;
        if inner >= placement.duration - 0.02 {
            self.select_next_clip(clip.id);
        } else {
            self.playhead = placement.start + inner.max(0.0);
        }
    }

    /// Select the clip that follows the given one and continue playing.
    fn select_next_clip(&mut self, clip_id: u64) {
        let layout = self.project.layout();
        let Some(index) = layout.iter().position(|item| item.id == clip_id) else {
            return;
        };
        let Some(next) = layout.get(index + 1) else {
            // The end of the timeline was reached.
            self.player.set_playing(false);
            self.playhead = self.project.total_duration();
            return;
        };
        let Some(clip) = self.project.clip(next.id) else {
            return;
        };
        let source_id = clip.source_id;
        let start = clip.start;
        self.project.select_only(next.id);
        self.project.selected_source = Some(source_id);
        self.open_player(source_id, start);
        self.playhead = next.start;
    }

    /// The clips that belong to a source, as ranges of that source.
    fn segments_of(&self, source_id: u64) -> Vec<Segment> {
        self.project
            .clips
            .iter()
            .filter(|clip| clip.source_id == source_id)
            .map(|clip| Segment::new(clip.start, clip.end))
            .collect()
    }

    /// The current position inside the source that is being edited.
    fn source_position(&self) -> f64 {
        self.player.position()
    }

    // ------------------------------------------------------------- shortcuts

    /// Handle the keyboard shortcuts of the editor.
    pub fn handle_keys(&mut self, ctx: &egui::Context, settings: &Settings, toasts: &mut Toasts) {
        if egui::Popup::is_any_open(ctx) {
            return;
        }

        let mut actions: Vec<Action> = Vec::new();
        ctx.input_mut(|input| {
            let none = egui::Modifiers::default();
            let shift = egui::Modifiers {
                shift: true,
                ..egui::Modifiers::default()
            };
            let ctrl = egui::Modifiers {
                ctrl: true,
                ..egui::Modifiers::default()
            };
            let ctrl_shift = egui::Modifiers {
                ctrl: true,
                shift: true,
                ..egui::Modifiers::default()
            };

            if input.consume_key(none, egui::Key::Space) {
                actions.push(Action::TogglePlay);
            }
            if input.consume_key(none, egui::Key::ArrowLeft) {
                actions.push(Action::Seek(-settings.step_seconds));
            }
            if input.consume_key(none, egui::Key::ArrowRight) {
                actions.push(Action::Seek(settings.step_seconds));
            }
            if input.consume_key(shift, egui::Key::ArrowLeft) {
                actions.push(Action::Seek(-settings.fast_step_seconds));
            }
            if input.consume_key(shift, egui::Key::ArrowRight) {
                actions.push(Action::Seek(settings.fast_step_seconds));
            }
            if input.consume_key(none, egui::Key::Comma) {
                actions.push(Action::Frame(-1));
            }
            if input.consume_key(none, egui::Key::Period) {
                actions.push(Action::Frame(1));
            }
            if input.consume_key(none, egui::Key::I) {
                actions.push(Action::MarkIn);
            }
            if input.consume_key(none, egui::Key::O) {
                actions.push(Action::MarkOut);
            }
            if input.consume_key(ctrl_shift, egui::Key::I) {
                actions.push(Action::MarkIn);
            }
            if input.consume_key(ctrl_shift, egui::Key::O) {
                actions.push(Action::MarkOut);
            }
            if input.consume_key(none, egui::Key::S)
                || input.consume_key(none, egui::Key::B)
                || input.consume_key(ctrl, egui::Key::B)
                || input.consume_key(ctrl, egui::Key::K)
            {
                actions.push(Action::Split);
            }
            if input.consume_key(none, egui::Key::Enter) {
                actions.push(Action::Extract);
            }
            if input.consume_key(none, egui::Key::Home) {
                actions.push(Action::JumpStart);
            }
            if input.consume_key(none, egui::Key::End) {
                actions.push(Action::JumpEnd);
            }
            if input.consume_key(none, egui::Key::Delete)
                || input.consume_key(none, egui::Key::Backspace)
            {
                actions.push(Action::Delete);
            }
            if input.consume_key(ctrl, egui::Key::D) {
                actions.push(Action::Duplicate);
            }
            if input.consume_key(ctrl, egui::Key::A) {
                actions.push(Action::SelectAll);
            }
            if input.consume_key(ctrl, egui::Key::Z) {
                actions.push(Action::Undo);
            }
            if input.consume_key(ctrl_shift, egui::Key::Z) || input.consume_key(ctrl, egui::Key::Y)
            {
                actions.push(Action::Redo);
            }
            if input.consume_key(none, egui::Key::Plus) || input.consume_key(ctrl, egui::Key::Plus)
            {
                actions.push(Action::ZoomIn);
            }
            if input.consume_key(none, egui::Key::Minus)
                || input.consume_key(ctrl, egui::Key::Minus)
            {
                actions.push(Action::ZoomOut);
            }
            if input.consume_key(ctrl, egui::Key::Num0) {
                actions.push(Action::ZoomFit);
            }
        });

        for action in actions {
            self.run(action, ctx, toasts);
        }
    }

    /// Carry out a shortcut.
    fn run(&mut self, action: Action, ctx: &egui::Context, toasts: &mut Toasts) {
        match action {
            Action::TogglePlay => self.toggle_play(),
            Action::Seek(delta) => {
                if !self.player.is_loaded() {
                    return;
                }
                self.player.nudge(delta);
                self.sync_playhead_with_source();
            }
            Action::Frame(delta) => {
                if !self.player.is_loaded() {
                    return;
                }
                self.player.step_frame(delta);
                self.sync_playhead_with_source();
            }
            Action::MarkIn => self.mark_in(ctx, toasts),
            Action::MarkOut => self.mark_out(ctx, toasts),
            Action::Split => self.split_selected_with_feedback(ctx, toasts),
            Action::Extract => self.extract_marked(ctx, toasts),
            Action::JumpStart => {
                self.player.seek(0.0);
                self.sync_playhead_with_source();
            }
            Action::JumpEnd => {
                let duration = self.player.duration();
                self.player.seek(duration);
                self.sync_playhead_with_source();
            }
            Action::Delete => self.delete_selected(ctx, toasts),
            Action::Duplicate => self.duplicate_selected(ctx, toasts),
            Action::SelectAll => self.project.select_all(),
            Action::Undo => {
                if self.project.undo() {
                    self.reveal_selection();
                } else {
                    toasts.info(ctx, tr(self.lang, "Nothing to undo."));
                }
            }
            Action::Redo => {
                if self.project.redo() {
                    self.reveal_selection();
                } else {
                    toasts.info(ctx, tr(self.lang, "Nothing to redo."));
                }
            }
            Action::ZoomIn => {
                let anchor = self.playhead;
                self.sequence.zoom(1.25, anchor);
            }
            Action::ZoomOut => {
                let anchor = self.playhead;
                self.sequence.zoom(1.0 / 1.25, anchor);
            }
            Action::ZoomFit => {
                let duration = self.project.total_duration();
                let width = self.sequence_width;
                self.sequence.fit(duration, width);
            }
        }
    }

    /// Start or stop the playback.
    ///
    /// Before the playback starts the preview is moved to the part of the
    /// timeline the play head is standing on.
    fn toggle_play(&mut self) {
        if !self.player.is_loaded() {
            return;
        }
        if let Some((clip_id, source_time)) = self.project.locate(self.playhead) {
            if let Some(clip) = self.project.clip(clip_id) {
                let source_id = clip.source_id;
                if self.player_source != Some(source_id) {
                    self.project.select_only(clip_id);
                    self.project.selected_source = Some(source_id);
                    self.open_player(source_id, source_time);
                } else {
                    // The preview shows the right file already, but it may
                    // stand at another position.
                    self.player.seek(source_time);
                }
            }
        }
        self.player.toggle_play();
    }

    /// The position inside the source that belongs to the play head.
    ///
    /// The play head lives on the timeline, while the cut points live inside
    /// the source of the selected clip. This converts the first into the
    /// second and falls back to the position of the preview.
    fn playhead_in_source(&self) -> f64 {
        let Some(clip) = self.project.selected_clip() else {
            return self.source_position();
        };
        let Some(placement) = self.project.placement_of(clip.id) else {
            return self.source_position();
        };
        let offset = self.playhead - placement.start;
        if offset < 0.0 || offset > placement.duration {
            return self.source_position();
        }
        clip.start + offset
    }

    /// Keep the play head in step with the position inside the source.
    fn sync_playhead_with_source(&mut self) {
        let Some(clip) = self.project.selected_clip() else {
            return;
        };
        if self.player_source != Some(clip.source_id) {
            return;
        }
        let Some(placement) = self.project.placement_of(clip.id) else {
            return;
        };
        let inner = (self.player.position() - clip.start).max(0.0);
        self.playhead = placement.start + inner.min(placement.duration);
    }

    /// Mark the beginning of the range that is cut out.
    fn mark_in(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(source_id) = self.project.selected_source else {
            return;
        };
        let position = self.source_position();
        let snap = self.snap;
        if let Some(time) = self.project.set_in_point(source_id, position, snap) {
            toasts.info(
                ctx,
                tf(
                    self.lang,
                    "In point set to {time}",
                    &[("time", &format_timecode_ms(time))],
                ),
            );
        }
    }

    /// Mark the end of the range that is cut out.
    fn mark_out(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(source_id) = self.project.selected_source else {
            return;
        };
        let position = self.source_position();
        let snap = self.snap;
        if let Some(time) = self.project.set_out_point(source_id, position, snap) {
            toasts.info(
                ctx,
                tf(
                    self.lang,
                    "Out point set to {time}",
                    &[("time", &format_timecode_ms(time))],
                ),
            );
        }
    }

    /// Cut the selected clip at the play head.
    ///
    /// The clip is only cut when the preview shows the file the clip was cut
    /// from, otherwise the position of the play head would belong to a
    /// different file.
    pub fn split_selected(&mut self) -> bool {
        let Some(clip) = self.project.selected_clip() else {
            return false;
        };
        let clip_id = clip.id;
        let source_id = clip.source_id;
        if self.player_source != Some(source_id) {
            return false;
        }
        let position = self.playhead_in_source();
        let Some(new_id) = self.project.split_clip(clip_id, position) else {
            return false;
        };
        self.project.selected_source = Some(source_id);
        self.project.select_only(new_id);
        true
    }

    /// Cut the selected clip and tell the user what happened.
    fn split_selected_with_feedback(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        if self.split_selected() {
            toasts.info(ctx, tr(self.lang, "The clip was cut at the play head."));
        } else if self.project.selected_clip().is_none() {
            toasts.info(ctx, tr(self.lang, "No clip is selected"));
        } else {
            toasts.info(ctx, tr(self.lang, "There is nothing to cut here."));
        }
    }

    /// Cut the marked range out of the source and append it to the timeline.
    fn extract_marked(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(source_id) = self.project.selected_source else {
            toasts.info(
                ctx,
                tr(
                    self.lang,
                    "Nothing to cut — mark an in and an out point first.",
                ),
            );
            return;
        };
        let position = self.source_position();
        let Some((start, end)) = self.project.marked_range(source_id, position) else {
            toasts.info(
                ctx,
                tr(
                    self.lang,
                    "Nothing to cut — mark an in and an out point first.",
                ),
            );
            return;
        };
        if self.project.cut(source_id, start, end).is_some() {
            toasts.info(ctx, tr(self.lang, "The clip was added to the timeline."));
        }
    }

    /// Remove the selected clips from the timeline.
    fn delete_selected(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let selection = self.project.selected_clips.clone();
        if selection.is_empty() {
            return;
        }
        for id in selection {
            self.project.remove_clip(id);
        }
        toasts.info(ctx, tr(self.lang, "The clip was removed."));
    }

    /// Copy the selected clip.
    fn duplicate_selected(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(clip) = self.project.selected_clip() else {
            return;
        };
        if self.project.duplicate_clip(clip.id).is_some() {
            toasts.info(ctx, tr(self.lang, "The clip was duplicated."));
        }
    }

    /// Scroll the sequence so that the selected clip is visible.
    fn reveal_selection(&mut self) {
        let Some(clip) = self.project.selected_clip() else {
            return;
        };
        let Some(placement) = self.project.placement_of(clip.id) else {
            return;
        };
        let width = self.sequence_width;
        self.sequence
            .reveal(placement.start, self.project.total_duration(), width);
    }

    // ------------------------------------------------------------------ ui

    /// Draw the central part of the editor.
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
        self.follow_player();

        if self.project.sources.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    RichText::new(tr(self.lang, "Open a video file to get started"))
                        .size(18.0)
                        .color(Color32::from_rgb(160, 168, 185)),
                );
                ui.label(tr(
                    self.lang,
                    "Drag and drop a file onto this window, or use “Open file”.",
                ));
            });
            return;
        }

        self.preview(ui);
        ui.add_space(4.0);
        self.transport(ui);
        ui.add_space(6.0);
        self.source_timeline(ui);
        ui.add_space(8.0);
        self.sequence_track(ui);
    }

    /// Draw the preview of the source that is being edited.
    fn preview(&mut self, ui: &mut egui::Ui) {
        if !self.player.is_loaded() {
            let text = tr(self.lang, "Reading the media file…");
            if self.project.is_loading() {
                ui.centered_and_justified(|ui| {
                    ui.spinner();
                    ui.label(text);
                });
            } else {
                ui.centered_and_justified(|ui| ui.label(tr(self.lang, "Nothing is selected")));
            }
            return;
        }

        let available = ui.available_size();
        let texture_size = self.player.texture_size();
        let aspect = if texture_size[1] == 0 {
            16.0 / 9.0
        } else {
            texture_size[0] as f32 / texture_size[1] as f32
        };
        // The preview gets at most half of the room, so that the two
        // timelines underneath it stay usable.
        let max_height = (available.y * 0.5).clamp(140.0, 640.0);
        let mut size = Vec2::new(available.x, available.x / aspect);
        if size.y > max_height {
            size.y = max_height;
            size.x = size.y * aspect;
        }

        ui.horizontal(|ui| {
            let margin = (ui.available_width() - size.x).max(0.0) / 2.0;
            ui.add_space(margin);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, Color32::from_rgb(16, 18, 22));
            if response.clicked() {
                self.player.toggle_play();
            }

            let handle = self.player.texture().clone();
            let uv = egui::epaint::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(handle.id(), rect, uv, Color32::WHITE);

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
        });
    }

    /// Draw the transport controls.
    fn transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let playing = self.player.is_playing();
            let label = if playing {
                tr(self.lang, "Pause")
            } else {
                tr(self.lang, "Play")
            };
            if ui.button(label).clicked() {
                self.toggle_play();
            }

            ui.label(tr(self.lang, "Speed"));
            egui::ComboBox::from_id_salt("speed")
                .selected_text(format!("{}×", self.player.speed()))
                .show_ui(ui, |ui| {
                    for speed in crate::player::SPEEDS {
                        let label = format!("{speed}×");
                        ui.selectable_value(&mut self.player.speed(), speed, label);
                    }
                });
            if self.player.speed() != 1.0 && ui.button(tr(self.lang, "Reset")).clicked() {
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

            if ui.button(tr(self.lang, "Start")).clicked() {
                self.player.seek(0.0);
                self.sync_playhead_with_source();
            }
            if ui.button(tr(self.lang, "End")).clicked() {
                let duration = self.player.duration();
                self.player.seek(duration);
                self.sync_playhead_with_source();
            }

            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(source_id) = self.project.selected_source {
                    let name = self.source_name(source_id);
                    ui.label(name);
                }
            });
        });
    }

    /// Draw the timeline of the source that is being edited.
    fn source_timeline(&mut self, ui: &mut egui::Ui) {
        let Some(source_id) = self.project.selected_source else {
            return;
        };
        let Some(source) = self.project.source(source_id) else {
            return;
        };

        // The data of the source is copied out, so that the timeline can be
        // drawn without borrowing the project while it is being edited.
        let name = source.name();
        let duration = source.duration();
        let in_point = source.in_point;
        let out_point = source.out_point;
        let keyframe_count = source.keyframes.len();
        let description = match source.info.as_ref() {
            Some(info) => {
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
                    .unwrap_or_default();
                format!(
                    "{} · {video} · {}",
                    format_timecode(info.duration),
                    format_bytes(info.size_bytes)
                )
            }
            None => String::new(),
        };

        ui.horizontal(|ui| {
            ui.label(RichText::new(tr(self.lang, "Source")).strong());
            ui.label(name);
            ui.label(description);
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr(self.lang, "Fit")).clicked() {
                    let width = ui.available_width();
                    self.timeline.fit(duration, width);
                }
            });
        });

        let segments = self.segments_of(source_id);
        let response = {
            let Some(source) = self.project.source(source_id) else {
                return;
            };
            let view = TimelineView {
                duration,
                position: self.source_position(),
                keyframes: &source.keyframes,
                waveform: &source.waveform,
                segments: &segments,
                in_point,
                out_point,
            };
            timeline::show(ui, &mut self.timeline, &view)
        };
        if let Some(time) = response.seek {
            self.player.seek(time);
            self.sync_playhead_with_source();
        }
        if let Some(time) = response.in_point {
            let snap = self.snap;
            self.project.set_in_point(source_id, time, snap);
        }
        if let Some(time) = response.out_point {
            let snap = self.snap;
            self.project.set_out_point(source_id, time, snap);
        }
        let width = ui.available_width();
        self.timeline
            .reveal(self.source_position(), duration, width);

        ui.horizontal_wrapped(|ui| {
            let in_point = in_point
                .map(format_timecode_ms)
                .unwrap_or_else(|| "—".to_string());
            let out_point = out_point
                .map(format_timecode_ms)
                .unwrap_or_else(|| "—".to_string());
            ui.label(format!("{}: {in_point}", tr(self.lang, "In")));
            ui.label(format!("{}: {out_point}", tr(self.lang, "Out")));
            ui.label(format!(
                "{}: {}",
                tr(self.lang, "Key frames"),
                keyframe_count
            ));
            if self.snap {
                ui.label(tr(self.lang, "Snapping to key frames"));
            }
        });
    }

    /// Draw the track that holds the clips of the timeline.
    fn sequence_track(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr(self.lang, "Timeline")).strong());
            let total = self.project.total_duration();
            ui.label(tf(
                self.lang,
                "Total {time}",
                &[("time", &format_timecode(total))],
            ));
            ui.label(tf(
                self.lang,
                "{count} clip(s)",
                &[("count", &self.project.clips.len().to_string())],
            ));
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr(self.lang, "Fit")).clicked() {
                    let width = ui.available_width();
                    self.sequence.fit(total, width);
                }
            });
        });

        if self.project.clips.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(tr(self.lang, "The timeline is empty"))
                        .color(Color32::from_rgb(160, 168, 185)),
                );
                ui.label(tr(
                    self.lang,
                    "Drop videos here, cut them with the scissors and drag the pieces into order.",
                ));
            });
            return;
        }

        let layout = self.project.layout();
        let items: Vec<SequenceItem> = layout
            .iter()
            .filter_map(|placement| {
                let clip = self.project.clip(placement.id)?;
                Some(SequenceItem {
                    id: clip.id,
                    name: self.source_name(clip.source_id),
                    start: placement.start,
                    duration: placement.duration,
                    color: sequence::source_color(clip.source_id),
                    selected: self.project.is_selected(clip.id),
                    dragging: false,
                })
            })
            .collect();

        let view = SequenceView {
            items: &items,
            total_duration: self.project.total_duration(),
            playhead: self.playhead,
        };
        self.sequence_width = ui.available_width();
        // The track fills the rest of the window, so that there is no empty
        // space below it.
        let height = ui.available_height().clamp(90.0, 420.0);
        let response = sequence::show(ui, &mut self.sequence, &view, height);

        if let Some(id) = response.select {
            self.project.select_only(id);
        }
        if response.drag_started.is_some() {
            // The whole drag is a single step of the history.
            self.project.remember_change();
        }
        if let Some((id, target)) = response.reorder {
            if let Some(from) = self.project.clips.iter().position(|clip| clip.id == id) {
                self.project.move_clip(from, target);
            }
        }
        if let Some(time) = response.seek {
            self.playhead = time;
            self.activate_playhead();
        }

        let width = ui.available_width();
        self.sequence
            .reveal(self.playhead, self.project.total_duration(), width);
    }

    /// Move the preview to the part of the timeline the play head is on.
    fn activate_playhead(&mut self) {
        let Some((clip_id, source_time)) = self.project.locate(self.playhead) else {
            return;
        };
        let Some(clip) = self.project.clip(clip_id) else {
            return;
        };
        let source_id = clip.source_id;
        self.project.select_only(clip_id);
        self.project.selected_source = Some(source_id);
        self.open_player(source_id, source_time);
    }

    // ------------------------------------------------------------ side panels

    /// Draw the list of media files.
    pub fn media_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, toasts: &mut Toasts) {
        ui.heading(tr(self.lang, "Media"));
        ui.separator();

        if ui.button(tr(self.lang, "Add media…")).clicked() {
            self.open_media_dialog();
        }

        ui.separator();

        if self.project.sources.is_empty() {
            ui.label(
                RichText::new(tr(self.lang, "No media yet"))
                    .color(Color32::from_rgb(150, 158, 175)),
            );
            ui.label(
                RichText::new(tr(self.lang, "Drag video files here"))
                    .color(Color32::from_rgb(150, 158, 175)),
            );
            return;
        }

        let mut add_clip: Option<u64> = None;
        let mut remove: Option<u64> = None;
        let mut select: Option<u64> = None;

        egui::ScrollArea::vertical().show(ui, |ui| {
            for source in self.project.visible_sources() {
                let id = source.id;
                let selected = self.project.selected_source == Some(id);
                ui.push_id(id, |ui| {
                    let response = ui
                        .selectable_label(
                            selected,
                            RichText::new(source.name()).strong().color(if selected {
                                Color32::from_rgb(120, 186, 245)
                            } else {
                                Color32::from_rgb(230, 233, 240)
                            }),
                        )
                        .clicked();
                    if response {
                        select = Some(id);
                    }

                    let description = match &source.state {
                        crate::project::SourceState::Loading => {
                            tr(self.lang, "Reading…").to_string()
                        }
                        crate::project::SourceState::Failed(message) => tf(
                            self.lang,
                            "Could not read the file: {error}",
                            &[("error", message)],
                        ),
                        crate::project::SourceState::Ready => match source.info.as_ref() {
                            Some(info) => format!(
                                "{} · {}",
                                format_timecode(info.duration),
                                format_bytes(info.size_bytes)
                            ),
                            None => String::new(),
                        },
                    };
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("+")
                            .on_hover_text(tr(self.lang, "Add to timeline"))
                            .clicked()
                        {
                            add_clip = Some(id);
                        }
                        if ui
                            .small_button("✕")
                            .on_hover_text(tr(self.lang, "Delete"))
                            .clicked()
                        {
                            remove = Some(id);
                        }
                        ui.label(
                            RichText::new(description)
                                .small()
                                .color(Color32::from_rgb(150, 158, 175)),
                        );
                    });
                });
            }
        });

        // The buttons of the rows are collected above, but the click on the
        // small button also selects the row, so the buttons win.
        if let Some(id) = add_clip {
            self.project.add_whole_source(id);
        }
        if let Some(id) = remove {
            self.remove_source(id);
        }
        if let Some(id) = select {
            self.select_source(id);
        }
        let _ = toasts;
        let _ = ctx;
    }

    /// Select a source and show it in the preview.
    fn select_source(&mut self, id: u64) {
        self.project.selected_source = Some(id);
        // A different file starts at its beginning, the position of the
        // preview only belongs to the file that is already open.
        let position = if self.player_source == Some(id) {
            self.player.position()
        } else {
            0.0
        };
        self.open_player(id, position);
    }

    /// Remove a source and every clip that was cut from it.
    fn remove_source(&mut self, id: u64) {
        if !self.project.remove_source(id) {
            return;
        }
        if self.project.selected_source == Some(id) {
            let next = self
                .project
                .visible_sources()
                .next()
                .map(|source| source.id);
            self.project.selected_source = next;
            match next {
                Some(next) => self.open_player(next, 0.0),
                None => {
                    self.player.close();
                    self.player_source = None;
                }
            }
        }
    }

    /// Draw the settings of the selected clip and the export options.
    pub fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        jobs: &mut JobManager,
        toasts: &mut Toasts,
    ) {
        ui.heading(tr(self.lang, "Clip"));
        ui.separator();

        match self.project.selected_clip() {
            Some(clip) => {
                let source_id = clip.source_id;
                let clip_id = clip.id;
                let start = clip.start;
                let end = clip.end;
                ui.label(RichText::new(self.source_name(source_id)).strong());
                ui.label(format!(
                    "{}: {}",
                    tr(self.lang, "In"),
                    format_timecode_ms(start)
                ));
                ui.label(format!(
                    "{}: {}",
                    tr(self.lang, "Out"),
                    format_timecode_ms(end)
                ));
                ui.label(format!(
                    "{}: {}",
                    tr(self.lang, "Duration"),
                    format_timecode(end - start)
                ));
                ui.horizontal(|ui| {
                    if ui.button(tr(self.lang, "Duplicate")).clicked() {
                        self.duplicate_selected(ctx, toasts);
                    }
                    if ui.button(tr(self.lang, "Delete")).clicked() {
                        self.project.remove_clip(clip_id);
                    }
                });
            }
            None => {
                ui.label(
                    RichText::new(tr(self.lang, "No clip is selected"))
                        .color(Color32::from_rgb(150, 158, 175)),
                );
            }
        }

        ui.separator();
        ui.heading(tr(self.lang, "Export"));
        ui.separator();

        ui.horizontal(|ui| {
            ui.label(tr(self.lang, "Encoding"));
            let label = match self.encode_mode {
                EncodeMode::Lossless => tr(self.lang, "Lossless (copy)"),
                EncodeMode::Precise => tr(self.lang, "Precise (re-encode)"),
            };
            egui::ComboBox::from_id_salt("encode-mode")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.encode_mode,
                        EncodeMode::Lossless,
                        tr(self.lang, "Lossless (copy)"),
                    );
                    ui.selectable_value(
                        &mut self.encode_mode,
                        EncodeMode::Precise,
                        tr(self.lang, "Precise (re-encode)"),
                    );
                });
        });

        if let Some(details) = self.project.incompatibility() {
            ui.colored_label(Color32::from_rgb(240, 180, 80), "⚠");
            let reason = tf(
                self.lang,
                "The files differ: codecs [{codecs}], resolutions [{resolutions}], frame rates [{rates}], audio [{audio}]. A stream copy may fail, use “Precise” to re-encode.",
                &[
                    ("codecs", &details.codecs),
                    ("resolutions", &details.resolutions),
                    ("rates", &details.rates),
                    ("audio", &details.audio),
                ],
            );
            ui.label(RichText::new(reason).color(Color32::from_rgb(240, 196, 130)));
        } else if self.project.clips.len() >= 2 {
            ui.colored_label(Color32::from_rgb(96, 200, 120), "✔");
            ui.label(tr(self.lang, "All clips share the same format."));
        }

        ui.checkbox(
            &mut self.snap,
            tr(self.lang, "Snap to key frames (lossless)"),
        );

        ui.horizontal_wrapped(|ui| {
            ui.label(tr(self.lang, "Output file"));
            let label = self
                .output
                .as_ref()
                .map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .unwrap_or_else(|| tr(self.lang, "not chosen").to_string());
            ui.label(label);
            if ui.button(tr(self.lang, "Choose…")).clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Choose the output file")
                    .add_filter("Video", &["mp4", "mkv", "mov", "webm"])
                    .save_file()
                {
                    self.output = Some(path);
                }
            }
        });

        let ready = !self.project.clips.is_empty() && self.output.is_some();
        let export = ui
            .add_enabled(
                ready,
                egui::Button::new(tr(self.lang, "Export the timeline")),
            )
            .on_hover_text(tr(self.lang, "Export the timeline"));
        if export.clicked() {
            self.export(jobs, toasts, ctx);
        }

        if self.project.clips.is_empty() {
            ui.label(
                RichText::new(tr(self.lang, "Add at least one clip before exporting."))
                    .color(Color32::from_rgb(150, 158, 175)),
            );
        } else if self.output.is_none() {
            ui.label(
                RichText::new(tr(self.lang, "Choose where the result should be written."))
                    .color(Color32::from_rgb(150, 158, 175)),
            );
        }
    }

    /// Open the file dialog of the media list.
    fn open_media_dialog(&mut self) {
        let Some(tools) = self.tools.clone() else {
            return;
        };
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mkv", "mov", "webm", "avi", "m4v", "ts"])
            .pick_files()
        {
            for file in files {
                self.open_path(&file, &tools, true);
            }
        }
    }

    /// Write the timeline to a single file.
    ///
    /// When no output file has been chosen yet, the file dialog opens first.
    pub fn export_now(&mut self, jobs: &mut JobManager, toasts: &mut Toasts, ctx: &egui::Context) {
        if self.output.is_none() {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Choose the output file")
                .add_filter("Video", &["mp4", "mkv", "mov", "webm"])
                .save_file()
            {
                self.output = Some(path);
            }
        }
        self.export(jobs, toasts, ctx);
    }

    /// Write the timeline to a single file.
    fn export(&mut self, jobs: &mut JobManager, toasts: &mut Toasts, ctx: &egui::Context) {
        let Some(output) = self.output.clone() else {
            return;
        };
        let Some(tools) = self.tools.clone() else {
            toasts.error(ctx, tr(self.lang, "The FFmpeg tools are not available."));
            return;
        };

        let mut clips: Vec<ExportClip> = Vec::new();
        let mut skipped = 0usize;
        for clip in &self.project.clips {
            match self.project.source(clip.source_id) {
                Some(source) => clips.push(ExportClip {
                    path: source.path.clone(),
                    start: clip.start,
                    end: clip.end,
                }),
                None => skipped += 1,
            }
        }
        if clips.is_empty() {
            toasts.error(ctx, tr(self.lang, "There is nothing to export."));
            return;
        }
        if skipped > 0 {
            toasts.warning(
                ctx,
                tf(
                    self.lang,
                    "{count} clip(s) were skipped because their file is gone.",
                    &[("count", &skipped.to_string())],
                ),
            );
        }

        let count = clips.len();
        let mode = self.encode_mode;
        let encoder = self.encoder.clone();

        jobs.start(
            tf(
                self.lang,
                "Exporting {count} clip(s)",
                &[("count", &count.to_string())],
            ),
            move |cancel, progress| {
                Box::pin(async move {
                    processor::export_clips(
                        &tools.ffmpeg,
                        &clips,
                        &output,
                        mode,
                        &encoder,
                        &progress,
                        &cancel,
                    )
                    .await
                })
            },
        );
        toasts.info(
            ctx,
            tf(
                self.lang,
                "Started exporting {count} clip(s).",
                &[("count", &count.to_string())],
            ),
        );
    }
}

/// The actions that a shortcut can trigger.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    /// Start or stop the playback.
    TogglePlay,
    /// Move the play head inside the source.
    Seek(f64),
    /// Move one frame forwards or backwards.
    Frame(i64),
    /// Mark the beginning of the range.
    MarkIn,
    /// Mark the end of the range.
    MarkOut,
    /// Cut the selected clip at the play head.
    Split,
    /// Cut the marked range out of the source.
    Extract,
    /// Jump to the beginning of the source.
    JumpStart,
    /// Jump to the end of the source.
    JumpEnd,
    /// Remove the selected clips.
    Delete,
    /// Copy the selected clip.
    Duplicate,
    /// Select every clip.
    SelectAll,
    /// Undo the last change.
    Undo,
    /// Redo the change that was undone.
    Redo,
    /// Make the timeline larger.
    ZoomIn,
    /// Make the timeline smaller.
    ZoomOut,
    /// Show the whole timeline.
    ZoomFit,
}
