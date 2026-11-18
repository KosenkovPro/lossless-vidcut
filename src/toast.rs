//! Transient notifications that are shown in the corner of the window.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, FontId, Margin, Stroke, Vec2};

/// The severity of a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Neutral information.
    Info,
    /// The operation finished successfully.
    Success,
    /// Something needs attention but is not fatal.
    Warning,
    /// The operation failed.
    Error,
}

impl Kind {
    fn color(self) -> Color32 {
        match self {
            Kind::Info => Color32::from_rgb(96, 165, 224),
            Kind::Success => Color32::from_rgb(96, 200, 120),
            Kind::Warning => Color32::from_rgb(240, 180, 80),
            Kind::Error => Color32::from_rgb(240, 96, 96),
        }
    }
}

/// A single notification.
#[derive(Debug, Clone)]
struct Toast {
    message: String,
    kind: Kind,
    expires: Instant,
}

/// A collection of notifications.
#[derive(Debug, Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    /// Show a notification.
    pub fn show(&mut self, ctx: &Context, message: impl Into<String>, kind: Kind) {
        let message = message.into();
        log::info!("{message}");
        self.items.push(Toast {
            message,
            kind,
            expires: Instant::now() + Duration::from_secs(7),
        });
        // Keep the list short so that it cannot cover the whole window.
        if self.items.len() > 5 {
            self.items.remove(0);
        }
        ctx.request_repaint();
    }

    /// Show an informational notification.
    pub fn info(&mut self, ctx: &Context, message: impl Into<String>) {
        self.show(ctx, message, Kind::Info);
    }

    /// Show a success notification.
    pub fn success(&mut self, ctx: &Context, message: impl Into<String>) {
        self.show(ctx, message, Kind::Success);
    }

    /// Show a warning.
    pub fn warning(&mut self, ctx: &Context, message: impl Into<String>) {
        self.show(ctx, message, Kind::Warning);
    }

    /// Show an error.
    pub fn error(&mut self, ctx: &Context, message: impl Into<String>) {
        self.show(ctx, message, Kind::Error);
    }

    /// Draw the notifications and remove the expired ones.
    pub fn draw(&mut self, ctx: &Context) {
        let now = Instant::now();
        self.items.retain(|toast| toast.expires > now);
        if self.items.is_empty() {
            return;
        }

        let area = egui::Area::new(egui::Id::new("vidcut-toasts"))
            .anchor(Align2::RIGHT_TOP, Vec2::new(-16.0, 16.0))
            .order(egui::Order::Tooltip);
        area.show(ctx, |ui| {
            for toast in &self.items {
                let color = toast.kind.color();
                egui::Frame::default()
                    .fill(Color32::from_rgb(38, 41, 49))
                    .stroke(Stroke::new(1.0_f32, color))
                    .corner_radius(6.0)
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.colored_label(color, "●");
                            ui.label(&toast.message);
                        });
                    });
                ui.add_space(4.0);
            }
        });
    }
}

/// The font used for the small captions in the preview.
pub fn caption_font() -> FontId {
    FontId::proportional(11.0)
}
