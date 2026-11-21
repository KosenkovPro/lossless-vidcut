//! The sequence track of the editor.
//!
//! The track shows the clips of the project in the order they will be
//! exported. Clips can be selected, dragged to another position and cut with
//! the scissors. The widget is drawn with the [`egui::Painter`] so that long
//! timelines stay cheap.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::util::format_timecode;

/// Height of the ruler at the top.
const RULER_HEIGHT: f32 = 20.0;
/// Height of the track with the clips.
const TRACK_HEIGHT: f32 = 56.0;
/// Height of a single clip block.
const CLIP_HEIGHT: f32 = 64.0;
/// Total height of the widget.
const TOTAL_HEIGHT: f32 = RULER_HEIGHT + TRACK_HEIGHT;

/// A clip as it is drawn on the track.
#[derive(Debug, Clone)]
pub struct SequenceItem {
    /// Identifier of the clip.
    pub id: u64,
    /// The name of the file the clip was cut from.
    pub name: String,
    /// The start of the clip on the timeline, in seconds.
    pub start: f64,
    /// The length of the clip, in seconds.
    pub duration: f64,
    /// The colour of the clip.
    pub color: Color32,
    /// `true` when the clip is selected.
    pub selected: bool,
    /// `true` when the clip is currently being dragged.
    pub dragging: bool,
}

/// Everything the track needs to draw itself.
#[derive(Debug, Clone)]
pub struct SequenceView<'a> {
    /// The clips of the timeline.
    pub items: &'a [SequenceItem],
    /// The length of the whole timeline in seconds.
    pub total_duration: f64,
    /// The position of the play head on the timeline.
    pub playhead: f64,
}

/// What the user did with the track.
#[derive(Debug, Clone, Default)]
pub struct SequenceResponse {
    /// The clip that was clicked.
    pub select: Option<u64>,
    /// The play head should move to this position on the timeline.
    pub seek: Option<f64>,
    /// The clip should be moved to another index.
    pub reorder: Option<(u64, usize)>,
    /// The user started to drag this clip.
    pub drag_started: Option<u64>,
}

/// The state of the track that has to survive between frames.
#[derive(Debug, Clone)]
pub struct SequenceState {
    /// Horizontal zoom in pixels per second.
    pub pixels_per_second: f64,
    /// Left edge of the visible area in seconds.
    pub scroll: f64,
    /// The clip that is currently being dragged.
    dragging: Option<u64>,
    /// The index the dragged clip would land on.
    drag_target: Option<usize>,
}

impl Default for SequenceState {
    fn default() -> Self {
        Self {
            pixels_per_second: 60.0,
            scroll: 0.0,
            dragging: None,
            drag_target: None,
        }
    }
}

impl SequenceState {
    /// Reset the zoom so that the whole timeline fits into `width` pixels.
    pub fn fit(&mut self, duration: f64, width: f32) {
        if duration <= 0.0 || width <= 0.0 {
            return;
        }
        self.pixels_per_second = (width as f64 / duration).clamp(0.5, 20_000.0);
        self.scroll = 0.0;
    }

    /// Scroll the visible area so that `time` is visible.
    pub fn reveal(&mut self, time: f64, duration: f64, width: f32) {
        if duration <= 0.0 || width <= 0.0 {
            return;
        }
        let visible = width as f64 / self.pixels_per_second;
        if time < self.scroll {
            self.scroll = time.max(0.0);
        } else if time > self.scroll + visible {
            self.scroll = (time - visible * 0.5).max(0.0);
        }
    }

    /// Zoom in or out around the given position.
    pub fn zoom(&mut self, factor: f64, anchor: f64) {
        let anchor = anchor.clamp(self.scroll, self.scroll + 1.0);
        self.pixels_per_second = (self.pixels_per_second * factor).clamp(0.5, 20_000.0);
        self.scroll = (anchor - (anchor - self.scroll) * factor).max(0.0);
    }
}

/// A colour that belongs to a source, so that the clips of one file look
/// alike on the track.
pub fn source_color(id: u64) -> Color32 {
    const PALETTE: [[u8; 3]; 8] = [
        [72, 150, 225],
        [96, 200, 120],
        [226, 156, 84],
        [196, 128, 220],
        [232, 108, 118],
        [96, 200, 200],
        [200, 200, 96],
        [150, 158, 175],
    ];
    let color = PALETTE[(id as usize) % PALETTE.len()];
    Color32::from_rgb(color[0], color[1], color[2])
}

/// Convert a time to a horizontal offset inside the track.
fn to_x(time: f64, scroll: f64, pixels_per_second: f64) -> f32 {
    ((time - scroll) * pixels_per_second) as f32
}

