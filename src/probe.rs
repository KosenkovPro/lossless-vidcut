//! Metadata extraction via `ffprobe`.
//!
//! Everything in this module is pure parsing: the FFmpeg command line is
//! built once and the JSON / CSV output is turned into strongly typed values.
//! No function here panics on malformed input - every failure is reported as
//! an [`anyhow::Error`] that the UI turns into a toast.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// Everything the UI needs to know about a media file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaInfo {
    /// Path of the probed file.
    pub path: PathBuf,
    /// Total duration in seconds.
    pub duration: f64,
    /// Size of the file on disk in bytes.
    pub size_bytes: u64,
    /// Container format name as reported by FFmpeg (`mov,mp4,m4a,3gp,3g2,mj2`).
    pub format_name: String,
    /// Overall bitrate in bits per second.
    pub bitrate: u64,
    /// The first video stream, if the file contains one.
    pub video: Option<VideoInfo>,
    /// The first audio stream, if the file contains one.
    pub audio: Option<AudioInfo>,
}

impl MediaInfo {
    /// Frames per second of the video stream, defaulting to 25 when unknown.
    pub fn fps(&self) -> f64 {
        self.video
            .as_ref()
            .map(|v| v.fps)
            .unwrap_or(25.0)
            .max(0.001)
    }

    /// `true` when the file carries an audio track.
    pub fn has_audio(&self) -> bool {
        self.audio.is_some()
    }
}

/// Description of the video stream.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoInfo {
    /// Codec name, e.g. `h264`.
    pub codec: String,
    /// Coded width in pixels.
    pub width: u32,
    /// Coded height in pixels.
    pub height: u32,
    /// Average frame rate in frames per second.
    pub fps: f64,
    /// Pixel format, e.g. `yuv420p`.
    pub pix_fmt: String,
    /// Stream bitrate in bits per second.
    pub bitrate: u64,
}

/// Description of the audio stream.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AudioInfo {
    /// Codec name, e.g. `aac`.
    pub codec: String,
    /// Number of channels.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Stream bitrate in bits per second.
    pub bitrate: u64,
}

