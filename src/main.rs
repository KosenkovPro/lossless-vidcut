//! Lossless VidCut - a fast desktop tool for cutting and joining videos
//! without re-encoding them.
//!
//! The application is a graphical program. On Windows it is therefore linked
//! with the `windows` subsystem, which means that no console window appears
//! when the program is started. The debug build keeps the console, because it
//! makes the log output visible while developing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use lossless_vidcut::app;
use std::path::PathBuf;

/// The name of the application, also used as the window title.
const APP_NAME: &str = "Lossless VidCut";

fn main() -> eframe::Result<()> {
    init_logging();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([1040.0, 680.0])
            .with_icon(app_icon()),
        ..Default::default()
    };

    // An optional file name opens directly in the editor.
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

/// Start the logger.
///
/// A release build has no console, so the log is written into a file instead
/// of the standard error stream.
fn init_logging() {
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    );
    builder.format_timestamp_millis();

    #[cfg(not(debug_assertions))]
    {
        if let Some(path) = log_file() {
            if let Some(parent) = path.parent() {
                if std::fs::create_dir_all(parent).is_ok() {
                    if let Ok(file) = std::fs::File::create(&path) {
                        builder.target(env_logger::Target::Pipe(Box::new(file)));
                    }
                }
            }
        }
    }

    let _ = builder.try_init();
}

/// The file the release build writes its log to.
#[cfg(not(debug_assertions))]
fn log_file() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join("lossless-vidcut").join("vidcut.log"))
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
                if px > -14.0 && px < 14.0 && py.abs() < (14.0 - px) * 0.8 {
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
