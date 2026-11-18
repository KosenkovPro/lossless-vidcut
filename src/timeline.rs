//! The custom timeline widget.
//!
//! The widget draws the time ruler, the audio waveform, the key frame markers,
//! the list of selected segments and the play head. It is drawn directly with
//! the [`egui::Painter`] so that thousands of elements stay cheap.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::util::format_timecode;

/// A selected range of the source file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Start of the segment in seconds.
    pub start: f64,
    /// End of the segment in seconds.
    pub end: f64,
}

impl Segment {
    /// Create a segment, making sure the bounds are ordered.
    pub fn new(start: f64, end: f64) -> Self {
        if end < start {
            Self {
                start: end,
                end: start,
            }
        } else {
            Self { start, end }
        }
    }

    /// Length of the segment in seconds.
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    /// `true` when `time` lies inside the segment.
    pub fn contains(&self, time: f64) -> bool {
        time >= self.start && time <= self.end
    }
}

/// Height of the ruler at the top.
const RULER_HEIGHT: f32 = 22.0;
/// Height of the waveform band.
const WAVEFORM_HEIGHT: f32 = 62.0;
/// Height of the segment band at the bottom.
const SEGMENT_HEIGHT: f32 = 26.0;
/// Width of the grab area of the in and out markers.
const HANDLE_GRAB: f32 = 7.0;

/// Which part of the timeline is currently being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    /// Nothing is being dragged.
    None,
    /// The play head follows the pointer.
    Seek,
    /// The in marker follows the pointer.
    In,
    /// The out marker follows the pointer.
    Out,
}

/// The state of the timeline widget that has to survive between frames.
#[derive(Debug, Clone)]
pub struct TimelineState {
    /// Horizontal zoom in pixels per second.
    pub pixels_per_second: f64,
    /// Left edge of the visible area in seconds.
    pub scroll: f64,
    /// What the pointer is currently dragging.
    drag: Drag,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            pixels_per_second: 90.0,
            scroll: 0.0,
            drag: Drag::None,
        }
    }
}

impl TimelineState {
    /// Reset the zoom so that the whole file fits into `width` pixels.
    pub fn fit(&mut self, duration: f64, width: f32) {
        if duration <= 0.0 || width <= 0.0 {
            return;
        }
        self.pixels_per_second = (width as f64 / duration).max(0.001);
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
}

/// Everything the timeline needs to draw itself.
pub struct TimelineView<'a> {
    /// Length of the media file in seconds.
    pub duration: f64,
    /// Current play head position in seconds.
    pub position: f64,
    /// Timestamps of the key frames.
    pub keyframes: &'a [f64],
    /// Normalised peak amplitudes of the audio track.
    pub waveform: &'a [f32],
    /// The selected segments.
    pub segments: &'a [Segment],
    /// Start of the segment that is being marked.
    pub in_point: Option<f64>,
    /// End of the segment that is being marked.
    pub out_point: Option<f64>,
}

/// What the user did with the timeline.
#[derive(Debug, Clone, Default)]
pub struct TimelineResponse {
    /// The play head should move to this position.
    pub seek: Option<f64>,
    /// The in marker should move to this position.
    pub in_point: Option<f64>,
    /// The out marker should move to this position.
    pub out_point: Option<f64>,
}

fn color_waveform() -> Color32 {
    Color32::from_rgb(96, 165, 224)
}

fn color_waveform_background() -> Color32 {
    Color32::from_rgb(38, 42, 52)
}

fn color_keyframe() -> Color32 {
    Color32::from_rgb(240, 196, 86)
}

fn color_segment() -> Color32 {
    Color32::from_rgba_unmultiplied(72, 150, 225, 110)
}

fn color_segment_border() -> Color32 {
    Color32::from_rgb(120, 186, 245)
}

fn color_in_out() -> Color32 {
    Color32::from_rgb(96, 224, 152)
}

fn color_playhead() -> Color32 {
    Color32::from_rgb(244, 96, 96)
}

fn color_ruler() -> Color32 {
    Color32::from_rgb(52, 57, 68)
}

/// Convert a time to a horizontal offset inside the timeline.
fn to_x(time: f64, scroll: f64, pixels_per_second: f64) -> f32 {
    ((time - scroll) * pixels_per_second) as f32
}

/// Convert a horizontal offset inside the timeline to a time.
fn to_time(x: f32, scroll: f64, pixels_per_second: f64) -> f64 {
    scroll + f64::from(x) / pixels_per_second
}