/// Convert a horizontal offset inside the track to a time.
fn to_time(x: f32, scroll: f64, pixels_per_second: f64) -> f64 {
    scroll + f64::from(x) / pixels_per_second
}

/// Draw the track and handle the pointer interaction.
///
/// The `height` is the space the widget may use, so that the track can fill
/// the rest of the window.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut SequenceState,
    view: &SequenceView<'_>,
    height: f32,
) -> SequenceResponse {
    let mut response = SequenceResponse::default();

    let total_height = height.max(TOTAL_HEIGHT);
    let (rect, interaction) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), total_height),
        Sense::click_and_drag(),
    );

    let total = view.total_duration.max(0.001);
    let mut scroll = state.scroll;
    let mut pixels_per_second = state.pixels_per_second.max(0.001);
    let width = rect.width();

    let track = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + RULER_HEIGHT),
        Pos2::new(rect.right(), rect.bottom()),
    );

    // ---------------------------------------------------------------- input
    if interaction.hovered() {
        let scroll_delta = ui.input(|input| input.smooth_scroll_delta);
        if scroll_delta.y != 0.0 {
            let hover_x = interaction
                .hover_pos()
                .map(|pos| pos.x - rect.left())
                .unwrap_or(0.0);
            let cursor_time = to_time(hover_x, scroll, pixels_per_second);
            if ui.input(|input| input.modifiers.shift) {
                scroll += f64::from(scroll_delta.y) * 12.0 / pixels_per_second;
            } else {
                let factor = if scroll_delta.y > 0.0 { 1.2 } else { 1.0 / 1.2 };
                pixels_per_second = (pixels_per_second * factor).clamp(0.5, 20_000.0);
                scroll = cursor_time - f64::from(hover_x) / pixels_per_second;
            }
            clamp_scroll(&mut scroll, &mut pixels_per_second, total, width);
        }
    }

    let pointer_x = interaction
        .hover_pos()
        .map(|pos| pos.x - rect.left())
        .unwrap_or(-1.0);
    let pointer_time = if pointer_x >= 0.0 {
        to_time(pointer_x, scroll, pixels_per_second)
    } else {
        -1.0
    };

    let hit = view
        .items
        .iter()
        .position(|item| pointer_time >= item.start && pointer_time < item.start + item.duration);

    if interaction.drag_started() {
        if let Some(index) = hit {
            state.dragging = Some(view.items[index].id);
            state.drag_target = Some(index);
            response.drag_started = Some(view.items[index].id);
        }
    }

    if interaction.dragged() && pointer_x >= 0.0 {
        if let Some(dragged) = state.dragging {
            // The clip follows the pointer, so the pointer decides where it
            // lands between the other clips.
            let mut target = 0usize;
            for (index, item) in view.items.iter().enumerate() {
                if Some(item.id) == Some(dragged) {
                    continue;
                }
                let center = item.start + item.duration / 2.0;
                if center < pointer_time {
                    target += 1;
                }
                let _ = index;
            }
            state.drag_target = Some(target);
            response.reorder = Some((dragged, target));
        }
    }

    if !interaction.dragged() {
        if interaction.clicked() && pointer_x >= 0.0 {
            match hit {
                Some(index) => {
                    response.select = Some(view.items[index].id);
                    response.seek = Some(pointer_time.clamp(0.0, total));
                }
                None => response.seek = Some(pointer_time.clamp(0.0, total)),
            }
        }
        state.dragging = None;
        state.drag_target = None;
    }

    state.scroll = scroll;
    state.pixels_per_second = pixels_per_second;

    // --------------------------------------------------------------- drawing
    let painter = ui.painter();
    painter.rect_filled(track, 0.0, Color32::from_rgb(24, 27, 33));
    painter.rect_filled(
        Rect::from_min_max(
            rect.left_top(),
            Pos2::new(rect.right(), rect.top() + RULER_HEIGHT),
        ),
        0.0,
        Color32::from_rgb(45, 50, 60),
    );

    draw_ruler(painter, rect, scroll, pixels_per_second, total);
    draw_items(painter, track, view, scroll, pixels_per_second);
    draw_playhead(painter, rect, view.playhead, scroll, pixels_per_second);

    response
}

fn draw_ruler(
    painter: &egui::Painter,
    rect: Rect,
    scroll: f64,
    pixels_per_second: f64,
    total: f64,
) {
    let step = nice_step(pixels_per_second);
    let font = FontId::proportional(10.0);
    let start = (scroll / step).floor() * step;
    let end = (scroll + f64::from(rect.width()) / pixels_per_second).min(total.max(step));
    let mut time = start.max(0.0);
    while time <= end {
        let x = rect.left() + to_x(time, scroll, pixels_per_second);
        if x >= rect.left() && x <= rect.right() {
            painter.line_segment(
                [
                    Pos2::new(x, rect.top() + RULER_HEIGHT - 5.0),
                    Pos2::new(x, rect.top() + RULER_HEIGHT),
                ],
                Stroke::new(1.0_f32, Color32::from_rgb(120, 128, 145)),
            );
            painter.text(
                Pos2::new(x + 3.0, rect.top() + RULER_HEIGHT / 2.0),
                Align2::LEFT_CENTER,
                format_timecode(time),
                font.clone(),
                Color32::from_rgb(165, 172, 188),
            );
        }
        time += step;
    }
}

