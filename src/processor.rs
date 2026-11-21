//! Generation and execution of `ffmpeg` jobs.
//!
//! The command builders in this module are pure functions so that they can be
//! unit tested without touching the file system. The runner around them turns
//! an argument list into a cancellable background job that reports progress.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// How the resulting file should be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeMode {
    /// Stream copy. Instant, but the cut points snap to key frames.
    Lossless,
    /// Re-encode. Exact cut points, but slow.
    Precise,
}

impl EncodeMode {
    /// Human readable label used in the UI.
    pub fn label(self) -> &'static str {
        match self {
            EncodeMode::Lossless => "Lossless (copy)",
            EncodeMode::Precise => "Precise (re-encode)",
        }
    }
}

/// The video encoder that is used for precise cuts.
///
/// FFmpeg builds differ wildly in the encoders they ship, so the encoder is
/// detected at runtime instead of being hard coded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoEncoder {
    /// Name of the encoder, e.g. `libx264`.
    pub name: String,
    /// The quality arguments that belong to the encoder.
    pub quality: Vec<OsString>,
}

impl VideoEncoder {
    /// The encoder that is used when nothing else can be detected.
    pub fn fallback() -> Self {
        Self {
            name: "libx264".to_string(),
            quality: vec!["-crf".into(), "18".into()],
        }
    }
}

/// Detect the best available H.264 encoder of the given FFmpeg build.
pub fn detect_video_encoder(ffmpeg: &Path) -> VideoEncoder {
    let mut command = StdCommand::new(ffmpeg);
    command.args(["-hide_banner", "-loglevel", "error", "-encoders"]);
    crate::sys::hide_console(&mut command);
    let Ok(output) = command.output() else {
        return VideoEncoder::fallback();
    };
    let listing = String::from_utf8_lossy(&output.stdout);

    // The candidates are ordered from the best quality to the most widely
    // available one.
    let candidates: [(&str, &[&str]); 5] = [
        ("libx264", &["-crf", "18"]),
        ("libopenh264", &["-b:v", "4M"]),
        ("h264_nvenc", &["-cq", "20"]),
        ("h264_videotoolbox", &["-q:v", "60"]),
        ("mpeg4", &["-q:v", "3"]),
    ];
    for (name, quality) in candidates {
        if listing
            .lines()
            .any(|line| line.split_whitespace().any(|token| token == name))
        {
            return VideoEncoder {
                name: name.to_string(),
                quality: quality.iter().map(OsString::from).collect(),
            };
        }
    }
    VideoEncoder::fallback()
}

/// Progress of a running job.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// Completion ratio between 0 and 1.
    pub fraction: f32,
    /// Number of seconds of output produced so far.
    pub elapsed_seconds: f64,
    /// Description of the step that is currently running.
    pub stage: String,
}

/// Build the argument list that cuts `[start, start + duration)` out of
/// `input` and writes it to `output`.
///
/// `-ss` is placed before `-i` so that FFmpeg can seek directly to the
/// nearest key frame, which is what makes a stream copy cheap. The duration is
/// expressed with `-t` because it is unambiguous regardless of where `-ss`
/// ended up.
pub fn cut_args(
    input: &Path,
    output: &Path,
    start: f64,
    duration: f64,
    mode: EncodeMode,
    encoder: &VideoEncoder,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-ss".into(),
        format!("{start:.6}").into(),
        "-i".into(),
        input.as_os_str().to_os_string(),
        "-t".into(),
        format!("{duration:.6}").into(),
    ];

    match mode {
        EncodeMode::Lossless => {
            args.push("-c".into());
            args.push("copy".into());
            args.push("-avoid_negative_ts".into());
            args.push("make_zero".into());
        }
        EncodeMode::Precise => {
            args.push("-c:v".into());
            args.push(encoder.name.as_str().into());
            args.extend(encoder.quality.iter().cloned());
            args.push("-c:a".into());
            args.push("aac".into());
            args.push("-b:a".into());
            args.push("192k".into());
        }
    }

    args.push("-progress".into());
    args.push("pipe:1".into());
    args.push("-nostats".into());
    args.push(output.as_os_str().to_os_string());
    args
}

/// Build the argument list that concatenates the files listed in `list`.
pub fn concat_args(
    list: &Path,
    output: &Path,
    mode: EncodeMode,
    encoder: &VideoEncoder,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list.as_os_str().to_os_string(),
    ];

    match mode {
        EncodeMode::Lossless => {
            args.push("-c".into());
            args.push("copy".into());
        }
        EncodeMode::Precise => {
            args.push("-c:v".into());
            args.push(encoder.name.as_str().into());
            args.extend(encoder.quality.iter().cloned());
            args.push("-c:a".into());
            args.push("aac".into());
            args.push("-b:a".into());
            args.push("192k".into());
        }
    }

    args.push("-progress".into());
    args.push("pipe:1".into());
    args.push("-nostats".into());
    args.push(output.as_os_str().to_os_string());
    args
}