/// Draw the timeline and handle the pointer interaction.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut TimelineState,
    view: &TimelineView<'_>,
) -> TimelineResponse {
    let mut response = TimelineResponse::default();
    let height = RULER_HEIGHT + WAVEFORM_HEIGHT + SEGMENT_HEIGHT;
    let (rect, interaction) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), height),
        Sense::click_and_drag(),
    );

    let duration = view.duration.max(0.001);
    let mut scroll = state.scroll;
    let mut pps = state.pixels_per_second.max(0.001);
    let width = rect.width();

    let ruler = Rect::from_min_max(
        rect.left_top(),
        Pos2::new(rect.right(), rect.top() + RULER_HEIGHT),
    );
    let waveform = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + RULER_HEIGHT),
        Pos2::new(rect.right(), rect.top() + RULER_HEIGHT + WAVEFORM_HEIGHT),
    );
    let segments = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + RULER_HEIGHT + WAVEFORM_HEIGHT),
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
            let cursor_time = to_time(hover_x, scroll, pps);
            if ui.input(|input| input.modifiers.shift) {
                // Shift scrolls the timeline sideways.
                scroll += f64::from(scroll_delta.y) * 12.0 / pps;
            } else {
                let factor = if scroll_delta.y > 0.0 { 1.2 } else { 1.0 / 1.2 };
                pps = (pps * factor).clamp(0.5, 20_000.0);
                // Keep the time under the pointer in place.
                scroll = cursor_time - f64::from(hover_x) / pps;
            }
            clamp_scroll(&mut scroll, &mut pps, duration, width);
        }
    }

    let pointer_x = interaction
        .hover_pos()
        .map(|pos| pos.x - rect.left())
        .unwrap_or(-1.0);

    if interaction.drag_started() {
        let in_x = view.in_point.map(|time| to_x(time, scroll, pps));
        let out_x = view.out_point.map(|time| to_x(time, scroll, pps));
        let grab_in = in_x
            .map(|x| (x - pointer_x).abs() <= HANDLE_GRAB)
            .unwrap_or(false);
        let grab_out = out_x
            .map(|x| (x - pointer_x).abs() <= HANDLE_GRAB)
            .unwrap_or(false);
        state.drag = if grab_in {
            Drag::In
        } else if grab_out {
            Drag::Out
        } else {
            Drag::Seek
        };
    }

    if interaction.dragged() && pointer_x >= 0.0 {
        let time = to_time(pointer_x, scroll, pps).clamp(0.0, duration);
        match state.drag {
            Drag::In => response.in_point = Some(time),
            Drag::Out => response.out_point = Some(time),
            Drag::Seek | Drag::None => response.seek = Some(time),
        }
    }

    if !interaction.dragged() && interaction.clicked() && pointer_x >= 0.0 {
        let in_x = view.in_point.map(|time| to_x(time, scroll, pps));
        let out_x = view.out_point.map(|time| to_x(time, scroll, pps));
        let grab_in = in_x
            .map(|x| (x - pointer_x).abs() <= HANDLE_GRAB)
            .unwrap_or(false);
        let grab_out = out_x
            .map(|x| (x - pointer_x).abs() <= HANDLE_GRAB)
            .unwrap_or(false);
        if !grab_in && !grab_out {
            response.seek = Some(to_time(pointer_x, scroll, pps).clamp(0.0, duration));
        }
    }

    if !interaction.dragged() {
        state.drag = Drag::None;
    }

    state.scroll = scroll;
    state.pixels_per_second = pps;

    // --------------------------------------------------------------- drawing
    let painter = ui.painter();
    painter.rect_filled(waveform, 0.0, color_waveform_background());
    painter.rect_filled(ruler, 0.0, color_ruler());

    draw_ruler(painter, rect, ruler, scroll, pps);
    draw_waveform(painter, waveform, view.waveform, duration, scroll, pps);
    draw_keyframes(painter, waveform, view.keyframes, scroll, pps);
    draw_segments(painter, segments, view.segments, scroll, pps);
    draw_markers(painter, rect, view, scroll, pps);
    draw_playhead(painter, rect, view.position, scroll, pps);

    response
}

fn draw_ruler(
    painter: &egui::Painter,
    rect: Rect,
    ruler: Rect,
    scroll: f64,
    pixels_per_second: f64,
) {
    let step = nice_step(pixels_per_second);
    let font = FontId::proportional(10.0);
    let start = (scroll / step).floor() * step;
    let end = scroll + f64::from(rect.width()) / pixels_per_second;
    let mut time = start.max(0.0);
    while time <= end {
        let x = to_x(time, scroll, pixels_per_second);
        if x >= 0.0 {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + x, ruler.bottom() - 6.0),
                    Pos2::new(rect.left() + x, ruler.bottom()),
                ],
                Stroke::new(1.0_f32, Color32::from_rgb(120, 128, 145)),
            );
            painter.text(
                Pos2::new(rect.left() + x + 3.0, ruler.center().y),
                Align2::LEFT_CENTER,
                format_timecode(time),
                font.clone(),
                Color32::from_rgb(165, 172, 188),
            );
        }
        time += step;
    }
}