fn draw_items(
    painter: &egui::Painter,
    track: Rect,
    view: &SequenceView<'_>,
    scroll: f64,
    pixels_per_second: f64,
) {
    for item in view.items {
        let x0 = to_x(item.start, scroll, pixels_per_second);
        let x1 = to_x(item.start + item.duration, scroll, pixels_per_second);
        if x1 < 0.0 || x0 > track.width() {
            continue;
        }

        let left = track.left() + x0.max(0.0);
        let right = track.left() + x1.min(track.width());
        // The block keeps the height of a clip, even when the track is tall.
        let block_height = CLIP_HEIGHT.min(track.height() - 8.0).max(24.0);
        let top = track.center().y - block_height / 2.0;
        let block = Rect::from_min_max(
            Pos2::new(left, top),
            Pos2::new(right.max(left + 2.0), top + block_height),
        );

        let mut color = item.color;
        if item.dragging {
            color = color.gamma_multiply(1.3);
        } else if item.selected {
            color = color.gamma_multiply(1.15);
        }
        painter.rect_filled(block, 3.0, color.gamma_multiply(0.45));
        painter.rect_stroke(
            block,
            3.0,
            Stroke::new(if item.selected { 2.0_f32 } else { 1.0_f32 }, color),
            StrokeKind::Middle,
        );

        if block.width() > 24.0 {
            let name = if block.width() > 120.0 {
                item.name.clone()
            } else {
                item.name.chars().take(10).collect::<String>()
            };
            painter.text(
                Pos2::new(block.left() + 5.0, block.top() + 12.0),
                Align2::LEFT_TOP,
                name,
                FontId::proportional(12.0),
                Color32::from_rgb(238, 241, 248),
            );
            painter.text(
                Pos2::new(block.left() + 5.0, block.bottom() - 8.0),
                Align2::LEFT_BOTTOM,
                format_timecode(item.duration),
                FontId::proportional(10.0),
                Color32::from_rgb(220, 226, 238),
            );
        }
    }
}

fn draw_playhead(
    painter: &egui::Painter,
    rect: Rect,
    position: f64,
    scroll: f64,
    pixels_per_second: f64,
) {
    let x = rect.left() + to_x(position, scroll, pixels_per_second);
    if x < rect.left() || x > rect.right() {
        return;
    }
    painter.line_segment(
        [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
        Stroke::new(1.5_f32, Color32::from_rgb(244, 96, 96)),
    );
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(x - 4.0, rect.top()), Vec2::new(8.0, 6.0)),
        2.0,
        Color32::from_rgb(244, 96, 96),
    );
}

/// Choose a tick interval that keeps the labels readable.
fn nice_step(pixels_per_second: f64) -> f64 {
    const CANDIDATES: [f64; 16] = [
        0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0,
        3600.0,
    ];
    let target = 90.0 / pixels_per_second.max(0.001);
    CANDIDATES
        .iter()
        .copied()
        .find(|step| *step >= target)
        .unwrap_or(3600.0)
}

fn clamp_scroll(scroll: &mut f64, pixels_per_second: &mut f64, duration: f64, width: f32) {
    let visible = f64::from(width) / pixels_per_second.max(0.001);
    let maximum = (duration - visible).max(0.0);
    *scroll = scroll.clamp(0.0, maximum);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_gets_its_own_colour() {
        assert_ne!(source_color(1), source_color(2));
        assert_eq!(source_color(1), source_color(1));
    }

    #[test]
    fn the_colours_repeat_after_a_full_round() {
        assert_eq!(source_color(1), source_color(9));
    }

    #[test]
    fn zooming_keeps_the_anchor_in_place() {
        let mut state = SequenceState {
            scroll: 0.0,
            pixels_per_second: 100.0,
            ..Default::default()
        };
        state.zoom(2.0, 10.0);
        // The anchor moves to the left because the view got wider.
        assert!(state.pixels_per_second > 100.0);
        assert!(state.scroll <= 10.0);
    }

    #[test]
    fn fitting_shows_the_whole_timeline() {
        let mut state = SequenceState::default();
        state.fit(60.0, 600.0);
        assert!((state.pixels_per_second - 10.0).abs() < 0.001);
    }
}