/// Probe `path` and return its metadata.
pub fn probe(ffprobe: &Path, path: &Path) -> Result<MediaInfo> {
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path);
    crate::sys::hide_console(&mut command);
    let output = command
        .output()
        .with_context(|| format!("could not run ffprobe on {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let root: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("ffprobe returned malformed JSON")?;

    let format = &root["format"];
    let mut info = MediaInfo {
        path: path.to_path_buf(),
        duration: as_f64(&format["duration"]).unwrap_or(0.0),
        size_bytes: as_u64(&format["size"])
            .unwrap_or_else(|| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)),
        format_name: format["format_name"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        bitrate: as_u64(&format["bit_rate"]).unwrap_or(0),
        video: None,
        audio: None,
    };

    let streams = root["streams"].as_array().cloned().unwrap_or_default();
    for stream in &streams {
        let kind = stream["codec_type"].as_str().unwrap_or_default();
        match kind {
            "video" if info.video.is_none() => {
                let fps = as_ratio(&stream["avg_frame_rate"])
                    .or_else(|| as_ratio(&stream["r_frame_rate"]))
                    .unwrap_or(0.0);
                info.video = Some(VideoInfo {
                    codec: stream["codec_name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    width: as_u64(&stream["width"]).unwrap_or(0) as u32,
                    height: as_u64(&stream["height"]).unwrap_or(0) as u32,
                    fps,
                    pix_fmt: stream["pix_fmt"].as_str().unwrap_or_default().to_string(),
                    bitrate: as_u64(&stream["bit_rate"]).unwrap_or(0),
                });
            }
            "audio" if info.audio.is_none() => {
                info.audio = Some(AudioInfo {
                    codec: stream["codec_name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    channels: as_u64(&stream["channels"]).unwrap_or(0) as u16,
                    sample_rate: as_u64(&stream["sample_rate"]).unwrap_or(0) as u32,
                    bitrate: as_u64(&stream["bit_rate"]).unwrap_or(0),
                });
            }
            _ => {}
        }
    }

    if info.duration <= 0.0 {
        info.duration = streams
            .iter()
            .filter_map(|s| as_f64(&s["duration"]))
            .fold(0.0_f64, f64::max);
    }

    Ok(info)
}

/// Collect the presentation timestamps of all key frames of the first video
/// stream.
///
/// The packets are inspected instead of the decoded frames, which makes this
/// orders of magnitude faster on long files.
pub fn keyframes(ffprobe: &Path, path: &Path) -> Result<Vec<f64>> {
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_packets",
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "json",
        ])
        .arg(path);
    crate::sys::hide_console(&mut command);
    let output = command
        .output()
        .with_context(|| format!("could not run ffprobe on {}", path.display()))?;

    if !output.status.success() {
        bail!(
            "ffprobe failed while reading key frames: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let root: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("ffprobe returned malformed JSON")?;

    let mut times = Vec::new();
    for packet in root["packets"].as_array().into_iter().flatten() {
        let flags = packet["flags"].as_str().unwrap_or_default();
        if !flags.contains('K') {
            continue;
        }
        if let Some(time) = as_f64(&packet["pts_time"]) {
            if time.is_finite() && time >= 0.0 {
                times.push(time);
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Ok(times)
}

/// Compute the peak amplitude envelope of the audio track.
///
/// The audio is decoded to mono 8 kHz float PCM on the fly and bucketed into
/// `buckets` slots, each holding the loudest absolute sample of its range.
/// The result is normalised to `0.0..=1.0` and is used to draw the waveform
/// behind the timeline.
pub fn waveform(ffmpeg: &Path, path: &Path, duration: f64, buckets: usize) -> Result<Vec<f32>> {
    if buckets == 0 {
        return Ok(Vec::new());
    }

    const RATE: u32 = 8000;
    let mut peaks = vec![0.0_f32; buckets];

    let mut command = Command::new(ffmpeg);
    command
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-sn", "-dn", "-ac", "1", "-ar"])
        .arg(RATE.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::sys::hide_console(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("could not run ffmpeg on {}", path.display()))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg produced no audio stream"))?;

    let total_samples = (duration.max(0.0) * f64::from(RATE)) as usize;
    let mut sample_index: usize = 0;
    let mut byte_buffer = vec![0_u8; 8192 * 4];

    use std::io::Read;
    loop {
        let read = stdout
            .read(&mut byte_buffer)
            .context("could not read the decoded audio")?;
        if read == 0 {
            break;
        }
        let count = read / 4;
        for slot in byte_buffer[..count * 4].chunks_exact(4) {
            let value = f32::from_le_bytes([slot[0], slot[1], slot[2], slot[3]]);
            let amplitude = value.abs();
            let bucket = sample_index
                .checked_mul(buckets)
                .and_then(|value| value.checked_div(total_samples))
                .unwrap_or(0)
                .min(buckets - 1);
            if amplitude > peaks[bucket] {
                peaks[bucket] = amplitude;
            }
            sample_index += 1;
        }
    }

    let status = child.wait().context("ffmpeg did not shut down cleanly")?;
    if !status.success() {
        bail!("ffmpeg failed while decoding the audio track");
    }

    let loudest = peaks.iter().copied().fold(0.0_f32, f32::max);
    if loudest > 0.0 {
        for peak in &mut peaks {
            *peak /= loudest;
        }
    }
    Ok(peaks)
}

/// Find the key frame that is closest to `time`.
pub fn nearest_keyframe(keyframes: &[f64], time: f64) -> Option<f64> {
    if keyframes.is_empty() {
        return None;
    }
    keyframes.iter().copied().min_by(|a, b| {
        (a - time)
            .abs()
            .partial_cmp(&(b - time).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Find the key frame at or before `time`.
pub fn previous_keyframe(keyframes: &[f64], time: f64) -> Option<f64> {
    if keyframes.is_empty() {
        return None;
    }
    let time_limit = time + 1e-6;
    keyframes
        .iter()
        .copied()
        .rev()
        .find(|time| *time <= time_limit)
}

fn as_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .filter(|number| number.is_finite())
}

fn as_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn as_ratio(value: &serde_json::Value) -> Option<f64> {
    let text = value.as_str()?;
    let (numerator, denominator) = text.split_once('/')?;
    let numerator: f64 = numerator.trim().parse().ok()?;
    let denominator: f64 = denominator.trim().parse().ok()?;
    if denominator.abs() < f64::EPSILON {
        return None;
    }
    let result = numerator / denominator;
    result.is_finite().then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ratios() {
        let value = serde_json::Value::String("30000/1001".to_string());
        let fps = as_ratio(&value).expect("ratio");
        assert!((fps - 29.97).abs() < 0.01);
    }

    #[test]
    fn rejects_zero_denominators() {
        let value = serde_json::Value::String("0/0".to_string());
        assert!(as_ratio(&value).is_none());
    }

    #[test]
    fn finds_the_closest_key_frame() {
        let keys = [0.0, 2.0, 4.0, 6.0];
        assert_eq!(nearest_keyframe(&keys, 3.1), Some(4.0));
        assert_eq!(nearest_keyframe(&keys, 3.9), Some(4.0));
        assert_eq!(nearest_keyframe(&keys, 1.0), Some(0.0));
        assert_eq!(nearest_keyframe(&[], 1.0), None);
    }

    #[test]
    fn finds_the_previous_key_frame() {
        let keys = [0.0, 2.0, 4.0];
        assert_eq!(previous_keyframe(&keys, 3.5), Some(2.0));
        assert_eq!(previous_keyframe(&keys, 4.0), Some(4.0));
        assert_eq!(previous_keyframe(&keys, -1.0), None);
    }
}