/// Render the contents of a concat demuxer list file.
pub fn concat_list_content(paths: &[PathBuf]) -> String {
    let mut out = String::new();
    for path in paths {
        out.push_str("file '");
        out.push_str(&escape_concat_path(path));
        out.push_str("'\n");
    }
    out
}

/// Escape a path for the concat demuxer, which uses single quotes.
fn escape_concat_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "'\\''")
}

/// Derive the name of the `n`-th part of a split export.
pub fn part_file_name(stem: &str, extension: &str, index: usize) -> String {
    format!("{stem}{index:04}.{extension}")
}

/// Derive the name of an output file for a segment.
pub fn segment_file_name(stem: &str, extension: &str, index: usize) -> String {
    format!("{stem}_segment{index}.{extension}")
}

/// The extension of a path without the leading dot, lower cased.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "mkv".to_string())
}

/// Run `ffmpeg` with `args`, reporting progress and honouring `cancel`.
///
/// The returned error contains the tail of the FFmpeg log, which is what the
/// user needs in order to understand a failure.
pub async fn run_ffmpeg(
    ffmpeg: &Path,
    args: &[OsString],
    duration: f64,
    progress: &Arc<Mutex<Progress>>,
    cancel: &AtomicBool,
) -> Result<()> {
    run_ffmpeg_within(ffmpeg, args, duration, progress, cancel, 0.0, 1.0).await
}