fn draw_waveform(
    painter: &egui::Painter,
    area: Rect,
    waveform: &[f32],
    duration: f64,
    scroll: f64,
    pixels_per_second: f64,
) {
    if waveform.is_empty() {
        return;
    }
    let total = waveform.len() as f64;
    let visible_start = scroll;
    let visible_end = scroll + f64::from(area.width()) / pixels_per_second;
    let first = ((visible_start / duration) * total).floor().max(0.0) as usize;
    let last = (((visible_end / duration) * total).ceil().max(0.0) as usize).min(waveform.len());
    if first >= last {
        return;
    }

    let center = area.center().y;
    let half = area.height() * 0.5;
    let stroke = Stroke::new(1.0_f32, color_waveform());
    for (index, peak) in waveform[first..last].iter().enumerate() {
        let time = (index as f64 / total) * duration;
        let x = to_x(time, scroll, pixels_per_second);
        let amplitude = peak.clamp(0.0, 1.0) * half;
        if amplitude < 0.4 {
            continue;
        }
        painter.line_segment(
            [
                Pos2::new(area.left() + x, center - amplitude),
                Pos2::new(area.left() + x, center + amplitude),
            ],
            stroke,
        );
    }
}

fn draw_keyframes(
    painter: &egui::Painter,
    area: Rect,
    keyframes: &[f64],
    scroll: f64,
    pixels_per_second: f64,
) {
    if keyframes.is_empty() {
        return;
    }
    let visible_end = scroll + f64::from(area.width()) / pixels_per_second;
    let stroke = Stroke::new(1.0_f32, color_keyframe());
    for time in keyframes {
        if *time < scroll {
            continue;
        }
        if *time > visible_end {
            break;
        }
        let x = to_x(*time, scroll, pixels_per_second);
        painter.line_segment(
            [
                Pos2::new(area.left() + x, area.top()),
                Pos2::new(area.left() + x, area.top() + 8.0),
            ],
            stroke,
        );
    }
}

fn draw_segments(
    painter: &egui::Painter,
    area: Rect,
    segments: &[Segment],
    scroll: f64,
    pixels_per_second: f64,
) {
    for segment in segments {
        let x0 = to_x(segment.start, scroll, pixels_per_second);
        let x1 = to_x(segment.end, scroll, pixels_per_second);
        if x1 < 0.0 || x0 > area.width() {
            continue;
        }
        let band = Rect::from_min_max(
            Pos2::new(area.left() + x0.max(0.0), area.top() + 4.0),
            Pos2::new(area.left() + x1.min(area.width()), area.bottom() - 4.0),
        );
        painter.rect_filled(band, 3.0, color_segment());
        painter.rect_stroke(
            band,
            3.0,
            Stroke::new(1.0_f32, color_segment_border()),
            StrokeKind::Middle,
        );
    }
}

fn draw_markers(
    painter: &egui::Painter,
    rect: Rect,
    view: &TimelineView<'_>,
    scroll: f64,
    pixels_per_second: f64,
) {
    let stroke = Stroke::new(1.5_f32, color_in_out());
    for marker in [view.in_point, view.out_point].into_iter().flatten() {
        let x = rect.left() + to_x(marker, scroll, pixels_per_second);
        if x < rect.left() || x > rect.right() {
            continue;
        }
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            stroke,
        );
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x - 4.0, rect.top()), Vec2::new(8.0, 6.0)),
            2.0,
            color_in_out(),
        );
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
        Stroke::new(1.5_f32, color_playhead()),
    );
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(x - 4.0, rect.top()), Vec2::new(8.0, 6.0)),
        2.0,
        color_playhead(),
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
    fn segments_are_ordered() {
        let segment = Segment::new(10.0, 4.0);
        assert_eq!(segment.start, 4.0);
        assert_eq!(segment.end, 10.0);
        assert_eq!(segment.duration(), 6.0);
        assert!(segment.contains(7.0));
        assert!(!segment.contains(2.0));
    }

    #[test]
    fn tick_steps_grow_with_the_zoom() {
        let coarse = nice_step(10.0);
        let fine = nice_step(500.0);
        assert!(fine < coarse);
        assert!(nice_step(0.001) <= 3600.0);
    }

    #[test]
    fn fitting_sets_the_zoom() {
        let mut state = TimelineState::default();
        state.fit(100.0, 800.0);
        assert!((state.pixels_per_second - 8.0).abs() < 0.001);
        assert_eq!(state.scroll, 0.0);
    }

    #[test]
    fn time_and_position_map_onto_each_other() {
        let x = to_x(12.5, 2.0, 40.0);
        assert!((x - 420.0).abs() < 0.001);
        assert!((to_time(x, 2.0, 40.0) - 12.5).abs() < 0.001);
    }
}
