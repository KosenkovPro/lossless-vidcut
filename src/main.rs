//! Lossless VidCut - a fast desktop tool for cutting and joining videos
//! without re-encoding them.

use eframe::egui;
use lossless_vidcut::app;
use std::path::PathBuf;

/// The name of the application, also used as the window title.
const APP_NAME: &str = "Lossless VidCut";

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .format_timestamp_millis()
    .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([960.0, 640.0])
            .with_icon(app_icon()),
        ..Default::default()
    };

    // An optional file name opens directly in the splitter.
    let initial_file: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            let mut application = app::App::new(cc);
            if let Some(path) = initial_file.as_ref() {
                application.open_path(path);
            }
            Ok(Box::new(application) as Box<dyn eframe::App>)
        }),
    )
}

/// Draw the application icon procedurally so that no image file is needed.
fn app_icon() -> egui::IconData {
    const SIZE: usize = 64;
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);

    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = x as f32 - SIZE as f32 / 2.0;
            let py = y as f32 - SIZE as f32 / 2.0;
            let radius = SIZE as f32 / 2.0 - 1.0;
            let distance = (px * px + py * py).sqrt();

            let (mut r, mut g, mut b) = (28_u8, 32, 40);
            if distance <= radius {
                // A blue disc.
                (r, g, b) = (46, 108, 178);
                // A white play triangle.
                let in_triangle = px > -14.0 && px < 14.0 && py.abs() < (14.0 - px) * 0.8;
                if in_triangle {
                    (r, g, b) = (240, 244, 250);
                }
            }
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }

    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}