/// Run `ffmpeg` with `args`, reporting the progress.
///
/// The progress bar covers the part of the job between `base` and
/// `base + span`, so that several steps can share one bar.
#[allow(clippy::too_many_arguments)]
pub async fn run_ffmpeg_within(
    ffmpeg: &Path,
    args: &[OsString],
    duration: f64,
    progress: &Arc<Mutex<Progress>>,
    cancel: &AtomicBool,
    base: f32,
    span: f32,
) -> Result<()> {
    let mut command = Command::new(ffmpeg);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::sys::hide_console_tokio(&mut command);

    let mut child = command
        .spawn()
        .with_context(|| format!("could not start {}", ffmpeg.display()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg did not expose its progress stream"))?;
    let stderr = child.stderr.take();

    let stderr_task = tokio::spawn(async move {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut text).await;
        }
        text
    });
    let mut stderr_task = stderr_task;

    let mut lines = BufReader::new(stdout).lines();
    let log: String;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if let Some(value) = line.strip_prefix("out_time_us=") {
                            if let Ok(microseconds) = value.trim().parse::<i64>() {
                                let seconds = microseconds as f64 / 1_000_000.0;
                                if let Ok(mut state) = progress.lock() {
                                    state.elapsed_seconds = seconds;
                                    if duration > 0.0 {
                                        let part = (seconds / duration).clamp(0.0, 1.0) as f32;
                                        state.fraction = (base + span * part).clamp(0.0, 1.0);
                                    }
                                }
                            }
                        } else if line.starts_with("progress=end") {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = child.kill().await;
                        bail!("could not read the ffmpeg progress stream: {error}");
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(40)) => {
                if cancel.load(Ordering::Relaxed) {
                    let _ = child.kill().await;
                    log = (&mut stderr_task).await.unwrap_or_default();
                    bail!(cancelled_message(&log));
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .context("ffmpeg did not shut down cleanly")?;
    log = (&mut stderr_task).await.unwrap_or_default();

    if cancel.load(Ordering::Relaxed) {
        bail!(cancelled_message(&log));
    }
    if !status.success() {
        bail!("ffmpeg failed: {}", tail(&log));
    }
    Ok(())
}

/// Cut every segment into its own file next to `output`.
#[allow(clippy::too_many_arguments)]
pub async fn export_segments(
    ffmpeg: &Path,
    input: &Path,
    segments: &[(f64, f64)],
    output_dir: &Path,
    stem: &str,
    extension: &str,
    mode: EncodeMode,
    encoder: &VideoEncoder,
    progress: &Arc<Mutex<Progress>>,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>> {
    let mut produced = Vec::new();
    let total = segments.len().max(1) as f32;

    for (index, (start, end)) in segments.iter().enumerate() {
        let name = segment_file_name(stem, extension, index + 1);
        let target = output_dir.join(name);
        if let Ok(mut state) = progress.lock() {
            state.stage = format!("Segment {} of {}", index + 1, segments.len());
            state.fraction = index as f32 / total;
        }
        let args = cut_args(input, &target, *start, end - start, mode, encoder);
        run_ffmpeg(ffmpeg, &args, end - start, progress, cancel).await?;
        produced.push(target);
    }

    if let Ok(mut state) = progress.lock() {
        state.fraction = 1.0;
        state.stage = "Done".to_string();
    }
    Ok(produced)
}

/// Remove the parts of the source that are *not* covered by `segments` and
/// glue the remaining pieces back together.
#[allow(clippy::too_many_arguments)]
pub async fn export_cut_and_join(
    ffmpeg: &Path,
    input: &Path,
    segments: &[(f64, f64)],
    output: &Path,
    mode: EncodeMode,
    encoder: &VideoEncoder,
    progress: &Arc<Mutex<Progress>>,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let workdir = tempfile::tempdir().context("could not create a temporary directory")?;
    let mut parts = Vec::new();
    let total_duration: f64 = segments.iter().map(|(start, end)| end - start).sum();
    let total = segments.len().max(1) as f32;

    for (index, (start, end)) in segments.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let part = workdir
            .path()
            .join(part_file_name("part", "mkv", index + 1));
        if let Ok(mut state) = progress.lock() {
            state.stage = format!("Cutting segment {} of {}", index + 1, segments.len());
            state.fraction = index as f32 / total * 0.7;
        }
        let args = cut_args(input, &part, *start, end - start, mode, encoder);
        run_ffmpeg(ffmpeg, &args, end - start, progress, cancel).await?;
        parts.push(part);
    }

    let list = workdir.path().join("concat.txt");
    std::fs::write(&list, concat_list_content(&parts))
        .context("could not write the concat list")?;

    if let Ok(mut state) = progress.lock() {
        state.stage = "Joining the remaining parts".to_string();
        state.fraction = 0.75;
    }
    let args = concat_args(&list, output, mode, encoder);
    run_ffmpeg(ffmpeg, &args, total_duration, progress, cancel).await?;

    if let Ok(mut state) = progress.lock() {
        state.fraction = 1.0;
        state.stage = "Done".to_string();
    }
    Ok(output.to_path_buf())
}

/// Join a list of files into a single file.
#[allow(clippy::too_many_arguments)]
pub async fn export_join(
    ffmpeg: &Path,
    inputs: &[PathBuf],
    output: &Path,
    mode: EncodeMode,
    encoder: &VideoEncoder,
    duration: f64,
    progress: &Arc<Mutex<Progress>>,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    let workdir = tempfile::tempdir().context("could not create a temporary directory")?;
    let list = workdir.path().join("concat.txt");
    std::fs::write(&list, concat_list_content(inputs))
        .context("could not write the concat list")?;

    if let Ok(mut state) = progress.lock() {
        state.stage = "Joining the files".to_string();
        state.fraction = 0.0;
    }
    let args = concat_args(&list, output, mode, encoder);
    run_ffmpeg(ffmpeg, &args, duration, progress, cancel).await?;

    if let Ok(mut state) = progress.lock() {
        state.fraction = 1.0;
        state.stage = "Done".to_string();
    }
    Ok(output.to_path_buf())
}

/// A piece of a source file that has to be exported.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportClip {
    /// The file the clip was cut from.
    pub path: PathBuf,
    /// The start of the clip inside its source, in seconds.
    pub start: f64,
    /// The end of the clip inside its source, in seconds.
    pub end: f64,
}

impl ExportClip {
    /// The length of the clip in seconds.
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
}

/// Cut every clip out of its source and glue the pieces together.
///
/// The clips may come from different files, which is why every piece is cut
/// into a temporary file first. The pieces are then joined with the concat
/// demuxer, which is fast because the streams are only copied.
#[allow(clippy::too_many_arguments)]
pub async fn export_clips(
    ffmpeg: &Path,
    clips: &[ExportClip],
    output: &Path,
    mode: EncodeMode,
    encoder: &VideoEncoder,
    progress: &Arc<Mutex<Progress>>,
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    if clips.is_empty() {
        bail!("there is nothing to export");
    }

    let workdir = tempfile::tempdir().context("could not create a temporary directory")?;
    let mut parts = Vec::new();
    let total_duration: f64 = clips.iter().map(ExportClip::duration).sum();
    let total = clips.len().max(1) as f32;
    let mut elapsed = 0.0f64;

    for (index, clip) in clips.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let part = workdir
            .path()
            .join(part_file_name("part", "mkv", index + 1));
        // The progress is weighted by the length of the clips, so that a long
        // clip does not appear to be finished before a short one.
        let base = if total_duration > 0.0 {
            (elapsed / total_duration) as f32 * 0.7
        } else {
            index as f32 / total * 0.7
        };
        let span = if total_duration > 0.0 {
            (clip.duration() / total_duration) as f32 * 0.7
        } else {
            0.7 / total
        };
        if let Ok(mut state) = progress.lock() {
            state.stage = format!("Cutting clip {} of {}", index + 1, clips.len());
            state.fraction = base;
        }
        let args = cut_args(
            &clip.path,
            &part,
            clip.start,
            clip.duration(),
            mode,
            encoder,
        );
        run_ffmpeg_within(ffmpeg, &args, clip.duration(), progress, cancel, base, span).await?;
        elapsed += clip.duration();
        parts.push(part);
    }

    let list = workdir.path().join("concat.txt");
    std::fs::write(&list, concat_list_content(&parts))
        .context("could not write the concat list")?;

    if let Ok(mut state) = progress.lock() {
        state.stage = "Joining the clips".to_string();
        state.fraction = 0.75;
    }
    let args = concat_args(&list, output, mode, encoder);
    run_ffmpeg_within(ffmpeg, &args, total_duration, progress, cancel, 0.7, 0.3).await?;

    if let Ok(mut state) = progress.lock() {
        state.fraction = 1.0;
        state.stage = "Done".to_string();
    }
    Ok(output.to_path_buf())
}

