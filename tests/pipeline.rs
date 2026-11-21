//! End to end tests that drive a real FFmpeg installation.
//!
//! The tests are skipped automatically when `ffmpeg` cannot be found, so the
//! suite still passes on a machine without FFmpeg.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use lossless_vidcut::ffmpeg_tools::Tools;
use lossless_vidcut::processor::{self, EncodeMode, ExportClip, VideoEncoder};

/// `true` when both FFmpeg binaries are available.
fn tools() -> Option<Tools> {
    let tools = Tools::detect()?;
    tools.valid().then_some(tools)
}

/// The duration of a file in seconds.
fn duration(ffprobe: &Path, file: &Path) -> f64 {
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(file)
        .output()
        .expect("ffprobe runs");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0)
}

/// Create a short test video with a key frame every second.
fn create_sample(ffmpeg: &Path, encoder: &VideoEncoder, target: &Path) -> bool {
    let status = Command::new(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x180:rate=30:duration=10",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=10"])
        .args(["-c:v", &encoder.name])
        .args(["-g", "30", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac", "-shortest"])
        .arg(target)
        .status();
    matches!(status, Ok(status) if status.success())
}

#[test]
fn splits_a_real_file_into_segments() {
    let Some(tools) = tools() else {
        eprintln!("ffmpeg is not installed, skipping");
        return;
    };
    let encoder = processor::detect_video_encoder(&tools.ffmpeg);
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("source.mp4");
    if !create_sample(&tools.ffmpeg, &encoder, &source) {
        eprintln!("could not create the test video, skipping");
        return;
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let progress = Arc::new(Mutex::new(processor::Progress::default()));
    let cancel = AtomicBool::new(false);

    let segments = vec![(0.0_f64, 2.0_f64), (4.0, 6.0)];
    let produced = runtime
        .block_on(processor::export_segments(
            &tools.ffmpeg,
            &source,
            &segments,
            directory.path(),
            "source",
            "mp4",
            EncodeMode::Lossless,
            &encoder,
            &progress,
            &cancel,
        ))
        .expect("the export runs");

    assert_eq!(produced.len(), 2);
    for file in &produced {
        assert!(file.is_file(), "{} was not written", file.display());
        let length = duration(&tools.ffprobe, file);
        assert!(
            (length - 2.0).abs() < 0.3,
            "expected about two seconds, got {length}"
        );
    }

    let state = progress.lock().expect("progress");
    assert_eq!(state.fraction, 1.0);
}

#[test]
fn cuts_and_joins_the_remaining_parts() {
    let Some(tools) = tools() else {
        eprintln!("ffmpeg is not installed, skipping");
        return;
    };
    let encoder = processor::detect_video_encoder(&tools.ffmpeg);
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("source.mp4");
    if !create_sample(&tools.ffmpeg, &encoder, &source) {
        eprintln!("could not create the test video, skipping");
        return;
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let progress = Arc::new(Mutex::new(processor::Progress::default()));
    let cancel = AtomicBool::new(false);

    let segments = vec![(1.0_f64, 3.0_f64), (6.0, 8.0)];
    let target = directory.path().join("joined.mp4");
    runtime
        .block_on(processor::export_cut_and_join(
            &tools.ffmpeg,
            &source,
            &segments,
            &target,
            EncodeMode::Lossless,
            &encoder,
            &progress,
            &cancel,
        ))
        .expect("the export runs");

    assert!(target.is_file());
    let length = duration(&tools.ffprobe, &target);
    assert!(
        (length - 4.0).abs() < 0.6,
        "expected about four seconds, got {length}"
    );
}

#[test]
fn joins_several_files() {
    let Some(tools) = tools() else {
        eprintln!("ffmpeg is not installed, skipping");
        return;
    };
    let encoder = processor::detect_video_encoder(&tools.ffmpeg);
    let directory = tempfile::tempdir().expect("temp dir");
    let first = directory.path().join("first.mp4");
    let second = directory.path().join("second.mp4");
    if !create_sample(&tools.ffmpeg, &encoder, &first)
        || !create_sample(&tools.ffmpeg, &encoder, &second)
    {
        eprintln!("could not create the test video, skipping");
        return;
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let progress = Arc::new(Mutex::new(processor::Progress::default()));
    let cancel = AtomicBool::new(false);

    let inputs: Vec<PathBuf> = vec![first.clone(), second.clone()];
    let target = directory.path().join("merged.mp4");
    runtime
        .block_on(processor::export_join(
            &tools.ffmpeg,
            &inputs,
            &target,
            EncodeMode::Lossless,
            &encoder,
            20.0,
            &progress,
            &cancel,
        ))
        .expect("the join runs");

    assert!(target.is_file());
    let length = duration(&tools.ffprobe, &target);
    assert!(
        (length - 20.0).abs() < 1.5,
        "expected about twenty seconds, got {length}"
    );
}

#[test]
fn exports_clips_from_several_sources() {
    let Some(tools) = tools() else {
        eprintln!("ffmpeg is not installed, skipping");
        return;
    };
    let encoder = processor::detect_video_encoder(&tools.ffmpeg);
    let directory = tempfile::tempdir().expect("temp dir");
    let first = directory.path().join("first.mp4");
    let second = directory.path().join("second.mp4");
    if !create_sample(&tools.ffmpeg, &encoder, &first)
        || !create_sample(&tools.ffmpeg, &encoder, &second)
    {
        eprintln!("could not create the test videos, skipping");
        return;
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let progress = Arc::new(Mutex::new(processor::Progress::default()));
    let cancel = AtomicBool::new(false);

    // Two pieces out of two different files, as the timeline produces them.
    let clips = vec![
        ExportClip {
            path: first.clone(),
            start: 1.0_f64,
            end: 3.0_f64,
        },
        ExportClip {
            path: second.clone(),
            start: 6.0_f64,
            end: 9.0_f64,
        },
    ];
    let target = directory.path().join("timeline.mp4");
    runtime
        .block_on(processor::export_clips(
            &tools.ffmpeg,
            &clips,
            &target,
            EncodeMode::Lossless,
            &encoder,
            &progress,
            &cancel,
        ))
        .expect("the export runs");

    assert!(target.is_file());
    let length = duration(&tools.ffprobe, &target);
    assert!(
        (length - 5.0).abs() < 1.0,
        "expected about five seconds, got {length}"
    );
}

#[test]
fn a_cancelled_job_stops_early() {
    let Some(tools) = tools() else {
        eprintln!("ffmpeg is not installed, skipping");
        return;
    };
    let encoder = processor::detect_video_encoder(&tools.ffmpeg);
    let directory = tempfile::tempdir().expect("temp dir");
    let source = directory.path().join("source.mp4");
    if !create_sample(&tools.ffmpeg, &encoder, &source) {
        eprintln!("could not create the test video, skipping");
        return;
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let progress = Arc::new(Mutex::new(processor::Progress::default()));
    let cancel = AtomicBool::new(true);

    let result = runtime.block_on(processor::export_segments(
        &tools.ffmpeg,
        &source,
        &[(0.0, 2.0)],
        directory.path(),
        "cancelled",
        "mp4",
        EncodeMode::Lossless,
        &encoder,
        &progress,
        &cancel,
    ));
    assert!(result.is_err(), "the job should have been cancelled");
}