/// The message that tells the user that the job was cancelled.
fn cancelled_message(log: &str) -> String {
    if log.trim().is_empty() {
        "cancelled".to_string()
    } else {
        format!("cancelled: {}", tail(log))
    }
}

/// The last few lines of the FFmpeg log, which usually contain the reason.
fn tail(log: &str) -> String {
    let lines: Vec<&str> = log.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(6);
    lines[start..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoder() -> VideoEncoder {
        VideoEncoder::fallback()
    }

    #[test]
    fn lossless_cut_uses_stream_copy() {
        let args = cut_args(
            Path::new("/in.mp4"),
            Path::new("/out.mp4"),
            12.5,
            30.0,
            EncodeMode::Lossless,
            &encoder(),
        );
        let text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(text.windows(2).any(|pair| pair == ["-c", "copy"]));
        assert!(text.windows(2).any(|pair| pair == ["-ss", "12.500000"]));
        assert!(text.windows(2).any(|pair| pair == ["-t", "30.000000"]));
        assert!(text.windows(2).any(|pair| pair == ["-i", "/in.mp4"]));
        assert!(text.windows(2).any(|pair| pair == ["-progress", "pipe:1"]));
    }

    #[test]
    fn precise_cut_re_encodes() {
        let args = cut_args(
            Path::new("/in.mp4"),
            Path::new("/out.mp4"),
            0.0,
            1.0,
            EncodeMode::Precise,
            &encoder(),
        );
        let text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(text.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
        assert!(text.windows(2).any(|pair| pair == ["-crf", "18"]));
        assert!(!text.windows(2).any(|pair| pair == ["-c", "copy"]));
    }

    #[test]
    fn concat_uses_the_concat_demuxer() {
        let args = concat_args(
            Path::new("/list.txt"),
            Path::new("/out.mp4"),
            EncodeMode::Lossless,
            &encoder(),
        );
        let text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(text.windows(2).any(|pair| pair == ["-f", "concat"]));
        assert!(text.windows(2).any(|pair| pair == ["-safe", "0"]));
        assert!(text.windows(2).any(|pair| pair == ["-c", "copy"]));
    }

    #[test]
    fn concat_list_escapes_quotes() {
        let list = concat_list_content(&[
            PathBuf::from("/tmp/a.mp4"),
            PathBuf::from("/tmp/it's here.mp4"),
        ]);
        assert_eq!(list, "file '/tmp/a.mp4'\nfile '/tmp/it'\\''s here.mp4'\n");
    }

    #[test]
    fn derives_part_names() {
        assert_eq!(part_file_name("movie", "mp4", 2), "movie0002.mp4");
        assert_eq!(segment_file_name("movie", "mp4", 1), "movie_segment1.mp4");
        assert_eq!(extension_of(Path::new("a/b.MOV")), "mov");
        assert_eq!(extension_of(Path::new("noext")), "mkv");
    }

    #[test]
    fn detects_the_available_encoder() {
        // The detection must never panic, whatever FFmpeg is installed.
        let encoder = detect_video_encoder(Path::new("ffmpeg-does-not-exist"));
        assert!(!encoder.name.is_empty());
    }
}
